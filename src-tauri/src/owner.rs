use crate::agent_loader::is_valid_seat_name;
use crate::journal::{ensure_private_dir, read_private_json, write_private_json_atomic};
use crate::state::{ExecutionTuple, OwnerState, OwnerSummary};
use crate::team_auth::AuthenticatedActor;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{File, OpenOptions};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Debug)]
pub(crate) struct AdvisoryLock {
    file: File,
}

impl Drop for AdvisoryLock {
    fn drop(&mut self) {
        unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) };
    }
}

use std::os::fd::AsRawFd;

pub(crate) fn try_lock(root: &Path, key: &str) -> Result<AdvisoryLock, String> {
    if !is_valid_seat_name(key) {
        return Err("E_NAME_INVALID: invalid lock key".into());
    }
    ensure_private_dir(root)?;
    let path = root.join(format!("{key}.lock"));
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)
        .map_err(|e| format!("E_LOCK_HELD: {e}"))?;
    let meta = file.metadata().map_err(|e| format!("E_PERMISSION_UNSAFE: {e}"))?;
    if !meta.is_file()
        || meta.uid() != unsafe { libc::geteuid() }
        || meta.mode() & 0o777 != 0o600
        || meta.nlink() != 1
    {
        return Err("E_PERMISSION_UNSAFE: unsafe lock file".into());
    }
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc != 0 {
        return Err("E_LOCK_HELD: lock is owned by another process".into());
    }
    Ok(AdvisoryLock { file })
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub start_time: u64,
    pub ppid: u32,
    pub pgid: u32,
    pub cmdline_sha256: String,
    pub cwd: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Incarnation {
    pub pid: u32,
    pub start_time: u64,
    pub thread_id: String,
    pub token_id: String,
    pub harness: crate::state::Harness,
    pub model: String,
    pub reasoning: Option<crate::state::ReasoningEffort>,
    pub processes: Vec<ProcessIdentity>,
}

