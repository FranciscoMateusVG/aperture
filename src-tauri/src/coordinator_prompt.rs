//! Persistent coordinator instructions. Never reads or copies auth.json.
use super::{local_pinned_bytes, LocalInputPin};
use std::{
    fs,
    path::{Path, PathBuf},
};
const CAP: u64 = 2 * 1024 * 1024;
const ERROR: &str = "E_LOCAL_PROMPT_UNAVAILABLE";

fn absent(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Ok(_) => Ok(false),
        Err(_) => Err(ERROR.into()),
    }
}
fn source(path: &Path) -> Result<Vec<u8>, String> {
    // Runtime source links are installed by setup; pin their resolved file and
    // recheck the link resolution, rather than silently skipping a resident.
    let resolved = fs::canonicalize(path).map_err(|_| ERROR)?;
    let (bytes, pin) = local_pinned_bytes(&resolved, CAP).map_err(|_| ERROR)?;
    if fs::canonicalize(path).map_err(|_| ERROR)? != resolved {
        return Err(ERROR.into());
    }
    pin.recheck(&resolved).map_err(|_| ERROR)?;
    std::str::from_utf8(&bytes).map_err(|_| ERROR)?;
    Ok(bytes)
}
fn assembled(roots: &Path, seat: &str, base: &Path) -> Result<Vec<u8>, String> {
    let mut bytes = source(base)?;
    let root = roots.join(seat);
    let resident = root.join("resident.txt");
    let mut names = if absent(&resident)? {
        let skills = root.join("skills");
        if absent(&skills)? {
            Vec::new()
        } else {
            fs::read_dir(skills)
                .map_err(|_| ERROR)?
                .take(129)
                .map(|e| {
                    e.map_err(|_| ERROR.to_string())
                        .and_then(|e| e.file_name().into_string().map_err(|_| ERROR.to_string()))
                })
                .collect::<Result<Vec<_>, _>>()?
        }
    } else {
        String::from_utf8(source(&resident)?)
            .map_err(|_| ERROR)?
            .lines()
            .map(|s| s.split('#').next().unwrap_or("").trim())
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect()
    };
    if names.len() > 128 {
        return Err(ERROR.into());
    }
    names.sort();
    names.dedup();
    for name in names {
        if !crate::daemon_registry::valid_name(&name) {
            return Err(ERROR.into());
        }
        let dir = root.join("skills").join(&name);
        let upper = dir.join("SKILL.md");
        let path = if absent(&upper)? {
            dir.join("skill.md")
        } else {
            upper
        };
        let body = source(&path)?;
        bytes.extend_from_slice(format!("\n\n---\n# Skill: {name}\n\n").as_bytes());
        bytes.extend(body);
        if bytes.len() as u64 > CAP {
            return Err(ERROR.into());
        }
    }
    // Memory is fetched through BEADS, never an unbounded shell/secret loader.
    bytes.extend_from_slice(b"\n\nUse BEADS recall for current memory; this recovered prompt contains the canonical prompt and resident skills.\n");
    Ok(bytes)
}

pub(super) fn configured_home(home: &Path, seat: &str) -> Result<PathBuf, String> {
    if !crate::daemon_registry::valid_name(seat) {
        return Err(ERROR.into());
    }
    let durable = home.join(".aperture/codex").join(seat);
    let legacy = PathBuf::from(format!("/private/tmp/aperture-codex-{seat}"));
    let selected = if !absent(&durable)? { durable } else { legacy };
    crate::controller::private_dir_readonly(&selected).map_err(|_| "E_CODEX_HOME_UNVERIFIED")?;
    Ok(selected)
}

