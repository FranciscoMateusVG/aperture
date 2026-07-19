use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

fn token_dir() -> PathBuf {
    std::env::var("APERTURE_HUB_TOKEN_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/tmp".into()))
                .join(".aperture/run/hub-tokens")
        })
}

fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.len() > 64
        || !name.bytes().enumerate().all(|(i, b)| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || (i > 0 && (b == b'_' || b == b'-'))
        })
    {
        return Err("invalid hub token principal".into());
    }
    Ok(())
}

fn secure_dir(path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        let meta = fs::symlink_metadata(parent).map_err(|e| e.to_string())?;
        if !meta.is_dir() || meta.file_type().is_symlink() {
            return Err(format!(
                "hub token parent is not a real directory: {}",
                parent.display()
            ));
        }
        if meta.uid() != unsafe { libc::geteuid() } || (meta.mode() & 0o022) != 0 {
            return Err(format!(
                "hub token parent is not private: {}",
                parent.display()
            ));
        }
    }
    if path.exists() {
        let meta = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
        if !meta.is_dir() || meta.file_type().is_symlink() {
            return Err(format!(
                "hub token directory is not a real directory: {}",
                path.display()
            ));
        }
    } else {
        fs::create_dir_all(path).map_err(|e| e.to_string())?;
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
    let meta = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if meta.uid() != unsafe { libc::geteuid() } {
        return Err(format!(
            "hub token directory has wrong owner: {}",
            path.display()
        ));
    }
    Ok(())
}

fn random_token() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .map_err(|e| format!("failed to obtain OS randomness: {e}"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Atomically provision a fresh mode-0600 bearer token for one principal.
/// Only the file path is exported to child processes; the token never enters
/// argv, launcher scripts, logs, or pane scrollback.
fn provision_token_in(dir: &Path, name: &str) -> Result<PathBuf, String> {
    validate_name(name)?;
    secure_dir(dir)?;
    let target = dir.join(format!("{name}.token"));
    let temp = dir.join(format!(".{name}.token.{}.tmp", std::process::id()));
    let token = random_token()?;
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp)
            .map_err(|e| e.to_string())?;
        file.write_all(token.as_bytes())
            .map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        fs::rename(&temp, &target).map_err(|e| e.to_string())?;
        fs::set_permissions(&target, fs::Permissions::from_mode(0o600))
            .map_err(|e| e.to_string())?;
        Ok(target)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

pub fn provision_token(name: &str) -> Result<PathBuf, String> {
    provision_token_in(&token_dir(), name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn private_parent() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "aperture-hub-auth-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        path
    }

    #[test]
    fn provisions_private_fresh_tokens() {
        let parent = private_parent();
        let dir = parent.join("tokens");
        let path = provision_token_in(&dir, "cipher").unwrap();
        let first = fs::read_to_string(&path).unwrap();
        assert_eq!(first.len(), 64);
        assert_eq!(fs::symlink_metadata(&dir).unwrap().mode() & 0o777, 0o700);
        assert_eq!(fs::symlink_metadata(&path).unwrap().mode() & 0o777, 0o600);
        provision_token_in(&dir, "cipher").unwrap();
        assert_ne!(first, fs::read_to_string(&path).unwrap());
        fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn rejects_invalid_principals() {
        let parent = private_parent();
        assert!(provision_token_in(&parent.join("tokens"), "../cipher").is_err());
        fs::remove_dir_all(parent).unwrap();
    }
}

pub fn token_path(name: &str) -> Result<PathBuf, String> {
    validate_name(name)?;
    Ok(token_dir().join(format!("{name}.token")))
}
