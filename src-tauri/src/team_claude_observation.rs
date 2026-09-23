//! Claude pre-input observation boundary. Status-line stdin contributes only the
//! documented model/session/pre-input fields; all runtime authority is native.
use crate::state::{ExecutionTuple, Harness};
use crate::team_claude_launch::{canonical_uuid, ClaudeAttempt, ClaudeError, MODEL, WINDOW_MS};
use serde::{Deserialize, Serialize};
use std::io::Read;

const SAMPLE_CAP: u64 = 64 * 1024;
/// Projected payload, never deserialized as a runtime receipt or owner proof.
struct StartupSample {
    session_id: String,
    model: String,
}
fn sample(reader: impl Read) -> Result<StartupSample, ClaudeError> {
    let mut bytes = Vec::new();
    reader
        .take(SAMPLE_CAP + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ClaudeError::Io)?;
    if bytes.len() as u64 > SAMPLE_CAP {
        return Err(ClaudeError::Invalid);
    }
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|_| ClaudeError::Invalid)?;
    let object = value.as_object().ok_or(ClaudeError::Invalid)?;
    // Full status-line data changes over time; only project an allowlist and
    // explicitly reject authority-shaped additions. No transcript/path/env read.
    for forbidden in [
        "seat",
        "team",
        "generation",
        "token",
        "token_id",
        "owner",
        "actor",
        "root_pid",
        "root_start_time_us",
        "observed_at_ms",
        "authenticated",
    ] {
        if object.contains_key(forbidden) {
            return Err(ClaudeError::Invalid);
        }
    }
    if object.contains_key("prompt_id") || object.contains_key("prompt_cache") {
        return Err(ClaudeError::PostInput);
    }
    let context = value
        .get("context_window")
        .and_then(|v| v.as_object())
        .ok_or(ClaudeError::PostInput)?;
    if context.get("current_usage") != Some(&serde_json::Value::Null) {
        return Err(ClaudeError::PostInput);
    }
    for field in ["total_input_tokens", "total_output_tokens"] {
        if let Some(v) = context.get(field) {
            if v.as_u64() != Some(0) {
                return Err(ClaudeError::PostInput);
            }
        }
    }
    let version = value
        .get("version")
        .and_then(|v| v.as_str())
        .ok_or(ClaudeError::Invalid)?;
    let parts: Vec<_> = version.split('.').collect();
    if parts.len() != 3
        || parts
            .iter()
            .any(|v| v.is_empty() || !v.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err(ClaudeError::Invalid);
    }
    let parsed: Vec<u32> = parts
        .iter()
        .map(|v| v.parse::<u32>())
        .collect::<Result<_, _>>()
        .map_err(|_| ClaudeError::Invalid)?;
    // prompt_id absent-before-input is documented from 2.1.196 onward. Version
    // text is not executable provenance; the launch adapter separately pins it.
    if parsed.as_slice() < [2, 1, 196].as_slice() {
        return Err(ClaudeError::Invalid);
    }
    let session = value
        .get("session_id")
        .and_then(|v| v.as_str())
        .ok_or(ClaudeError::Invalid)?;
    let model = value
        .get("model")
        .and_then(|v| v.get("id"))
        .and_then(|v| v.as_str())
        .ok_or(ClaudeError::Model)?;
    if !canonical_uuid(session) {
        return Err(ClaudeError::Invalid);
    }
    if model != MODEL {
        return Err(ClaudeError::Model);
    }
    Ok(StartupSample {
        session_id: session.into(),
        model: model.into(),
    })
}

