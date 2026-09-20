use std::fs::{self, File, OpenOptions};
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ActorKind {
    OperatorUi,
    GladosControl,
    Launcher,
}

/// Trusted provenance for audit fields. This type is never serialized or
/// accepted from a command payload; callers obtain it from a verified seam.
#[derive(Debug)]
pub(crate) struct AuthenticatedActor {
    kind: ActorKind,
    capability: Option<CapabilityIdentity>,
}

#[derive(Debug)]
struct CapabilityIdentity {
    file: File,
    path: PathBuf,
    dev: u64,
    ino: u64,
    len: u64,
}

impl AuthenticatedActor {
    pub(crate) fn operator_ui() -> Self {
        Self { kind: ActorKind::OperatorUi, capability: None }
    }

    pub(crate) fn launcher() -> Self {
        Self { kind: ActorKind::Launcher, capability: None }
    }

    pub(crate) fn principal(&self) -> &'static str {
        match self.kind {
            ActorKind::OperatorUi => "operator",
            ActorKind::GladosControl => "glados",
            ActorKind::Launcher => "launcher",
        }
    }

    pub(crate) fn is_glados(&self) -> bool {
        self.kind == ActorKind::GladosControl
    }

    /// Revalidate the already-open canonical GLaDOS capability immediately
    /// before the first team mutation. Both the descriptor and the canonical
    /// path must still name the same inode; an atomically replaced path must
    /// not inherit authority from an orphaned, still-readable descriptor.
    pub(crate) fn revalidate_before_mutation(&self) -> Result<(), String> {
        if self.kind != ActorKind::GladosControl {
            return Err("E_CONTROL_UNAUTHORIZED: exact glados actor required".into());
        }
        let proof = self.capability.as_ref().ok_or_else(|| {
            "E_CONTROL_UNAUTHORIZED: capability proof unavailable".to_string()
        })?;
        validate_canonical_parents()?;
        let fd_meta = proof.file.metadata().map_err(|_| {
            "E_CONTROL_UNAUTHORIZED: capability descriptor unavailable".to_string()
        })?;
        let path_meta = fs::symlink_metadata(&proof.path).map_err(|_| {
            "E_CONTROL_UNAUTHORIZED: canonical capability unavailable".to_string()
        })?;
        if !path_meta.is_file()
            || path_meta.file_type().is_symlink()
            || path_meta.uid() != current_uid()
            || path_meta.mode() & 0o777 != 0o600
            || path_meta.nlink() != 1
            || fd_meta.dev() != proof.dev
            || fd_meta.ino() != proof.ino
            || fd_meta.len() != proof.len
            || path_meta.dev() != proof.dev
            || path_meta.ino() != proof.ino
            || path_meta.len() != proof.len
        {
            return Err("E_CONTROL_UNAUTHORIZED: canonical capability changed".into());
        }
        let registry = crate::agent_loader::load_agents_from_disk();
        if !registry.contains_key("glados") {
            return Err("E_CONTROL_UNAUTHORIZED: glados is not enabled".into());
        }
        Ok(())
    }
}

fn canonical_token_root() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    PathBuf::from(home).join(".aperture/run/hub-tokens")
}

fn current_uid() -> u32 {
    unsafe { libc::geteuid() }
}

fn validate_private_dir(path: &Path) -> Result<(), String> {
    let meta = fs::symlink_metadata(path).map_err(|_| "E_CONTROL_UNAUTHORIZED: token root unavailable".to_string())?;
    if !meta.is_dir()
        || meta.file_type().is_symlink()
        || meta.uid() != current_uid()
        || meta.mode() & 0o077 != 0
    {
        return Err("E_CONTROL_UNAUTHORIZED: token root is not private".into());
    }
    Ok(())
}

fn validate_canonical_parents() -> Result<(), String> {
    let root = canonical_token_root();
    let aperture = root.parent().and_then(Path::parent).ok_or_else(|| {
        "E_CONTROL_UNAUTHORIZED: invalid canonical token path".to_string()
    })?;
    let run = root.parent().ok_or_else(|| {
        "E_CONTROL_UNAUTHORIZED: invalid canonical token path".to_string()
    })?;
    for path in [aperture, run, root.as_path()] {
        validate_private_dir(path)?;
    }
    Ok(())
}