pub(super) fn ensure(
    home: &Path,
    roots: &Path,
    seat: &str,
    base: &Path,
    codex_home: &Path,
    fresh: bool,
) -> Result<PathBuf, String> {
    if !crate::daemon_registry::valid_name(seat) {
        return Err(ERROR.into());
    }
    crate::controller::private_dir_readonly(codex_home)?;
    let config_path = codex_home.join("config.toml");
    let (config_bytes, config_pin) = local_pinned_bytes(&config_path, 256 * 1024)?;
    if config_pin.mode & 0o777 != 0o600 {
        return Err(ERROR.into());
    }
    let mut config: toml::Value = std::str::from_utf8(&config_bytes)
        .map_err(|_| ERROR)?
        .parse()
        .map_err(|_| ERROR)?;
    let table = config.as_table_mut().ok_or(ERROR)?;
    let dir = home.join(".aperture/prompts");
    crate::journal::ensure_private_dir(&dir)?;
    let durable = dir.join(format!("{seat}.md"));
    let legacy = codex_home.join("prompt.md");
    let bytes = if fresh {
        assembled(roots, seat, base)?
    } else if !absent(&durable)? {
        private_prompt(&durable)?.0
    } else if !absent(&legacy)? {
        private_prompt(&legacy)?.0
    } else {
        assembled(roots, seat, base)?
    };
    if bytes.is_empty() || bytes.len() as u64 > CAP {
        return Err(ERROR.into());
    }
    // Refuse unsafe compatibility/config leaves before any publication.
    if !absent(&legacy)? {
        private_prompt(&legacy)?;
    }
    config_pin.recheck(&config_path)?;
    std::str::from_utf8(&bytes).map_err(|_| ERROR)?;
    if fresh || absent(&durable)? {
        if !absent(&durable)? {
            private_prompt(&durable)?;
        }
        crate::journal::write_private_bytes_atomic(&durable, &bytes, fresh)?;
    }
    let (_, pin) = private_prompt(&durable)?;
    // Already-running appservers can retain their old config path. Restore only
    // an absent legacy prompt as a compatibility copy; never replace a leaf.
    if absent(&legacy)? {
        crate::journal::write_private_bytes_atomic(&legacy, &bytes, false)?;
    } else {
        private_prompt(&legacy)?;
    }
    let value = toml::Value::String(durable.to_string_lossy().into_owned());
    if fresh && table.get("model_instructions_file") != Some(&value) {
        table.insert("model_instructions_file".into(), value);
        let output = toml::to_string(&config).map_err(|_| ERROR)?;
        config_pin.recheck(&config_path)?;
        pin.recheck(&durable)?;
        crate::journal::write_private_bytes_atomic(&config_path, output.as_bytes(), true)?;
    }
    pin.recheck(&durable)?;
    Ok(durable)
}
fn private_prompt(path: &Path) -> Result<(Vec<u8>, LocalInputPin), String> {
    let result = local_pinned_bytes(path, CAP)?;
    if result.1.mode & 0o777 != 0o600 {
        return Err(ERROR.into());
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn prompt_survives_temp_loss_preserves_auth_and_operator_settings() {
        let root = PathBuf::from(format!(
            "/private/tmp/aperture-prompt-test-{}",
            uuid::Uuid::new_v4()
        ));
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        let c = root.join("private-home");
        crate::journal::ensure_private_dir(&c).unwrap();
        let write =
            |p: &Path, b: &[u8]| crate::journal::write_private_bytes_atomic(p, b, false).unwrap();
        write(
            &c.join("config.toml"),
            b"model='test'\napproval_policy='never'\n[unknown]\ncustom=17\n",
        );
        write(&c.join("auth.json"), b"synthetic-do-not-touch");
        write(
            &c.join("prompt.md"),
            b"original + residents + preserved memory",
        );
        let auth = LocalInputPin::of(&fs::metadata(c.join("auth.json")).unwrap());
        let p = ensure(
            &root,
            &root,
            "fixture",
            Path::new("missing-base"),
            &c,
            false,
        )
        .unwrap();
        assert!(p.starts_with(root.join(".aperture")));
        fs::remove_file(c.join("prompt.md")).unwrap();
        ensure(
            &root,
            &root,
            "fixture",
            Path::new("missing-base"),
            &c,
            false,
        )
        .unwrap();
        assert_eq!(fs::read(p).unwrap(), fs::read(c.join("prompt.md")).unwrap());
        assert_eq!(
            auth,
            LocalInputPin::of(&fs::metadata(c.join("auth.json")).unwrap())
        );
        let config: toml::Value = fs::read_to_string(c.join("config.toml"))
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(config["approval_policy"].as_str(), Some("never"));
        assert_eq!(config["unknown"]["custom"].as_integer(), Some(17));
        assert!(
            config.get("model_instructions_file").is_none(),
            "adoption does not rewrite daemon config"
        );
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn reconstructs_resident_prompt_without_shell_and_rejects_unsafe_leaf() {
        let root = PathBuf::from(format!(
            "/private/tmp/aperture-prompt-test-{}",
            uuid::Uuid::new_v4()
        ));
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        let c = root.join("codex");
        let r = root.join("agents/fixture");
        crate::journal::ensure_private_dir(&c).unwrap();
        crate::journal::ensure_private_dir(&r.join("skills/beads")).unwrap();
        for (p, b) in [
            (c.join("config.toml"), "model='test'"),
            (r.join("prompt.md"), "BASE"),
            (r.join("resident.txt"), "beads\n"),
            (r.join("skills/beads/SKILL.md"), "BODY"),
        ] {
            crate::journal::write_private_bytes_atomic(&p, b.as_bytes(), false).unwrap();
        }
        let durable = ensure(
            &root,
            &root.join("agents"),
            "fixture",
            &r.join("prompt.md"),
            &c,
            true,
        )
        .unwrap();
        let text = fs::read_to_string(&durable).unwrap();
        assert!(text.contains("BASE") && text.contains("BODY"));
        crate::journal::write_private_bytes_atomic(&r.join("prompt.md"), b"UPDATED", true).unwrap();
        ensure(
            &root,
            &root.join("agents"),
            "fixture",
            &r.join("prompt.md"),
            &c,
            true,
        )
        .unwrap();
        assert!(fs::read_to_string(&durable).unwrap().contains("UPDATED"));
        let conf: toml::Value = fs::read_to_string(c.join("config.toml"))
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(conf["model_instructions_file"].as_str(), durable.to_str());
        fs::remove_file(c.join("prompt.md")).unwrap();
        std::os::unix::fs::symlink(&durable, c.join("prompt.md")).unwrap();
        let config = fs::read(c.join("config.toml")).unwrap();
        assert!(ensure(
            &root,
            &root.join("agents"),
            "fixture",
            &r.join("prompt.md"),
            &c,
            true
        )
        .is_err());
        assert_eq!(config, fs::read(c.join("config.toml")).unwrap());
        fs::remove_dir_all(root).unwrap();
    }
}
