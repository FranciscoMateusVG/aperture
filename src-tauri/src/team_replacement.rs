//! V4 stop-before-replace orchestration. No token/owner/window creation occurs
//! until every preparation gate passes. Native effects are supplied by the
//! existing launcher/owner/hub adapters, never by a second broker.
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessIdentity {
    pub pid: u32,
    /// OS process birth identity, not elapsed time or a user-supplied PID.
    pub start_time: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnedProcess {
    pub identity: ProcessIdentity,
    pub parent_pid: u32,
    pub process_group: u32,
    pub depth: u32,
    /// Private metadata only; adapters must not log these fields.
    pub cmdline_sha256: String,
    pub cwd: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OwnershipSnapshot {
    pub seat: String,
    pub generation: u64,
    pub thread_id: String,
    pub processes: Vec<OwnedProcess>,
    /// Includes persisted descendants when the original pane process died.
    pub complete: bool,
    pub unowned_matches: Vec<ProcessIdentity>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessState {
    Same,
    Gone,
    Recycled,
    Unreadable,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    Term,
    Kill,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnState {
    Busy,
    Idle,
    Dead,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointRecovery {
    Valid,
    Stale,
    None,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteResolution {
    Finished,
    Cancelled,
    Unknown,
}
#[derive(Debug, Clone)]
pub struct RemoteEffect {
    pub reference: String,
    pub resolution: RemoteResolution,
}
#[derive(Debug, Clone)]
pub struct RemoteInventory {
    pub effects: Vec<RemoteEffect>,
    /// True only for adapters that durably track admission/result. Empty
    /// transcript/checkpoint metadata MUST NOT set this true.
    pub complete_observation: bool,
    /// Native append-only authorization projection. It never changes observed
    /// effect states or complete_observation and is not caller-deserializable.
    pub(crate) explicit_resolution: Option<remote::RemoteProjection>,
}
#[derive(Debug, Clone)]
pub struct RevocationProof {
    pub generation: u64,
    pub durable: bool,
    pub sockets_closed: bool,
    pub close_code: u16,
    pub close_elapsed_ms: u64,
    pub reconnect_code: u16,
    pub token_deleted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplacementPhase {
    Snapshot,
    CheckpointPending,
    Stopping,
    Revoking,
    Reconciling,
    Ready,
    Starting,
    Started,
    ModelUnverified,
    Blocked,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplacementError {
    GenerationMismatch,
    InvalidSnapshot,
    UnownedProcess,
    StopUnverified,
    RevocationUnverified,
    RemoteUncertain,
    AuthorizationRequired,
    FreshThreadUnverified,
    ModelUnverified,
    StartCleanupUnverified,
    NativeFailure,
    RepoBindingUnavailable,
    CheckpointUnavailable,
    LaunchUnavailable,
    Deadline,
    OutcomeUnknown,
    PreparationExpired,
    WorktreeUnbound,
}
impl ReplacementError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::GenerationMismatch => "E_GENERATION_MISMATCH",
            Self::InvalidSnapshot | Self::StopUnverified => "E_STOP_UNVERIFIED",
            Self::UnownedProcess => "E_UNOWNED_PROCESS",
            Self::RevocationUnverified => "E_REVOCATION_UNVERIFIED",
            Self::RemoteUncertain => "E_REMOTE_UNCERTAIN",
            Self::AuthorizationRequired => "E_REPLACEMENT_AUTHORIZATION",
            Self::FreshThreadUnverified => "E_FRESH_THREAD_UNVERIFIED",
            Self::ModelUnverified => "E_MODEL_UNVERIFIED",
            Self::StartCleanupUnverified => "E_START_CLEANUP_UNVERIFIED",
            Self::NativeFailure => "E_RUNTIME_IO",
            Self::RepoBindingUnavailable => "E_REPO_BINDING_UNAVAILABLE",
            Self::CheckpointUnavailable => "E_CHECKPOINT_UNAVAILABLE",
            Self::LaunchUnavailable => "E_LAUNCH_UNAVAILABLE",
            Self::Deadline => "E_RUNTIME_DEADLINE",
            Self::OutcomeUnknown => "E_CONTROL_UNKNOWN",
            Self::PreparationExpired => "E_PREPARATION_EXPIRED",
            Self::WorktreeUnbound => "E_WORKTREE_UNBOUND",
        }
    }
}

pub struct ReplacementPolicy {
    pub checkpoint_ms: u64,
    pub term_ms: u64,
    pub kill_verify_ms: u64,
    pub poll_ms: u64,
}
impl Default for ReplacementPolicy {
    fn default() -> Self {
        Self {
            checkpoint_ms: 90_000,
            term_ms: 10_000,
            kill_verify_ms: 1_000,
            poll_ms: 100,
        }
    }
}
#[derive(Debug, Clone)]
pub struct StartSelection {
    pub harness: String,
    pub model: String,
    pub reasoning: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartedReplacement {
    pub generation: u64,
    pub thread_id: String,
    pub requested_model: String,
    pub actual_model: Option<String>,
    pub model_verified: bool,
}

#[derive(Debug, Clone)]
pub struct StartedCandidate {
    pub observed: StartedReplacement,
    pub actual_harness: Option<String>,
    pub actual_reasoning: Option<String>,
    pub process: ProcessIdentity,
    /// Opaque id, never the bearer token.
    pub token_id: String,
}

/// Deliberately not Deserialize or Clone: UI cannot mint a ready permit, and a
/// successful preparation may be consumed once. The native owner adapter must
/// additionally hold its OS lock and compare generation at launch/commit.
#[derive(Debug)]
pub struct PreparedReplacement {
    snapshot: OwnershipSnapshot,
    recovery: CheckpointRecovery,
}
impl PreparedReplacement {
    pub fn seat(&self) -> &str {
        &self.snapshot.seat
    }
    pub fn generation(&self) -> u64 {
        self.snapshot.generation
    }
    pub fn checkpoint_recovery(&self) -> CheckpointRecovery {
        self.recovery
    }
}

pub trait ReplacementRuntime {
    fn now_ms(&self) -> u64;
    fn wait_ms(&mut self, ms: u64);
    fn event(&mut self, phase: ReplacementPhase);
    fn expected_owner(&mut self, seat: &str, generation: u64) -> Result<(), ReplacementError>;
    fn snapshot(
        &mut self,
        seat: &str,
        generation: u64,
    ) -> Result<OwnershipSnapshot, ReplacementError>;
    fn turn_state(&self) -> TurnState;
    /// Timestamp only for a native 429/rate_limit event; never inferred from idle.
    fn rate_limit_at_ms(&self) -> Option<u64>;
    fn request_checkpoint(&mut self) -> Result<(), ReplacementError>;
    fn checkpoint_recovery(&mut self) -> CheckpointRecovery;
    fn process_state(&mut self, process: &ProcessIdentity) -> ProcessState;
    fn signal(&mut self, process: &ProcessIdentity, signal: Signal)
        -> Result<(), ReplacementError>;
    /// Recheck cwd/cmdline matches outside the recorded owned set. Never kill them.
    fn unowned_matches(&mut self, snapshot: &OwnershipSnapshot) -> Result<bool, ReplacementError>;
    fn revoke(&mut self, snapshot: &OwnershipSnapshot)
        -> Result<RevocationProof, ReplacementError>;
    fn revocation_still_valid(
        &mut self,
        snapshot: &OwnershipSnapshot,
    ) -> Result<bool, ReplacementError>;
    fn remote_effects(
        &mut self,
        snapshot: &OwnershipSnapshot,
    ) -> Result<RemoteInventory, ReplacementError>;
    fn authorize_selection(
        &mut self,
        snapshot: &OwnershipSnapshot,
        selection: &StartSelection,
    ) -> Result<(), ReplacementError>;
    /// Existing native launcher only. Must allocate a NEW thread (not newest),
    /// NEW token, compare expected generation under owner lock, then record the
    /// actual model from the harness response/first assistant message.
    fn start_fresh(
        &mut self,
        snapshot: &OwnershipSnapshot,
        selection: &StartSelection,
    ) -> Result<StartedCandidate, ReplacementError>;
    /// Only after actual model verification; existing owner reservation/CAS.
    fn activate_started(&mut self, candidate: &StartedCandidate) -> Result<(), ReplacementError>;
    /// Stop exact NEW pid/start-time, revoke its generation/token durably,
    /// invalidate the active/starting owner and record the failed attempt.
    fn abort_started(&mut self, candidate: &StartedCandidate) -> Result<(), ReplacementError>;
}

fn validate_snapshot(
    s: &OwnershipSnapshot,
    seat: &str,
    generation: u64,
) -> Result<(), ReplacementError> {
    if s.seat != seat || s.generation != generation {
        return Err(ReplacementError::GenerationMismatch);
    }
    if !s.complete || s.processes.is_empty() || s.thread_id.is_empty() {
        return Err(ReplacementError::InvalidSnapshot);
    }
    if !s.unowned_matches.is_empty() {
        return Err(ReplacementError::UnownedProcess);
    }
    let mut ids = HashSet::new();
    for p in &s.processes {
        if p.identity.pid <= 1
            || p.identity.start_time.is_empty()
            || p.cmdline_sha256.len() != 64
            || !p.cmdline_sha256.bytes().all(|b| b.is_ascii_hexdigit())
            || !p.cwd.starts_with('/')
            || p.cwd.chars().any(char::is_control)
            || !ids.insert(p.identity.pid)
        {
            return Err(ReplacementError::InvalidSnapshot);
        }
    }
    Ok(())
}
fn surviving<R: ReplacementRuntime>(
    r: &mut R,
    s: &OwnershipSnapshot,
) -> Result<Vec<ProcessIdentity>, ReplacementError> {
    let mut live = Vec::new();
    for p in &s.processes {
        match r.process_state(&p.identity) {
            ProcessState::Same => live.push(p.identity.clone()),
            ProcessState::Gone => {}
            ProcessState::Recycled | ProcessState::Unreadable => {
                return Err(ReplacementError::StopUnverified)
            }
        }
    }
    Ok(live)
}
fn wait_for_stop<R: ReplacementRuntime>(
    r: &mut R,
    s: &OwnershipSnapshot,
    duration: u64,
    poll: u64,
) -> Result<Vec<ProcessIdentity>, ReplacementError> {
    let deadline = r.now_ms().saturating_add(duration);
    loop {
        let live = surviving(r, s)?;
        if live.is_empty() || r.now_ms() >= deadline {
            return Ok(live);
        }
        r.wait_ms(poll.min(deadline - r.now_ms()));
    }
}
fn reconcile<R: ReplacementRuntime>(
    r: &mut R,
    s: &OwnershipSnapshot,
) -> Result<(), ReplacementError> {
    if r.unowned_matches(s)? {
        return Err(ReplacementError::UnownedProcess);
    }
    let remote = r.remote_effects(s)?;
    let resolved = remote.explicit_resolution.as_ref()
        .is_some_and(|p| p.permits(s.generation, &remote.effects, remote.complete_observation));
    if remote.effects.iter().any(|e| e.reference.is_empty())
        || (!resolved && (!remote.complete_observation
            || remote.effects.iter().any(|e| e.resolution == RemoteResolution::Unknown)))
    {
        return Err(ReplacementError::RemoteUncertain);
    }
    Ok(())
}

pub fn prepare<R: ReplacementRuntime>(
    r: &mut R,
    seat: &str,
    generation: u64,
    policy: &ReplacementPolicy,
) -> Result<PreparedReplacement, ReplacementError> {
    let result = (|| {
        if policy.poll_ms == 0 {
            return Err(ReplacementError::NativeFailure);
        }
        r.expected_owner(seat, generation)?;
        r.event(ReplacementPhase::Snapshot);
        let mut snapshot = r.snapshot(seat, generation)?;
        validate_snapshot(&snapshot, seat, generation)?;
        // Check EVERY identity before the first signal; a recycled/unreadable
        // entry cannot cause us to partially kill a different known-good tree.
        surviving(r, &snapshot)?;
        if r.unowned_matches(&snapshot)? {
            return Err(ReplacementError::UnownedProcess);
        }
        let recent_limit = r
            .rate_limit_at_ms()
            .map(|at| at <= r.now_ms() && r.now_ms() - at <= 60_000)
            .unwrap_or(false);
        if r.turn_state() != TurnState::Dead && (r.turn_state() == TurnState::Busy || recent_limit)
        {
            r.event(ReplacementPhase::CheckpointPending);
            // A missing/broken hook is not permission to hang recovery forever.
            let requested = r.request_checkpoint().is_ok();
            let deadline = r.now_ms().saturating_add(policy.checkpoint_ms);
            while requested
                && r.checkpoint_recovery() != CheckpointRecovery::Valid
                && r.now_ms() < deadline
            {
                r.wait_ms(policy.poll_ms.min(deadline - r.now_ms()));
            }
        }
        let mut recovery = r.checkpoint_recovery();
        if recent_limit && recovery == CheckpointRecovery::Valid {
            recovery = CheckpointRecovery::Stale;
        }
        r.expected_owner(seat, generation)?;
        // A process can appear or recycle during the checkpoint window. Recheck
        // the entire ownership observation before the first destructive signal.
        surviving(r, &snapshot)?;
        if r.unowned_matches(&snapshot)? {
            return Err(ReplacementError::UnownedProcess);
        }
        snapshot.processes.sort_by(|a, b| {
            b.depth
                .cmp(&a.depth)
                .then(a.identity.pid.cmp(&b.identity.pid))
        });
        r.event(ReplacementPhase::Stopping);
        for p in &snapshot.processes {
            match r.process_state(&p.identity) {
                ProcessState::Same => r.signal(&p.identity, Signal::Term)?,
                ProcessState::Gone => {}
                _ => return Err(ReplacementError::StopUnverified),
            }
        }
        let live = wait_for_stop(r, &snapshot, policy.term_ms, policy.poll_ms)?;
        for pid in live {
            match r.process_state(&pid) {
                ProcessState::Same => r.signal(&pid, Signal::Kill)?,
                ProcessState::Gone => {}
                _ => return Err(ReplacementError::StopUnverified),
            }
        }
        if !wait_for_stop(r, &snapshot, policy.kill_verify_ms, policy.poll_ms)?.is_empty() {
            return Err(ReplacementError::StopUnverified);
        }
        if r.unowned_matches(&snapshot)? {
            return Err(ReplacementError::UnownedProcess);
        }
        r.event(ReplacementPhase::Revoking);
        let proof = r.revoke(&snapshot)?;
        if proof.generation != generation
            || !proof.durable
            || !proof.sockets_closed
            || proof.close_code != 4001
            || proof.close_elapsed_ms > 1_000
            || proof.reconnect_code != 4003
            || !proof.token_deleted
        {
            return Err(ReplacementError::RevocationUnverified);
        }
        r.event(ReplacementPhase::Reconciling);
        reconcile(r, &snapshot)?;
        r.event(ReplacementPhase::Ready);
        Ok(PreparedReplacement { snapshot, recovery })
    })();
    if result.is_err() {
        r.event(ReplacementPhase::Blocked);
    }
    result
}

pub fn start<R: ReplacementRuntime>(
    r: &mut R,
    prepared: PreparedReplacement,
    selection: &StartSelection,
) -> Result<StartedReplacement, ReplacementError> {
    let result = (|| {
        let s = &prepared.snapshot;
        r.expected_owner(&s.seat, s.generation)?;
        if !surviving(r, s)?.is_empty() {
            return Err(ReplacementError::StopUnverified);
        }
        if !r.revocation_still_valid(s)? {
            return Err(ReplacementError::RevocationUnverified);
        }
        reconcile(r, s)?;
        r.authorize_selection(s, selection)?;
        let next_generation = s
            .generation
            .checked_add(1)
            .ok_or(ReplacementError::GenerationMismatch)?;
        r.event(ReplacementPhase::Starting);
        let mut candidate = r.start_fresh(s, selection)?;
        let observed = &candidate.observed;
        let identity_error = if observed.generation != next_generation
            || observed.thread_id.is_empty()
            || observed.thread_id == s.thread_id
            || candidate.token_id.is_empty()
            || candidate.process.pid <= 1
            || candidate.process.start_time.is_empty()
        {
            Some(ReplacementError::FreshThreadUnverified)
        } else if observed.actual_model.as_deref() != Some(selection.model.as_str())
            || candidate.actual_harness.as_deref() != Some(selection.harness.as_str())
            || candidate.actual_reasoning != selection.reasoning
        {
            Some(ReplacementError::ModelUnverified)
        } else {
            None
        };
        if let Some(error) = identity_error {
            r.event(ReplacementPhase::ModelUnverified);
            r.abort_started(&candidate)
                .map_err(|_| ReplacementError::StartCleanupUnverified)?;
            return Err(error);
        }
        candidate.observed.requested_model = selection.model.clone();
        candidate.observed.model_verified = true;
        if let Err(error) = r.activate_started(&candidate) {
            r.abort_started(&candidate)
                .map_err(|_| ReplacementError::StartCleanupUnverified)?;
            return Err(error);
        }
        r.event(ReplacementPhase::Started);
        Ok(candidate.observed)
    })();
    if result.is_err() {
        r.event(ReplacementPhase::Blocked);
    }
    result
}

#[cfg(test)]
#[path = "team_runtime_tests.rs"]
mod runtime_tests;

#[path = "team_remote_native.rs"]
pub(crate) mod remote;

#[path = "team_launch_gate.rs"]
pub(crate) mod launch_gate;

#[path = "team_model_observation.rs"]
pub(crate) mod model_observation;

#[path = "team_replacement_native.rs"]
pub(crate) mod native;

#[path = "team_repository_native.rs"]
pub(crate) mod repository;

#[path = "team_runtime_deadline.rs"]
pub(crate) mod deadline;

#[path = "team_launch_native.rs"]
pub(crate) mod launch;
