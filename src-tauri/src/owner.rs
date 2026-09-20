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
    /// Process birth time as checked Unix-epoch microseconds. On macOS this is
    /// proc_bsdinfo.pbi_start_tvsec * 1_000_000 + pbi_start_tvusec.
    pub start_time: u64,
    pub ppid: u32,
    pub pgid: u32,
    pub cmdline_sha256: String,
    pub cwd: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Incarnation {
    pub pid: u32,
    /// Root process birth time in the same Unix-epoch microsecond unit used by
    /// every entry in `processes`.
    pub start_time: u64,
    pub thread_id: String,
    pub token_id: String,
    pub harness: crate::state::Harness,
    pub model: String,
    pub reasoning: Option<crate::state::ReasoningEffort>,
    /// False while process ownership is durable but the harness tuple has not
    /// yet been observed from an authoritative runtime event.
    #[serde(default)]
    pub observed: bool,
    pub processes: Vec<ProcessIdentity>,
}

impl Incarnation {
    fn execution_tuple(&self) -> Option<ExecutionTuple> {
        self.observed.then(|| ExecutionTuple {
            harness: self.harness.clone(),
            model: self.model.clone(),
            reasoning: self.reasoning.clone(),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OwnerRecord {
    pub schema_version: u32,
    pub seat: String,
    pub generation: u64,
    pub state: OwnerState,
    pub reservation_nonce_sha256: Option<String>,
    /// Digest reserved durably before the canonical token file is published.
    /// It exists only for a Starting generation and is never a bearer value.
    #[serde(default)]
    pub provisional_token_id: Option<String>,
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

/// Native runtime evidence bound to the exact gated candidate. This is not a
/// command DTO and cannot be supplied by a managed seat or UI caller.
#[derive(Debug)]
pub(crate) struct RuntimeObservation {
    pub pid: u32,
    pub start_time: u64,
    pub token_id: String,
    pub thread_id: String,
    pub actual: ExecutionTuple,
}

/// Exact native identity of a candidate whose start failed after ownership was
/// persisted. This is an internal CAS selector, not a command DTO or cleanup
/// assertion supplied by a managed seat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FailedStartIdentity {
    pub pid: u32,
    pub start_time: u64,
    pub token_id: String,
    pub thread_id: String,
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
            provisional_token_id: None,
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
        record.provisional_token_id = None;
        record.requested = requested;
        record.incarnation = None;
        record.since = now();
        record.writer = actor.principal().into();
        self.write_unlocked(&record, true)?;
        Ok(StartReservation { seat: seat.into(), generation: record.generation, nonce })
    }

    /// Persist the new token digest before publishing its canonical file. The
    /// callback runs while the reservation/seat lock is held and may publish
    /// only the already-generated token. A callback failure leaves the digest
    /// durably bound so cleanup can revoke that exact identity; it is never
    /// retried or replaced in this generation.
    pub(crate) fn bind_and_publish_token<F>(
        &self,
        actor: &AuthenticatedActor,
        reservation: &StartReservation,
        token_id: String,
        publish: F,
    ) -> Result<OwnerRecord, String>
    where
        F: FnOnce() -> Result<(), String>,
    {
        if !actor.is_launcher() {
            return Err("E_CONTROL_UNAUTHORIZED: launcher actor required".into());
        }
        if !is_token_id(&token_id) {
            return Err("E_TOKEN_INVALID: provisional token identity is invalid".into());
        }
        let _lock = self.lock(&reservation.seat)?;
        let mut record = self.read_unlocked(&reservation.seat)?;
        require_reservation(&record, reservation)?;
        if record.provisional_token_id.is_some() || record.incarnation.is_some() {
            return Err("E_STATE_CONFLICT: provisional token is already bound".into());
        }
        record.provisional_token_id = Some(token_id);
        record.writer = actor.principal().into();
        self.write_unlocked(&record, true)?;
        publish()?;
        Ok(record)
    }

    /// Persist exact process identity while the child is still blocked on its
    /// launcher gate. This makes crash recovery able to stop the right process.
    pub(crate) fn record_start_candidate(
        &self,
        actor: &AuthenticatedActor,
        reservation: &StartReservation,
        incarnation: Incarnation,
    ) -> Result<OwnerRecord, String> {
        if !actor.is_launcher() {
            return Err("E_CONTROL_UNAUTHORIZED: launcher actor required".into());
        }
        let _lock = self.lock(&reservation.seat)?;
        let mut record = self.read_unlocked(&reservation.seat)?;
        require_reservation(&record, reservation)?;
        if incarnation.observed || !incarnation.thread_id.is_empty() {
            return Err("E_STATE_CONFLICT: gated candidate cannot claim runtime observation".into());
        }
        validate_incarnation(&incarnation)?;
        if record.provisional_token_id.as_deref() != Some(&incarnation.token_id) {
            return Err("E_PROCESS_IDENTITY: candidate token is not the reserved identity".into());
        }
        if let Some(existing) = record.incarnation.as_ref() {
            if existing == &incarnation {
                return Ok(record);
            }
            return Err("E_PROCESS_IDENTITY: gated candidate is already recorded".into());
        }
        record.incarnation = Some(incarnation);
        record.writer = actor.principal().into();
        self.write_unlocked(&record, true)?;
        Ok(record)
    }

    /// Atomically bind the exact native thread and harness observation while
    /// the same pid/birth/token candidate is still Starting. Existing process
    /// identities are preserved byte-for-byte; caller payloads cannot reach
    /// this seam.
    pub(crate) fn record_runtime_observation(
        &self,
        actor: &AuthenticatedActor,
        reservation: &StartReservation,
        observation: RuntimeObservation,
    ) -> Result<OwnerRecord, String> {
        if !actor.is_launcher() { return Err("E_CONTROL_UNAUTHORIZED: launcher actor required".into()); }
        let _lock = self.lock(&reservation.seat)?;
        let mut record = self.read_unlocked(&reservation.seat)?;
        require_reservation(&record, reservation)?;
        let incarnation = record.incarnation.as_mut().ok_or_else(|| "E_STATE_CONFLICT: process identity not recorded".to_string())?;
        if incarnation.pid != observation.pid
            || incarnation.start_time != observation.start_time
            || incarnation.token_id != observation.token_id
        {
            return Err("E_PROCESS_IDENTITY: runtime observation does not match candidate".into());
        }
        if record.provisional_token_id.as_deref() != Some(&observation.token_id) {
            return Err("E_PROCESS_IDENTITY: runtime token is not the reserved identity".into());
        }
        if observation.thread_id.is_empty()
            || observation.thread_id.len() > 128
            || !observation
                .thread_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return Err("E_FRESH_THREAD_UNVERIFIED: runtime thread identity is invalid".into());
        }
        if incarnation.observed {
            if incarnation.thread_id == observation.thread_id
                && incarnation.execution_tuple().as_ref() == Some(&observation.actual)
            {
                return Ok(record);
            }
            return Err("E_MODEL_UNVERIFIED: runtime observation conflicts with owner".into());
        }
        if !incarnation.thread_id.is_empty() && incarnation.thread_id != observation.thread_id {
            return Err("E_FRESH_THREAD_UNVERIFIED: runtime thread identity changed".into());
        }
        incarnation.thread_id = observation.thread_id;
        incarnation.harness = observation.actual.harness;
        incarnation.model = observation.actual.model;
        incarnation.reasoning = observation.actual.reasoning;
        incarnation.observed = true;
        validate_incarnation(incarnation)?;
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
        let actual = record.incarnation.as_ref().ok_or_else(|| "E_STATE_CONFLICT: process identity not recorded".to_string())?
            .execution_tuple().ok_or_else(|| "E_MODEL_UNVERIFIED: execution tuple is not observed".to_string())?;
        if actual != record.requested {
            return Err("E_MODEL_MISMATCH: observed execution tuple differs".into());
        }
        record.state = OwnerState::Active;
        record.reservation_nonce_sha256 = None;
        record.provisional_token_id = None;
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

    /// Quarantine an exact failed candidate after native stop and durable
    /// revocation have already completed. Unlike `abort_start`, this also
    /// handles the narrow crash window after `commit_start` made the same
    /// candidate Active but before the lifecycle attempt could record its
    /// terminal success. No process evidence is discarded.
    pub(crate) fn quarantine_failed_start(
        &self,
        actor: &AuthenticatedActor,
        reservation: &StartReservation,
        expected: &FailedStartIdentity,
    ) -> Result<OwnerRecord, String> {
        if !actor.is_launcher() {
            return Err("E_CONTROL_UNAUTHORIZED: launcher actor required".into());
        }
        if expected.pid == 0
            || expected.start_time == 0
            || !is_token_id(&expected.token_id)
            || expected.thread_id.len() > 128
            || (!expected.thread_id.is_empty()
                && !expected
                    .thread_id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'))
        {
            return Err("E_PROCESS_IDENTITY: failed-start identity is invalid".into());
        }
        let _lock = self.lock(&reservation.seat)?;
        let mut record = self.read_unlocked(&reservation.seat)?;
        if record.generation != reservation.generation {
            return Err("E_GENERATION_MISMATCH: start reservation changed".into());
        }
        match record.state {
            OwnerState::Starting => require_reservation(&record, reservation)?,
            OwnerState::Active => {
                if record.reservation_nonce_sha256.is_some()
                    || record.provisional_token_id.is_some()
                {
                    return Err("E_OWNER_CORRUPT: active owner retains provisional authority".into());
                }
            }
            _ => return Err("E_STATE_CONFLICT: failed-start owner is not quarantinable".into()),
        }
        let incarnation = record
            .incarnation
            .as_ref()
            .ok_or_else(|| "E_OWNER_CORRUPT: owner incarnation is missing".to_string())?;
        validate_incarnation(incarnation)?;
        if incarnation.pid != expected.pid
            || incarnation.start_time != expected.start_time
            || incarnation.token_id != expected.token_id
            || incarnation.thread_id != expected.thread_id
        {
            return Err("E_PROCESS_IDENTITY: failed-start candidate changed".into());
        }
        if record.state == OwnerState::Active
            && incarnation.execution_tuple().as_ref() != Some(&record.requested)
        {
            return Err("E_MODEL_UNVERIFIED: active failed-start tuple is not exact".into());
        }
        record.state = OwnerState::Quarantined;
        record.reservation_nonce_sha256 = None;
        record.since = now();
        record.writer = actor.principal().into();
        self.write_unlocked(&record, true)?;
        Ok(record)
    }

    /// Refresh the exact root+descendant identity set immediately before a
    /// launcher stop. No caller-supplied provenance is persisted, and no
    /// process may be signalled unless this CAS succeeds first.
    pub(crate) fn record_process_snapshot(
        &self,
        actor: &AuthenticatedActor,
        seat: &str,
        expected_generation: u64,
        expected_root_pid: u32,
        expected_root_start_time: u64,
        processes: Vec<ProcessIdentity>,
    ) -> Result<OwnerRecord, String> {
        if !actor.is_launcher() { return Err("E_CONTROL_UNAUTHORIZED: launcher actor required".into()); }
        let _lock = self.lock(seat)?;
        let mut record = self.read_unlocked(seat)?;
        if record.generation != expected_generation { return Err("E_GENERATION_MISMATCH: owner generation changed".into()); }
        if !matches!(record.state, OwnerState::Starting | OwnerState::Active) {
            return Err("E_STATE_CONFLICT: owner process snapshot is not refreshable".into());
        }
        let incarnation = record.incarnation.as_mut().ok_or_else(|| "E_OWNER_CORRUPT: owner incarnation is missing".to_string())?;
        if incarnation.pid != expected_root_pid || incarnation.start_time != expected_root_start_time {
            return Err("E_PROCESS_IDENTITY: root process identity changed".into());
        }
        // A refreshed descendant walk can omit a previously recorded child
        // after it is reparented.  Absence from that walk is not proof that
        // the exact pid+birth identity disappeared, so retain the union.
        // Reused PIDs remain distinct when their birth time differs.
        let mut merged = incarnation.processes.clone();
        for process in processes {
            if !merged.iter().any(|known| known.pid == process.pid && known.start_time == process.start_time) {
                merged.push(process);
            }
        }
        merged.sort_by_key(|process| (process.pid, process.start_time));
        let mut candidate = incarnation.clone();
        candidate.processes = merged;
        validate_incarnation(&candidate)?;
        incarnation.processes = candidate.processes;
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
        let actual = record.incarnation.as_ref().and_then(Incarnation::execution_tuple);
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

fn is_token_id(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
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
    if value.pid == 0
        || value.start_time == 0
        || (value.observed && value.thread_id.is_empty())
        || value.token_id.is_empty()
        || value.processes.is_empty()
    {
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
    fn token_id() -> String { "a".repeat(64) }
    fn candidate(model: &str) -> Incarnation { Incarnation { pid:123, start_time:456, thread_id:String::new(), token_id:token_id(), harness:Harness::Codex, model:model.into(), reasoning:Some(ReasoningEffort::High), observed:false, processes:vec![ProcessIdentity { pid:123,start_time:456,ppid:1,pgid:123,cmdline_sha256:"a".repeat(64),cwd:"/tmp/work".into() }] } }
    fn failed_identity(thread_id: &str) -> FailedStartIdentity { FailedStartIdentity { pid:123, start_time:456, token_id:token_id(), thread_id:thread_id.into() } }
    fn bind_token(store: &OwnerStore, actor: &AuthenticatedActor, reservation: &StartReservation) {
        store.bind_and_publish_token(actor, reservation, token_id(), || Ok(())).unwrap();
    }
    fn observe(store: &OwnerStore, actor: &AuthenticatedActor, reservation: &StartReservation, model: &str) -> OwnerRecord {
        store.record_runtime_observation(actor, reservation, RuntimeObservation {
            pid:123, start_time:456, token_id:token_id(), thread_id:"thread".into(), actual:tuple(model),
        }).unwrap()
    }

    #[test]
    fn generation_cas_and_model_observation_gate() {
        let root = root();
        let store = OwnerStore::new(root.join("owner"));
        let actor = AuthenticatedActor::launcher();
        store.initialize_owner(&actor, "t1-backend", tuple("gpt-6-astra")).unwrap();
        let reservation = store.reserve_start(&actor, "t1-backend", 0, tuple("gpt-6-astra")).unwrap();
        assert!(store.reserve_start(&actor, "t1-backend", 0, tuple("gpt-6-astra")).is_err());
        bind_token(&store, &actor, &reservation);
        store.record_start_candidate(&actor, &reservation, candidate("gpt-5.6-sol")).unwrap();
        observe(&store, &actor, &reservation, "gpt-5.6-sol");
        assert!(store.commit_start(&actor, &reservation).unwrap_err().contains("E_MODEL_MISMATCH"));
        let quarantined = store.abort_start(&actor, &reservation).unwrap();
        assert_eq!(quarantined.state, OwnerState::Quarantined);
        assert_eq!(quarantined.generation, 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn gated_candidate_is_not_actual_until_native_observation() {
        let root = root();
        let store = OwnerStore::new(root.join("owner"));
        let actor = AuthenticatedActor::launcher();
        store.initialize_owner(&actor, "t1-backend", tuple("gpt-6-astra")).unwrap();
        let reservation = store.reserve_start(&actor, "t1-backend", 0, tuple("gpt-6-astra")).unwrap();
        bind_token(&store, &actor, &reservation);
        store.record_start_candidate(&actor, &reservation, candidate("gpt-6-astra")).unwrap();
        assert!(store.summary("t1-backend").unwrap().actual.is_none());
        assert!(!store.summary("t1-backend").unwrap().thread_bound);
        assert!(store.commit_start(&actor, &reservation).unwrap_err().contains("E_MODEL_UNVERIFIED"));
        observe(&store, &actor, &reservation, "gpt-6-astra");
        let committed = store.commit_start(&actor, &reservation).unwrap();
        assert_eq!(committed.state, OwnerState::Active);
        assert_eq!(store.summary("t1-backend").unwrap().actual, Some(tuple("gpt-6-astra")));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn provisional_token_precedes_publication_and_runtime_observation_is_exact() {
        let root = root();
        let store = OwnerStore::new(root.join("owner"));
        let actor = AuthenticatedActor::launcher();
        store.initialize_owner(&actor, "t1-backend", tuple("gpt-6-astra")).unwrap();
        let reservation = store.reserve_start(&actor, "t1-backend", 0, tuple("gpt-6-astra")).unwrap();

        assert!(store
            .record_start_candidate(&actor, &reservation, candidate("gpt-6-astra"))
            .unwrap_err()
            .contains("candidate token is not the reserved identity"));
        let observed_during_publish = std::cell::Cell::new(false);
        let publish_error = store
            .bind_and_publish_token(&actor, &reservation, token_id(), || {
                let record: OwnerRecord = read_private_json(&store.record_path("t1-backend"))?;
                observed_during_publish.set(record.provisional_token_id.as_deref() == Some(token_id().as_str()));
                Err("E_TOKEN_PUBLICATION: fixture failure".into())
            })
            .unwrap_err();
        assert!(publish_error.contains("E_TOKEN_PUBLICATION"));
        assert!(observed_during_publish.get(), "digest must be durable before publication callback");
        let after_failure = store.read_owner("t1-backend").unwrap();
        assert_eq!(after_failure.provisional_token_id, Some(token_id()));
        assert!(after_failure.incarnation.is_none());
        let callback_reached = std::cell::Cell::new(false);
        assert!(store
            .bind_and_publish_token(&actor, &reservation, token_id(), || {
                callback_reached.set(true);
                Ok(())
            })
            .unwrap_err()
            .contains("already bound"));
        assert!(!callback_reached.get(), "failed publication is not retried in the same generation");

        // A fresh fixture exercises the successful path and exact receipt CAS.
        let store = OwnerStore::new(root.join("owner-success"));
        store.initialize_owner(&actor, "t2-backend", tuple("gpt-6-astra")).unwrap();
        let reservation = store.reserve_start(&actor, "t2-backend", 0, tuple("gpt-6-astra")).unwrap();
        bind_token(&store, &actor, &reservation);
        let first_candidate = candidate("gpt-6-astra");
        store.record_start_candidate(&actor, &reservation, first_candidate.clone()).unwrap();
        assert_eq!(
            store.record_start_candidate(&actor, &reservation, first_candidate.clone()).unwrap().incarnation,
            Some(first_candidate.clone()),
        );
        let mut replacement_candidate = first_candidate;
        replacement_candidate.processes.push(ProcessIdentity {
            pid: 124,
            start_time: 457,
            ppid: 123,
            pgid: 123,
            cmdline_sha256: "b".repeat(64),
            cwd: "/tmp/work".into(),
        });
        assert!(store
            .record_start_candidate(&actor, &reservation, replacement_candidate)
            .unwrap_err()
            .contains("already recorded"));
        let before = store.read_owner("t2-backend").unwrap();
        let mut wrong = RuntimeObservation {
            pid: 999,
            start_time: 456,
            token_id: token_id(),
            thread_id: "thread".into(),
            actual: tuple("gpt-6-astra"),
        };
        assert!(store.record_runtime_observation(&actor, &reservation, wrong).unwrap_err().contains("E_PROCESS_IDENTITY"));
        assert_eq!(store.read_owner("t2-backend").unwrap(), before);
        wrong = RuntimeObservation {
            pid: 123,
            start_time: 456,
            token_id: token_id(),
            thread_id: "thread".into(),
            actual: tuple("gpt-6-astra"),
        };
        let observed = store.record_runtime_observation(&actor, &reservation, wrong).unwrap();
        assert_eq!(observed.incarnation.as_ref().unwrap().thread_id, "thread");
        assert_eq!(observed.incarnation.as_ref().unwrap().processes, before.incarnation.as_ref().unwrap().processes);
        let replay = store.record_runtime_observation(&actor, &reservation, RuntimeObservation {
            pid: 123,
            start_time: 456,
            token_id: token_id(),
            thread_id: "thread".into(),
            actual: tuple("gpt-6-astra"),
        }).unwrap();
        assert_eq!(replay, observed);
        let active = store.commit_start(&actor, &reservation).unwrap();
        assert_eq!(active.state, OwnerState::Active);
        assert_eq!(active.provisional_token_id, None);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_start_quarantine_is_exact_launcher_cas_and_preserves_starting_evidence() {
        let root = root();
        let store = OwnerStore::new(root.join("owner"));
        let launcher = AuthenticatedActor::launcher();
        store.initialize_owner(&launcher, "t1-backend", tuple("gpt-6-astra")).unwrap();
        let reservation = store.reserve_start(&launcher, "t1-backend", 0, tuple("gpt-6-astra")).unwrap();
        bind_token(&store, &launcher, &reservation);
        store.record_start_candidate(&launcher, &reservation, candidate("gpt-6-astra")).unwrap();
        let mut expanded = candidate("gpt-6-astra").processes;
        expanded.push(ProcessIdentity { pid:124,start_time:457,ppid:123,pgid:123,cmdline_sha256:"b".repeat(64),cwd:"/tmp/work".into() });
        store.record_process_snapshot(&launcher, "t1-backend", 1, 123, 456, expanded).unwrap();
        let before = store.read_owner("t1-backend").unwrap();

        assert!(store.quarantine_failed_start(
            &AuthenticatedActor::operator_ui(), &reservation, &failed_identity("")
        ).unwrap_err().contains("E_CONTROL_UNAUTHORIZED"));
        for wrong in [
            FailedStartIdentity { pid:999, ..failed_identity("") },
            FailedStartIdentity { start_time:999, ..failed_identity("") },
            FailedStartIdentity { token_id:"b".repeat(64), ..failed_identity("") },
            FailedStartIdentity { thread_id:"other".into(), ..failed_identity("") },
        ] {
            assert!(store.quarantine_failed_start(&launcher, &reservation, &wrong).unwrap_err().contains("E_PROCESS_IDENTITY"));
            assert_eq!(store.read_owner("t1-backend").unwrap(), before);
        }

        let quarantined = store.quarantine_failed_start(&launcher, &reservation, &failed_identity("")).unwrap();
        assert_eq!(quarantined.state, OwnerState::Quarantined);
        assert_eq!(quarantined.generation, 1);
        assert_eq!(quarantined.incarnation, before.incarnation);
        assert_eq!(quarantined.provisional_token_id, before.provisional_token_id);
        assert_eq!(quarantined.requested, before.requested);
        assert!(quarantined.reservation_nonce_sha256.is_none());

        let next = store.reserve_start(&launcher, "t1-backend", 1, tuple("gpt-6-astra")).unwrap();
        let next_before = store.read_owner("t1-backend").unwrap();
        assert!(store.quarantine_failed_start(&launcher, &reservation, &failed_identity("")).unwrap_err().contains("E_GENERATION_MISMATCH"));
        assert_eq!(store.read_owner("t1-backend").unwrap(), next_before);
        assert_eq!(next.generation, 2);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_start_quarantine_accepts_only_same_exact_active_candidate() {
        let root = root();
        let store = OwnerStore::new(root.join("owner"));
        let launcher = AuthenticatedActor::launcher();
        store.initialize_owner(&launcher, "t1-backend", tuple("gpt-6-astra")).unwrap();
        let reservation = store.reserve_start(&launcher, "t1-backend", 0, tuple("gpt-6-astra")).unwrap();
        bind_token(&store, &launcher, &reservation);
        store.record_start_candidate(&launcher, &reservation, candidate("gpt-6-astra")).unwrap();
        observe(&store, &launcher, &reservation, "gpt-6-astra");
        let active = store.commit_start(&launcher, &reservation).unwrap();

        let mut wrong = failed_identity("thread");
        wrong.thread_id = "other".into();
        assert!(store.quarantine_failed_start(&launcher, &reservation, &wrong).unwrap_err().contains("E_PROCESS_IDENTITY"));
        assert_eq!(store.read_owner("t1-backend").unwrap(), active);

        let mut corrupt = active.clone();
        corrupt.incarnation.as_mut().unwrap().model = "gpt-5.6-sol".into();
        write_private_json_atomic(&store.record_path("t1-backend"), &corrupt, true).unwrap();
        assert!(store.quarantine_failed_start(&launcher, &reservation, &failed_identity("thread")).unwrap_err().contains("E_MODEL_UNVERIFIED"));
        assert_eq!(store.read_owner("t1-backend").unwrap(), corrupt);
        write_private_json_atomic(&store.record_path("t1-backend"), &active, true).unwrap();

        let quarantined = store.quarantine_failed_start(&launcher, &reservation, &failed_identity("thread")).unwrap();
        assert_eq!(quarantined.state, OwnerState::Quarantined);
        assert_eq!(quarantined.generation, active.generation);
        assert_eq!(quarantined.incarnation, active.incarnation);
        assert_eq!(quarantined.requested, active.requested);
        assert!(quarantined.provisional_token_id.is_none());
        assert!(quarantined.reservation_nonce_sha256.is_none());
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
        bind_token(&store, &actor, &reservation);
        store.record_start_candidate(&actor, &reservation, candidate("gpt-6-astra")).unwrap();
        observe(&store, &actor, &reservation, "gpt-6-astra");
        store.commit_start(&actor, &reservation).unwrap();
        let mut record: OwnerRecord = read_private_json(&store.record_path("t1-backend")).unwrap();
        record.incarnation.as_mut().unwrap().model = "gpt-5.6-sol".into();
        write_private_json_atomic(&store.record_path("t1-backend"), &record, true).unwrap();
        assert!(store.summary("t1-backend").unwrap_err().contains("E_OWNER_CORRUPT"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn process_snapshot_refresh_is_launcher_only_and_root_bound() {
        let root = root();
        let store = OwnerStore::new(root.join("owner"));
        let launcher = AuthenticatedActor::launcher();
        store.initialize_owner(&launcher, "t1-backend", tuple("gpt-6-astra")).unwrap();
        let reservation = store.reserve_start(&launcher, "t1-backend", 0, tuple("gpt-6-astra")).unwrap();
        bind_token(&store, &launcher, &reservation);
        store.record_start_candidate(&launcher, &reservation, candidate("gpt-6-astra")).unwrap();
        observe(&store, &launcher, &reservation, "gpt-6-astra");
        store.commit_start(&launcher, &reservation).unwrap();
        let before = store.read_owner("t1-backend").unwrap();
        let mut refreshed = before.incarnation.as_ref().unwrap().processes.clone();
        refreshed.push(ProcessIdentity { pid:124,start_time:457,ppid:123,pgid:123,cmdline_sha256:"b".repeat(64),cwd:"/tmp/work".into() });
        assert!(store.record_process_snapshot(&AuthenticatedActor::operator_ui(), "t1-backend", 1, 123, 456, refreshed.clone()).unwrap_err().contains("E_CONTROL_UNAUTHORIZED"));
        assert!(store.record_process_snapshot(&launcher, "t1-backend", 1, 123, 999, refreshed.clone()).unwrap_err().contains("E_PROCESS_IDENTITY"));
        assert_eq!(store.read_owner("t1-backend").unwrap(), before, "failed refreshes are byte-semantic no-ops");
        let after = store.record_process_snapshot(&launcher, "t1-backend", 1, 123, 456, refreshed).unwrap();
        assert_eq!(after.incarnation.as_ref().unwrap().processes.len(), 2);
        assert_eq!(after.incarnation.as_ref().unwrap().token_id, before.incarnation.as_ref().unwrap().token_id);
        assert_eq!(after.incarnation.as_ref().unwrap().thread_id, before.incarnation.as_ref().unwrap().thread_id);
        assert_eq!(after.requested, before.requested);
        assert_eq!(after.state, OwnerState::Active);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn process_snapshot_refresh_preserves_reparented_and_pid_reuse_evidence() {
        let root = root();
        let store = OwnerStore::new(root.join("owner"));
        let launcher = AuthenticatedActor::launcher();
        store.initialize_owner(&launcher, "t1-backend", tuple("gpt-6-astra")).unwrap();
        let reservation = store.reserve_start(&launcher, "t1-backend", 0, tuple("gpt-6-astra")).unwrap();
        bind_token(&store, &launcher, &reservation);
        let mut first = candidate("gpt-6-astra");
        first.processes.push(ProcessIdentity { pid:124,start_time:457,ppid:123,pgid:123,cmdline_sha256:"b".repeat(64),cwd:"/tmp/work".into() });
        store.record_start_candidate(&launcher, &reservation, first).unwrap();

        let reused = ProcessIdentity { pid:124,start_time:999,ppid:1,pgid:124,cmdline_sha256:"c".repeat(64),cwd:"/tmp/reused".into() };
        let refreshed = store.record_process_snapshot(
            &launcher,
            "t1-backend",
            1,
            123,
            456,
            vec![
                ProcessIdentity { pid:123,start_time:456,ppid:1,pgid:123,cmdline_sha256:"a".repeat(64),cwd:"/tmp/work".into() },
                reused.clone(),
            ],
        ).unwrap();
        let identities = &refreshed.incarnation.unwrap().processes;
        assert!(identities.iter().any(|p| p.pid == 124 && p.start_time == 457), "omitted reparented child remains recorded");
        assert!(identities.iter().any(|p| p == &reused), "reused pid with a new birth remains separate");
        assert_eq!(identities.iter().filter(|p| p.pid == 124).count(), 2);
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