/// Different schema from the Codex receipt. None is literal Claude policy, not
/// a wildcard or a claim that runtime reasoning has been observed.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ClaudeReceipt {
    schema_version: u32,
    attempt_sha256: String,
    seat: String,
    generation: u64,
    token_id: String,
    root_pid: u32,
    root_start_time_us: u64,
    session_id: String,
    actual_model: String,
    reasoning_observation: String,
    observed_at_ms: i64,
}
fn project(
    attempt: &ClaudeAttempt,
    input: StartupSample,
    now: i64,
) -> Result<ClaudeReceipt, ClaudeError> {
    if attempt.schema_version != 1
        || attempt.requested_model != MODEL
        || attempt.session_id != input.session_id
        || attempt.created_at_ms <= 0
        || now < attempt.created_at_ms
        || now - attempt.created_at_ms > WINDOW_MS
    {
        return Err(ClaudeError::Invalid);
    }
    Ok(ClaudeReceipt {
        schema_version: 1,
        attempt_sha256: digest(&serde_json::to_vec(attempt).map_err(|_| ClaudeError::Invalid)?),
        seat: attempt.seat.clone(),
        generation: attempt.generation,
        token_id: attempt.token_id.clone(),
        root_pid: attempt.root_pid,
        root_start_time_us: attempt.root_start_time_us,
        session_id: input.session_id,
        actual_model: input.model,
        reasoning_observation: "not_observed".into(),
        observed_at_ms: now,
    })
}
impl ClaudeReceipt {
    fn actual(&self) -> Result<ExecutionTuple, ClaudeError> {
        if self.actual_model != MODEL || self.reasoning_observation != "not_observed" {
            return Err(ClaudeError::Model);
        }
        Ok(ExecutionTuple {
            harness: Harness::Claude,
            model: self.actual_model.clone(),
            reasoning: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::ReasoningEffort;
    fn input() -> serde_json::Value {
        serde_json::json!({"version":"2.1.263","session_id":uuid::Uuid::new_v4().to_string(),"model":{"id":MODEL},"context_window":{"current_usage":null,"total_input_tokens":0,"total_output_tokens":0}})
    }
    fn parse(v: &serde_json::Value) -> Result<StartupSample, ClaudeError> {
        sample(serde_json::to_vec(v).unwrap().as_slice())
    }
    fn attempt(v: &serde_json::Value) -> ClaudeAttempt {
        ClaudeAttempt {
            schema_version: 1,
            team: "t1".into(),
            seat: "t1-worker".into(),
            generation: 1,
            reservation_nonce_sha256: "b".repeat(64),
            snapshot_sha256: "c".repeat(64),
            team_generation: 1,
            token_id: "a".repeat(64),
            root_pid: 321,
            root_start_time_us: 1790000000000001,
            session_id: v["session_id"].as_str().unwrap().into(),
            requested_model: MODEL.into(),
            created_at_ms: 1000,
        }
    }
    #[test]
    fn preinput_exact_model_projects_unobserved_reasoning_not_wildcard() {
        let v = input();
        let a = attempt(&v);
        let receipt = project(&a, parse(&v).unwrap(), 1001).unwrap();
        let actual = receipt.actual().unwrap();
        assert_eq!(actual.reasoning, None);
        let mut requested = actual.clone();
        requested.reasoning = Some(ReasoningEffort::High);
        assert_ne!(requested, actual);
        assert_eq!(receipt.reasoning_observation, "not_observed");
    }
    #[test]
    fn rejects_postinput_even_after_compaction_or_null_prompt_id() {
        for change in 0..5 {
            let mut v = input();
            match change {
                0 => v["prompt_id"] = serde_json::Value::Null,
                1 => v["prompt_id"] = "a-prompt".into(),
                2 => v["context_window"]["current_usage"] = serde_json::json!({"input_tokens":1}),
                3 => v["prompt_cache"] = serde_json::json!({}),
                _ => v["context_window"]["total_input_tokens"] = 1.into(),
            };
            assert!(matches!(parse(&v), Err(ClaudeError::PostInput)));
        }
    }
    #[test]
    fn rejects_unknown_model_alias_session_drift_and_stale_window() {
        let mut v = input();
        v["model"]["id"] = "sonnet".into();
        assert!(matches!(parse(&v), Err(ClaudeError::Model)));
        let v = input();
        let mut a = attempt(&v);
        a.session_id = uuid::Uuid::new_v4().to_string();
        assert!(project(&a, parse(&v).unwrap(), 1001).is_err());
        let a = attempt(&v);
        assert!(project(&a, parse(&v).unwrap(), 999).is_err());
        assert!(project(&a, parse(&v).unwrap(), 1001 + WINDOW_MS).is_err());
    }
    #[test]
    fn arbitrary_metadata_never_becomes_runtime_authority() {
        for field in [
            "seat",
            "generation",
            "token_id",
            "actor",
            "authenticated",
            "root_pid",
        ] {
            let mut v = input();
            v[field] = "forged".into();
            assert!(matches!(parse(&v), Err(ClaudeError::Invalid)));
        }
        let mut v = input();
        v["transcript_path"] = "/must/not/read".into();
        v["effort"] = serde_json::json!({"level":"high"});
        let receipt = project(&attempt(&v), parse(&v).unwrap(), 1001).unwrap();
        assert_eq!(receipt.actual().unwrap().reasoning, None);
    }
    #[test]
    fn bounded_parse_requires_documented_preinput_fields() {
        assert!(sample(&vec![b' '; SAMPLE_CAP as usize + 1][..]).is_err());
        let mut v = input();
        v["context_window"]
            .as_object_mut()
            .unwrap()
            .remove("current_usage");
        assert!(parse(&v).is_err());
        let mut v = input();
        v["version"] = "2.1.100".into();
        assert!(parse(&v).is_err());
        let mut v = input();
        v["session_id"] = "not-uuid".into();
        assert!(parse(&v).is_err());
    }
}

use crate::journal::{
    open_private_file_nofollow, read_private_json, validate_component_path,
    write_private_json_atomic,
};
use crate::owner::{
    try_lock, AdvisoryLock, OwnerRecord, OwnerStore, RuntimeObservation, StartReservation,
};
use crate::state::OwnerState;
use crate::team_claude_launch::{exact_tuple, valid_selector};
use crate::team_process::{self, ProcessMetadata};
use crate::team_replacement::{ProcessIdentity, ProcessState};
use crate::teams::{classify_managed_seat, ManagedSeatState, TeamSnapshot};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

fn digest(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}
fn hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn bounded_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, ClaudeError> {
    let f = open_private_file_nofollow(path).map_err(|_| {
        if matches!(std::fs::symlink_metadata(path), Err(e) if e.kind() == std::io::ErrorKind::NotFound) { ClaudeError::Missing } else { ClaudeError::Unsafe }
    })?;
    let mut bytes = Vec::new();
    f.take(8193)
        .read_to_end(&mut bytes)
        .map_err(|_| ClaudeError::Io)?;
    if bytes.len() > 8192 {
        return Err(ClaudeError::Invalid);
    }
    serde_json::from_slice(&bytes).map_err(|_| ClaudeError::Invalid)
}
fn absent(path: &Path) -> Result<(), ClaudeError> {
    match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(ClaudeError::Closed),
        Err(_) => Err(ClaudeError::Unsafe),
    }
}
fn runtime_path(home: &Path, seat: &str, generation: u64, suffix: &str) -> PathBuf {
    home.join(".aperture/run")
        .join(format!("{seat}.g{generation}.claude-{suffix}.json"))
}
fn snapshot(
    home: &Path,
    team: &str,
    seat: &str,
    requested: &ExecutionTuple,
) -> Result<(u64, String), ClaudeError> {
    let generation = match classify_managed_seat(home, seat).map_err(|_| ClaudeError::Owner)? {
        Some(ManagedSeatState::Active {
            team: t,
            generation,
        }) if t == team => generation,
        _ => return Err(ClaudeError::Owner),
    };
    let dir = validate_component_path(&home.join(".aperture/teams"), team, false)
        .map_err(|_| ClaudeError::Unsafe)?;
    let f = open_private_file_nofollow(&dir.join("team.json")).map_err(|_| ClaudeError::Unsafe)?;
    let mut bytes = Vec::new();
    f.take(1_048_577)
        .read_to_end(&mut bytes)
        .map_err(|_| ClaudeError::Io)?;
    if bytes.len() > 1_048_576 {
        return Err(ClaudeError::Invalid);
    }
    let value: TeamSnapshot = serde_json::from_slice(&bytes).map_err(|_| ClaudeError::Invalid)?;
    let seats: Vec<_> = value.seats.iter().filter(|s| s.name == seat).collect();
    if value.team != team || seats.len() != 1 {
        return Err(ClaudeError::Owner);
    }
    let configured = ExecutionTuple {
        harness: seats[0].harness.clone(),
        model: seats[0].model.clone(),
        reasoning: seats[0].reasoning.clone(),
    };
    if configured != *requested && !value.fallbacks.contains(requested) {
        return Err(ClaudeError::Model);
    }
    Ok((generation, digest(&bytes)))
}
/// Drop the seat guard before the team guard. No public command accepts this
/// context, process evidence, attempt or receipt as input.
struct Locked {
    _seat: AdvisoryLock,
    _team: AdvisoryLock,
    owner: OwnerRecord,
    snapshot_sha256: String,
    team_generation: u64,
}
fn locked(
    home: &Path,
    team: &str,
    seat: &str,
    generation: Option<u64>,
) -> Result<Locked, ClaudeError> {
    valid_selector(team, seat, generation.unwrap_or(1))?;
    let team_guard =
        try_lock(&home.join(".aperture/run/team-locks"), team).map_err(|_| ClaudeError::Owner)?;
    let store = OwnerStore::new(home.join(".aperture/run/owner"));
    let seat_guard = store.lock(seat).map_err(|_| ClaudeError::Owner)?;
    let owner: OwnerRecord =
        read_private_json(&store.record_path(seat)).map_err(|_| ClaudeError::Owner)?;
    validate_owner(&owner, seat, generation)?;
    let (team_generation, snapshot_sha256) = snapshot(home, team, seat, &owner.requested)?;
    Ok(Locked {
        _seat: seat_guard,
        _team: team_guard,
        owner,
        snapshot_sha256,
        team_generation,
    })
}
fn validate_owner(
    owner: &OwnerRecord,
    seat: &str,
    generation: Option<u64>,
) -> Result<(), ClaudeError> {
    exact_tuple(&owner.requested)?;
    let c = owner.incarnation.as_ref().ok_or(ClaudeError::Owner)?;
    if owner.schema_version != 1
        || owner.seat != seat
        || owner.generation == 0
        || generation.is_some_and(|g| g != owner.generation)
        || owner.state != OwnerState::Starting
        || !owner.reservation_nonce_sha256.as_deref().is_some_and(hash)
        || owner.provisional_token_id.as_deref() != Some(c.token_id.as_str())
        || !hash(&c.token_id)
        || c.harness != owner.requested.harness
        || c.model != owner.requested.model
        || c.reasoning != owner.requested.reasoning
        || (!c.observed && !c.thread_id.is_empty())
        || (c.observed && !canonical_uuid(&c.thread_id))
        || !c
            .processes
            .iter()
            .any(|p| p.pid == c.pid && p.start_time == c.start_time)
    {
        return Err(ClaudeError::Owner);
    }
    Ok(())
}
fn current(
    home: &Path,
    team: &str,
    ctx: &Locked,
    state: &impl Fn(&ProcessIdentity) -> ProcessState,
) -> Result<(), ClaudeError> {
    let o = &ctx.owner;
    let c = o.incarnation.as_ref().ok_or(ClaudeError::Owner)?;
    let store = OwnerStore::new(home.join(".aperture/run/owner"));
    if read_private_json::<OwnerRecord>(&store.record_path(&o.seat))
        .map_err(|_| ClaudeError::Owner)?
        != *o
        || snapshot(home, team, &o.seat, &o.requested)?
            != (ctx.team_generation, ctx.snapshot_sha256.clone())
    {
        return Err(ClaudeError::Owner);
    }
    crate::team_replacement::model_observation::token_current(
        home,
        &o.seat,
        o.generation,
        &c.token_id,
    )
    .map_err(|_| ClaudeError::Revoked)?;
    let identity =
        team_process::identity_from_owner(c.pid, c.start_time).map_err(|_| ClaudeError::Process)?;
    if state(&identity) != ProcessState::Same {
        return Err(ClaudeError::Process);
    }
    Ok(())
}
fn attempt_matches(a: &ClaudeAttempt, ctx: &Locked, team: &str) -> Result<(), ClaudeError> {
    let o = &ctx.owner;
    let c = o.incarnation.as_ref().ok_or(ClaudeError::Owner)?;
    if a.schema_version != 1
        || a.team != team
        || a.seat != o.seat
        || a.generation != o.generation
        || a.team_generation != ctx.team_generation
        || a.snapshot_sha256 != ctx.snapshot_sha256
        || Some(a.reservation_nonce_sha256.as_str()) != o.reservation_nonce_sha256.as_deref()
        || a.token_id != c.token_id
        || a.root_pid != c.pid
        || a.root_start_time_us != c.start_time
        || a.requested_model != o.requested.model
        || !canonical_uuid(&a.session_id)
        || (c.observed && c.thread_id != a.session_id)
    {
        return Err(ClaudeError::Owner);
    }
    Ok(())
}
/// Recovery only: caller holds canonical team and owner locks. This proves the
/// durable attempt binding, NOT startup observation, liveness or a new permit.
pub(crate) fn abandoned_attempt_matches_locked(home: &Path, team: &str, owner: &OwnerRecord) -> Result<(), ClaudeError> {
    validate_owner(owner, &owner.seat, Some(1))?;
    if owner.incarnation.as_ref().is_none_or(|i| i.observed) { return Err(ClaudeError::Owner); }
    let (generation, hash) = snapshot(home, team, &owner.seat, &owner.requested)?;
    let a: ClaudeAttempt = bounded_json(&runtime_path(home, &owner.seat, 1, "attempt"))?;
    let i = owner.incarnation.as_ref().ok_or(ClaudeError::Owner)?;
    if a.schema_version != 1 || a.team != team || a.seat != owner.seat || a.generation != 1
        || a.team_generation != generation || a.snapshot_sha256 != hash
        || Some(a.reservation_nonce_sha256.as_str()) != owner.reservation_nonce_sha256.as_deref()
        || a.token_id != i.token_id || a.root_pid != i.pid || a.root_start_time_us != i.start_time
        || a.requested_model != owner.requested.model || !canonical_uuid(&a.session_id) {
        return Err(ClaudeError::Owner);
    }
    Ok(())
}
/// Launcher-only in-memory reservation. Called AFTER candidate ownership is
/// durable and BEFORE releasing the harness gate; no PID/session from a DTO.
pub(crate) fn record_attempt(
    home: &Path,
    team: &str,
    reservation: &StartReservation,
    session_uuid: &str,
) -> Result<(), ClaudeError> {
    record_checked(
        home,
        team,
        reservation,
        session_uuid,
        &team_process::state,
        chrono::Utc::now().timestamp_millis(),
    )
}
fn record_checked(
    home: &Path,
    team: &str,
    reservation: &StartReservation,
    session_uuid: &str,
    state: &impl Fn(&ProcessIdentity) -> ProcessState,
    now: i64,
) -> Result<(), ClaudeError> {
    if !canonical_uuid(session_uuid) || now <= 0 {
        return Err(ClaudeError::Invalid);
    }
    let ctx = locked(home, team, &reservation.seat, Some(reservation.generation))?;
    let o = &ctx.owner;
    let c = o.incarnation.as_ref().ok_or(ClaudeError::Owner)?;
    if c.observed
        || o.reservation_nonce_sha256.as_deref()
            != Some(digest(reservation.nonce().as_bytes()).as_str())
    {
        return Err(ClaudeError::Owner);
    }
    current(home, team, &ctx, state)?;
    let a = ClaudeAttempt {
        schema_version: 1,
        team: team.into(),
        seat: o.seat.clone(),
        generation: o.generation,
        reservation_nonce_sha256: o
            .reservation_nonce_sha256
            .clone()
            .ok_or(ClaudeError::Owner)?,
        snapshot_sha256: ctx.snapshot_sha256.clone(),
        team_generation: ctx.team_generation,
        token_id: c.token_id.clone(),
        root_pid: c.pid,
        root_start_time_us: c.start_time,
        session_id: session_uuid.into(),
        requested_model: o.requested.model.clone(),
        created_at_ms: now,
    };
    write_private_json_atomic(
        &runtime_path(home, &o.seat, o.generation, "attempt"),
        &a,
        false,
    )
    .map_err(|_| ClaudeError::Unsafe)
}
/// Observe the helper's own ancestry, not stdin/env claims. Every link and the
/// exact root birth are read twice; reparent/recycle/unreadable never succeeds.
fn ancestor_chain(
    root: &ProcessIdentity,
    pid: u32,
    observe: &impl Fn(u32) -> Result<Option<ProcessMetadata>, ClaudeError>,
) -> Result<Vec<ProcessMetadata>, ClaudeError> {
    let mut chain = Vec::new();
    let mut cursor = pid;
    for _ in 0..64 {
        if cursor <= 1
            || chain
                .iter()
                .any(|p: &ProcessMetadata| p.identity.pid == cursor)
        {
            return Err(ClaudeError::Process);
        }
        let p = observe(cursor)?.ok_or(ClaudeError::Process)?;
        if p.identity.pid != cursor || p.uid != unsafe { libc::geteuid() } {
            return Err(ClaudeError::Process);
        }
        let reached = p.identity == *root;
        if cursor == root.pid && !reached {
            return Err(ClaudeError::Process);
        }
        cursor = p.ppid;
        chain.push(p);
        if reached {
            return Ok(chain);
        }
    }
    Err(ClaudeError::Process)
}
fn ancestry(
    root: &ProcessIdentity,
    pid: u32,
    observe: &impl Fn(u32) -> Result<Option<ProcessMetadata>, ClaudeError>,
) -> Result<(), ClaudeError> {
    let a = ancestor_chain(root, pid, observe)?;
    let b = ancestor_chain(root, pid, observe)?;
    if a.len() != b.len()
        || a.iter().zip(&b).any(|(a, b)| {
            a.identity != b.identity || a.ppid != b.ppid || a.uid != b.uid || a.pgid != b.pgid
        })
    {
        return Err(ClaudeError::Process);
    }
    Ok(())
}
/// Fixed helper entrypoint. Reads a bounded payload before taking locks; no
/// payload identity, path, token, generation or claimed ancestry is accepted.
/// Publication itself closes the first-receipt window by no-replace.
pub(crate) fn write_startup_observation(
    home: &Path,
    team: &str,
    seat: &str,
    reader: impl Read,
) -> Result<(), ClaudeError> {
    let input = sample(reader);
    write_checked(
        home,
        team,
        seat,
        input,
        std::process::id(),
        &|p| team_process::observe(p).map_err(|_| ClaudeError::Process),
        &team_process::state,
        chrono::Utc::now().timestamp_millis(),
    )
}
fn write_checked(
    home: &Path,
    team: &str,
    seat: &str,
    input: Result<StartupSample, ClaudeError>,
    pid: u32,
    observe: &impl Fn(u32) -> Result<Option<ProcessMetadata>, ClaudeError>,
    state: &impl Fn(&ProcessIdentity) -> ProcessState,
    now: i64,
) -> Result<(), ClaudeError> {
    let ctx = locked(home, team, seat, None)?;
    let o = &ctx.owner;
    let a: ClaudeAttempt = bounded_json(&runtime_path(home, seat, o.generation, "attempt"))?;
    attempt_matches(&a, &ctx, team)?;
    let root = team_process::identity_from_owner(a.root_pid, a.root_start_time_us)
        .map_err(|_| ClaudeError::Process)?;
    ancestry(&root, pid, observe)?;
    let observed_path = runtime_path(home, seat, o.generation, "observation");
    let rejected_path = runtime_path(home, seat, o.generation, "rejected");
    absent(&observed_path)?;
    absent(&rejected_path)?;
    let projected = input.and_then(|v| project(&a, v, now));
    current(home, team, &ctx, state)?;
    ancestry(&root, pid, observe)?;
    // The first ancestry/owner-authenticated payload closes the window even
    // when malformed, post-input or mismatched. A later good sample cannot
    // erase the failed attempt. Never retain raw payload in rejection facts.
    match projected {
        Ok(receipt) => write_private_json_atomic(&observed_path, &receipt, false)
            .map_err(|_| ClaudeError::Closed),
        Err(error) => {
            let fact = serde_json::json!({"schema_version":1,"attempt_sha256":digest(&serde_json::to_vec(&a).map_err(|_|ClaudeError::Invalid)?),"code":error.code(),"at_ms":now});
            write_private_json_atomic(&rejected_path, &fact, false)
                .map_err(|_| ClaudeError::Closed)?;
            Err(error)
        }
    }
}
/// Native-only return: cannot deserialize or manufacture a caller observation.
pub(crate) struct VerifiedClaudeObservation(RuntimeObservation);
impl VerifiedClaudeObservation {
    pub(crate) fn into_runtime_observation(self) -> RuntimeObservation {
        self.0
    }
}
pub(crate) fn read_native(
    home: &Path,
    team: &str,
    reservation: &StartReservation,
) -> Result<VerifiedClaudeObservation, ClaudeError> {
    read_checked(
        home,
        team,
        reservation,
        &team_process::state,
        chrono::Utc::now().timestamp_millis(),
        || {},
    )
}
fn read_checked(
    home: &Path,
    team: &str,
    reservation: &StartReservation,
    state: &impl Fn(&ProcessIdentity) -> ProcessState,
    now: i64,
    before_recheck: impl FnOnce(),
) -> Result<VerifiedClaudeObservation, ClaudeError> {
    let ctx = locked(home, team, &reservation.seat, Some(reservation.generation))?;
    let o = &ctx.owner;
    if o.reservation_nonce_sha256.as_deref()
        != Some(digest(reservation.nonce().as_bytes()).as_str())
    {
        return Err(ClaudeError::Owner);
    }
    let a: ClaudeAttempt = bounded_json(&runtime_path(home, &o.seat, o.generation, "attempt"))?;
    attempt_matches(&a, &ctx, team)?;
    absent(&runtime_path(home, &o.seat, o.generation, "rejected"))?;
    let r: ClaudeReceipt = bounded_json(&runtime_path(home, &o.seat, o.generation, "observation"))?;
    let since = chrono::DateTime::parse_from_rfc3339(&o.since)
        .map_err(|_| ClaudeError::Owner)?
        .timestamp_millis();
    if r.schema_version != 1
        || r.attempt_sha256 != digest(&serde_json::to_vec(&a).map_err(|_| ClaudeError::Invalid)?)
        || r.seat != a.seat
        || r.generation != a.generation
        || r.token_id != a.token_id
        || r.root_pid != a.root_pid
        || r.root_start_time_us != a.root_start_time_us
        || r.session_id != a.session_id
        || r.actual()? != o.requested
        || a.created_at_ms < since
        || r.observed_at_ms < a.created_at_ms
        || r.observed_at_ms > now
        || now < a.created_at_ms
        || now - a.created_at_ms > WINDOW_MS
    {
        return Err(ClaudeError::Invalid);
    }
    before_recheck();
    current(home, team, &ctx, state)?;
    let actual = r.actual()?;
    Ok(VerifiedClaudeObservation(RuntimeObservation {
        pid: r.root_pid,
        start_time: r.root_start_time_us,
        token_id: r.token_id,
        thread_id: r.session_id,
        actual,
    }))
}

#[cfg(test)]
mod native_tests {
    use super::*;
    use crate::journal::{ensure_private_dir, write_private_bytes_atomic};
    use crate::owner::{Incarnation, ProcessIdentity as StoredProcess};
    use crate::team_auth::AuthenticatedActor;
    use serde_json::{json, Value};
    use std::os::unix::fs::{symlink, PermissionsExt};
    struct Fixture {
        home: PathBuf,
        store: OwnerStore,
        reservation: StartReservation,
        session: String,
        now: i64,
    }
    impl Fixture {
        fn new() -> Self {
            let home = std::env::temp_dir().join(format!(
                "aperture-claude-observation-{}",
                uuid::Uuid::new_v4()
            ));
            let dir = home.join(".aperture/teams/t1");
            ensure_private_dir(&dir).unwrap();
            write_private_json_atomic(&dir.join("team.json"),&json!({"schema_version":1,"team":"t1","project":"project:aperture","repo":"aperture","mission":"synthetic","acceptance":"synthetic","preset":{"id":null,"sha256":null},"lead":"t1-worker","seats":[{"name":"t1-worker","role":"backend","harness":"claude","model":MODEL,"reasoning":null}],"fallbacks":[],"grants":[],"created_at":"2026-09-20T00:00:00Z","creation_request_id":uuid::Uuid::new_v4().to_string(),"staging_uuid":uuid::Uuid::new_v4().to_string()}),false).unwrap();
            write_private_json_atomic(&dir.join("state.json"),&json!({"schema_version":1,"state":"active","generation":1,"epic_id":"aperture-fixture","failure":null,"updated_at":"2026-09-20T00:00:00Z"}),false).unwrap();
            let seat = home.join(".claude/aperture/t1-worker");
            ensure_private_dir(&seat).unwrap();
            for name in ["TEAM", ".complete"] {
                write_private_bytes_atomic(&seat.join(name), b"", false).unwrap();
            }
            let store = OwnerStore::new(home.join(".aperture/run/owner"));
            let actor = AuthenticatedActor::launcher();
            let tuple = ExecutionTuple {
                harness: Harness::Claude,
                model: MODEL.into(),
                reasoning: None,
            };
            store
                .initialize_owner(&actor, "t1-worker", tuple.clone())
                .unwrap();
            let reservation = store.reserve_start(&actor, "t1-worker", 0, tuple).unwrap();
            let token =
                crate::hub_auth::managed::provision(&home, "t1", &actor, &reservation).unwrap();
            store
                .record_start_candidate(
                    &actor,
                    &reservation,
                    Incarnation {
                        pid: 900001,
                        start_time: 1_790_000_000_000_001,
                        thread_id: String::new(),
                        token_id: token.token_id().into(),
                        harness: Harness::Claude,
                        model: MODEL.into(),
                        reasoning: None,
                        observed: false,
                        processes: vec![StoredProcess {
                            pid: 900001,
                            start_time: 1_790_000_000_000_001,
                            ppid: 1,
                            pgid: 900001,
                            cmdline_sha256: "c".repeat(64),
                            cwd: "/fixture".into(),
                        }],
                    },
                )
                .unwrap();
            let f = Self {
                home,
                store,
                reservation,
                session: uuid::Uuid::new_v4().to_string(),
                now: chrono::Utc::now().timestamp_millis(),
            };
            record_checked(
                &f.home,
                "t1",
                &f.reservation,
                &f.session,
                &|_| ProcessState::Same,
                f.now,
            )
            .unwrap();
            f
        }
        fn owner(&self) -> OwnerRecord {
            self.store.read_owner("t1-worker").unwrap()
        }
        fn input(&self) -> Value {
            json!({"session_id":self.session,"version":"2.1.263","model":{"id":MODEL},"context_window":{"current_usage":null,"total_input_tokens":0,"total_output_tokens":0}})
        }
        fn path(&self, suffix: &str) -> PathBuf {
            runtime_path(&self.home, "t1-worker", 1, suffix)
        }
        fn observe(pid: u32) -> Result<Option<ProcessMetadata>, ClaudeError> {
            let (birth, parent) = match pid {
                900003 => (1_790_000_000_000_003, 900002),
                900002 => (1_790_000_000_000_002, 900001),
                900001 => (1_790_000_000_000_001, 1),
                _ => return Ok(None),
            };
            Ok(Some(ProcessMetadata {
                identity: team_process::identity_from_owner(pid, birth).unwrap(),
                ppid: parent,
                pgid: 900001,
                uid: unsafe { libc::geteuid() },
            }))
        }
        fn write(&self) -> Result<(), ClaudeError> {
            self.write_value(self.input())
        }
        fn write_value(&self, value: Value) -> Result<(), ClaudeError> {
            let input = sample(serde_json::to_vec(&value).unwrap().as_slice());
            write_checked(
                &self.home,
                "t1",
                "t1-worker",
                input,
                900003,
                &Self::observe,
                &|_| ProcessState::Same,
                self.now + 1,
            )
        }
        fn read(&self) -> Result<VerifiedClaudeObservation, ClaudeError> {
            read_checked(
                &self.home,
                "t1",
                &self.reservation,
                &|_| ProcessState::Same,
                self.now + 2,
                || {},
            )
        }
        fn alter(&self, suffix: &str, key: &str, value: Value) {
            let mut v: Value = read_private_json(&self.path(suffix)).unwrap();
            v[key] = value;
            write_private_json_atomic(&self.path(suffix), &v, true).unwrap();
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.home).unwrap();
        }
    }
    #[test]
    fn native_first_receipt_is_append_only_and_shared_owner_cas_retains_literal_none() {
        let f = Fixture::new();
        let before = f.owner();
        f.write().unwrap();
        let bytes = std::fs::read(f.path("observation")).unwrap();
        assert_eq!(f.write(), Err(ClaudeError::Closed));
        assert_eq!(std::fs::read(f.path("observation")).unwrap(), bytes);
        assert!(record_checked(
            &f.home,
            "t1",
            &f.reservation,
            &uuid::Uuid::new_v4().to_string(),
            &|_| ProcessState::Same,
            f.now
        )
        .is_err());
        let observed = f.read().unwrap().into_runtime_observation();
        assert_eq!(f.owner(), before);
        assert_eq!(observed.actual, before.requested);
        assert_eq!(observed.actual.reasoning, None);
        assert_eq!(observed.thread_id, f.session);
        let actor = AuthenticatedActor::launcher();
        f.store
            .record_runtime_observation(&actor, &f.reservation, observed)
            .unwrap();
        assert_eq!(
            f.read().unwrap().into_runtime_observation().thread_id,
            f.session
        );
        f.store.commit_start(&actor, &f.reservation).unwrap();
        assert!(matches!(f.read(), Err(ClaudeError::Owner)));
        assert!(matches!(f.write(), Err(ClaudeError::Owner)));
    }
    #[test]
    fn native_input_identity_model_or_preturn_failure_publishes_nothing() {
        for field in [
            "prompt_id",
            "session_id",
            "model",
            "generation",
            "context_window",
        ] {
            let f = Fixture::new();
            let mut v = f.input();
            v[field] = match field {
                "session_id" => json!(uuid::Uuid::new_v4().to_string()),
                "model" => json!({"id":"sonnet"}),
                _ => json!(null),
            };
            assert!(f.write_value(v).is_err(), "{field}");
            assert!(!f.path("observation").exists());
            assert!(f.path("rejected").exists());
            assert_eq!(f.write(), Err(ClaudeError::Closed));
        }
    }
    #[test]
    fn ancestry_requires_stable_uid_every_parent_and_exact_root_birth() {
        let root = team_process::identity_from_owner(900001, 1_790_000_000_000_001).unwrap();
        for bad in 0..5 {
            let counter = std::cell::Cell::new(0);
            let observe = |pid| {
                let mut p = Fixture::observe(pid)?.unwrap();
                counter.set(counter.get() + 1);
                match bad {
                    0 if pid == 900001 => p.identity.start_time = "wrong".into(),
                    1 if pid == 900002 => p.uid = p.uid + 1,
                    2 if pid == 900002 => p.ppid = 1,
                    3 if pid == 900002 => p.ppid = 900003,
                    4 if counter.get() > 3 => p.pgid = 900010,
                    _ => {}
                }
                Ok(Some(p))
            };
            assert_eq!(ancestry(&root, 900003, &observe), Err(ClaudeError::Process));
        }
        let f = Fixture::new();
        assert!(write_checked(
            &f.home,
            "t1",
            "t1-worker",
            sample(serde_json::to_vec(&f.input()).unwrap().as_slice()),
            900099,
            &Fixture::observe,
            &|_| ProcessState::Same,
            f.now
        )
        .is_err());
        assert!(!f.path("observation").exists());
    }
    #[test]
    fn reader_rejects_attempt_or_receipt_identity_drift_without_owner_write() {
        for (suffix, key, value) in [
            ("attempt", "team", json!("other")),
            ("attempt", "snapshot_sha256", json!("d".repeat(64))),
            (
                "attempt",
                "session_id",
                json!(uuid::Uuid::new_v4().to_string()),
            ),
            ("attempt", "team_generation", json!(2)),
            ("attempt", "reservation_nonce_sha256", json!("e".repeat(64))),
            ("observation", "attempt_sha256", json!("d".repeat(64))),
            ("observation", "root_pid", json!(900002)),
            ("observation", "root_start_time_us", json!(2)),
            ("observation", "actual_model", json!("sonnet")),
            ("observation", "reasoning_observation", json!("high")),
            ("observation", "generation", json!(2)),
            ("observation", "unexpected", json!(true)),
        ] {
            let f = Fixture::new();
            f.write().unwrap();
            let before = f.owner();
            f.alter(suffix, key, value);
            assert!(f.read().is_err(), "{suffix}/{key}");
            assert_eq!(f.owner(), before);
        }
    }
    #[test]
    fn snapshot_owner_token_or_process_drift_blocks_both_native_boundaries() {
        for bad in 0..5 {
            let f = Fixture::new();
            f.write().unwrap();
            match bad {
                0 => {
                    let path = f.home.join(".aperture/teams/t1/team.json");
                    let mut v: Value = read_private_json(&path).unwrap();
                    v["mission"] = json!("changed");
                    write_private_json_atomic(&path, &v, true).unwrap();
                }
                1 => {
                    let mut o = f.owner();
                    o.generation = 2;
                    write_private_json_atomic(&f.store.record_path("t1-worker"), &o, true).unwrap();
                }
                2 => {
                    let mut o = f.owner();
                    o.incarnation.as_mut().unwrap().start_time += 1;
                    write_private_json_atomic(&f.store.record_path("t1-worker"), &o, true).unwrap();
                }
                3 => {
                    let p = f.home.join(".aperture/run/hub-tokens/t1-worker.token");
                    write_private_bytes_atomic(&p, &vec![b'f'; 64], true).unwrap();
                }
                _ => {
                    let p = f.home.join(".aperture/run/revocations");
                    ensure_private_dir(&p).unwrap();
                    write_private_json_atomic(&p.join("t1-worker.json"),&json!({"schema_version":1,"seat":"t1-worker","revoked_through_generation":1,"revoked_token_ids":[f.owner().incarnation.unwrap().token_id]}),false).unwrap();
                }
            }
            assert!(f.read().is_err());
            assert!(f.write().is_err());
        }
        for state in [
            ProcessState::Gone,
            ProcessState::Recycled,
            ProcessState::Unreadable,
        ] {
            let f = Fixture::new();
            f.write().unwrap();
            assert!(read_checked(
                &f.home,
                "t1",
                &f.reservation,
                &|_| state.clone(),
                f.now + 2,
                || {}
            )
            .is_err());
        }
    }
    #[test]
    fn immutable_paths_reject_missing_corrupt_links_permissions_and_oversize() {
        for bad in 0..6 {
            let f = Fixture::new();
            f.write().unwrap();
            match bad {
                0 => std::fs::remove_file(f.path("observation")).unwrap(),
                1 => write_private_bytes_atomic(&f.path("observation"), b"{", true).unwrap(),
                2 => {
                    std::fs::remove_file(f.path("observation")).unwrap();
                    symlink(f.path("attempt"), f.path("observation")).unwrap();
                }
                3 => std::fs::hard_link(f.path("observation"), f.home.join("extra")).unwrap(),
                4 => std::fs::set_permissions(
                    f.path("observation"),
                    std::fs::Permissions::from_mode(0o644),
                )
                .unwrap(),
                _ => write_private_bytes_atomic(&f.path("observation"), &vec![b' '; 8193], true)
                    .unwrap(),
            }
            assert!(f.read().is_err());
        }
    }
    #[test]
    fn concurrent_first_samples_publish_one_terminal_fact_without_overwrite() {
        let f = Fixture::new();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let handles: Vec<_> = (0..2)
            .map(|_| {
                let home = f.home.clone();
                let value = f.input();
                let now = f.now + 1;
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    write_checked(
                        &home,
                        "t1",
                        "t1-worker",
                        sample(serde_json::to_vec(&value).unwrap().as_slice()),
                        900003,
                        &Fixture::observe,
                        &|_| ProcessState::Same,
                        now,
                    )
                })
            })
            .collect();
        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
        assert!(f.read().is_ok());
        assert!(!f.path("rejected").exists());
    }
    #[test]
    fn absent_or_unsafe_attempt_and_closed_rejection_never_become_readiness() {
        let f = Fixture::new();
        std::fs::remove_file(f.path("attempt")).unwrap();
        assert!(matches!(f.write(), Err(ClaudeError::Missing)));
        assert!(!f.path("observation").exists());
        let f = Fixture::new();
        f.write().unwrap();
        write_private_bytes_atomic(&f.path("rejected"), b"{", false).unwrap();
        assert!(matches!(f.read(), Err(ClaudeError::Closed)));
        let f = Fixture::new();
        std::fs::set_permissions(f.path("attempt"), std::fs::Permissions::from_mode(0o644))
            .unwrap();
        assert!(matches!(f.write(), Err(ClaudeError::Unsafe)));
        assert!(!f.path("observation").exists());
    }
    #[test]
    fn native_clock_window_and_under_lock_recheck_do_not_return_stale_proof() {
        let f = Fixture::new();
        f.write().unwrap();
        for now in [f.now - 1, f.now + WINDOW_MS + 1] {
            assert!(read_checked(
                &f.home,
                "t1",
                &f.reservation,
                &|_| ProcessState::Same,
                now,
                || {}
            )
            .is_err());
        }
        assert!(read_checked(
            &f.home,
            "t1",
            &f.reservation,
            &|_| ProcessState::Same,
            f.now + 2,
            || {
                let mut o = f.owner_unlocked();
                o.generation += 1;
                write_private_json_atomic(&f.store.record_path("t1-worker"), &o, true).unwrap();
            }
        )
        .is_err());
    }
    impl Fixture {
        fn owner_unlocked(&self) -> OwnerRecord {
            read_private_json(&self.store.record_path("t1-worker")).unwrap()
        }
    }
}

/// Internal release/exec seam. Holds the same team→seat locks while a native
/// callback publishes release or validates its fixed receipt. Never a DTO.
pub(crate) fn with_gated_attempt<T>(
    home: &Path,
    team: &str,
    seat: &str,
    generation: u64,
    f: impl FnOnce(&ClaudeAttempt) -> Result<T, ClaudeError>,
) -> Result<T, ClaudeError> {
    let ctx = locked(home, team, seat, Some(generation))?;
    if ctx.owner.incarnation.as_ref().is_some_and(|c| c.observed) {
        return Err(ClaudeError::Closed);
    }
    let a: ClaudeAttempt = bounded_json(&runtime_path(home, seat, generation, "attempt"))?;
    attempt_matches(&a, &ctx, team)?;
    absent(&runtime_path(home, seat, generation, "observation"))?;
    absent(&runtime_path(home, seat, generation, "rejected"))?;
    let now = chrono::Utc::now().timestamp_millis();
    if a.created_at_ms <= 0 || now < a.created_at_ms || now - a.created_at_ms > WINDOW_MS {
        return Err(ClaudeError::Closed);
    }
    current(home, team, &ctx, &team_process::state)?;
    f(&a)
}