impl Incarnation {
    fn execution_tuple(&self) -> ExecutionTuple {
        ExecutionTuple {
            harness: self.harness.clone(),
            model: self.model.clone(),
            reasoning: self.reasoning.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OwnerRecord {
    pub schema_version: u32,
    pub seat: String,
    pub generation: u64,
    pub state: OwnerState,
    pub reservation_nonce_sha256: Option<String>,
    pub requested: ExecutionTuple,
    pub incarnation: Option<Incarnation>,
    pub since: String,
    pub writer: String,
}

/// Raw nonce stays in launcher memory and is deliberately not serializable.
#[derive(Debug)]
pub struct StartReservation {
    pub seat: String,
    pub generation: u64,
    nonce: String,
}

impl StartReservation {
    pub(crate) fn nonce(&self) -> &str { &self.nonce }
}

#[derive(Debug, Clone)]
pub struct OwnerStore {
    pub root: PathBuf,
}

impl OwnerStore {
    pub fn new(root: PathBuf) -> Self { Self { root } }

    pub(crate) fn lock_root(&self) -> PathBuf { self.root.join("locks") }
    pub(crate) fn record_path(&self, seat: &str) -> PathBuf { self.root.join(format!("{seat}.json")) }

    pub(crate) fn lock(&self, seat: &str) -> Result<AdvisoryLock, String> {
        try_lock(&self.lock_root(), seat)
    }

    fn read_unlocked(&self, seat: &str) -> Result<OwnerRecord, String> {
        if !is_valid_seat_name(seat) { return Err("E_NAME_INVALID: invalid owner seat".into()); }
        let path = self.record_path(seat);
        let record: OwnerRecord = read_private_json(&path)
            .map_err(|_| "E_OWNER_CORRUPT: missing or malformed owner record".to_string())?;
        if record.schema_version != 1 || record.seat != seat {
            return Err("E_OWNER_CORRUPT: owner identity mismatch".into());
        }
        Ok(record)
    }

    fn write_unlocked(&self, record: &OwnerRecord, replace: bool) -> Result<(), String> {
        write_private_json_atomic(&self.record_path(&record.seat), record, replace)
    }

    pub(crate) fn initial_record(
        actor: &AuthenticatedActor,
        seat: &str,
        configured: ExecutionTuple,
    ) -> Result<OwnerRecord, String> {
        if !is_valid_seat_name(seat) { return Err("E_NAME_INVALID: invalid owner seat".into()); }
        Ok(OwnerRecord {
            schema_version: 1,
            seat: seat.into(),
            generation: 0,
            state: OwnerState::Stale,
            reservation_nonce_sha256: None,
            requested: configured,
            incarnation: None,
            since: now(),
            writer: actor.principal().into(),
        })
    }

    /// Called during activation, before active state publication. Missing is
    /// allowed only here; every later owner operation fails closed on missing.
    pub(crate) fn initialize_owner(
        &self,
        actor: &AuthenticatedActor,
        seat: &str,
        configured: ExecutionTuple,
    ) -> Result<OwnerRecord, String> {
        if !is_valid_seat_name(seat) { return Err("E_NAME_INVALID: invalid owner seat".into()); }
        ensure_private_dir(&self.root)?;
        let _lock = self.lock(seat)?;
        if self.record_path(seat).exists() {
            return Err("E_NAME_COLLISION: owner record already exists".into());
        }
        let record = Self::initial_record(actor, seat, configured)?;
        self.write_unlocked(&record, false)?;
        Ok(record)
    }

    pub(crate) fn reserve_start(
        &self,
        actor: &AuthenticatedActor,
        seat: &str,
        expected_generation: u64,
        requested: ExecutionTuple,
    ) -> Result<StartReservation, String> {
        let _lock = self.lock(seat)?;
        let mut record = self.read_unlocked(seat)?;
        if record.generation != expected_generation {
            return Err("E_GENERATION_MISMATCH: owner generation changed".into());
        }
        if !matches!(record.state, OwnerState::Stale | OwnerState::Quarantined) {
            return Err("E_STATE_CONFLICT: owner is not startable".into());
        }
        let nonce = Uuid::new_v4().to_string();
        record.generation = expected_generation.checked_add(1).ok_or_else(|| "E_GENERATION_MISMATCH: generation overflow".to_string())?;
        record.state = OwnerState::Starting;
        record.reservation_nonce_sha256 = Some(hash_text(&nonce));
        record.requested = requested;
        record.incarnation = None;
        record.since = now();
        record.writer = actor.principal().into();
        self.write_unlocked(&record, true)?;
        Ok(StartReservation { seat: seat.into(), generation: record.generation, nonce })
    }

    /// Persist exact process identity while the child is still blocked on its
    /// launcher gate. This makes crash recovery able to stop the right process.
    pub(crate) fn record_start_candidate(
        &self,
        actor: &AuthenticatedActor,
        reservation: &StartReservation,
        incarnation: Incarnation,
    ) -> Result<OwnerRecord, String> {
        let _lock = self.lock(&reservation.seat)?;
        let mut record = self.read_unlocked(&reservation.seat)?;
        require_reservation(&record, reservation)?;
        validate_incarnation(&incarnation)?;
        record.incarnation = Some(incarnation);
        record.writer = actor.principal().into();
        self.write_unlocked(&record, true)?;
        Ok(record)
    }

    /// Commit does not release the child. The runtime releases its gate only
    /// after this returns an exact-tuple active owner.
    pub(crate) fn commit_start(
        &self,
        actor: &AuthenticatedActor,
        reservation: &StartReservation,
    ) -> Result<OwnerRecord, String> {
        let _lock = self.lock(&reservation.seat)?;
        let mut record = self.read_unlocked(&reservation.seat)?;
        require_reservation(&record, reservation)?;
        let actual = record.incarnation.as_ref().ok_or_else(|| "E_STATE_CONFLICT: process identity not recorded".to_string())?.execution_tuple();
        if actual != record.requested {
            return Err("E_MODEL_MISMATCH: observed execution tuple differs".into());
        }
        record.state = OwnerState::Active;
        record.reservation_nonce_sha256 = None;
        record.since = now();
        record.writer = actor.principal().into();
        self.write_unlocked(&record, true)?;
        Ok(record)
    }

    /// Called only after the runtime has stopped the exact candidate process
    /// and durably revoked its generation/token. Evidence remains for review.
    pub(crate) fn abort_start(
        &self,
        actor: &AuthenticatedActor,
        reservation: &StartReservation,
    ) -> Result<OwnerRecord, String> {
        let _lock = self.lock(&reservation.seat)?;
        let mut record = self.read_unlocked(&reservation.seat)?;
        require_reservation(&record, reservation)?;
        record.state = OwnerState::Quarantined;
        record.reservation_nonce_sha256 = None;
        record.since = now();
        record.writer = actor.principal().into();
        self.write_unlocked(&record, true)?;
        Ok(record)
    }

    pub(crate) fn mark_stale(&self, actor: &AuthenticatedActor, seat: &str, expected_generation: u64) -> Result<OwnerRecord, String> {
        let _lock = self.lock(seat)?;
        let mut record = self.read_unlocked(seat)?;
        if record.generation != expected_generation { return Err("E_GENERATION_MISMATCH: owner generation changed".into()); }
        record.state = OwnerState::Stale;
        record.since = now();
        record.writer = actor.principal().into();
        self.write_unlocked(&record, true)?;
        Ok(record)
    }

    pub(crate) fn read_owner(&self, seat: &str) -> Result<OwnerRecord, String> {
        let _lock = self.lock(seat)?;
        self.read_unlocked(seat)
    }

    pub(crate) fn summary(&self, seat: &str) -> Result<OwnerSummary, String> {
        let record = self.read_owner(seat)?;
        let actual = record.incarnation.as_ref().map(Incarnation::execution_tuple);
        if record.state == OwnerState::Active && actual.as_ref() != Some(&record.requested) {
            return Err("E_OWNER_CORRUPT: active owner tuple is not the observed tuple".into());
        }
        Ok(OwnerSummary {
            generation: record.generation,
            state: record.state,
            since: record.since,
            configured: record.requested,
            actual,
            process_count: record.incarnation.as_ref().map_or(0, |i| i.processes.len() as u32),
            thread_bound: record.incarnation.as_ref().is_some_and(|i| !i.thread_id.is_empty()),
        })
    }
}

fn now() -> String { Utc::now().to_rfc3339() }

fn hash_text(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

fn require_reservation(record: &OwnerRecord, reservation: &StartReservation) -> Result<(), String> {
    if record.generation != reservation.generation || record.state != OwnerState::Starting {
        return Err("E_GENERATION_MISMATCH: start reservation changed".into());
    }
    if record.reservation_nonce_sha256.as_deref() != Some(&hash_text(reservation.nonce())) {
        return Err("E_GENERATION_MISMATCH: start reservation mismatch".into());
    }
    Ok(())
}

fn validate_incarnation(value: &Incarnation) -> Result<(), String> {
    if value.pid == 0 || value.start_time == 0 || value.thread_id.is_empty() || value.token_id.is_empty() || value.processes.is_empty() {
        return Err("E_STATE_CONFLICT: incomplete incarnation identity".into());
    }
    if !value.processes.iter().any(|p| p.pid == value.pid && p.start_time == value.start_time) {
        return Err("E_STATE_CONFLICT: root process missing from snapshot".into());
    }
    if value.processes.iter().any(|p| p.pid == 0 || p.start_time == 0 || p.cmdline_sha256.len() != 64 || p.cwd.is_empty()) {
        return Err("E_STATE_CONFLICT: invalid process identity".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{Harness, ReasoningEffort};
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::time::{SystemTime, UNIX_EPOCH};
    use std::process::Command;

    fn root() -> PathBuf {
        let path = std::env::temp_dir().join(format!("aperture-owner-{}-{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        path
    }
    fn tuple(model: &str) -> ExecutionTuple { ExecutionTuple { harness: Harness::Codex, model:model.into(), reasoning:Some(ReasoningEffort::High) } }
    fn incarnation(model: &str) -> Incarnation { Incarnation { pid:123, start_time:456, thread_id:"thread".into(), token_id:"token-id".into(), harness:Harness::Codex, model:model.into(), reasoning:Some(ReasoningEffort::High), processes:vec![ProcessIdentity { pid:123,start_time:456,ppid:1,pgid:123,cmdline_sha256:"a".repeat(64),cwd:"/tmp/work".into() }] } }

    #[test]
    fn generation_cas_and_model_observation_gate() {
        let root = root();
        let store = OwnerStore::new(root.join("owner"));
        let actor = AuthenticatedActor::launcher();
        store.initialize_owner(&actor, "t1-backend", tuple("gpt-6-astra")).unwrap();
        let reservation = store.reserve_start(&actor, "t1-backend", 0, tuple("gpt-6-astra")).unwrap();
        assert!(store.reserve_start(&actor, "t1-backend", 0, tuple("gpt-6-astra")).is_err());
        store.record_start_candidate(&actor, &reservation, incarnation("gpt-5.6-sol")).unwrap();
        assert!(store.commit_start(&actor, &reservation).unwrap_err().contains("E_MODEL_MISMATCH"));
        let quarantined = store.abort_start(&actor, &reservation).unwrap();
        assert_eq!(quarantined.state, OwnerState::Quarantined);
        assert_eq!(quarantined.generation, 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_or_corrupt_owner_never_resets_generation() {
        let root = root();
        let store = OwnerStore::new(root.join("owner"));
        let actor = AuthenticatedActor::launcher();
        ensure_private_dir(&store.root).unwrap();
        assert!(store.reserve_start(&actor, "t1-backend", 0, tuple("gpt-6-astra")).unwrap_err().contains("E_OWNER_CORRUPT"));
        write_private_json_atomic(&store.record_path("t1-backend"), &serde_json::json!({"bad":true}), false).unwrap();
        assert!(store.reserve_start(&actor, "t1-backend", 0, tuple("gpt-6-astra")).unwrap_err().contains("E_OWNER_CORRUPT"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn active_owner_rejects_divergent_observed_tuple() {
        let root = root();
        let store = OwnerStore::new(root.join("owner"));
        let actor = AuthenticatedActor::launcher();
        store.initialize_owner(&actor, "t1-backend", tuple("gpt-6-astra")).unwrap();
        let reservation = store.reserve_start(&actor, "t1-backend", 0, tuple("gpt-6-astra")).unwrap();
        store.record_start_candidate(&actor, &reservation, incarnation("gpt-6-astra")).unwrap();
        store.commit_start(&actor, &reservation).unwrap();
        let mut record: OwnerRecord = read_private_json(&store.record_path("t1-backend")).unwrap();
        record.incarnation.as_mut().unwrap().model = "gpt-5.6-sol".into();
        write_private_json_atomic(&store.record_path("t1-backend"), &record, true).unwrap();
        assert!(store.summary("t1-backend").unwrap_err().contains("E_OWNER_CORRUPT"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn owner_cas_process_child() {
        let Some(root) = std::env::var_os("APERTURE_OWNER_CAS_CHILD_ROOT") else { return; };
        let gate = PathBuf::from(&root).join("go");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !gate.exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(gate.exists(), "parent did not release CAS child");
        let store = OwnerStore::new(PathBuf::from(root).join("owner"));
        match store.reserve_start(&AuthenticatedActor::launcher(), "t1-backend", 0, tuple("gpt-6-astra")) {
            Ok(_) => println!("OWNER_CAS_WIN"),
            Err(error) => assert!(error.contains("E_LOCK_HELD") || error.contains("E_GENERATION_MISMATCH") || error.contains("E_STATE_CONFLICT"), "unexpected loser: {error}"),
        }
    }

    #[test]
    fn generation_cas_is_atomic_across_two_os_processes() {
        if std::env::var_os("APERTURE_OWNER_CAS_CHILD_ROOT").is_some() { return; }
        let root = root();
        let store = OwnerStore::new(root.join("owner"));
        store.initialize_owner(&AuthenticatedActor::launcher(), "t1-backend", tuple("gpt-6-astra")).unwrap();
        let exe = std::env::current_exe().unwrap();
        // Spawn both children before releasing the shared file gate.
        let root_a = root.clone();
        let exe_a = exe.clone();
        let child_a = std::thread::spawn(move || Command::new(exe_a).args(["--exact", "owner::tests::owner_cas_process_child", "--nocapture"]).env("APERTURE_OWNER_CAS_CHILD_ROOT", root_a).output().unwrap());
        let root_b = root.clone();
        let exe_b = exe.clone();
        let child_b = std::thread::spawn(move || Command::new(exe_b).args(["--exact", "owner::tests::owner_cas_process_child", "--nocapture"]).env("APERTURE_OWNER_CAS_CHILD_ROOT", root_b).output().unwrap());
        std::thread::sleep(std::time::Duration::from_millis(50));
        fs::write(root.join("go"), b"go").unwrap();
        let outputs = [child_a.join().unwrap(), child_b.join().unwrap()];
        assert!(outputs.iter().all(|output| output.status.success()), "child failure: {:?}", outputs.iter().map(|o| String::from_utf8_lossy(&o.stderr)).collect::<Vec<_>>());
        let wins = outputs.iter().filter(|output| String::from_utf8_lossy(&output.stdout).contains("OWNER_CAS_WIN")).count();
        assert_eq!(wins, 1, "exactly one process must win generation zero");
        let record = store.read_owner("t1-backend").unwrap();
        assert_eq!(record.generation, 1);
        assert_eq!(record.state, OwnerState::Starting);
        fs::remove_dir_all(root).unwrap();
    }
}
