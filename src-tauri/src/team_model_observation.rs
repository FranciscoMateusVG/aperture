//! Consumer of the existing bridge's fixed managed Codex receipt. Internal
//! native observation only: no caller proof, owner write, start or RPC here.
use crate::journal::{open_private_file_nofollow, read_private_json, validate_component_path};
use crate::owner::{try_lock, OwnerRecord, OwnerStore, RuntimeObservation, StartReservation};
use crate::state::{ExecutionTuple, Harness, OwnerState, ReasoningEffort};
use crate::team_process;
use crate::team_replacement::{ProcessIdentity, ProcessState};
use crate::teams::{classify_managed_seat, ManagedSeatState};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
const CAP: u64 = 8192;
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ObservationError {
    Missing,
    Unsafe,
    Invalid,
    Owner,
    Process,
    Revoked,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    schema_version: u32,
    seat: String,
    generation: u64,
    token_id: String,
    root_pid: u32,
    root_start_time_us: u64,
    thread_id: String,
    actual_model: String,
    actual_reasoning: ReasoningEffort,
    observed_at_ms: i64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Attempt {
    schema_version: u32,
    seat: String,
    generation: u64,
    token_id: String,
    root_pid: u32,
    root_start_time_us: u64,
    requested_model: String,
    requested_reasoning: ReasoningEffort,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Revocations {
    schema_version: u32,
    seat: String,
    revoked_through_generation: u64,
    revoked_token_ids: Vec<String>,
}
/// Not a command DTO. The contained observation can only be obtained by the
/// fixed-path reader; caller data cannot construct or deserialize this proof.
pub(crate) struct VerifiedObservation(RuntimeObservation);
impl VerifiedObservation {
    pub(crate) fn into_runtime_observation(self) -> RuntimeObservation {
        self.0
    }
}
fn digest(v: &[u8]) -> String {
    format!("{:x}", Sha256::digest(v))
}
fn hash(v: &str) -> bool {
    v.len() == 64
        && v.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn json<T: DeserializeOwned>(path: &Path) -> Result<T, ObservationError> {
    let file = open_private_file_nofollow(path).map_err(|_| {
        if matches!(std::fs::symlink_metadata(path), Err(e) if e.kind()==std::io::ErrorKind::NotFound) {
            ObservationError::Missing
        } else { ObservationError::Unsafe }
    })?;
    let mut bytes = vec![];
    file.take(CAP + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ObservationError::Unsafe)?;
    if bytes.len() as u64 > CAP {
        return Err(ObservationError::Invalid);
    }
    serde_json::from_slice(&bytes).map_err(|_| ObservationError::Invalid)
}
fn token_current(
    home: &Path,
    seat: &str,
    generation: u64,
    token_id: &str,
) -> Result<(), ObservationError> {
    let path = home
        .join(".aperture/run/hub-tokens")
        .join(format!("{seat}.token"));
    let file = open_private_file_nofollow(&path).map_err(|_| ObservationError::Unsafe)?;
    struct Bytes(Vec<u8>);
    impl Drop for Bytes {
        fn drop(&mut self) {
            self.0.fill(0);
        }
    }
    let mut bytes = Bytes(vec![]);
    file.take(65)
        .read_to_end(&mut bytes.0)
        .map_err(|_| ObservationError::Unsafe)?;
    if bytes.0.len() != 64
        || !bytes
            .0
            .iter()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b))
        || digest(&bytes.0) != token_id
    {
        return Err(ObservationError::Revoked);
    }
    let root = validate_component_path(&home.join(".aperture/run"), "revocations", true)
        .map_err(|_| ObservationError::Revoked)?;
    match std::fs::symlink_metadata(&root) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && generation == 1 => return Ok(()),
        Ok(m) if m.is_dir() && m.uid() == unsafe { libc::geteuid() } && m.mode() & 0o077 == 0 => {}
        _ => return Err(ObservationError::Revoked),
    }
    let path = root.join(format!("{seat}.json"));
    match std::fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && generation == 1 => return Ok(()),
        Ok(_) => {}
        _ => return Err(ObservationError::Revoked),
    }
    let value: Revocations = read_private_json(&path).map_err(|_| ObservationError::Revoked)?;
    let mut sorted = value.revoked_token_ids.clone();
    sorted.sort();
    sorted.dedup();
    if value.schema_version != 1
        || value.seat != seat
        || value.revoked_through_generation != generation - 1
        || sorted != value.revoked_token_ids
        || sorted.len() > 4096
        || sorted.iter().any(|v| !hash(v))
        || sorted.iter().any(|v| v == token_id)
    {
        return Err(ObservationError::Revoked);
    }
    Ok(())
}
pub(crate) fn read_native(
    home: &Path,
    team: &str,
    reservation: &StartReservation,
) -> Result<VerifiedObservation, ObservationError> {
    read_checked(
        home,
        team,
        reservation,
        team_process::state,
        || chrono::Utc::now().timestamp_millis(),
        || {},
    )
}
fn read_checked<S, N, F>(
    home: &Path,
    team: &str,
    reservation: &StartReservation,
    state: S,
    now: N,
    before_recheck: F,
) -> Result<VerifiedObservation, ObservationError>
where
    S: Fn(&ProcessIdentity) -> ProcessState,
    N: FnOnce() -> i64,
    F: FnOnce(),
{
    if !crate::agent_loader::is_valid_seat_name(team)
        || team.len() > 16
        || !crate::agent_loader::is_valid_seat_name(&reservation.seat)
        || reservation.generation == 0
    {
        return Err(ObservationError::Invalid);
    }
    let _team = try_lock(&home.join(".aperture/run/team-locks"), team)
        .map_err(|_| ObservationError::Owner)?;
    match classify_managed_seat(home, &reservation.seat).map_err(|_| ObservationError::Owner)? {
        Some(ManagedSeatState::Active { team: actual, .. }) if actual == team => {}
        _ => return Err(ObservationError::Owner),
    }
    let store = OwnerStore::new(home.join(".aperture/run/owner"));
    let _seat = store
        .lock(&reservation.seat)
        .map_err(|_| ObservationError::Owner)?;
    let owner: OwnerRecord = read_private_json(&store.record_path(&reservation.seat))
        .map_err(|_| ObservationError::Owner)?;
    let candidate = owner.incarnation.as_ref().ok_or(ObservationError::Owner)?;
    if owner.schema_version != 1
        || owner.seat != reservation.seat
        || owner.generation != reservation.generation
        || owner.state != OwnerState::Starting
        || owner.requested.harness != Harness::Codex
        || owner.requested.reasoning.is_none()
        || candidate.observed
        || !candidate.thread_id.is_empty()
        || owner.reservation_nonce_sha256.as_deref()
            != Some(digest(reservation.nonce().as_bytes()).as_str())
        || owner.provisional_token_id.as_deref() != Some(candidate.token_id.as_str())
        || !hash(&candidate.token_id)
        || !candidate
            .processes
            .iter()
            .any(|p| p.pid == candidate.pid && p.start_time == candidate.start_time)
    {
        return Err(ObservationError::Owner);
    }
    let identity = team_process::identity_from_owner(candidate.pid, candidate.start_time)
        .map_err(|_| ObservationError::Process)?;
    if state(&identity) != ProcessState::Same {
        return Err(ObservationError::Process);
    }
    token_current(
        home,
        &reservation.seat,
        reservation.generation,
        &candidate.token_id,
    )?;
    let root = home.join(".aperture/run");
    let base = format!("{}.g{}", reservation.seat, reservation.generation);
    let attempt: Attempt = json(&root.join(format!("{base}.managed-start-attempt.json")))?;
    let receipt: Receipt = json(&root.join(format!("{base}.managed-observation.json")))?;
    let since = chrono::DateTime::parse_from_rfc3339(&owner.since)
        .map_err(|_| ObservationError::Owner)?
        .timestamp_millis();
    let now = now();
    if attempt.schema_version != 1
        || receipt.schema_version != 1
        || attempt.seat != owner.seat
        || receipt.seat != owner.seat
        || attempt.generation != owner.generation
        || receipt.generation != owner.generation
        || attempt.token_id != candidate.token_id
        || receipt.token_id != candidate.token_id
        || attempt.root_pid != candidate.pid
        || receipt.root_pid != candidate.pid
        || attempt.root_start_time_us != candidate.start_time
        || receipt.root_start_time_us != candidate.start_time
        || attempt.requested_model != owner.requested.model
        || receipt.actual_model != owner.requested.model
        || Some(attempt.requested_reasoning) != owner.requested.reasoning
        || Some(receipt.actual_reasoning.clone()) != owner.requested.reasoning
        || receipt.thread_id.is_empty()
        || receipt.thread_id.len() > 128
        || !receipt
            .thread_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        || receipt.observed_at_ms < 1
        || receipt.observed_at_ms < since
        || receipt.observed_at_ms > now
    {
        return Err(ObservationError::Invalid);
    }
    before_recheck();
    if read_private_json::<OwnerRecord>(&store.record_path(&reservation.seat))
        .map_err(|_| ObservationError::Owner)?
        != owner
    {
        return Err(ObservationError::Owner);
    }
    token_current(
        home,
        &reservation.seat,
        reservation.generation,
        &candidate.token_id,
    )?;
    if state(&identity) != ProcessState::Same {
        return Err(ObservationError::Process);
    }
    Ok(VerifiedObservation(RuntimeObservation {
        pid: candidate.pid,
        start_time: candidate.start_time,
        token_id: candidate.token_id.clone(),
        thread_id: receipt.thread_id,
        actual: ExecutionTuple {
            harness: Harness::Codex,
            model: receipt.actual_model,
            reasoning: Some(receipt.actual_reasoning),
        },
    }))
}
#[cfg(test)]
#[path = "team_model_observation_tests.rs"]
mod tests;
