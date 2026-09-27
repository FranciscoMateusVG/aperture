//! One cooperating controller. Never authorizes daemon adoption or port-based kills.
use serde::{Deserialize, Serialize};
use std::os::{
    fd::AsRawFd,
    unix::fs::{MetadataExt, OpenOptionsExt},
};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Holder {
    pid: u32,
    start_time: String,
}
pub(crate) struct ControllerLock {
    _file: File,
    hub_gate: std::sync::Mutex<()>,
    hub_child: std::sync::Mutex<Option<std::process::Child>>,
    run: PathBuf,
    identity: crate::team_replacement::ProcessIdentity,
}
/// Validation only: unlike journal::ensure_private_dir, never repairs absence.
pub(crate) fn private_dir_readonly(path: &Path) -> Result<(), String> {
    let m = std::fs::symlink_metadata(path).map_err(|_| "E_PATH_UNSAFE: missing directory")?;
    if !m.is_dir()
        || m.file_type().is_symlink()
        || m.uid() != unsafe { libc::geteuid() }
        || m.mode() & 0o077 != 0
    {
        return Err("E_PATH_UNSAFE: unsafe directory".into());
    }
    Ok(())
}
impl ControllerLock {
    pub fn acquire(home: &Path) -> Result<Self, String> {
        let root = home.join(".aperture");
        crate::journal::ensure_private_dir(&root).map_err(|_| "controller directory unsafe")?;
        let run = crate::journal::validate_component_path(&root, "run", true)
            .map_err(|_| "controller directory unsafe")?;
        crate::journal::ensure_private_dir(&run).map_err(|_| "controller directory unsafe")?;
        let path = run.join("daemons.lock");
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&path)
            .map_err(|_| "controller lock unavailable")?;
        let m = file.metadata().map_err(|_| "controller lock unavailable")?;
        if !m.is_file()
            || m.uid() != unsafe { libc::geteuid() }
            || m.nlink() != 1
            || m.mode() & 0o777 != 0o600
        {
            return Err("controller lock unsafe".into());
        }
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            let mut raw = String::new();
            let _ = (&mut file).take(512).read_to_string(&mut raw);
            if let Ok(h) = serde_json::from_str::<Holder>(&raw) {
                if h.pid > 1
                    && h.start_time.len() <= 40
                    && h.start_time
                        .bytes()
                        .all(|b| b.is_ascii_digit() || b == b'.')
                {
                    return Err(format!(
                        "controller already held by pid {} birth {}",
                        h.pid, h.start_time
                    ));
                }
            }
            return Err("controller already held; identity unavailable".into());
        }
        let on_path =
            std::fs::symlink_metadata(&path).map_err(|_| "controller lock unavailable")?;
        if on_path.dev() != m.dev() || on_path.ino() != m.ino() || on_path.file_type().is_symlink()
        {
            return Err("controller lock changed".into());
        }
        let process = crate::team_process::observe(std::process::id())
            .map_err(|_| "controller identity unavailable")?
            .ok_or("controller identity unavailable")?;
        let bytes = serde_json::to_vec(&Holder {
            pid: process.identity.pid,
            start_time: process.identity.start_time.clone(),
        })
        .map_err(|_| "controller identity unavailable")?;
        file.set_len(0)
            .and_then(|_| file.seek(SeekFrom::Start(0)))
            .and_then(|_| file.write_all(&bytes))
            .and_then(|_| file.sync_all())
            .map_err(|_| "controller lock write failed")?;
        Ok(Self {
            _file: file,
            hub_gate: std::sync::Mutex::new(()),
            hub_child: std::sync::Mutex::new(None),
            run,
            identity: process.identity,
        })
    }
    /// A borrowed lease cannot outlive its file. Also reject a forked holder or
    /// replaced lock path before registry mutation; possession of a pathname is
    /// not controller authority.
    pub(crate) fn verify_live(&self) -> Result<(), String> {
        use crate::team_replacement::ProcessState;
        if self.identity.pid != std::process::id()
            || crate::team_process::state(&self.identity) != ProcessState::Same
        {
            return Err("E_CONTROLLER_LEASE: holder identity changed".into());
        }
        private_dir_readonly(self.run.parent().ok_or("controller root unavailable")?)?;
        private_dir_readonly(&self.run)?;
        let held = self
            ._file
            .metadata()
            .map_err(|_| "controller lock unavailable")?;
        let path = std::fs::symlink_metadata(self.run.join("daemons.lock"))
            .map_err(|_| "controller lock unavailable")?;
        if !path.is_file()
            || path.file_type().is_symlink()
            || path.dev() != held.dev()
            || path.ino() != held.ino()
            || path.nlink() != 1
            || path.uid() != unsafe { libc::geteuid() }
            || path.mode() & 0o777 != 0o600
        {
            return Err("E_CONTROLLER_LEASE: lock path changed".into());
        }
        Ok(())
    }
    /// All hub supervisors borrowing this lease share one transition lock.
    /// A synchronous borrower cannot outlive the lease; no authority thread is
    /// detached or left to mutate after releasing the controller file lock.
    pub(crate) fn hub_transition(&self) -> Result<std::sync::MutexGuard<'_, ()>, String> {
        let guard = self.hub_gate.lock().map_err(|_| "E_CONTROLLER_POISONED")?;
        self.verify_live()?;
        Ok(guard)
    }
    /// Retain direct-child wait authority across synchronous supervisors. A
    /// dropped supervisor must not lose the handle and leave an unreapable
    /// zombie misclassified as Same. Dropping the lease never signals the child.
    pub(crate) fn hub_child(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, Option<std::process::Child>>, String> {
        self.verify_live()?;
        self.hub_child
            .lock()
            .map_err(|_| "E_CONTROLLER_POISONED".into())
    }
    pub(crate) fn run_dir(&self) -> Result<&Path, String> {
        self.verify_live()?;
        Ok(&self.run)
    }
    pub(crate) fn identity(&self) -> Result<&crate::team_replacement::ProcessIdentity, String> {
        self.verify_live()?;
        Ok(&self.identity)
    }
    pub fn rotate_open_capability(&self, value: &str) -> Result<(), String> {
        self.verify_live()?;
        crate::journal::write_private_bytes_atomic(
            &self.run.join("operator.token"),
            value.as_bytes(),
            true,
        )
        .map_err(|_| "operator capability publication failed".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn second_controller_cannot_rotate_or_mutate() {
        let home =
            std::env::temp_dir().join(format!("aperture-controller-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&home).unwrap();
        let first = ControllerLock::acquire(&home).unwrap();
        first.rotate_open_capability("synthetic").unwrap();
        assert!(ControllerLock::acquire(&home)
            .err()
            .unwrap()
            .contains("already held by pid"));
        assert_eq!(
            std::fs::read(home.join(".aperture/run/operator.token")).unwrap(),
            b"synthetic"
        );
        drop(first);
        drop(ControllerLock::acquire(&home).unwrap());
        std::fs::remove_dir_all(home).unwrap();
    }
}
