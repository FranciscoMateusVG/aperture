//! Native pipe-gated child for the existing managed app-server launch path.
//! No harness exec until the exact Starting PID/birth is durably owned.
//! No public command, caller-provided ownership proof, supervisor or auto-retry.
use crate::journal::read_private_json;
use crate::owner::{OwnerRecord, OwnerStore, StartReservation};
use crate::state::OwnerState;
use crate::team_process;
use crate::team_replacement::{ProcessIdentity, ProcessState};
use sha2::{Digest, Sha256};
use std::ffi::OsString;
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

// Positional arguments are data, never shell source. EOF from parent death or
// failure exits before exec. The child has its own process group/session.
const GATE:&str="IFS= read -r aperture_gate || exit 125; [ \"$aperture_gate\" = APERTURE_RELEASE ] || exit 125; exec \"$@\"";
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GateError {
    Invalid,
    Spawn,
    Identity,
    Owner,
    Release,
    ExitUnverified,
}
/// Constructed only from native launch bindings. Never Deserialize/Debug: env
/// values may be private credentials; argv and paths must not enter receipts.
pub(crate) struct LaunchSpec {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub cwd: PathBuf,
    pub env: Vec<(OsString, OsString)>,
}
pub(crate) struct PendingChild {
    child: Option<Child>,
    gate: Option<ChildStdin>,
    identity: ProcessIdentity,
}
pub(crate) struct ReleasedChild {
    child: Child,
    identity: ProcessIdentity,
}
impl ReleasedChild {
    pub(crate) fn identity(&self) -> &ProcessIdentity {
        &self.identity
    }
    pub(crate) fn try_wait(&mut self) -> Result<Option<ExitStatus>, GateError> {
        self.child.try_wait().map_err(|_| GateError::ExitUnverified)
    }
}
fn valid(spec: &LaunchSpec) -> bool {
    let clean = |s: &std::ffi::OsStr| !s.as_bytes().contains(&0);
    spec.program.is_absolute()
        && spec.cwd.is_absolute()
        && clean(spec.program.as_os_str())
        && clean(spec.cwd.as_os_str())
        && spec.args.len() <= 256
        && spec.env.len() <= 128
        && spec.args.iter().all(|a| clean(a) && a.len() <= 65536)
        && spec.args.iter().map(|a| a.len()).sum::<usize>() <= 1024 * 1024
        && spec.env.iter().all(|(k, v)| {
            !k.is_empty()
                && clean(k)
                && clean(v)
                && k.len() <= 128
                && v.len() <= 65536
                && k.as_bytes()
                    .iter()
                    .all(|b| b.is_ascii_alphanumeric() || *b == b'_')
        })
}
pub(crate) fn spawn(spec: LaunchSpec) -> Result<PendingChild, GateError> {
    if !valid(&spec) {
        return Err(GateError::Invalid);
    }
    let mut command = Command::new("/bin/sh");
    command
        .args(["-c", GATE, "aperture-managed-gate"])
        .arg(&spec.program)
        .args(&spec.args)
        .current_dir(&spec.cwd)
        .envs(spec.env)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // SAFETY: only async-signal-safe libc call in post-fork/pre-exec closure.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
    let mut child = command.spawn().map_err(|_| GateError::Spawn)?;
    let gate = child.stdin.take().ok_or(GateError::Spawn)?;
    let metadata = match team_process::observe(child.id()) {
        Ok(Some(p)) if p.uid == unsafe { libc::geteuid() } && p.pgid == p.identity.pid => p,
        _ => {
            drop(gate);
            let _ = wait_exit(&mut child);
            return Err(GateError::Identity);
        }
    };
    if child
        .try_wait()
        .map_err(|_| GateError::ExitUnverified)?
        .is_some()
    {
        return Err(GateError::Spawn);
    }
    Ok(PendingChild {
        child: Some(child),
        gate: Some(gate),
        identity: metadata.identity,
    })
}
impl PendingChild {
    pub(crate) fn identity(&self) -> &ProcessIdentity {
        &self.identity
    }
    /// Release only after the actual owner file, under its OS lock, contains
    /// this exact reserved generation/root birth. Actual-model observation is
    /// deliberately still false until the harness native response arrives.
    pub(crate) fn release(
        mut self,
        store: &OwnerStore,
        reservation: &StartReservation,
    ) -> Result<ReleasedChild, GateError> {
        let _lock = store
            .lock(&reservation.seat)
            .map_err(|_| GateError::Owner)?;
        let owner: OwnerRecord = read_private_json(&store.record_path(&reservation.seat))
            .map_err(|_| GateError::Owner)?;
        let actual = owner.incarnation.as_ref().ok_or(GateError::Owner)?;
        let nonce = format!("{:x}", Sha256::digest(reservation.nonce().as_bytes()));
        if owner.schema_version != 1
            || owner.seat != reservation.seat
            || owner.state != OwnerState::Starting
            || owner.generation != reservation.generation
            || owner.reservation_nonce_sha256.as_deref() != Some(nonce.as_str())
            || actual.observed
            || actual.pid != self.identity.pid
            || team_process::birth_micros(&self.identity).ok() != Some(actual.start_time)
            || !actual
                .processes
                .iter()
                .any(|p| p.pid == actual.pid && p.start_time == actual.start_time)
            || owner.provisional_token_id.as_deref() != Some(actual.token_id.as_str())
            || actual.token_id.len() != 64
            || team_process::state(&self.identity) != ProcessState::Same
        {
            return Err(GateError::Owner);
        }
        let child = self.child.as_mut().ok_or(GateError::Release)?;
        if child
            .try_wait()
            .map_err(|_| GateError::ExitUnverified)?
            .is_some()
        {
            return Err(GateError::Release);
        }
        let mut gate = self.gate.take().ok_or(GateError::Release)?;
        // One short pipe write, no replay. A lost/ambiguous result requires the
        // owner's exact stop/revoke cleanup, never another release/start.
        if gate.write_all(b"APERTURE_RELEASE\n").is_err() {
            drop(gate);
            let _ = wait_exit(child);
            return Err(GateError::Release);
        }
        drop(gate);
        Ok(ReleasedChild {
            child: self.child.take().ok_or(GateError::Release)?,
            identity: self.identity.clone(),
        })
    }
    pub(crate) fn cancel(mut self) -> Result<ExitStatus, GateError> {
        self.gate.take();
        wait_exit(self.child.as_mut().ok_or(GateError::ExitUnverified)?)
    }
}
fn wait_exit(child: &mut Child) -> Result<ExitStatus, GateError> {
    let until = Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(status) = child.try_wait().map_err(|_| GateError::ExitUnverified)? {
            return Ok(status);
        }
        if Instant::now() >= until {
            return Err(GateError::ExitUnverified);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}
impl Drop for PendingChild {
    fn drop(&mut self) {
        // Closing an unreleased gate is safe parent-failure cleanup: no signal
        // by guessed PID and no execution of the harness. No force fallback.
        self.gate.take();
        if let Some(child) = self.child.as_mut() {
            let _ = wait_exit(child);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::{ensure_private_dir, write_private_json_atomic};
    use crate::owner::{Incarnation, ProcessIdentity as StoredProcess};
    use crate::state::{ExecutionTuple, Harness};
    use crate::team_auth::AuthenticatedActor;
    struct Fixture {
        root: PathBuf,
        store: OwnerStore,
        reservation: StartReservation,
    }
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir()
                .join(format!("aperture-gated-fixture-{}", uuid::Uuid::new_v4()));
            ensure_private_dir(&root).unwrap();
            let store = OwnerStore::new(root.join("owner"));
            let actor = AuthenticatedActor::launcher();
            let tuple = ExecutionTuple {
                harness: Harness::Codex,
                model: "fixture-model".into(),
                reasoning: None,
            };
            store
                .initialize_owner(&actor, "t1-worker", tuple.clone())
                .unwrap();
            let reservation = store.reserve_start(&actor, "t1-worker", 0, tuple).unwrap();
            Self {
                root,
                store,
                reservation,
            }
        }
        fn marker(&self) -> PathBuf {
            self.root.join("executed marker;literal")
        }
        fn spec(&self) -> LaunchSpec {
            LaunchSpec {
                program: "/usr/bin/touch".into(),
                args: vec![self.marker().into_os_string()],
                cwd: self.root.clone(),
                env: vec![],
            }
        }
        fn attach(&self, p: &PendingChild) {
            let micros = team_process::birth_micros(p.identity()).unwrap();
            self.store
                .bind_and_publish_token(
                    &AuthenticatedActor::launcher(),
                    &self.reservation,
                    "a".repeat(64),
                    || Ok(()),
                )
                .unwrap();
            self.store
                .record_start_candidate(
                    &AuthenticatedActor::launcher(),
                    &self.reservation,
                    Incarnation {
                        pid: p.identity().pid,
                        start_time: micros,
                        thread_id: String::new(),
                        token_id: "a".repeat(64),
                        harness: Harness::Codex,
                        model: "fixture-model".into(),
                        reasoning: None,
                        observed: false,
                        processes: vec![StoredProcess {
                            pid: p.identity().pid,
                            start_time: micros,
                            ppid: std::process::id(),
                            pgid: p.identity().pid,
                            cmdline_sha256: "a".repeat(64),
                            cwd: self.root.to_string_lossy().into_owned(),
                        }],
                    },
                )
                .unwrap();
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.root).unwrap();
        }
    }
    #[test]
    fn native_child_does_not_exec_before_exact_durable_owner_then_releases_once() {
        let f = Fixture::new();
        let child = spawn(f.spec()).unwrap();
        std::thread::sleep(Duration::from_millis(25));
        assert!(!f.marker().exists());
        f.attach(&child);
        assert!(f.store.summary("t1-worker").unwrap().actual.is_none());
        assert!(!f.marker().exists());
        let mut released = child.release(&f.store, &f.reservation).unwrap();
        assert_eq!(released.identity().pid, released.child.id());
        let status = wait_exit(&mut released.child).unwrap();
        assert!(status.success());
        assert!(f.marker().exists());
    }
    #[test]
    fn absent_owner_or_changed_birth_never_opens_gate() {
        let f = Fixture::new();
        let child = spawn(f.spec()).unwrap();
        assert!(matches!(
            child.release(&f.store, &f.reservation),
            Err(GateError::Owner)
        ));
        assert!(!f.marker().exists());
        let f = Fixture::new();
        let child = spawn(f.spec()).unwrap();
        f.attach(&child);
        let path = f.store.record_path("t1-worker");
        let mut record: OwnerRecord = read_private_json(&path).unwrap();
        record.incarnation.as_mut().unwrap().start_time += 1;
        write_private_json_atomic(&path, &record, true).unwrap();
        assert!(matches!(
            child.release(&f.store, &f.reservation),
            Err(GateError::Owner)
        ));
        assert!(!f.marker().exists());
    }
    #[test]
    fn parent_pipe_loss_cancels_without_harness_exec_or_signal() {
        let f = Fixture::new();
        let child = spawn(f.spec()).unwrap();
        let pid = child.identity().clone();
        let status = child.cancel().unwrap();
        assert!(!status.success());
        assert!(!f.marker().exists());
        assert_eq!(team_process::state(&pid), ProcessState::Gone);
        let child = spawn(f.spec()).unwrap();
        let pid = child.identity().clone();
        drop(child);
        assert_eq!(team_process::state(&pid), ProcessState::Gone);
        assert!(!f.marker().exists());
    }
    #[test]
    fn invalid_spec_fails_without_spawn_and_arguments_remain_data() {
        let f = Fixture::new();
        let mut spec = f.spec();
        spec.program = "relative".into();
        assert!(matches!(spawn(spec), Err(GateError::Invalid)));
        let mut spec = f.spec();
        spec.env.push(("INVALID=KEY".into(), "not-used".into()));
        assert!(matches!(spawn(spec), Err(GateError::Invalid)));
        assert!(!f.marker().exists());
        let child = spawn(f.spec()).unwrap();
        f.attach(&child);
        let mut released = child.release(&f.store, &f.reservation).unwrap();
        assert!(wait_exit(&mut released.child).unwrap().success());
        assert!(f.marker().is_file());
    }
    #[test]
    fn gated_native_metadata_has_real_birth_group_and_digest_before_exec() {
        let f = Fixture::new();
        let child = spawn(f.spec()).unwrap();
        let metadata = crate::team_process::native::capture_gated_child(&child).unwrap();
        assert_eq!(metadata.pid, child.identity().pid);
        assert_eq!(
            metadata.start_time,
            team_process::birth_micros(child.identity()).unwrap()
        );
        assert_eq!(metadata.pgid, metadata.pid);
        assert_eq!(metadata.ppid, std::process::id());
        assert_eq!(metadata.cmdline_sha256.len(), 64);
        assert_eq!(
            std::path::Path::new(&metadata.cwd).canonicalize().unwrap(),
            f.root.canonicalize().unwrap()
        );
        assert!(!f.marker().exists());
        child.cancel().unwrap();
        assert!(!f.marker().exists());
    }
}