/// Authenticate the GLaDOS-only control entrypoint against the current
/// canonical launcher bearer file. The file path is fixed; actor/env/stdin
/// fields are not authority. This reuses the P0 same-UID bearer trust model.
pub(crate) fn authenticate_glados_control() -> Result<AuthenticatedActor, String> {
    let root = canonical_token_root();
    validate_canonical_parents()?;
    let token_path = root.join("glados.token");
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&token_path)
        .map_err(|_| "E_CONTROL_UNAUTHORIZED: canonical glados capability unavailable".to_string())?;
    let before = file.metadata().map_err(|_| "E_CONTROL_UNAUTHORIZED: capability metadata unavailable".to_string())?;
    if !before.is_file()
        || before.uid() != current_uid()
        || before.mode() & 0o777 != 0o600
        || before.nlink() != 1
    {
        return Err("E_CONTROL_UNAUTHORIZED: invalid capability metadata".into());
    }
    let mut token = Vec::new();
    file.by_ref()
        .take(257)
        .read_to_end(&mut token)
        .map_err(|_| "E_CONTROL_UNAUTHORIZED: capability unreadable".to_string())?;
    let after = file.metadata().map_err(|_| "E_CONTROL_UNAUTHORIZED: capability metadata unavailable".to_string())?;
    if before.dev() != after.dev()
        || before.ino() != after.ino()
        || before.len() != after.len()
        || token.len() < 32
        || token.len() > 256
    {
        return Err("E_CONTROL_UNAUTHORIZED: unstable or invalid capability".into());
    }
    token.fill(0);

    // Resolve the current registry after capability validation and immediately
    // before the caller takes the team lock. A copied/fake token path alone is
    // insufficient; the fixed principal must remain enabled.
    let registry = crate::agent_loader::load_agents_from_disk();
    if !registry.contains_key("glados") {
        return Err("E_CONTROL_UNAUTHORIZED: glados is not enabled".into());
    }
    Ok(AuthenticatedActor {
        kind: ActorKind::GladosControl,
        capability: Some(CapabilityIdentity {
            file,
            path: token_path,
            dev: before.dev(),
            ino: before.ino(),
            len: before.len(),
        }),
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Mutex;
    use std::time::{SystemTime, UNIX_EPOCH};

    pub(crate) static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn root() -> PathBuf {
        std::env::temp_dir().join(format!("aperture-team-auth-{}-{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()))
    }

    #[test]
    fn actor_is_not_deserializable_and_plain_env_is_not_authority() {
        let _guard = ENV_LOCK.lock().unwrap();
        let home = root();
        fs::create_dir_all(home.join(".aperture/run/hub-tokens")).unwrap();
        fs::set_permissions(home.join(".aperture"), fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(home.join(".aperture/run"), fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(home.join(".aperture/run/hub-tokens"), fs::Permissions::from_mode(0o700)).unwrap();
        let old_home = std::env::var_os("HOME");
        let old_agent = std::env::var_os("AGENT_NAME");
        std::env::set_var("HOME", &home);
        std::env::set_var("AGENT_NAME", "glados");
        assert!(authenticate_glados_control().unwrap_err().contains("E_CONTROL_UNAUTHORIZED"));
        match old_home { Some(v) => std::env::set_var("HOME", v), None => std::env::remove_var("HOME") }
        match old_agent { Some(v) => std::env::set_var("AGENT_NAME", v), None => std::env::remove_var("AGENT_NAME") }
        fs::remove_dir_all(home).unwrap();
    }


    fn write_registry(home: &Path) {
        let agent = home.join(".claude/aperture/glados");
        fs::create_dir_all(&agent).unwrap();
        fs::write(agent.join("prompt.md"), "test").unwrap();
        fs::write(agent.join("manifest.json"), r#"{"name":"GLaDOS","model":"sonnet","window":"glados","role":"orchestrator","enabled":true}"#).unwrap();
    }

    #[test]
    fn atomic_capability_replacement_fails_before_mutation_revalidation() {
        let _guard = ENV_LOCK.lock().unwrap();
        let home = root();
        let token_root = home.join(".aperture/run/hub-tokens");
        fs::create_dir_all(&token_root).unwrap();
        for path in [home.join(".aperture"), home.join(".aperture/run"), token_root.clone()] {
            fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let token_path = token_root.join("glados.token");
        fs::write(&token_path, b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap();
        fs::set_permissions(&token_path, fs::Permissions::from_mode(0o600)).unwrap();
        write_registry(&home);
        let old_home = std::env::var_os("HOME");
        let old_agents = std::env::var_os("APERTURE_AGENTS_DIR");
        let old_teams = std::env::var_os("APERTURE_TEAMS_DIR");
        std::env::set_var("HOME", &home);
        std::env::set_var("APERTURE_AGENTS_DIR", home.join(".claude/aperture"));
        std::env::set_var("APERTURE_TEAMS_DIR", home.join(".aperture/teams"));
        fs::create_dir_all(home.join(".aperture/teams")).unwrap();
        let actor = authenticate_glados_control().unwrap();
        actor.revalidate_before_mutation().unwrap();

        let replacement = token_root.join("replacement");
        fs::write(&replacement, b"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb").unwrap();
        fs::set_permissions(&replacement, fs::Permissions::from_mode(0o600)).unwrap();
        fs::rename(&replacement, &token_path).unwrap();
        assert!(actor.revalidate_before_mutation().unwrap_err().contains("canonical capability changed"));

        match old_home { Some(v) => std::env::set_var("HOME", v), None => std::env::remove_var("HOME") }
        match old_agents { Some(v) => std::env::set_var("APERTURE_AGENTS_DIR", v), None => std::env::remove_var("APERTURE_AGENTS_DIR") }
        match old_teams { Some(v) => std::env::set_var("APERTURE_TEAMS_DIR", v), None => std::env::remove_var("APERTURE_TEAMS_DIR") }
        fs::remove_dir_all(home).unwrap();
    }
}
