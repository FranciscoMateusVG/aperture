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
    admission: std::sync::Mutex<()>,
    #[cfg(test)]
    admission_probe: Option<AdmissionProbe>,
    hub_child: std::sync::Mutex<Option<std::process::Child>>,
    codex_slots: std::sync::Mutex<
        std::collections::BTreeMap<String, std::sync::Arc<std::sync::Mutex<CodexSlotState>>>,
    >,
    run: PathBuf,
    identity: crate::team_replacement::ProcessIdentity,
}
// Test-only finite rendezvous on the real admission path; no alternate writer.
#[cfg(test)]
pub(crate) struct AdmissionProbe {
    pub counted: std::sync::mpsc::Sender<()>,
    pub contended: std::sync::mpsc::Sender<()>,
    pub resume: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
    pub pause_once: std::sync::atomic::AtomicBool,
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
            admission: std::sync::Mutex::new(()),
            #[cfg(test)]
            admission_probe: None,
            hub_child: std::sync::Mutex::new(None),
            codex_slots: std::sync::Mutex::new(std::collections::BTreeMap::new()),
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
    /// Operational writer lane, never projected through Registry's RO API.
    /// Lock order is slot/hub -> admission, never the reverse. Hold only through
    /// namespace validation, mkdir and durable intent; no spawn/probe/wait here.
    pub(crate) fn registry_admission(&self) -> Result<std::sync::MutexGuard<'_, ()>, String> {
        #[cfg(test)]
        let guard = match self.admission.try_lock() {
            Ok(guard) => guard,
            Err(std::sync::TryLockError::WouldBlock) => {
                if let Some(probe) = &self.admission_probe {
                    let _ = probe.contended.send(());
                }
                self.admission.lock().map_err(|_| "E_CONTROLLER_POISONED")?
            }
            Err(std::sync::TryLockError::Poisoned(_)) => return Err("E_CONTROLLER_POISONED".into()),
        };
        #[cfg(not(test))]
        let guard = self.admission.lock().map_err(|_| "E_CONTROLLER_POISONED")?;
        self.verify_live()?; // including after contention; a stale lease never writes
        Ok(guard)
    }
    #[cfg(test)]
    pub(crate) fn set_admission_probe(&mut self, probe: AdmissionProbe) {
        self.admission_probe = Some(probe);
    }
    #[cfg(test)]
    pub(crate) fn admission_after_capacity(&self) {
        if let Some(probe) = &self.admission_probe {
            if probe
                .pause_once
                .swap(false, std::sync::atomic::Ordering::SeqCst)
            {
                probe.counted.send(()).expect("capacity observer alive");
                probe
                    .resume
                    .lock()
                    .unwrap()
                    .recv_timeout(std::time::Duration::from_secs(10))
                    .expect("bounded admission release");
            }
        }
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
    /// Operational lane only, never reachable through Registry's RO projection.
    /// The slot handle and every operation borrow this lease; no eviction or
    /// detached worker can escape its lifetime. Hub serialization is unchanged.
    pub(crate) fn codex_slot(&self, seat: &str) -> Result<CodexSlot<'_>, String> {
        self.verify_live()?;
        if !crate::daemon_registry::valid_name(seat) {
            return Err("E_CODEX_SLOT".into());
        }
        let mut slots = self
            .codex_slots
            .lock()
            .map_err(|_| "E_CONTROLLER_POISONED")?;
        if !slots.contains_key(seat) && slots.len() >= 128 {
            return Err("E_DAEMON_CAPACITY".into());
        }
        let state = slots.entry(seat.into()).or_default().clone();
        Ok(CodexSlot {
            lease: self,
            seat: seat.into(),
            state,
        })
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

#[derive(Default)]
struct CodexSlotState {
    child: Option<std::process::Child>,
    identity: Option<crate::team_replacement::ProcessIdentity>,
}
/// An operational handle, not a value returned by Registry observation.
pub(crate) struct CodexSlot<'a> {
    lease: &'a ControllerLock,
    seat: String,
    state: std::sync::Arc<std::sync::Mutex<CodexSlotState>>,
}
pub(crate) struct CodexOperation<'a> {
    lease: &'a ControllerLock,
    seat: &'a str,
    nonce: String,
    state: std::sync::MutexGuard<'a, CodexSlotState>,
}
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum WaitObservation {
    NotObserved,
    Running,
    Exited(std::process::ExitStatus),
}
impl CodexSlot<'_> {
    pub(crate) fn enter(&self) -> Result<CodexOperation<'_>, String> {
        // Map lock was released by codex_slot; unrelated slots can progress.
        let state = self.state.lock().map_err(|_| "E_CONTROLLER_POISONED")?;
        self.lease.verify_live()?;
        Ok(CodexOperation {
            lease: self.lease,
            seat: &self.seat,
            nonce: uuid::Uuid::new_v4().to_string(),
            state,
        })
    }
}
impl CodexOperation<'_> {
    pub(crate) fn seat(&self) -> &str {
        self.seat
    }
    pub(crate) fn nonce(&self) -> &str {
        &self.nonce
    }
    pub(crate) fn verify_for(&self, lease: &ControllerLock) -> Result<(), String> {
        if !std::ptr::eq(self.lease, lease) {
            return Err("E_CONTROLLER_CONTEXT".into());
        }
        self.lease.verify_live()
    }
    /// Retain wait ownership without exporting Child or signal authority. On
    /// rejection return ownership to the operational caller, never kill/drop it
    /// silently. An occupied slot is never replaced, even after observed exit.
    pub(crate) fn retain_child(
        &mut self,
        child: std::process::Child,
    ) -> Result<(), std::process::Child> {
        if self.lease.verify_live().is_err() || self.state.child.is_some() {
            return Err(child);
        }
        let native = match crate::team_process::observe(child.id()) {
            Ok(Some(p)) if p.ppid == std::process::id() && p.uid == unsafe { libc::geteuid() } => p,
            _ => return Err(child),
        };
        self.state.identity = Some(native.identity);
        self.state.child = Some(child);
        Ok(())
    }
    pub(crate) fn try_wait(&mut self) -> Result<WaitObservation, String> {
        self.lease.verify_live()?;
        match self.state.child.as_mut() {
            None => Ok(WaitObservation::NotObserved),
            Some(child) => child
                .try_wait()
                .map(|s| {
                    s.map(WaitObservation::Exited)
                        .unwrap_or(WaitObservation::Running)
                })
                .map_err(|_| "E_CODEX_WAIT_UNKNOWN".into()),
        }
    }
}
// No Drop implementation: releasing a context/lease never signals the daemon.

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
    #[test]
    fn codex_same_slot_serializes_but_other_slots_progress() {
        let home = std::env::temp_dir().join(format!("aperture-c2-lock-{}", uuid::Uuid::new_v4()));
        let lease = ControllerLock::acquire(&home).unwrap();
        let first = lease.codex_slot("one").unwrap();
        let held = first.enter().unwrap();
        let (attempt_tx, attempt_rx) = std::sync::mpsc::channel();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        std::thread::scope(|scope| {
            let other = &lease;
            scope.spawn(move || {
                let slot = other.codex_slot("one").unwrap();
                attempt_tx.send(()).unwrap();
                let operation = slot.enter().unwrap();
                operation.verify_for(other).unwrap();
                entered_tx.send(()).unwrap();
            });
            attempt_rx
                .recv_timeout(std::time::Duration::from_secs(2))
                .unwrap();
            assert!(entered_rx
                .recv_timeout(std::time::Duration::from_millis(50))
                .is_err());
            let (tx, rx) = std::sync::mpsc::channel();
            let other = &lease;
            scope.spawn(move || {
                let slot = other.codex_slot("two").unwrap();
                let operation = slot.enter().unwrap();
                operation.verify_for(other).unwrap();
                tx.send(()).unwrap();
            });
            rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap();
            drop(held);
            entered_rx
                .recv_timeout(std::time::Duration::from_secs(2))
                .unwrap();
        });
        drop(first);
        drop(lease);
        std::fs::remove_dir_all(home).unwrap();
    }
    #[test]
    fn codex_slot_capacity_no_eviction_and_context_is_exact_lease() {
        let home = std::env::temp_dir().join(format!("aperture-c2-map-{}", uuid::Uuid::new_v4()));
        let lease = ControllerLock::acquire(&home).unwrap();
        for i in 0..128 {
            drop(lease.codex_slot(&format!("seat-{i}")).unwrap());
        }
        assert!(lease.codex_slot("overflow").is_err());
        let slot = lease.codex_slot("seat-0").unwrap();
        let context = slot.enter().unwrap();
        let other_home = home.join("other");
        let other = ControllerLock::acquire(&other_home).unwrap();
        assert!(context.verify_for(&other).is_err());
        assert_eq!(context.seat(), "seat-0");
        drop(context);
        drop(slot);
        drop(other);
        drop(lease);
        std::fs::remove_dir_all(home).unwrap();
    }
    struct WaitFixture {
        home: PathBuf,
        identity: Option<crate::team_replacement::ProcessIdentity>,
    }
    impl WaitFixture {
        fn new() -> Self {
            let home =
                std::env::temp_dir().join(format!("aperture-c2-wait-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&home).unwrap();
            Self {
                home,
                identity: None,
            }
        }
        fn child(&mut self) -> std::process::Child {
            let child = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "controller::tests::codex_inert_wait_entry",
                    "--ignored",
                    "--nocapture",
                ])
                .env("APERTURE_CODEX_WAIT_FIXTURE", &self.home)
                .env("HOME", &self.home)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap();
            self.identity = Some(
                crate::team_process::observe(child.id())
                    .unwrap()
                    .unwrap()
                    .identity,
            );
            let until = std::time::Instant::now() + std::time::Duration::from_secs(3);
            while !self.home.join("ready").exists() {
                assert!(std::time::Instant::now() < until);
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            child
        }
    }
    impl Drop for WaitFixture {
        fn drop(&mut self) {
            if let Some(identity) = &self.identity {
                if crate::team_process::state(identity)
                    == crate::team_replacement::ProcessState::Same
                {
                    assert_eq!(unsafe { libc::kill(identity.pid as i32, libc::SIGKILL) }, 0);
                }
                let until = std::time::Instant::now() + std::time::Duration::from_secs(3);
                loop {
                    unsafe {
                        libc::waitpid(identity.pid as i32, std::ptr::null_mut(), libc::WNOHANG);
                    }
                    if crate::team_process::state(identity)
                        == crate::team_replacement::ProcessState::Gone
                    {
                        break;
                    }
                    assert!(
                        std::time::Instant::now() < until,
                        "own inert cleanup unverified"
                    );
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                eprintln!("C2a own inert wait fixture reaped/Gone");
            }
            std::fs::remove_dir_all(&self.home).unwrap();
        }
    }
    #[test]
    #[ignore = "inert child entry; explicit owned wait fixtures only"]
    fn codex_inert_wait_entry() {
        let root = PathBuf::from(std::env::var_os("APERTURE_CODEX_WAIT_FIXTURE").unwrap());
        assert!(root
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("aperture-c2-wait-"));
        std::fs::write(root.join("ready"), b"ready").unwrap();
        let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !root.join("finish").exists() {
            assert!(
                std::time::Instant::now() < until,
                "inert wait fixture deadline"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
    #[test]
    fn codex_wait_handle_retained_across_operations_and_drop_never_kills() {
        for detach in [false, true] {
            let mut fixture = WaitFixture::new();
            let lease = ControllerLock::acquire(&fixture.home).unwrap();
            let slot = lease.codex_slot("fixture").unwrap();
            let mut operation = slot.enter().unwrap();
            assert_eq!(operation.try_wait().unwrap(), WaitObservation::NotObserved);
            operation.retain_child(fixture.child()).unwrap();
            drop(operation);
            drop(slot);
            let slot = lease.codex_slot("fixture").unwrap();
            let mut operation = slot.enter().unwrap();
            assert_eq!(operation.try_wait().unwrap(), WaitObservation::Running);
            if !detach {
                std::fs::write(fixture.home.join("finish"), b"finish").unwrap();
                let until = std::time::Instant::now() + std::time::Duration::from_secs(3);
                loop {
                    match operation.try_wait().unwrap() {
                        WaitObservation::Exited(status) => {
                            assert!(status.success());
                            break;
                        }
                        WaitObservation::Running => {
                            assert!(std::time::Instant::now() < until);
                            std::thread::sleep(std::time::Duration::from_millis(5));
                        }
                        _ => panic!("lost own wait handle"),
                    }
                }
            }
            drop(operation);
            drop(slot);
            drop(lease);
            if detach {
                assert_eq!(
                    crate::team_process::state(fixture.identity.as_ref().unwrap()),
                    crate::team_replacement::ProcessState::Same
                );
            }
            drop(fixture); // Only this test owner signals/reaps; never lease Drop.
        }
    }
}
