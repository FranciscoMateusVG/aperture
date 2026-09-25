//! Concrete native composition, deliberately NOT registered as a command.
//! Missing repository authority fails before collection or lifecycle effects.
//! Neither project labels, worker checkpoints nor observed cwd are authority.
use super::*;
use crate::journal::{read_private_json, validate_component_path};
use crate::owner::{Incarnation, OwnerRecord, OwnerStore, StartReservation};
use crate::state::{ExecutionTuple, Harness, OwnerState};
use crate::team_auth::{AuthenticatedActor, AuthenticatedSeat};
use crate::team_process;
use crate::teams::TeamSnapshot;
use std::path::Path;
use std::time::{Duration, Instant};

enum NativePlan {
    Retirement,
    Codex(launch::NativeLaunchBinding),
    Claude(crate::team_claude_launch::ClaudeBinding),
    ClaudeSmoke(crate::team_claude_launch::ClaudeBinding, SmokeAdmission),
    /// Explicit recovery of a normal Claude bootstrap that ended Quarantined
    /// unobserved at g1, or at g2 after one such recovery failed the same way.
    /// Same normal binding; admission is a native proof, never a caller field,
    /// and never a retry. Nothing later than g2 is recoverable.
    ClaudeRecovery(crate::team_claude_launch::ClaudeBinding, RecoveryAdmission),
}
impl NativePlan {
    fn preflight(
        home: &Path,
        team: &str,
        seat: &str,
        tuple: &ExecutionTuple,
        repo: &repository::BoundRepository,
        checkpoint: Option<&crate::team_checkpoint::CheckpointEntry>,
        budget: &deadline::Deadline,
    ) -> Result<Self, ReplacementError> {
        Self::preflight_selected(
            home,
            team,
            seat,
            tuple,
            repo,
            checkpoint.map(|e| e.payload.worktree.as_str()),
            budget,
        )
    }
    fn preflight_selected(
        home: &Path,
        team: &str,
        seat: &str,
        tuple: &ExecutionTuple,
        repo: &repository::BoundRepository,
        worktree: Option<&str>,
        budget: &deadline::Deadline,
    ) -> Result<Self, ReplacementError> {
        if !crate::teams::managed_execution_enabled(tuple) {
            return Err(ReplacementError::LaunchUnavailable);
        }
        match tuple.harness {
            Harness::Codex => launch::NativeLaunchBinding::preflight_selected(
                home, team, seat, tuple, repo, worktree, budget,
            )
            .map(Self::Codex),
            Harness::Claude => crate::team_claude_launch::ClaudeBinding::preflight_normal(
                home, team, seat, tuple, repo, worktree, budget,
            )
            .map(Self::Claude)
            .map_err(|_| ReplacementError::LaunchUnavailable),
        }
    }
    fn revalidate(&self, budget: &deadline::Deadline) -> Result<(), ReplacementError> {
        if matches!(self, Self::Retirement) { return Err(ReplacementError::AuthorizationRequired); }
        if let Self::ClaudeSmoke(binding, admission) = self {
            admission.revalidate()?;
            return binding.revalidate(budget).map_err(|_| ReplacementError::LaunchUnavailable);
        }
        if let Self::ClaudeRecovery(binding, admission) = self {
            admission.revalidate()?;
            return binding.revalidate(budget).map_err(|_| ReplacementError::LaunchUnavailable);
        }
        let harness = match self { Self::Retirement => return Err(ReplacementError::AuthorizationRequired), Self::Codex(_) => Harness::Codex, Self::Claude(_) | Self::ClaudeSmoke(..) | Self::ClaudeRecovery(..) => Harness::Claude };
        if !crate::teams::managed_launch_enabled(&harness) {
            return Err(ReplacementError::LaunchUnavailable);
        }
        match self {
            Self::Retirement | Self::ClaudeSmoke(..) | Self::ClaudeRecovery(..) => Err(ReplacementError::AuthorizationRequired),
            Self::Codex(p) => p.revalidate(budget),
            Self::Claude(p) => p
                .revalidate(budget)
                .map_err(|_| ReplacementError::LaunchUnavailable),
        }
    }
    fn bind_recovery(
        &mut self,
        e: Option<&crate::team_checkpoint::CheckpointEntry>,
    ) -> Result<(), ReplacementError> {
        match self {
            Self::Retirement | Self::ClaudeSmoke(..) | Self::ClaudeRecovery(..) => Err(ReplacementError::AuthorizationRequired),
            Self::Codex(p) => p.bind_recovery(e),
            Self::Claude(p) => p
                .bind_recovery(e)
                .map_err(|_| ReplacementError::CheckpointUnavailable),
        }
    }
}
fn runtime_observation(
    home: &Path,
    team: &str,
    res: &StartReservation,
    harness: &Harness,
) -> Result<Option<crate::owner::RuntimeObservation>, ReplacementError> {
    match harness {
        Harness::Codex => match model_observation::read_native(home, team, res) {
            Ok(v) => Ok(Some(v.into_runtime_observation())),
            Err(model_observation::ObservationError::Missing) => Ok(None),
            Err(_) => Err(ReplacementError::ModelUnverified),
        },
        Harness::Claude => match crate::team_claude_observation::read_native(home, team, res) {
            Ok(v) => Ok(Some(v.into_runtime_observation())),
            Err(crate::team_claude_launch::ClaudeError::Missing | crate::team_claude_launch::ClaudeError::Busy) => Ok(None),
            Err(_) => Err(ReplacementError::ModelUnverified),
        },
    }
}
/// Native ordering seam used by the Claude branch. No public DTO or proof.
/// Any failure retains candidate identity for the same exact cleanup path.
fn candidate_then_claude_release(
    store: &OwnerStore,
    actor: &AuthenticatedActor,
    res: &StartReservation,
    candidate: Incarnation,
    attempt: impl FnOnce() -> Result<(), ReplacementError>,
    release: impl FnOnce() -> Result<(), ReplacementError>,
) -> Result<(), ReplacementError> {
    store
        .record_start_candidate(actor, res, candidate)
        .map_err(|_| ReplacementError::OutcomeUnknown)?;
    attempt()?;
    release()
}
struct NativeStarted {
    reservation: StartReservation,
    harness: Harness,
    child: Option<launch_gate::ReleasedChild>,
    candidate: StartedCandidate,
}
// Only native UI authority or the current authenticated GLaDOS capability may
// bootstrap; a launcher/worker or a caller-supplied principal is not authority.
fn authorize_bootstrap(actor: &AuthenticatedActor) -> Result<(), ReplacementError> {
    if actor.is_glados() {
        actor.revalidate_before_mutation().map_err(|_| ReplacementError::AuthorizationRequired)
    } else if actor.principal() == "operator" {
        Ok(())
    } else {
        Err(ReplacementError::AuthorizationRequired)
    }
}

/// Recovery is GLaDOS-only: the operator UI arm of `authorize_bootstrap` does
/// not apply, and the capability is revalidated again under every lock.
fn authorize_recovery(actor: &AuthenticatedActor) -> Result<(), ReplacementError> {
    if !actor.is_glados() { return Err(ReplacementError::AuthorizationRequired); }
    actor.revalidate_before_mutation().map_err(|_| ReplacementError::AuthorizationRequired)
}
/// Bounded projection of the private launch record (`claude-launch.json`) written by
/// `team_claude_launch`; read-only, never re-serialized, never a launch input.
#[derive(serde::Deserialize)]
struct LaunchFacts {
    schema_version: u32,
    team: String,
    seat: String,
    generation: u64,
    session_id: String,
    token_id: String,
    snapshot_sha256: String,
    #[serde(default)]
    mode: crate::team_claude_launch::ClaudeLaunchMode,
}
/// Bounded projection of the gate release (`claude-release.json`). Limit: its
/// `launch_sha256` is the digest of the private `LaunchRecord` serialization
/// and is NOT recomputed here; the release is bound through `attempt_sha256`
/// (same producer as the observation receipt) and the root identity, and the
/// launch record is bound separately by session, token and snapshot digest.
#[derive(serde::Deserialize)]
struct ReleaseFacts {
    schema_version: u32,
    attempt_sha256: String,
    root_pid: u32,
    root_start_time_us: u64,
}
fn absent(path: &Path) -> Result<(), ReplacementError> {
    match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        _ => Err(ReplacementError::OutcomeUnknown),
    }
}
/// Native proof that one seat is exactly the recoverable failure at
/// `generation` (1 = the first normal Claude bootstrap, 2 = its first explicit
/// recovery; nothing later): LaunchRecord AND ClaudeAttempt NormalPositional at
/// that generation, owner Quarantined there with an unobserved incarnation,
/// every recorded process Gone, no hub token, revocation floor exactly that
/// generation for the same token digest, no reservation nonce, the previous
/// bootstrap admission (g0, or the g1 recovery admission) expired/Unknown, and
/// no next-generation or same-generation runtime-attempt artefact yet. Issued
/// only by the GLaDOS-authenticated recovery entry under team then seat locks;
/// never deserialized, never a caller field, and it does not grant a second
/// attempt.
pub(crate) struct RecoveryAdmission {
    home: std::path::PathBuf,
    team: String,
    seat: String,
    /// The quarantined generation being recovered (1 or 2); the reserve mints
    /// exactly `generation + 1`.
    generation: u64,
    snapshot_sha256: String,
    token_id: String,
    /// Set once the deadline admission for THIS recovery exists; the
    /// pre-reserve proof accepts only that attempt's open `g1`.
    attempt_id: Option<String>,
}
/// Which `runtime-attempts/<seat>/g<generation>` state the proof must find.
/// Before the deadline admission nothing may exist there (one admission ever);
/// before the reserve exactly the current attempt must be open (admitted +
/// effects, no terminal). Neither phase accepts another attempt.
#[derive(Clone, Copy)]
pub(crate) enum RecoveryPhase<'a> {
    PreAdmission,
    PreReserve { attempt_id: &'a str },
}
impl RecoveryAdmission {
    fn issue(home: &Path, actor: &AuthenticatedActor, team: &str, seat: &str, generation: u64)
        -> Result<Self, ReplacementError> {
        authorize_recovery(actor)?;
        let admission = Self::issue_checked(home, team, seat, generation, || authorize_recovery(actor))?;
        authorize_recovery(actor)?;
        Ok(admission)
    }
    /// Locked proof without the caller's authority: `authorize` runs again
    /// inside the locks. Used by `issue` and, in tests, with an inert authority.
    /// Only generations 1 and 2 are recoverable; anything else is refused
    /// before any lock or read (no generic restart).
    fn issue_checked(home: &Path, team: &str, seat: &str, generation: u64,
        authorize: impl Fn() -> Result<(), ReplacementError>) -> Result<Self, ReplacementError> {
        if generation != 1 && generation != 2 {
            return Err(ReplacementError::GenerationMismatch);
        }
        if !crate::teams::managed_launch_enabled(&Harness::Claude) {
            return Err(ReplacementError::LaunchUnavailable);
        }
        if team.len() > 16 || !crate::agent_loader::is_valid_seat_name(team)
            || !crate::agent_loader::is_valid_seat_name(seat) {
            return Err(ReplacementError::AuthorizationRequired);
        }
        let _team = crate::owner::try_lock(&home.join(".aperture/run/team-locks"), team)
            .map_err(|_| ReplacementError::NativeFailure)?;
        let store = OwnerStore::new(home.join(".aperture/run/owner"));
        let _seat = store.lock(seat).map_err(|_| ReplacementError::NativeFailure)?;
        authorize()?;
        let owner = store.read_owner_locked(seat).map_err(|_| ReplacementError::GenerationMismatch)?;
        let (snapshot_sha256, token_id) = Self::proof_locked(home, team, seat, &owner, generation, RecoveryPhase::PreAdmission)?;
        Ok(Self { home: home.into(), team: team.into(), seat: seat.into(), generation, snapshot_sha256, token_id, attempt_id: None })
    }
    /// Bind this admission to the deadline attempt just admitted for it.
    fn bind_attempt(mut self, attempt_id: &str) -> Self {
        self.attempt_id = Some(attempt_id.into());
        self
    }
    /// Full owner proof against the locked owner for one phase at the
    /// quarantined `generation` (1 or 2). Returns the typed snapshot digest and
    /// the incarnation token id it was proved for.
    fn proof_locked(home: &Path, team: &str, seat: &str, owner: &OwnerRecord, generation: u64, phase: RecoveryPhase<'_>)
        -> Result<(String, String), ReplacementError> {
        use sha2::{Digest, Sha256};
        use std::io::Read;
        if generation != 1 && generation != 2 {
            return Err(ReplacementError::GenerationMismatch);
        }
        let next = generation + 1;
        match crate::teams::classify_managed_seat(home, seat)
            .map_err(|_| ReplacementError::AuthorizationRequired)? {
            Some(crate::teams::ManagedSeatState::Active { team: t, .. }) if t == team => {},
            _ => return Err(ReplacementError::AuthorizationRequired),
        }
        let team_dir = home.join(".aperture/teams").join(team);
        let mut raw = Vec::new();
        crate::journal::open_private_file_nofollow(&team_dir.join("team.json"))
            .map_err(|_| ReplacementError::AuthorizationRequired)?
            .take(1_048_577).read_to_end(&mut raw).map_err(|_| ReplacementError::AuthorizationRequired)?;
        if raw.len() > 1_048_576 { return Err(ReplacementError::AuthorizationRequired); }
        let snapshot: TeamSnapshot = serde_json::from_slice(&raw).map_err(|_| ReplacementError::AuthorizationRequired)?;
        let raw_sha = format!("{:x}", Sha256::digest(&raw));
        let typed_sha = smoke_hash(&snapshot)?;
        let seats: Vec<_> = snapshot.seats.iter().filter(|s| s.name == seat).collect();
        if snapshot.schema_version != 1 || snapshot.team != team || seats.len() != 1 {
            return Err(ReplacementError::AuthorizationRequired);
        }
        let tuple = ExecutionTuple { harness: seats[0].harness.clone(), model: seats[0].model.clone(), reasoning: seats[0].reasoning.clone() };
        if tuple.harness != Harness::Claude || !crate::teams::managed_execution_enabled(&tuple) {
            return Err(ReplacementError::LaunchUnavailable);
        }
        let state: serde_json::Value = read_private_json(&team_dir.join("state.json"))
            .map_err(|_| ReplacementError::AuthorizationRequired)?;
        let team_generation = match (state.get("state").and_then(|v| v.as_str()), state.get("generation").and_then(|v| v.as_u64())) {
            (Some("active"), Some(g)) if g > 0 => g,
            _ => return Err(ReplacementError::AuthorizationRequired),
        };
        // Owner: exactly the quarantined, unobserved recovered generation.
        let inc = owner.incarnation.as_ref().ok_or(ReplacementError::GenerationMismatch)?;
        if owner.schema_version != 1 || owner.seat != seat || owner.generation != generation
            || owner.state != OwnerState::Quarantined
            || owner.reservation_nonce_sha256.is_some()
            || owner.requested != tuple
            || inc.observed || !inc.thread_id.is_empty()
            || inc.harness != tuple.harness || inc.model != tuple.model || inc.reasoning != tuple.reasoning
            || !owner.provisional_token_id.as_deref().is_none_or(|p| p == inc.token_id)
            || inc.processes.is_empty() || inc.processes.len() > 256
            || !inc.processes.iter().any(|p| p.pid == inc.pid && p.start_time == inc.start_time) {
            return Err(ReplacementError::GenerationMismatch);
        }
        // Every recorded process, root included, must be Gone. No collector:
        // the collector admits only Starting/Active owners and this proof holds
        // the same locks it would need. Live, recycled or unreadable is a stop.
        for p in &inc.processes {
            if team_process::state(&team_process::identity_from_owner(p.pid, p.start_time)?) != ProcessState::Gone {
                return Err(ReplacementError::StopUnverified);
            }
        }
        // Token gone, floor exactly this generation for this incarnation's token digest.
        absent(&home.join(".aperture/run/hub-tokens").join(format!("{seat}.token")))
            .map_err(|_| ReplacementError::RevocationUnverified)?;
        crate::ws_hub::managed_control::verify_floor(home, seat, generation, &inc.token_id)
            .map_err(|_| ReplacementError::RevocationUnverified)?;
        // Previous admission (g0 for a g1 recovery, the g1 recovery admission for
        // a g2 recovery): expired bootstrap with effects and an Unknown/absent
        // terminal, never reconciled. Read-only; no handle is kept or renewed.
        deadline::expired_bootstrap_locked(home, team, seat, generation - 1)?;
        // Same-generation facts: the category comes from BOTH the attempt and the
        // launch record being NormalPositional and bound to this owner, never
        // from the tuple.
        let run = home.join(".aperture/run");
        let attempt: crate::team_claude_launch::ClaudeAttempt =
            read_private_json(&run.join(format!("{seat}.g{generation}.claude-attempt.json")))
                .map_err(|_| ReplacementError::OutcomeUnknown)?;
        if attempt.schema_version != 1 || attempt.team != team || attempt.seat != seat || attempt.generation != generation
            || attempt.mode != crate::team_claude_launch::ClaudeLaunchMode::NormalPositional
            || attempt.team_generation != team_generation || attempt.snapshot_sha256 != raw_sha
            || attempt.token_id != inc.token_id || attempt.root_pid != inc.pid || attempt.root_start_time_us != inc.start_time
            || attempt.requested_model != tuple.model
            || !crate::team_claude_launch::canonical_uuid(&attempt.session_id) {
            return Err(ReplacementError::OutcomeUnknown);
        }
        let managed = run.join("managed").join(seat).join(format!("g{generation}"));
        let launch: LaunchFacts = read_private_json(&managed.join("claude-launch.json"))
            .map_err(|_| ReplacementError::OutcomeUnknown)?;
        if launch.schema_version != 1 || launch.team != team || launch.seat != seat || launch.generation != generation
            || launch.mode != crate::team_claude_launch::ClaudeLaunchMode::NormalPositional
            || launch.session_id != attempt.session_id || launch.token_id != inc.token_id
            || launch.snapshot_sha256 != typed_sha {
            return Err(ReplacementError::OutcomeUnknown);
        }
        let release: ReleaseFacts = read_private_json(&managed.join("claude-release.json"))
            .map_err(|_| ReplacementError::OutcomeUnknown)?;
        if release.schema_version != 1 || release.attempt_sha256 != smoke_hash(&attempt)?
            || release.root_pid != inc.pid || release.root_start_time_us != inc.start_time {
            return Err(ReplacementError::OutcomeUnknown);
        }
        // Unobserved means neither an observation nor a proven rejection exists.
        absent(&run.join(format!("{seat}.g{generation}.claude-observation.json")))?;
        absent(&run.join(format!("{seat}.g{generation}.claude-rejected.json")))?;
        // Nothing of the next generation may exist. The same-generation
        // runtime-attempt is phase-dependent: absent before the deadline
        // admission (one admission ever), exactly the current attempt (admitted
        // + effects, no terminal) before the reserve.
        absent(&run.join(format!("{seat}.g{next}.claude-attempt.json")))?;
        absent(&run.join("managed").join(seat).join(format!("g{next}")))?;
        match phase {
            RecoveryPhase::PreAdmission => absent(&team_dir.join("runtime-attempts").join(seat).join(format!("g{generation}")))?,
            RecoveryPhase::PreReserve { attempt_id } => deadline::recovery_attempt_open_locked(home, team, seat, generation, attempt_id)?,
        }
        Ok((typed_sha, inc.token_id.clone()))
    }
    /// Re-run the full owner proof under the caller's locks for one phase. The
    /// owner must still be the proved one (same snapshot, same token digest).
    fn reprove_locked(&self, store: &OwnerStore, phase: RecoveryPhase<'_>) -> Result<(), ReplacementError> {
        let owner = store.read_owner_locked(&self.seat).map_err(|_| ReplacementError::GenerationMismatch)?;
        let (snapshot_sha256, token_id) = Self::proof_locked(&self.home, &self.team, &self.seat, &owner, self.generation, phase)?;
        if snapshot_sha256 != self.snapshot_sha256 || token_id != self.token_id {
            return Err(ReplacementError::GenerationMismatch);
        }
        Ok(())
    }
    fn matches_target(&self, home: &Path, team: &str, seat: &str, old_generation: u64) -> Result<(), ReplacementError> {
        if self.home != home || self.team != team || self.seat != seat || old_generation != self.generation {
            return Err(ReplacementError::AuthorizationRequired);
        }
        Ok(())
    }
    fn revalidate(&self) -> Result<(), ReplacementError> {
        let _team = crate::owner::try_lock(&self.home.join(".aperture/run/team-locks"), &self.team)
            .map_err(|_| ReplacementError::NativeFailure)?;
        self.revalidate_locked()
    }
    /// Immutable context only (valid after the reserve): same active seat, same
    /// sealed snapshot, same exact Claude tuple. Owner state is not re-required.
    fn revalidate_locked(&self) -> Result<(), ReplacementError> {
        match crate::teams::classify_managed_seat(&self.home, &self.seat)
            .map_err(|_| ReplacementError::AuthorizationRequired)? {
            Some(crate::teams::ManagedSeatState::Active { team, .. }) if team == self.team => {},
            _ => return Err(ReplacementError::AuthorizationRequired),
        }
        let snapshot: TeamSnapshot = read_private_json(&self.home.join(".aperture/teams").join(&self.team).join("team.json"))
            .map_err(|_| ReplacementError::AuthorizationRequired)?;
        let seats: Vec<_> = snapshot.seats.iter().filter(|s| s.name == self.seat).collect();
        if snapshot.team != self.team || smoke_hash(&snapshot)? != self.snapshot_sha256 || seats.len() != 1
            || seats[0].harness != Harness::Claude
            || !crate::teams::managed_execution_enabled(&ExecutionTuple { harness: seats[0].harness.clone(), model: seats[0].model.clone(), reasoning: seats[0].reasoning.clone() }) {
            return Err(ReplacementError::AuthorizationRequired);
        }
        Ok(())
    }
}

/// Issued only by the GLaDOS-authenticated diagnostic entry, never deserialized.
/// It is not a switch on the public plan: its snapshot/target remain fixed.
struct SmokeAdmission {
    home: std::path::PathBuf,
    team: String,
    seat: String,
    snapshot_sha256: String,
}
fn smoke_hash<T: serde::Serialize>(value: &T) -> Result<String, ReplacementError> {
    use sha2::{Digest, Sha256};
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(value)
        .map_err(|_| ReplacementError::OutcomeUnknown)?)))
}
fn smoke_tuple(t: &ExecutionTuple) -> bool {
    t.harness == Harness::Claude && t.model == "claude-sonnet-5" && t.reasoning.is_none()
}
fn authorize_smoke(actor: &AuthenticatedActor) -> Result<(), ReplacementError> {
    if !actor.is_glados() { return Err(ReplacementError::AuthorizationRequired); }
    actor.revalidate_before_mutation().map_err(|_| ReplacementError::AuthorizationRequired)
}
impl SmokeAdmission {
    fn issue(home: &Path, actor: &AuthenticatedActor, team: &str, seat: &str,
        generation: u64) -> Result<Self, ReplacementError> {
        authorize_smoke(actor)?;
        if generation != 0 { return Err(ReplacementError::GenerationMismatch); }
        if team.len() > 16 || !crate::agent_loader::is_valid_seat_name(team)
            || !crate::agent_loader::is_valid_seat_name(seat) {
            return Err(ReplacementError::AuthorizationRequired);
        }
        let _team = crate::owner::try_lock(&home.join(".aperture/run/team-locks"), team)
            .map_err(|_| ReplacementError::NativeFailure)?;
        let store = OwnerStore::new(home.join(".aperture/run/owner"));
        let _seat = store.lock(seat).map_err(|_| ReplacementError::NativeFailure)?;
        authorize_smoke(actor)?;
        let snapshot: TeamSnapshot = read_private_json(&home.join(".aperture/teams").join(team).join("team.json"))
            .map_err(|_| ReplacementError::AuthorizationRequired)?;
        let admission = Self { home: home.into(), team: team.into(), seat: seat.into(), snapshot_sha256: smoke_hash(&snapshot)? };
        admission.revalidate_locked()?;
        let owner = store.read_owner_locked(seat).map_err(|_| ReplacementError::GenerationMismatch)?;
        if owner.schema_version != 1 || owner.seat != seat || owner.generation != 0
            || owner.state != OwnerState::Stale || owner.incarnation.is_some()
            || owner.provisional_token_id.is_some() || owner.reservation_nonce_sha256.is_some()
            || !smoke_tuple(&owner.requested) {
            return Err(ReplacementError::GenerationMismatch);
        }
        Ok(admission)
    }
    fn matches_target(&self, home: &Path, team: &str, seat: &str, old_generation: u64) -> Result<(), ReplacementError> {
        if self.home != home || self.team != team || self.seat != seat || old_generation != 0 {
            return Err(ReplacementError::AuthorizationRequired);
        }
        Ok(())
    }
    fn revalidate(&self) -> Result<(), ReplacementError> {
        let _team = crate::owner::try_lock(&self.home.join(".aperture/run/team-locks"), &self.team)
            .map_err(|_| ReplacementError::NativeFailure)?;
        self.revalidate_locked()
    }
    fn revalidate_locked(&self) -> Result<(), ReplacementError> {
        match crate::teams::classify_managed_seat(&self.home, &self.seat)
            .map_err(|_| ReplacementError::AuthorizationRequired)? {
            Some(crate::teams::ManagedSeatState::Active { team, .. }) if team == self.team => {},
            _ => return Err(ReplacementError::AuthorizationRequired),
        }
        let snapshot: TeamSnapshot = read_private_json(&self.home.join(".aperture/teams").join(&self.team).join("team.json"))
            .map_err(|_| ReplacementError::AuthorizationRequired)?;
        let seats: Vec<_> = snapshot.seats.iter().filter(|s| s.name == self.seat).collect();
        if snapshot.team != self.team || smoke_hash(&snapshot)? != self.snapshot_sha256 || seats.len() != 1
            || seats[0].harness != Harness::Claude || seats[0].model != "claude-sonnet-5" || seats[0].reasoning.is_some() {
            return Err(ReplacementError::AuthorizationRequired);
        }
        Ok(())
    }
}

/// Diagnostic only: no worker availability, PID, thread, bearer or raw payload.
/// Successful construction requires consumption of the native teardown proof.
pub(crate) struct ClaudeSmokeDiagnostic {
    pub(crate) team: String,
    pub(crate) seat: String,
    pub(crate) generation: u64,
    pub(crate) actual: ExecutionTuple,
}

/// No caller proof and no reservation reconstruction. Owns the existing
/// process-snapshot locks until revocation and the exact owner CAS complete.
pub(crate) struct StoppedSmokeProof {
    home: std::path::PathBuf,
    team: String,
    expected: OwnerRecord,
    attempt: deadline::UnfinishedBootstrap,
    guard: team_process::PersistedProcessSnapshot,
}
impl StoppedSmokeProof {
    pub(crate) fn verified_owner(&self, root: &Path, actor: &AuthenticatedActor) -> Result<OwnerRecord, ReplacementError> {
        authorize_smoke(actor)?;
        if root != self.home.join(".aperture/run/owner") { return Err(ReplacementError::AuthorizationRequired); }
        self.attempt.revalidate_locked(&self.home)?;
        let current: OwnerRecord = read_private_json(&root.join(format!("{}.json", self.expected.seat)))
            .map_err(|_| ReplacementError::OutcomeUnknown)?;
        if current != self.expected { return Err(ReplacementError::GenerationMismatch); }
        crate::team_claude_observation::abandoned_attempt_matches_locked(&self.home, &self.team, &current)
            .map_err(|_| ReplacementError::OutcomeUnknown)?;
        if self.guard.snapshot().processes.iter().any(|p| team_process::state(&p.identity) != ProcessState::Gone) {
            return Err(ReplacementError::StopUnverified);
        }
        let i = current.incarnation.as_ref().ok_or(ReplacementError::OutcomeUnknown)?;
        crate::ws_hub::managed_control::verify_floor(&self.home, &current.seat, current.generation, &i.token_id)?;
        let token = self.home.join(".aperture/run/hub-tokens").join(format!("{}.token", current.seat));
        if !matches!(std::fs::symlink_metadata(token), Err(e) if e.kind()==std::io::ErrorKind::NotFound) {
            return Err(ReplacementError::RevocationUnverified);
        }
        Ok(current)
    }
}

/// Explicit recovery of an already stopped, unobserved g1 diagnostic. Never
/// signals, spawns, resets, fabricates a nonce or claims startup succeeded.
pub(crate) fn reconcile_stopped_claude_smoke(home: &Path, actor: &AuthenticatedActor,
    team: &str, seat: &str, generation: u64) -> Result<(), ReplacementError> {
    authorize_smoke(actor)?;
    if generation != 1 || crate::teams::managed_launch_enabled(&Harness::Claude)
        || team.len() > 16 || !crate::agent_loader::is_valid_seat_name(team)
        || !crate::agent_loader::is_valid_seat_name(seat) { return Err(ReplacementError::GenerationMismatch); }
    let store = OwnerStore::new(home.join(".aperture/run/owner"));
    let (before, attempt) = {
        let _team = crate::owner::try_lock(&home.join(".aperture/run/team-locks"), team).map_err(|_| ReplacementError::NativeFailure)?;
        let _seat = store.lock(seat).map_err(|_| ReplacementError::NativeFailure)?;
        authorize_smoke(actor)?;
        let before = store.read_owner_locked(seat).map_err(|_| ReplacementError::GenerationMismatch)?;
        crate::team_claude_observation::abandoned_attempt_matches_locked(home, team, &before)
            .map_err(|_| ReplacementError::GenerationMismatch)?;
        let attempt = deadline::UnfinishedBootstrap::read_locked(home, team, seat)?;
        (before, attempt)
    };
    // Fresh collection and exact CAS are the same substrate as native stop;
    // recovery refuses any live/recycled/unreadable member before any mutation.
    let until = Instant::now() + Duration::from_secs(30);
    let snapshot = team_process::native::collect_native_until(home, team, seat, generation, until)?;
    if snapshot.processes.iter().any(|p| team_process::state(&p.identity) != ProcessState::Gone) {
        return Err(ReplacementError::StopUnverified);
    }
    authorize_smoke(actor)?;
    if store.read_owner(seat).map_err(|_| ReplacementError::OutcomeUnknown)? != before {
        return Err(ReplacementError::GenerationMismatch);
    }
    let guard = team_process::persist_for_stop(home, team, &AuthenticatedActor::launcher(), snapshot)?;
    let expected = store.read_owner_locked(seat).map_err(|_| ReplacementError::OutcomeUnknown)?;
    crate::team_claude_observation::abandoned_attempt_matches_locked(home, team, &expected)
        .map_err(|_| ReplacementError::GenerationMismatch)?;
    attempt.revalidate_locked(home)?;
    authorize_smoke(actor)?;
    crate::ws_hub::managed_control::revoke_stopped_before(home, &guard, until)?;
    let proof = StoppedSmokeProof {home:home.into(), team:team.into(), expected, attempt, guard};
    store.quarantine_reconciled_smoke(actor, &proof).map_err(|_| ReplacementError::OutcomeUnknown)?;
    // Same guard still owns both locks. Original UNKNOWN/absent terminal stays
    // untouched; the recovery fact records stop/revocation, not smoke success.
    proof.attempt.record_locked(home)?;
    let owner = store.read_owner_locked(seat).map_err(|_| ReplacementError::OutcomeUnknown)?;
    if owner.generation != generation || owner.state != OwnerState::Quarantined
        || owner.reservation_nonce_sha256.is_some() || owner.provisional_token_id.is_some() {
        return Err(ReplacementError::OutcomeUnknown);
    }
    Ok(())
}

// Shared pre-input admission and launch; both diagnostic paths use the same
// g0 capability, snapshot, D1 and cleanup substrate. No public policy switch.
fn start_claude_diagnostic(home: &Path, actor: &AuthenticatedActor, team: &str,
    seat: &str, expected_generation: u64,
) -> Result<(NativePlan, deadline::RuntimeAttempt, NativeStarted), ReplacementError> {
    let budget = deadline::Deadline::new();
    let admission = SmokeAdmission::issue(home, actor, team, seat, expected_generation)?;
    let repo = repository::resolve_native(home, team, budget.forward_until(Duration::from_secs(10))?)
        .map_err(|_| ReplacementError::RepoBindingUnavailable)?;
    let selected = ExecutionTuple { harness: Harness::Claude, model: "claude-sonnet-5".into(), reasoning: None };
    let binding = crate::team_claude_launch::ClaudeBinding::preflight(home, team, seat, &selected, &repo, None, &budget)
        .map_err(|_| ReplacementError::LaunchUnavailable)?;
    let plan = NativePlan::ClaudeSmoke(binding, admission);
    authorize_smoke(actor)?;
    plan.revalidate(&budget)?;
    let mut attempt = deadline::RuntimeAttempt::begin_bootstrap(home, &AuthenticatedActor::launcher(), team, seat, budget)?;
    attempt.admit_effects()?;
    let started = match start_native(home, team, seat, 0, selected, &plan, &attempt, Some(actor)) {
        Ok(v) => v,
        Err(e) => { let _ = attempt.finish_unknown(); return Err(e); }
    };
    Ok((plan, attempt, started))
}

/// A cleaned diagnostic after ONE fixed kickoff. This is not evidence
/// of a callable MCP, a Monitor hello or a mission-ready worker.
pub(crate) struct ClaudeInboxProbe {
    pub(crate) team: String,
    pub(crate) seat: String,
    pub(crate) generation: u64,
    pub(crate) actual: ExecutionTuple,
}

/// Operator-approved diagnostic only. The ordinary Claude launch gate remains
/// false. The first input is the native constant, strictly AFTER D1/Active.
pub(crate) fn bootstrap_claude_inbox_probe_authorized(home: &Path,
    actor: &AuthenticatedActor, team: &str, seat: &str, expected_generation: u64,
) -> Result<ClaudeInboxProbe, ReplacementError> {
    if crate::teams::managed_launch_enabled(&Harness::Claude) {
        return Err(ReplacementError::AuthorizationRequired);
    }
    let (plan, mut attempt, mut started) = start_claude_diagnostic(home, actor, team, seat, expected_generation)?;
    let NativePlan::ClaudeSmoke(_, admission) = &plan else { unreachable!() };
    let observed = inbox_after_activation(
        || activate_native_checked(home, team, &started, &attempt, Some(actor), Some(admission)),
        || crate::team_claude_kickoff::kickoff_active(home, actor, team, seat,
            started.reservation.generation, attempt.budget().forward_until(Duration::from_secs(15))?),
        || {
            // Keep the ORIGINAL reservation alive, never create a new stop
            // authority. This diagnostic carries no business task. BEADS
            // reply/read-state is inspected externally, not inferred here.
            let until = attempt.budget().forward_until(Duration::from_secs(30))?;
            std::thread::sleep(until.saturating_duration_since(Instant::now()));
            authorize_smoke(actor)
        },
    );
    let completed = smoke_finally(observed, || cleanup_native(home, team,
        &started.reservation, started.child.as_mut(), attempt.budget().cleanup_until()));
    if let Err(e) = completed { let _ = attempt.finish_unknown(); return Err(e); }
    let result = (|| {
        let proof = SmokeCleanupProof::capture(admission, &started, attempt.id(), attempt.budget(), actor)?;
        let actual = proof.actual.clone();
        let generation = proof.generation;
        attempt.finish_smoke_cleaned(proof, actor)?;
        Ok(ClaudeInboxProbe {team:team.into(), seat:seat.into(), generation, actual})
    })();
    if result.is_err() { let _ = attempt.finish_unknown(); }
    result
}

// Production sequence, tested with inert effects: activation failure cannot
// submit input, and an uncertain submission cannot be repeated or readied.
fn inbox_after_activation<T>(activate: impl FnOnce() -> Result<(), ReplacementError>,
    kickoff: impl FnOnce() -> Result<(), ReplacementError>,
    readback: impl FnOnce() -> Result<T, ReplacementError>) -> Result<T, ReplacementError> {
    activate()?;
    kickoff()?;
    readback()
}

pub(crate) fn bootstrap_claude_smoke_authorized(
    home: &Path, actor: &AuthenticatedActor, team: &str, seat: &str,
    expected_generation: u64, _sentinels: &[String],
) -> Result<ClaudeSmokeDiagnostic, ReplacementError> {
    let (plan, mut attempt, mut started) = start_claude_diagnostic(home, actor, team, seat, expected_generation)?;
    let NativePlan::ClaudeSmoke(_, admission) = &plan else { unreachable!() };
    // No prompt, kickoff or tool call. This is startup-only, never mission-ready.
    let observed = authorize_smoke(actor).and_then(|_| activate_native_checked(home, team, &started, &attempt, Some(actor), Some(admission)));
    // Finally uses the ORIGINAL cleanup deadline on every post-start outcome.
    let completed = smoke_finally(observed, || cleanup_native(home, team, &started.reservation,
        started.child.as_mut(), attempt.budget().cleanup_until()));
    if let Err(e) = completed { let _ = attempt.finish_unknown(); return Err(e); }
    let result = (|| {
        let proof = SmokeCleanupProof::capture(admission, &started, attempt.id(), attempt.budget(), actor)?;
        let actual = proof.actual.clone();
        let generation = proof.generation;
        attempt.finish_smoke_cleaned(proof, actor)?;
        Ok(ClaudeSmokeDiagnostic { team: team.into(), seat: seat.into(), generation, actual })
    })();
    if result.is_err() { let _ = attempt.finish_unknown(); }
    result
}

// Even an expired/revoked observation must run exact cleanup. A failed cleanup
// takes precedence: an observation PASS cannot hide an uncertain live process.
fn smoke_finally(observed: Result<(), ReplacementError>, cleanup: impl FnOnce() -> Result<(), ReplacementError>) -> Result<(), ReplacementError> {
    cleanup()?;
    observed
}

/// No serde, no public fields/constructor and no caller-supplied observation.
/// Only the diagnostic's successful native finally can mint this value.
pub(super) struct SmokeCleanupProof {
    home: std::path::PathBuf,
    team: String,
    seat: String,
    generation: u64,
    attempt_id: String,
    snapshot_sha256: String,
    owner_sha256: String,
    actual: ExecutionTuple,
}
impl SmokeCleanupProof {
    fn capture(admission: &SmokeAdmission, started: &NativeStarted, attempt_id: &str,
        budget: &deadline::Deadline, actor: &AuthenticatedActor) -> Result<Self, ReplacementError> {
        let _team = crate::owner::try_lock(&admission.home.join(".aperture/run/team-locks"), &admission.team)
            .map_err(|_| ReplacementError::OutcomeUnknown)?;
        let store = OwnerStore::new(admission.home.join(".aperture/run/owner"));
        let _seat = store.lock(&admission.seat).map_err(|_| ReplacementError::OutcomeUnknown)?;
        authorize_smoke(actor)?;
        admission.revalidate_locked()?;
        let owner = store.read_owner_locked(&admission.seat).map_err(|_| ReplacementError::OutcomeUnknown)?;
        let inc = owner.incarnation.as_ref().ok_or(ReplacementError::OutcomeUnknown)?;
        if started.reservation.seat != admission.seat || started.reservation.generation != 1
            || owner.generation != started.reservation.generation
            || inc.pid != started.candidate.process.pid
            || inc.start_time != team_process::birth_micros(&started.candidate.process)?
            || inc.token_id != started.candidate.token_id
            || inc.thread_id != started.candidate.observed.thread_id {
            return Err(ReplacementError::OutcomeUnknown);
        }
        let proof = Self { home: admission.home.clone(), team: admission.team.clone(), seat: admission.seat.clone(),
            generation: owner.generation, attempt_id: attempt_id.into(), snapshot_sha256: admission.snapshot_sha256.clone(),
            owner_sha256: smoke_hash(&owner)?, actual: ExecutionTuple { harness: inc.harness.clone(), model: inc.model.clone(), reasoning: inc.reasoning.clone() } };
        proof.verify_locked(budget)?;
        Ok(proof)
    }
    fn verify_locked(&self, budget: &deadline::Deadline) -> Result<(), ReplacementError> {
        if Instant::now() >= budget.cleanup_until() { return Err(ReplacementError::Deadline); }
        let owner: OwnerRecord = read_private_json(&self.home.join(".aperture/run/owner").join(format!("{}.json", self.seat)))
            .map_err(|_| ReplacementError::OutcomeUnknown)?;
        let inc = owner.incarnation.as_ref().ok_or(ReplacementError::OutcomeUnknown)?;
        if owner.schema_version != 1 || owner.seat != self.seat || owner.generation != self.generation
            || owner.state != OwnerState::Quarantined || owner.reservation_nonce_sha256.is_some()
            || smoke_hash(&owner)? != self.owner_sha256 || !smoke_tuple(&owner.requested) || !smoke_tuple(&self.actual)
            || !inc.observed || inc.harness != self.actual.harness || inc.model != self.actual.model || inc.reasoning != self.actual.reasoning
            || inc.thread_id.is_empty() || inc.processes.is_empty() || inc.processes.len() > 256
            || !inc.processes.iter().any(|p| p.pid == inc.pid && p.start_time == inc.start_time) {
            return Err(ReplacementError::OutcomeUnknown);
        }
        for p in &inc.processes {
            if team_process::state(&team_process::identity_from_owner(p.pid, p.start_time)?) != ProcessState::Gone {
                return Err(ReplacementError::StartCleanupUnverified);
            }
        }
        let rev: RevokedState = read_private_json(&self.home.join(".aperture/run/revocations").join(format!("{}.json", self.seat)))
            .map_err(|_| ReplacementError::RevocationUnverified)?;
        let mut sorted = rev.revoked_token_ids.clone(); sorted.sort(); sorted.dedup();
        if rev.schema_version != 1 || rev.seat != self.seat || rev.revoked_through_generation != self.generation
            || sorted != rev.revoked_token_ids || sorted.is_empty() || sorted.len() > 4096
            || sorted.iter().any(|s| s.len()!=64 || !s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
            || !sorted.contains(&inc.token_id) {
            return Err(ReplacementError::RevocationUnverified);
        }
        let token = validate_component_path(&self.home.join(".aperture/run/hub-tokens"), &format!("{}.token", self.seat), true)
            .map_err(|_| ReplacementError::RevocationUnverified)?;
        if !matches!(std::fs::symlink_metadata(token), Err(e) if e.kind()==std::io::ErrorKind::NotFound) {
            return Err(ReplacementError::RevocationUnverified);
        }
        match crate::teams::classify_managed_seat(&self.home, &self.seat)
            .map_err(|_| ReplacementError::OutcomeUnknown)? {
            Some(crate::teams::ManagedSeatState::Active { team, .. }) if team == self.team => {},
            _ => return Err(ReplacementError::OutcomeUnknown),
        }
        let snapshot: TeamSnapshot = read_private_json(&self.home.join(".aperture/teams").join(&self.team).join("team.json"))
            .map_err(|_| ReplacementError::OutcomeUnknown)?;
        if smoke_hash(&snapshot)? != self.snapshot_sha256 { return Err(ReplacementError::OutcomeUnknown); }
        if Instant::now() >= budget.cleanup_until() { return Err(ReplacementError::Deadline); }
        Ok(())
    }
    /// Lock order matches owner/attempt publication. No proof bytes leave here.
    pub(super) fn with_revalidated<T>(self, home: &Path, team: &str, seat: &str,
        old_generation: u64, attempt_id: &str, budget: &deadline::Deadline,
        actor: &AuthenticatedActor, publish: impl FnOnce() -> Result<T, ReplacementError>) -> Result<T, ReplacementError> {
        if self.home != home || self.team != team || self.seat != seat || old_generation != 0
            || self.generation != 1 || self.attempt_id != attempt_id { return Err(ReplacementError::OutcomeUnknown); }
        let _team = crate::owner::try_lock(&home.join(".aperture/run/team-locks"), team).map_err(|_| ReplacementError::OutcomeUnknown)?;
        let store = OwnerStore::new(home.join(".aperture/run/owner"));
        let _seat = store.lock(seat).map_err(|_| ReplacementError::OutcomeUnknown)?;
        authorize_smoke(actor)?;
        self.verify_locked(budget)?;
        publish()
    }
}

/// Internal native result; transport must project OwnerSummary and never expose
/// StartedReplacement.thread_id. No caller tuple in the bootstrap command.
pub(crate) fn bootstrap_authorized(
    home: &Path,
    actor: &AuthenticatedActor,
    team: &str,
    seat: &str,
    expected_generation: u64,
) -> Result<StartedReplacement, ReplacementError> {
    let budget = deadline::Deadline::new();
    authorize_bootstrap(actor)?;
    // 0 = first start. 1 | 2 = explicit recovery of the quarantined, unobserved
    // owner at that generation (a first bootstrap, or one recovery that failed
    // the same way). 3 and above are never selectors: no generic restart.
    match expected_generation {
        0 => {}
        1 | 2 => return bootstrap_recovery_authorized(home, actor, team, seat, expected_generation, budget),
        _ => return Err(ReplacementError::GenerationMismatch),
    }
    let repo =
        repository::resolve_native(home, team, budget.forward_until(Duration::from_secs(10))?)
            .map_err(|_| ReplacementError::RepoBindingUnavailable)?;
    let snapshot: TeamSnapshot =
        read_private_json(&home.join(".aperture/teams").join(team).join("team.json"))
            .map_err(|_| ReplacementError::AuthorizationRequired)?;
    let seats: Vec<_> = snapshot.seats.iter().filter(|s| s.name == seat).collect();
    if seats.len() != 1 {
        return Err(ReplacementError::AuthorizationRequired);
    }
    let s = seats[0];
    let selected = ExecutionTuple {
        harness: s.harness.clone(),
        model: s.model.clone(),
        reasoning: s.reasoning.clone(),
    };
    let plan =
        NativePlan::preflight(home, team, seat, &selected, &repo, None, &budget)?;
    authorize_bootstrap(actor)?;
    let mut attempt = deadline::RuntimeAttempt::begin_bootstrap(
        home,
        &AuthenticatedActor::launcher(),
        team,
        seat,
        budget,
    )?;
    attempt.admit_effects()?;
    let mut started = match start_native(home, team, seat, 0, selected, &plan, &attempt, Some(actor)) {
        Ok(v) => v,
        Err(e) => {
            let _ = attempt.finish_unknown();
            return Err(e);
        }
    };
    if let Err(e) = authorize_bootstrap(actor).and_then(|_| activate_native(home, team, &started, &attempt, Some(actor))) {
        let cleaned = cleanup_native(
            home,
            team,
            &started.reservation,
            started.child.as_mut(),
            attempt.budget().cleanup_until(),
        );
        let _ = attempt.finish_unknown();
        return Err(if cleaned.is_ok() {
            e
        } else {
            ReplacementError::StartCleanupUnverified
        });
    }
    if let Err(e) = attempt.finish_active() {
        let cleaned = cleanup_native(
            home,
            team,
            &started.reservation,
            started.child.as_mut(),
            attempt.budget().cleanup_until(),
        );
        let _ = attempt.finish_unknown();
        return Err(if cleaned.is_ok() {
            e
        } else {
            ReplacementError::StartCleanupUnverified
        });
    }
    Ok(started.candidate.observed)
}

/// Explicit recovery of a normal Claude bootstrap that ended Quarantined and
/// unobserved at `generation` (1, or 2 after one recovery failed the same way).
/// GLaDOS-only. The proof is issued under locks before anything, re-run before
/// the deadline admission and again before the effective `generation + 1`
/// reserve. Existing native start/activation/observation/cleanup follow; all
/// earlier facts stay; a second call finds the new attempt and is refused.
fn bootstrap_recovery_authorized(
    home: &Path,
    actor: &AuthenticatedActor,
    team: &str,
    seat: &str,
    generation: u64,
    budget: deadline::Deadline,
) -> Result<StartedReplacement, ReplacementError> {
    let admission = RecoveryAdmission::issue(home, actor, team, seat, generation)?;
    let repo =
        repository::resolve_native(home, team, budget.forward_until(Duration::from_secs(10))?)
            .map_err(|_| ReplacementError::RepoBindingUnavailable)?;
    let snapshot: TeamSnapshot =
        read_private_json(&home.join(".aperture/teams").join(team).join("team.json"))
            .map_err(|_| ReplacementError::AuthorizationRequired)?;
    let seats: Vec<_> = snapshot.seats.iter().filter(|s| s.name == seat).collect();
    if seats.len() != 1 || smoke_hash(&snapshot)? != admission.snapshot_sha256 {
        return Err(ReplacementError::AuthorizationRequired);
    }
    let selected = ExecutionTuple {
        harness: seats[0].harness.clone(),
        model: seats[0].model.clone(),
        reasoning: seats[0].reasoning.clone(),
    };
    let binding = match NativePlan::preflight(home, team, seat, &selected, &repo, None, &budget)? {
        NativePlan::Claude(binding) => binding,
        _ => return Err(ReplacementError::LaunchUnavailable),
    };
    authorize_recovery(actor)?;
    // Re-prove (pre-admission phase: no same-generation runtime-attempt yet)
    // under team then seat locks immediately before the deadline admission,
    // which re-checks the owner itself and creates
    // runtime-attempts/<seat>/g<generation> for THIS recovery.
    {
        let _team = crate::owner::try_lock(&home.join(".aperture/run/team-locks"), team)
            .map_err(|_| ReplacementError::NativeFailure)?;
        let store = OwnerStore::new(home.join(".aperture/run/owner"));
        let _seat = store.lock(seat).map_err(|_| ReplacementError::NativeFailure)?;
        authorize_recovery(actor)?;
        admission.reprove_locked(&store, RecoveryPhase::PreAdmission)?;
    }
    let mut attempt = deadline::RuntimeAttempt::begin_bootstrap_recovery(
        home,
        &AuthenticatedActor::launcher(),
        team,
        seat,
        generation,
        budget,
    )?;
    attempt.admit_effects()?;
    // From here the proof is pre-reserve: exactly this attempt's open admission.
    let plan = NativePlan::ClaudeRecovery(binding, admission.bind_attempt(attempt.id()));
    let mut started = match start_native(home, team, seat, generation, selected, &plan, &attempt, Some(actor)) {
        Ok(v) => v,
        Err(e) => {
            let _ = attempt.finish_unknown();
            return Err(e);
        }
    };
    if let Err(e) = authorize_recovery(actor).and_then(|_| activate_native(home, team, &started, &attempt, Some(actor))) {
        let cleaned = cleanup_native(
            home,
            team,
            &started.reservation,
            started.child.as_mut(),
            attempt.budget().cleanup_until(),
        );
        let _ = attempt.finish_unknown();
        return Err(if cleaned.is_ok() {
            e
        } else {
            ReplacementError::StartCleanupUnverified
        });
    }
    if let Err(e) = attempt.finish_active() {
        let cleaned = cleanup_native(
            home,
            team,
            &started.reservation,
            started.child.as_mut(),
            attempt.budget().cleanup_until(),
        );
        let _ = attempt.finish_unknown();
        return Err(if cleaned.is_ok() {
            e
        } else {
            ReplacementError::StartCleanupUnverified
        });
    }
    Ok(started.candidate.observed)
}

/// The only reserve a recovery may take. Caller holds the team lock. The owner
/// is re-proved (pre-reserve phase) under the seat lock, which is then
/// released: `OwnerStore::reserve_start` re-acquires it and applies its own
/// guarantees (expected generation, Stale|Quarantined, fresh nonce, no
/// incarnation). The reservation is then bound to that proof by readback:
/// exactly the proved generation + 1, Starting, the reservation's nonce digest,
/// the selected tuple, no incarnation, no provisional id. Anything else is not
/// the proved owner.
fn reserve_recovery(
    store: &OwnerStore,
    admission: &RecoveryAdmission,
    seat: &str,
    selected: &ExecutionTuple,
) -> Result<StartReservation, ReplacementError> {
    use sha2::{Digest, Sha256};
    let attempt_id = admission.attempt_id.as_deref().ok_or(ReplacementError::OutcomeUnknown)?;
    {
        let _seat = store.lock(seat).map_err(|_| ReplacementError::NativeFailure)?;
        admission.reprove_locked(store, RecoveryPhase::PreReserve { attempt_id })?;
    }
    let next = admission.generation.checked_add(1).ok_or(ReplacementError::OutcomeUnknown)?;
    let reservation = store
        .reserve_start(&AuthenticatedActor::launcher(), seat, admission.generation, selected.clone())
        .map_err(|_| ReplacementError::GenerationMismatch)?;
    let owner = store.read_owner(seat).map_err(|_| ReplacementError::OutcomeUnknown)?;
    let nonce_sha256 = format!("{:x}", Sha256::digest(reservation.nonce().as_bytes()));
    if reservation.seat != seat || reservation.generation != next
        || owner.seat != seat || owner.generation != next || owner.state != OwnerState::Starting
        || owner.incarnation.is_some() || owner.provisional_token_id.is_some()
        || owner.reservation_nonce_sha256.as_deref() != Some(nonce_sha256.as_str())
        || owner.requested != *selected {
        return Err(ReplacementError::OutcomeUnknown);
    }
    Ok(reservation)
}

fn start_native(
    home: &Path,
    team: &str,
    seat: &str,
    generation: u64,
    selected: ExecutionTuple,
    plan: &NativePlan,
    attempt: &deadline::RuntimeAttempt,
    bootstrap_actor: Option<&AuthenticatedActor>,
) -> Result<NativeStarted, ReplacementError> {
    attempt.budget().forward(Duration::from_secs(90))?;
    plan.revalidate(attempt.budget())?;
    let store = OwnerStore::new(home.join(".aperture/run/owner"));
    let launcher = AuthenticatedActor::launcher();
    let reservation = {
        let _team = crate::owner::try_lock(&home.join(".aperture/run/team-locks"), team)
            .map_err(|_| ReplacementError::NativeFailure)?;
        if let Some(actor) = bootstrap_actor {
            authorize_bootstrap(actor)?;
        }
        if let NativePlan::ClaudeSmoke(_, admission) = plan {
            admission.matches_target(home, team, seat, generation)?;
            admission.revalidate_locked()?;
        }
        if let NativePlan::ClaudeRecovery(_, admission) = plan {
            // GLaDOS again, then the pre-reserve proof and the bound reserve.
            authorize_recovery(bootstrap_actor.ok_or(ReplacementError::AuthorizationRequired)?)?;
            admission.matches_target(home, team, seat, generation)?;
            reserve_recovery(&store, admission, seat, &selected)?
        } else {
            store
                .reserve_start(&launcher, seat, generation, selected.clone())
                .map_err(|_| ReplacementError::GenerationMismatch)?
        }
    };
    let mut child = None;
    let result = (|| {
        let token = crate::hub_auth::managed::provision(home, team, &launcher, &reservation)
            .map_err(|_| ReplacementError::NativeFailure)?;
        let smoke_admission = match plan { NativePlan::ClaudeSmoke(_, a) => Some(a), _ => None };
        let recovery_admission = match plan { NativePlan::ClaudeRecovery(_, a) => Some(a), _ => None };
        match plan {
            NativePlan::Retirement => return Err(ReplacementError::AuthorizationRequired),
            NativePlan::Codex(plan) => {
                let spec = plan.publish(&reservation, &token, attempt.budget())?;
                attempt.budget().forward(Duration::from_secs(85))?;
                if let Some(actor) = bootstrap_actor {
                    authorize_bootstrap(actor)?;
                }
                let pending =
                    launch_gate::spawn(spec).map_err(|_| ReplacementError::OutcomeUnknown)?;
                let metadata = match team_process::native::capture_gated_child(&pending) {
                    Ok(p) => p,
                    Err(e) => {
                        pending
                            .cancel()
                            .map_err(|_| ReplacementError::StartCleanupUnverified)?;
                        return Err(e);
                    }
                };
                let incarnation = Incarnation {
                    pid: metadata.pid,
                    start_time: metadata.start_time,
                    thread_id: String::new(),
                    token_id: token.token_id().into(),
                    harness: selected.harness.clone(),
                    model: selected.model.clone(),
                    reasoning: selected.reasoning.clone(),
                    observed: false,
                    processes: vec![metadata],
                };
                if store
                    .record_start_candidate(&launcher, &reservation, incarnation)
                    .is_err()
                {
                    pending
                        .cancel()
                        .map_err(|_| ReplacementError::StartCleanupUnverified)?;
                    return Err(ReplacementError::OutcomeUnknown);
                }
                match pending.release_retaining(&store, &reservation) {
                    Ok(released) => child = Some(released),
                    Err(failure) => {
                        child = failure.child;
                        return Err(ReplacementError::OutcomeUnknown);
                    }
                }
            }
            NativePlan::Claude(plan) | NativePlan::ClaudeSmoke(plan, _) | NativePlan::ClaudeRecovery(plan, _) => {
                let published = plan
                    .publish(&reservation, &token, attempt.budget())
                    .map_err(|_| ReplacementError::LaunchUnavailable)?;
                attempt.budget().forward(Duration::from_secs(85))?;
                if let Some(actor) = bootstrap_actor {
                    authorize_bootstrap(actor)?;
                }
                if let Some(admission) = smoke_admission {
                    admission.revalidate()?;
                }
                if let Some(admission) = recovery_admission {
                    authorize_recovery(bootstrap_actor.ok_or(ReplacementError::AuthorizationRequired)?)?;
                    admission.revalidate()?;
                }
                let pending = published
                    .spawn(attempt.budget())
                    .map_err(|_| ReplacementError::OutcomeUnknown)?;
                let metadata = pending.process.clone();
                let candidate = Incarnation {
                    pid: metadata.pid,
                    start_time: metadata.start_time,
                    thread_id: String::new(),
                    token_id: token.token_id().into(),
                    harness: selected.harness.clone(),
                    model: selected.model.clone(),
                    reasoning: selected.reasoning.clone(),
                    observed: false,
                    processes: vec![metadata],
                };
                let result = candidate_then_claude_release(
                    &store,
                    &launcher,
                    &reservation,
                    candidate,
                    || {
                        crate::team_claude_observation::record_attempt(
                            home,
                            team,
                            &reservation,
                            pending.session_id(),
                            pending.launch_mode(),
                        )
                        .map_err(|_| ReplacementError::ModelUnverified)
                    },
                    || {
                        pending
                            .release(&reservation, attempt.budget())
                            .map_err(|_| ReplacementError::OutcomeUnknown)
                    },
                );
                if let Err(error) = result {
                    // Before candidate publication there is no durable signal
                    // authority. Never kill a guessed tmux PID: await the bounded
                    // unreleased gate's exact exit, otherwise report uncertainty.
                    if store
                        .read_owner(seat)
                        .map_err(|_| ReplacementError::StartCleanupUnverified)?
                        .incarnation
                        .is_none()
                    {
                        pending
                            .cancel_unreleased(attempt.budget().cleanup_until())
                            .map_err(|_| ReplacementError::StartCleanupUnverified)?;
                    }
                    return Err(error);
                }
            }
        }
        let until = attempt.budget().forward_until(Duration::from_secs(85))?;
        let observation = loop {
            attempt.budget().forward(Duration::ZERO)?;
            if Instant::now() >= until {
                return Err(ReplacementError::ModelUnverified);
            }
            if let Some(child) = child.as_mut() {
                if child
                    .try_wait()
                    .map_err(|_| ReplacementError::StartCleanupUnverified)?
                    .is_some()
                {
                    return Err(ReplacementError::ModelUnverified);
                }
            }
            match runtime_observation(home, team, &reservation, &selected.harness)? {
                Some(v) => break v,
                None => std::thread::sleep(Duration::from_millis(25)),
            }
        };
        let record = store
            .record_runtime_observation(&launcher, &reservation, observation)
            .map_err(|_| ReplacementError::ModelUnverified)?;
        let inc = record
            .incarnation
            .ok_or(ReplacementError::ModelUnverified)?;
        let candidate = StartedCandidate {
            observed: StartedReplacement {
                generation: record.generation,
                thread_id: inc.thread_id,
                requested_model: selected.model.clone(),
                actual_model: Some(inc.model),
                model_verified: true,
            },
            actual_harness: Some(
                match inc.harness {
                    Harness::Codex => "codex",
                    Harness::Claude => "claude",
                }
                .into(),
            ),
            actual_reasoning: inc
                .reasoning
                .as_ref()
                .and_then(|r| serde_json::to_value(r).ok())
                .and_then(|v| v.as_str().map(str::to_string)),
            process: team_process::identity_from_owner(inc.pid, inc.start_time)?,
            token_id: inc.token_id,
        };
        Ok(candidate)
    })();
    match result {
        Ok(candidate) => Ok(NativeStarted {
            reservation,
            harness: selected.harness,
            child,
            candidate,
        }),
        Err(e) => {
            let cleanup = cleanup_native(
                home,
                team,
                &reservation,
                child.as_mut(),
                attempt.budget().cleanup_until(),
            );
            Err(if cleanup.is_ok() {
                e
            } else {
                ReplacementError::StartCleanupUnverified
            })
        }
    }
}
/// Revalidate at the actual Active commit boundary, not before observation IO.
pub(crate) fn lock_activation(
    home: &Path, team: &str, bootstrap_actor: Option<&AuthenticatedActor>,
) -> Result<crate::owner::AdvisoryLock, ReplacementError> {
    let lock = crate::owner::try_lock(&home.join(".aperture/run/team-locks"), team)
        .map_err(|_| ReplacementError::NativeFailure)?;
    if let Some(actor) = bootstrap_actor { authorize_bootstrap(actor)?; }
    Ok(lock)
}

fn activate_native(
    home: &Path,
    team: &str,
    started: &NativeStarted,
    attempt: &deadline::RuntimeAttempt,
    bootstrap_actor: Option<&AuthenticatedActor>,
) -> Result<(), ReplacementError> {
    activate_native_checked(home, team, started, attempt, bootstrap_actor, None)
}
fn activate_native_checked(
    home: &Path, team: &str, started: &NativeStarted,
    attempt: &deadline::RuntimeAttempt, bootstrap_actor: Option<&AuthenticatedActor>,
    smoke: Option<&SmokeAdmission>,
) -> Result<(), ReplacementError> {
    attempt.budget().forward(Duration::from_secs(1))?;
    let observation = runtime_observation(home, team, &started.reservation, &started.harness)?
        .ok_or(ReplacementError::ModelUnverified)?;
    let store = OwnerStore::new(home.join(".aperture/run/owner"));
    let launcher = AuthenticatedActor::launcher();
    store
        .record_runtime_observation(&launcher, &started.reservation, observation)
        .map_err(|_| ReplacementError::ModelUnverified)?;
    let _team = lock_activation(home, team, bootstrap_actor)?;
    if let Some(admission) = smoke { admission.revalidate_locked()?; }
    attempt.budget().forward(Duration::from_secs(1))?;
    let owner = store
        .commit_start(&launcher, &started.reservation)
        .map_err(|_| ReplacementError::ModelUnverified)?;
    attempt.budget().forward(Duration::ZERO)?;
    if owner.state != OwnerState::Active || owner.generation != started.reservation.generation {
        return Err(ReplacementError::ModelUnverified);
    }
    Ok(())
}
fn cleanup_native(
    home: &Path,
    team: &str,
    res: &StartReservation,
    mut child: Option<&mut launch_gate::ReleasedChild>,
    until: Instant,
) -> Result<(), ReplacementError> {
    let fail = || ReplacementError::StartCleanupUnverified;
    let time = || {
        if Instant::now() < until {
            Ok(())
        } else {
            Err(fail())
        }
    };
    time()?;
    let store = OwnerStore::new(home.join(".aperture/run/owner"));
    let before = store.read_owner(&res.seat).map_err(|_| fail())?;
    if before.generation != res.generation
        || !matches!(before.state, OwnerState::Starting | OwnerState::Active)
    {
        return Err(fail());
    }
    let identity = before
        .incarnation
        .as_ref()
        .map(|i| crate::owner::FailedStartIdentity {
            pid: i.pid,
            start_time: i.start_time,
            token_id: i.token_id.clone(),
            thread_id: i.thread_id.clone(),
        });
    if let Some(c) = child.as_ref() {
        if identity.as_ref().is_none_or(|i| {
            i.pid != c.identity().pid
                || team_process::birth_micros(c.identity()).ok() != Some(i.start_time)
        }) {
            return Err(fail());
        }
    }
    if before.incarnation.is_none() {
        crate::ws_hub::managed_control::revoke_unlaunched_before(home, team, res, until)?;
    } else {
        if let Some(c) = child.as_mut() {
            let _ = c.try_wait().map_err(|_| fail())?;
        }
        let mut snapshot = team_process::native::collect_native_until(
            home,
            team,
            &res.seat,
            res.generation,
            until,
        )?;
        time()?;
        snapshot
            .processes
            .sort_by_key(|p| std::cmp::Reverse(p.depth));
        let guard =
            team_process::persist_for_stop(home, team, &AuthenticatedActor::launcher(), snapshot)?;
        for signal in [Signal::Term, Signal::Kill] {
            for p in &guard.snapshot().processes {
                time()?;
                team_process::signal_recorded(&guard, &p.identity, signal)?;
            }
            let phase = (Instant::now()
                + if signal == Signal::Term {
                    Duration::from_secs(10)
                } else {
                    Duration::from_secs(1)
                })
            .min(until);
            loop {
                time()?;
                if let Some(c) = child.as_mut() {
                    let _ = c.try_wait().map_err(|_| fail())?;
                }
                let states: Vec<_> = guard
                    .snapshot()
                    .processes
                    .iter()
                    .map(|p| team_process::state(&p.identity))
                    .collect();
                if states.iter().all(|s| *s == ProcessState::Gone) {
                    break;
                }
                if states
                    .iter()
                    .any(|s| matches!(s, ProcessState::Recycled | ProcessState::Unreadable))
                {
                    return Err(fail());
                }
                if Instant::now() >= phase {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            if guard
                .snapshot()
                .processes
                .iter()
                .all(|p| team_process::state(&p.identity) == ProcessState::Gone)
            {
                break;
            }
        }
        // Never claim the first snapshot proves no child appeared later.
        drop(guard);
        time()?;
        let final_snapshot = team_process::native::collect_native_until(
            home,
            team,
            &res.seat,
            res.generation,
            until,
        )?;
        time()?;
        let final_guard = team_process::persist_for_stop(
            home,
            team,
            &AuthenticatedActor::launcher(),
            final_snapshot,
        )?;
        crate::ws_hub::managed_control::revoke_stopped_before(home, &final_guard, until)?;
    }
    time()?;
    let current = store.read_owner(&res.seat).map_err(|_| fail())?;
    if current.generation != res.generation {
        return Err(fail());
    }
    let _team =
        crate::owner::try_lock(&home.join(".aperture/run/team-locks"), team).map_err(|_| fail())?;
    match identity {
        Some(identity) => {
            store
                .quarantine_failed_start(&AuthenticatedActor::launcher(), res, &identity)
                .map_err(|_| fail())?;
        }
        None => {
            store
                .abort_start(&AuthenticatedActor::launcher(), res)
                .map_err(|_| fail())?;
        }
    }
    Ok(())
}

/// Internal authenticated contexts only; not a command DTO or caller proof.
pub(crate) enum ReplacementAuthority<'a> {
    Operator(&'a AuthenticatedActor),
    Lead(&'a AuthenticatedSeat),
    GladosArchive(&'a AuthenticatedActor),
    GladosRetirement(&'a AuthenticatedActor),
}
impl ReplacementAuthority<'_> {
    fn revalidate(&self, target: &remote::RemoteTarget) -> Result<(), ReplacementError> {
        match self {
            Self::Operator(actor) if actor.principal() == "operator" => Ok(()),
            Self::GladosArchive(actor) | Self::GladosRetirement(actor) if actor.is_glados() => actor
                .revalidate_before_mutation()
                .map_err(|_| ReplacementError::AuthorizationRequired),
            Self::Lead(actor) if actor.team() == target.team && actor.seat() != target.seat => {
                actor
                    .revalidate_before_effect()
                    .map_err(|_| ReplacementError::AuthorizationRequired)
            }
            _ => Err(ReplacementError::AuthorizationRequired),
        }
    }
    fn inspect(&self, home: &Path, target: &remote::RemoteTarget, sentinels: &[String])
        -> Result<remote::RemoteInventoryView, remote::RemoteError>
    {
        self.revalidate(target).map_err(|_| remote::RemoteError::Authority)?;
        let result = match self {
            Self::Operator(actor) => remote::inspect_authorized(home, remote::ResolutionAuthority::Operator(actor), target, sentinels),
            Self::Lead(actor) => remote::inspect_authorized(home, remote::ResolutionAuthority::Lead(actor), target, sentinels),
            // Read only: GLaDOS archival authority does not become an operator
            // principal and cannot manufacture a remote-resolution decision.
            Self::GladosArchive(_) | Self::GladosRetirement(_) => remote::inspect_native(home, target, sentinels),
        }?;
        self.revalidate(target).map_err(|_| remote::RemoteError::Authority)?;
        Ok(result)
    }
}

/// Existing authenticated lead-validation lane, with the real bound Git/gh
/// collector. The caller selects target generation/sequence, never a path,
/// observation, validation result, clock, team role or author identity.
fn validate_checkpoint_authorized(
    home: &Path,
    actor: &AuthenticatedSeat,
    target: &remote::RemoteTarget,
    seq: u64,
    sentinels: &[String],
) -> Result<crate::team_checkpoint::CheckpointValidation, ReplacementError> {
    use crate::team_checkpoint::{native::validation, CheckpointError};
    selectors(target)?;
    if seq == 0 || actor.team() != target.team {
        return Err(ReplacementError::AuthorizationRequired);
    }
    actor
        .revalidate_before_effect()
        .map_err(|_| ReplacementError::AuthorizationRequired)?;
    let until = Instant::now() + Duration::from_secs(10);
    let repo = repository::resolve_native(home, &target.team, until)
        .map_err(|_| ReplacementError::RepoBindingUnavailable)?;
    let ctx = validation::ValidationContext {
        team: target.team.clone(),
        seat: target.seat.clone(),
        generation: target.expected_generation,
        lead_seat: actor.seat().into(),
        lead_generation: actor.generation(),
    };
    let revalidate = || {
        actor
            .revalidate_before_effect()
            .map_err(|_| CheckpointError::Generation)
    };
    validation::validate_native(
        home,
        &ctx,
        seq,
        chrono::Utc::now()
            .timestamp_millis()
            .try_into()
            .map_err(|_| ReplacementError::NativeFailure)?,
        sentinels,
        revalidate,
        |entry| {
            repository::collect_native(&repo, entry, until).map_err(|_| CheckpointError::Invalid)
        },
    )
    .map_err(|_| ReplacementError::CheckpointUnavailable)?;
    let entries = validation::validated_entries_native(home, &ctx, sentinels, revalidate)
        .map_err(|_| ReplacementError::CheckpointUnavailable)?;
    entries
        .into_iter()
        .find(|e| e.seq == seq)
        .map(|e| e.validation)
        .ok_or(ReplacementError::CheckpointUnavailable)
}

/// Explicit root checkpoint validation without replace/start/stop effects.
/// The selector approves an immutable checkpoint's task/worktree binding;
/// Git/PR observations and the resulting verdict are collected natively.
pub(crate) fn validate_checkpoint_glados(
    home: &Path, actor: &AuthenticatedActor, target: &remote::RemoteTarget,
    seq: u64, sentinels: &[String],
) -> Result<crate::team_checkpoint::CheckpointValidation, ReplacementError> {
    use crate::team_checkpoint::{native::validation, CheckpointError};
    if !actor.is_glados() { return Err(ReplacementError::AuthorizationRequired); }
    actor.revalidate_before_mutation().map_err(|_| ReplacementError::AuthorizationRequired)?;
    selectors(target)?;
    if seq == 0 { return Err(ReplacementError::CheckpointUnavailable); }
    let snapshot: TeamSnapshot = read_private_json(&home.join(".aperture/teams")
        .join(&target.team).join("team.json")).map_err(|_| ReplacementError::GenerationMismatch)?;
    let owners = OwnerStore::new(home.join(".aperture/run/owner"));
    let owner = owners.read_owner(&target.seat).map_err(|_| ReplacementError::GenerationMismatch)?;
    if owner.generation != target.expected_generation || owner.state != OwnerState::Active {
        return Err(ReplacementError::GenerationMismatch);
    }
    let lead = owners.read_owner(&snapshot.lead).map_err(|_| ReplacementError::GenerationMismatch)?;
    let ctx = validation::ValidationContext {
        team: target.team.clone(), seat: target.seat.clone(), generation: target.expected_generation,
        lead_seat: snapshot.lead, lead_generation: lead.generation,
    };
    let until = Instant::now() + Duration::from_secs(10);
    let repo = repository::resolve_native(home, &target.team, until)
        .map_err(|_| ReplacementError::RepoBindingUnavailable)?;
    let fact = validation::validate_glados_native(home, actor, &ctx, seq,
        chrono::Utc::now().timestamp_millis().try_into().map_err(|_| ReplacementError::NativeFailure)?,
        sentinels, |entry| {
            // Validation holds the owner locks. Reject a generation that changed
            // between preflight and lock acquisition, before collecting anything.
            let current: OwnerRecord = read_private_json(&owners.record_path(&target.seat))
                .map_err(|_| CheckpointError::Corrupt)?;
            if current != owner { return Err(CheckpointError::Generation); }
            repository::collect_native(&repo, entry, until).map_err(|_| CheckpointError::Invalid)
        }).map_err(|_| ReplacementError::CheckpointUnavailable)?;
    actor.revalidate_before_mutation().map_err(|_| ReplacementError::OutcomeUnknown)?;
    Ok(fact.result().clone())
}

struct RecoveryContext {
    worktree: String,
    entry: Option<crate::team_checkpoint::CheckpointEntry>,
    recovery: CheckpointRecovery,
}
fn checkpoint_for_target(
    home: &Path,
    authority: &ReplacementAuthority<'_>,
    target: &remote::RemoteTarget,
    repo: &repository::BoundRepository,
    sentinels: &[String],
    budget: &deadline::Deadline,
) -> Result<RecoveryContext, ReplacementError> {
    use crate::team_checkpoint::{native::validation, CheckpointError, CheckpointValidation};
    authority.revalidate(target)?;
    let candidate = (|| {
        let snapshot: TeamSnapshot = read_private_json(
            &home
                .join(".aperture/teams")
                .join(&target.team)
                .join("team.json"),
        )
        .ok()?;
        let lead: OwnerRecord = read_private_json(
            &home
                .join(".aperture/run/owner")
                .join(format!("{}.json", snapshot.lead)),
        )
        .ok()?;
        let ctx = validation::ValidationContext {
            team: target.team.clone(),
            seat: target.seat.clone(),
            generation: target.expected_generation,
            lead_seat: snapshot.lead,
            lead_generation: lead.generation,
        };
        let revalidate = || {
            authority
                .revalidate(target)
                .map_err(|_| CheckpointError::Generation)
        };
        let entries =
            validation::validated_entries_native(home, &ctx, sentinels, revalidate).ok()?;
        // Lead replace may validate the latest bounded candidate internally.
        // Operator never impersonates a lead or manufactures a validation fact.
        if let (ReplacementAuthority::Lead(actor), Some(latest)) =
            (authority, entries.iter().max_by_key(|e| e.seq))
        {
            let _ = validate_checkpoint_authorized(home, actor, target, latest.seq, sentinels);
        }
        // A later divergent observation does not erase the previously
        // authenticated task/seat/worktree identity. It does invalidate recovery
        // contents: historical binding is never projected as current validity.
        validation::historical_bindings_native(home, &ctx, sentinels, revalidate)
            .ok()?
            .into_iter()
            .max_by_key(|e| e.seq)
    })();
    // Git membership is not mission authority, even for exactly one worktree.
    // Pending/none with no historical authenticated binding fails before effects.
    let trusted = candidate.ok_or(ReplacementError::WorktreeUnbound)?;
    repository::replacement_cwd(
        repo,
        &trusted.payload.worktree,
        budget.forward_until(Duration::from_secs(10))?,
    )
    .map_err(|_| ReplacementError::WorktreeUnbound)?;
    let worktree = trusted.payload.worktree.clone();
    let mut recovery = CheckpointRecovery::Stale;
    let mut entry = None;
    if let Ok(actual) = repository::collect_native(
        repo,
        &trusted,
        budget.forward_until(Duration::from_secs(10))?,
    ) {
        if crate::team_checkpoint::validate_against_artifacts(&trusted, &actual)
            == CheckpointValidation::Ok
        {
            let mut current = trusted;
            current.validation = CheckpointValidation::Ok;
            recovery = CheckpointRecovery::Valid;
            entry = Some(current);
        }
    }
    authority.revalidate(target)?;
    Ok(RecoveryContext {
        worktree,
        entry,
        recovery,
    })
}

// The binding is derived from the immutable native snapshot and single catalog,
// never a caller path. Launch composition remains a distinct fail-closed gate.
struct RepositoryBinding(super::repository::BoundRepository);
fn require_repository_binding(
    home: &Path,
    target: &remote::RemoteTarget,
    budget: &deadline::Deadline,
) -> Result<RepositoryBinding, ReplacementError> {
    super::repository::resolve_native(
        home,
        &target.team,
        budget.forward_until(Duration::from_secs(10))?,
    )
    .map(RepositoryBinding)
    .map_err(|_| ReplacementError::RepoBindingUnavailable)
}
fn selectors(target: &remote::RemoteTarget) -> Result<(), ReplacementError> {
    if target.expected_generation == 0
        || target.team.len() > 16
        || !crate::agent_loader::is_valid_seat_name(&target.team)
        || !crate::agent_loader::is_valid_seat_name(&target.seat)
    {
        return Err(ReplacementError::GenerationMismatch);
    }
    Ok(())
}

/// Agent one-shot seam. Target generation is CAS, not actor identity. No
/// PreparedReplacement, inventory, filesystem path or native proof in the DTO.
/// Native repo/checkpoint/launch preflight precedes effects. It then consumes
/// exactly one continuous attempt through prepare/start/cleanup. Registration
/// remains a separate integrated gate; no caller-observed facts are accepted.
/// Internal result only. Transport projects the safe owner summary, never the
/// started thread identity. Recovery is the actual prepare result, not inferred.
pub(crate) struct NativeReplacementResult {
    pub(crate) started: StartedReplacement,
    pub(crate) checkpoint_recovery: CheckpointRecovery,
}
impl std::fmt::Debug for NativeReplacementResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeReplacementResult")
            .field("checkpoint_recovery", &self.checkpoint_recovery)
            .finish_non_exhaustive()
    }
}
pub(crate) fn replace_authorized(
    home: &Path,
    authority: ReplacementAuthority<'_>,
    target: remote::RemoteTarget,
    selection: &StartSelection,
    sentinels: &[String],
) -> Result<NativeReplacementResult, ReplacementError> {
    if matches!(authority, ReplacementAuthority::GladosArchive(_) | ReplacementAuthority::GladosRetirement(_)) {
        return Err(ReplacementError::AuthorizationRequired);
    }
    let budget = deadline::Deadline::new();
    selectors(&target)?;
    let binding = require_repository_binding(home, &target, &budget)?;
    authority.revalidate(&target)?;
    if !crate::teams::managed_execution_enabled(&tuple(selection)?) {
        return Err(ReplacementError::LaunchUnavailable);
    }
    let checkpoint =
        checkpoint_for_target(home, &authority, &target, &binding.0, sentinels, &budget)?;
    let mut plan = NativePlan::preflight_selected(
        home,
        &target.team,
        &target.seat,
        &tuple(selection)?,
        &binding.0,
        Some(checkpoint.worktree.as_str()),
        &budget,
    )?;
    plan.bind_recovery(checkpoint.entry.as_ref())?;
    let mut runtime = NativeRuntime::new(
        home, authority, target, sentinels, binding, budget, plan, checkpoint,
    )?;
    let seat = runtime.target.seat.clone();
    let generation = runtime.target.expected_generation;
    let result = (|| {
        let snapshot = runtime.snapshot(&seat, generation)?;
        runtime.authorize_selection(&snapshot, selection)?;
        let prepared = prepare(
            &mut runtime,
            &seat,
            generation,
            &ReplacementPolicy::default(),
        )?;
        // Binding must survive reconciliation too; never reset total budget.
        runtime.attempt.budget().forward(Duration::from_secs(10))?;
        require_repository_binding(home, &runtime.target, runtime.attempt.budget())?;
        let checkpoint_recovery = prepared.checkpoint_recovery();
        let started = start(&mut runtime, prepared, selection)?;
        Ok(NativeReplacementResult {
            started,
            checkpoint_recovery,
        })
    })();
    match result {
        Ok(value) => {
            if let Err(e) = runtime.attempt.finish_active() {
                if let Some(candidate) = runtime.started.as_ref().map(|s| s.candidate.clone()) {
                    runtime.abort_started(&candidate)?;
                }
                let _ = runtime.attempt.finish_unknown();
                return Err(e);
            }
            Ok(value)
        }
        Err(error) => {
            runtime.attempt.finish_failed()?;
            Err(error)
        }
    }
}

/// Retained only in the launcher process, behind an opaque ID. No Serialize,
/// Deserialize, Clone or caller proofs. No action clock runs while human waits.
pub(crate) struct NativePreparedReplacement {
    home: std::path::PathBuf,
    target: remote::RemoteTarget,
    sentinels: Vec<String>,
    binding: RepositoryBinding,
    plan: NativePlan,
    checkpoint: RecoveryContext,
    prepared: PreparedReplacement,
    revoked: RevocationProof,
    ready: deadline::ReadyAttempt,
}
impl NativePreparedReplacement {
    pub(crate) fn team(&self) -> &str {
        &self.target.team
    }
    pub(crate) fn seat(&self) -> &str {
        &self.target.seat
    }
    pub(crate) fn generation(&self) -> u64 {
        self.target.expected_generation
    }
    pub(crate) fn recovery(&self) -> CheckpointRecovery {
        self.prepared.checkpoint_recovery()
    }
}
pub(crate) fn prepare_operator(
    home: &Path,
    actor: &AuthenticatedActor,
    team: &str,
    seat: &str,
    expected_generation: u64,
    sentinels: &[String],
) -> Result<NativePreparedReplacement, ReplacementError> {
    if actor.principal() != "operator" {
        return Err(ReplacementError::AuthorizationRequired);
    }
    prepare_authenticated(home, actor, team, seat, expected_generation, sentinels, false)
}

/// GLaDOS-only stop/revoke, no replacement permit leaves this function.
/// The archived owner transition belongs to the existing archive journal.
pub(crate) fn stop_for_archive(
    home: &Path, actor: &AuthenticatedActor, team: &str, seat: &str,
    expected_generation: u64, sentinels: &[String],
) -> Result<CheckpointRecovery, ReplacementError> {
    if !actor.is_glados() {
        return Err(ReplacementError::AuthorizationRequired);
    }
    actor.revalidate_before_mutation().map_err(|_| ReplacementError::AuthorizationRequired)?;
    let stopped = prepare_authenticated(home, actor, team, seat, expected_generation, sentinels, true)?;
    actor.revalidate_before_mutation().map_err(|_| ReplacementError::OutcomeUnknown)?;
    if stopped.prepared.checkpoint_recovery() != CheckpointRecovery::Valid
        || !stopped.revoked.durable || !stopped.revoked.token_deleted
        || stopped.revoked.generation != expected_generation
    { return Err(ReplacementError::OutcomeUnknown); }
    // Ready evidence is durable. Drop the in-memory replacement permit: there
    // is no Start call and no automatic generation/model change.
    Ok(CheckpointRecovery::Valid)
}

/// Explicit, root-authorized retirement; no replacement, checkpoint, or mission PASS is invented.
pub(crate) fn stop_for_retirement(
    home: &Path, actor: &AuthenticatedActor, team: &str, seat: &str,
    expected_generation: u64, accept_checkpoint_loss: bool,
) -> Result<CheckpointRecovery, ReplacementError> {
    if !actor.is_glados() { return Err(ReplacementError::AuthorizationRequired); }
    actor.revalidate_before_mutation().map_err(|_| ReplacementError::AuthorizationRequired)?;
    let target = remote::RemoteTarget {team:team.into(), seat:seat.into(), expected_generation};
    selectors(&target)?;
    let budget = deadline::Deadline::new();
    let binding = require_repository_binding(home, &target, &budget)?;
    let authority = ReplacementAuthority::GladosRetirement(actor);
    let checkpoint = if accept_checkpoint_loss {
        RecoveryContext {worktree:String::new(), entry:None, recovery:CheckpointRecovery::None}
    } else { checkpoint_for_target(home, &authority, &target, &binding.0, &[], &budget)? };
    let mut runtime = NativeRuntime::new(home, authority, target, &[], binding, budget,
        NativePlan::Retirement, checkpoint)?;
    let result = prepare_for_retirement(&mut runtime, seat, expected_generation,
        &ReplacementPolicy::default(), accept_checkpoint_loss);
    let recovery = match result {
        Ok(r) => r,
        Err(e) => { let _ = runtime.attempt.finish_failed(); return Err(e); }
    };
    let proof = runtime.revoked.take().ok_or(ReplacementError::RevocationUnverified)?;
    // Ready is factual stop/revocation evidence; no start permit leaves this function.
    let _ready = runtime.attempt.finish_ready(&proof)?;
    actor.revalidate_before_mutation().map_err(|_| ReplacementError::OutcomeUnknown)?;
    crate::team_archive::retirement::record_stopped(home, actor, team, seat,
        expected_generation, recovery.clone(), accept_checkpoint_loss)
        .map_err(|_| ReplacementError::OutcomeUnknown)?;
    Ok(recovery)
}

fn prepare_authenticated(
    home: &Path, actor: &AuthenticatedActor, team: &str, seat: &str,
    expected_generation: u64, sentinels: &[String], archive: bool,
) -> Result<NativePreparedReplacement, ReplacementError> {
    let target = remote::RemoteTarget {
        team: team.into(),
        seat: seat.into(),
        expected_generation,
    };
    selectors(&target)?;
    let budget = deadline::Deadline::new();
    let authority = if archive { ReplacementAuthority::GladosArchive(actor) }
        else { ReplacementAuthority::Operator(actor) };
    let binding = require_repository_binding(home, &target, &budget)?;
    authority.revalidate(&target)?;
    let owner = OwnerStore::new(home.join(".aperture/run/owner"))
        .read_owner(seat)
        .map_err(|_| ReplacementError::GenerationMismatch)?;
    if owner.generation != expected_generation || owner.state != OwnerState::Active {
        return Err(ReplacementError::GenerationMismatch);
    }
    if !crate::teams::managed_execution_enabled(&owner.requested) {
        return Err(ReplacementError::LaunchUnavailable);
    }
    let checkpoint =
        checkpoint_for_target(home, &authority, &target, &binding.0, sentinels, &budget)?;
    let mut plan = NativePlan::preflight_selected(
        home,
        team,
        seat,
        &owner.requested,
        &binding.0,
        Some(checkpoint.worktree.as_str()),
        &budget,
    )?;
    plan.bind_recovery(checkpoint.entry.as_ref())?;
    let mut runtime = NativeRuntime::new(
        home, authority, target, sentinels, binding, budget, plan, checkpoint,
    )?;
    let policy = ReplacementPolicy::default();
    let result = if archive {
        prepare_for_archive(&mut runtime, seat, expected_generation, &policy)
    } else {
        prepare(&mut runtime, seat, expected_generation, &policy)
    };
    let prepared = match result {
        Ok(v) => v,
        Err(e) => {
            let _ = runtime.attempt.finish_failed();
            return Err(e);
        }
    };
    let revoked = runtime
        .revoked
        .take()
        .ok_or(ReplacementError::RevocationUnverified)?;
    let ready = runtime.attempt.finish_ready(&revoked)?;
    Ok(NativePreparedReplacement {
        home: home.into(),
        target: runtime.target,
        sentinels: sentinels.to_vec(),
        binding: runtime._binding,
        plan: runtime.plan,
        checkpoint: runtime.checkpoint,
        prepared,
        revoked,
        ready,
    })
}
/// Transport removes its opaque map entry before invoking this consuming seam.
/// Revalidation failure before new effects is expired/blocked, not UNKNOWN.
pub(crate) fn start_operator(
    actor: &AuthenticatedActor,
    permit: NativePreparedReplacement,
    selection: &StartSelection,
) -> Result<StartedReplacement, ReplacementError> {
    if actor.principal() != "operator" {
        return Err(ReplacementError::AuthorizationRequired);
    }
    let NativePreparedReplacement {
        home,
        target,
        sentinels,
        binding,
        plan,
        checkpoint,
        prepared,
        revoked,
        ready,
    } = permit;
    let budget = deadline::Deadline::new();
    let authority = ReplacementAuthority::Operator(actor);
    authority
        .revalidate(&target)
        .map_err(|_| ReplacementError::PreparationExpired)?;
    plan.revalidate(&budget)
        .map_err(|_| ReplacementError::PreparationExpired)?;
    if !crate::teams::managed_execution_enabled(&tuple(selection)?) {
        return Err(ReplacementError::LaunchUnavailable);
    }
    let attempt = ready.start(budget)?;
    let mut runtime = NativeRuntime {
        home: &home,
        authority,
        target,
        sentinels: &sentinels,
        _binding: binding,
        epoch: Instant::now(),
        snapshot: Some(prepared.snapshot.clone()),
        revoked: Some(revoked),
        phases: vec![],
        attempt,
        plan,
        checkpoint,
        started: None,
        archival_signal_admitted: false,
    };
    let result = start(&mut runtime, prepared, selection);
    match result {
        Ok(value) => {
            if let Err(e) = runtime.attempt.finish_active() {
                if let Some(candidate) = runtime.started.as_ref().map(|s| s.candidate.clone()) {
                    runtime.abort_started(&candidate)?;
                }
                let _ = runtime.attempt.finish_unknown();
                return Err(e);
            }
            Ok(value)
        }
        Err(error) => {
            let uncertain = runtime.attempt.effects_admitted();
            let _ = runtime.attempt.finish_failed();
            Err(if uncertain {
                error
            } else {
                ReplacementError::PreparationExpired
            })
        }
    }
}

struct NativeRuntime<'a> {
    home: &'a Path,
    authority: ReplacementAuthority<'a>,
    target: remote::RemoteTarget,
    sentinels: &'a [String],
    _binding: RepositoryBinding,
    epoch: Instant,
    snapshot: Option<OwnershipSnapshot>,
    revoked: Option<RevocationProof>,
    phases: Vec<ReplacementPhase>,
    attempt: deadline::RuntimeAttempt,
    plan: NativePlan,
    checkpoint: RecoveryContext,
    started: Option<NativeStarted>,
    archival_signal_admitted: bool,
}
impl<'a> NativeRuntime<'a> {
    fn new(
        home: &'a Path,
        authority: ReplacementAuthority<'a>,
        target: remote::RemoteTarget,
        sentinels: &'a [String],
        binding: RepositoryBinding,
        budget: deadline::Deadline,
        plan: NativePlan,
        checkpoint: RecoveryContext,
    ) -> Result<Self, ReplacementError> {
        authority.revalidate(&target)?;
        authority.inspect(home, &target, sentinels)
            .map_err(|_| ReplacementError::AuthorizationRequired)?;
        let begin = if matches!(authority, ReplacementAuthority::GladosRetirement(_)) {
            deadline::RuntimeAttempt::begin_retirement
        } else { deadline::RuntimeAttempt::begin };
        let attempt = begin(home, &AuthenticatedActor::launcher(), &target.team,
            &target.seat, target.expected_generation, budget)?;
        let runtime = Self {
            home,
            authority,
            target,
            sentinels,
            _binding: binding,
            epoch: Instant::now(),
            snapshot: None,
            revoked: None,
            phases: vec![],
            attempt,
            plan,
            checkpoint,
            started: None,
            archival_signal_admitted: false,
        };
        runtime.admission()?;
        Ok(runtime)
    }
    fn admission(&self) -> Result<(), ReplacementError> {
        self.attempt.budget().forward(Duration::ZERO)?;
        self.authority.revalidate(&self.target)?;
        // Uses the SAME team -> lexical seat locks and immutable lead identity
        // as resolution. This does not accept or synthesize remote authorization.
        self.authority.inspect(self.home, &self.target, self.sentinels)
        .map_err(|e| match e {
            remote::RemoteError::Authority => ReplacementError::AuthorizationRequired,
            _ => ReplacementError::RemoteUncertain,
        })?;
        Ok(())
    }
    fn matches_target(&self, snapshot: &OwnershipSnapshot) -> Result<(), ReplacementError> {
        if snapshot.seat != self.target.seat
            || snapshot.generation != self.target.expected_generation
        {
            return Err(ReplacementError::GenerationMismatch);
        }
        Ok(())
    }
    /// Per-effect guard only. Never retain owner locks across checkpoint waits,
    /// collectors, remote projection or another owner API (all can reenter).
    fn with_stop_guard<T>(
        &self,
        snapshot: &OwnershipSnapshot,
        effect: impl FnOnce(&team_process::PersistedProcessSnapshot) -> Result<T, ReplacementError>,
    ) -> Result<T, ReplacementError> {
        self.matches_target(snapshot)?;
        self.admission()?;
        let guard = team_process::persist_for_stop(
            self.home,
            &self.target.team,
            &AuthenticatedActor::launcher(),
            snapshot.clone(),
        )?;
        // Team lock is held here; revalidate bearer/owner without recursively
        // taking the team lock through inspect_authorized.
        self.authority.revalidate(&self.target)?;
        effect(&guard)
    }
}
fn claude_stopped(snapshot: &OwnershipSnapshot, state: impl Fn(&ProcessIdentity) -> ProcessState) -> Result<(), ReplacementError> {
    if !snapshot.complete || snapshot.processes.is_empty() || !snapshot.unowned_matches.is_empty()
        || snapshot.processes.iter().any(|p| state(&p.identity) != ProcessState::Gone)
    { return Err(ReplacementError::StopUnverified); }
    Ok(())
}
fn same_identities(a: &OwnershipSnapshot, b: &OwnershipSnapshot) -> bool {
    a.seat == b.seat
        && a.generation == b.generation
        && a.thread_id == b.thread_id
        && a.complete
        && b.complete
        && a.processes.len() == b.processes.len()
        && a.processes
            .iter()
            .all(|p| b.processes.iter().any(|q| p.identity == q.identity))
}
fn tuple(selection: &StartSelection) -> Result<ExecutionTuple, ReplacementError> {
    serde_json::from_value(serde_json::json!({"harness":selection.harness,
        "model":selection.model,"reasoning":selection.reasoning}))
    .map_err(|_| ReplacementError::AuthorizationRequired)
}
fn unchanged_execution_controls(
    current: &ExecutionTuple,
    selected: &ExecutionTuple,
) -> Result<(), ReplacementError> {
    if current.harness != selected.harness || current.reasoning != selected.reasoning {
        return Err(ReplacementError::AuthorizationRequired);
    }
    Ok(())
}
fn selected_in_snapshot(
    snapshot: &TeamSnapshot,
    seat: &str,
    selected: &ExecutionTuple,
) -> Result<(), ReplacementError> {
    let seats: Vec<_> = snapshot.seats.iter().filter(|s| s.name == seat).collect();
    if seats.len() != 1 {
        return Err(ReplacementError::AuthorizationRequired);
    }
    let configured = ExecutionTuple {
        harness: seats[0].harness.clone(),
        model: seats[0].model.clone(),
        reasoning: seats[0].reasoning.clone(),
    };
    if *selected != configured && !snapshot.fallbacks.contains(selected) {
        return Err(ReplacementError::AuthorizationRequired);
    }
    Ok(())
}
impl ReplacementRuntime for NativeRuntime<'_> {
    fn now_ms(&self) -> u64 {
        self.epoch
            .elapsed()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX)
    }
    fn wait_ms(&mut self, ms: u64) {
        std::thread::sleep(Duration::from_millis(ms).min(self.attempt.budget().remaining()));
    }
    fn event(&mut self, phase: ReplacementPhase) {
        self.phases.push(phase);
    }
    fn expected_owner(&mut self, seat: &str, generation: u64) -> Result<(), ReplacementError> {
        if seat != self.target.seat || generation != self.target.expected_generation {
            return Err(ReplacementError::GenerationMismatch);
        }
        self.admission()
    }
    fn snapshot(
        &mut self,
        seat: &str,
        generation: u64,
    ) -> Result<OwnershipSnapshot, ReplacementError> {
        self.expected_owner(seat, generation)?;
        self.attempt.budget().forward(Duration::from_secs(10))?;
        let snapshot =
            team_process::native::collect_native(self.home, &self.target.team, seat, generation)?;
        self.snapshot = Some(snapshot.clone());
        Ok(snapshot)
    }
    // No durable turn/rate-limit observer is wired. Conservative Busy is not a
    // claim about an actual turn; missing hook below is explicit, never fake ACK.
    fn turn_state(&self) -> TurnState {
        TurnState::Busy
    }
    fn rate_limit_at_ms(&self) -> Option<u64> {
        None
    }
    fn request_checkpoint(&mut self) -> Result<(), ReplacementError> {
        Err(ReplacementError::CheckpointUnavailable)
    }
    fn checkpoint_recovery(&mut self) -> CheckpointRecovery {
        if matches!(self.authority, ReplacementAuthority::GladosArchive(_))
            || (matches!(self.authority, ReplacementAuthority::GladosRetirement(_)) && self.checkpoint.entry.is_some()) {
            return checkpoint_for_target(self.home, &self.authority, &self.target,
                &self._binding.0, self.sentinels, self.attempt.budget())
                .ok().filter(|fresh| fresh.worktree == self.checkpoint.worktree)
                .map(|fresh| fresh.recovery).unwrap_or(CheckpointRecovery::None);
        }
        self.checkpoint.recovery
    }
    fn process_state(&mut self, process: &ProcessIdentity) -> ProcessState {
        team_process::state(process)
    }
    fn signal(
        &mut self,
        process: &ProcessIdentity,
        signal: Signal,
    ) -> Result<(), ReplacementError> {
        if (matches!(self.authority, ReplacementAuthority::GladosArchive(_))
            || (matches!(self.authority, ReplacementAuthority::GladosRetirement(_)) && self.checkpoint.entry.is_some()))
            && !self.archival_signal_admitted
            && self.checkpoint_recovery() != CheckpointRecovery::Valid
        { return Err(ReplacementError::CheckpointUnavailable); }
        self.attempt.admit_effects()?;
        let snapshot = self
            .snapshot
            .as_ref()
            .ok_or(ReplacementError::InvalidSnapshot)?;
        if !snapshot.processes.iter().any(|p| p.identity == *process) {
            return Err(ReplacementError::UnownedProcess);
        }
        self.with_stop_guard(snapshot, |guard| {
            team_process::signal_recorded(guard, process, signal)
        })?;
        self.archival_signal_admitted = true;
        Ok(())
    }
    fn unowned_matches(&mut self, snapshot: &OwnershipSnapshot) -> Result<bool, ReplacementError> {
        self.matches_target(snapshot)?;
        self.admission()?;
        self.attempt.budget().forward(Duration::from_secs(10))?;
        let fresh = team_process::native::collect_native(
            self.home,
            &self.target.team,
            &self.target.seat,
            self.target.expected_generation,
        )?;
        // A late/reparented identity must not vanish from the frozen stop set.
        if !same_identities(snapshot, &fresh) {
            return Err(ReplacementError::StopUnverified);
        }
        Ok(!fresh.unowned_matches.is_empty())
    }
    fn revoke(
        &mut self,
        snapshot: &OwnershipSnapshot,
    ) -> Result<RevocationProof, ReplacementError> {
        self.attempt.admit_effects()?;
        let proof = self.with_stop_guard(snapshot, |guard| {
            if let Some(prior) = self.attempt.prior_ready() {
                crate::ws_hub::managed_control::reaffirm_stopped(
                    self.home,
                    guard,
                    prior,
                    self.attempt
                        .budget()
                        .forward_until(Duration::from_secs(3))?,
                )
            } else {
                crate::ws_hub::managed_control::revoke_stopped(self.home, guard)
            }
        })?;
        self.revoked = Some(proof.clone());
        Ok(proof)
    }
    fn revocation_still_valid(
        &mut self,
        snapshot: &OwnershipSnapshot,
    ) -> Result<bool, ReplacementError> {
        self.matches_target(snapshot)?;
        // Store metadata alone is NOT the prior factual close/reconnect ACK.
        let proof = self
            .revoked
            .as_ref()
            .ok_or(ReplacementError::RevocationUnverified)?;
        if proof.generation != snapshot.generation
            || !proof.durable
            || !proof.sockets_closed
            || !proof.token_deleted
            || proof.close_code != 4001
            || proof.reconnect_code != 4003
            || proof.close_elapsed_ms > 1000
        {
            return Ok(false);
        }
        self.with_stop_guard(snapshot, |_| revoked_metadata(self.home, snapshot))
    }
    fn remote_effects(
        &mut self,
        snapshot: &OwnershipSnapshot,
    ) -> Result<RemoteInventory, ReplacementError> {
        self.matches_target(snapshot)?;
        self.admission()?;
        let projection = remote::project_native(self.home, &self.target, self.sentinels)
            .map_err(|_| ReplacementError::RemoteUncertain)?;
        self.authority.revalidate(&self.target)?;
        Ok(projection.into_core())
    }
    fn authorize_selection(
        &mut self,
        snapshot: &OwnershipSnapshot,
        selection: &StartSelection,
    ) -> Result<(), ReplacementError> {
        if matches!(self.authority, ReplacementAuthority::GladosArchive(_) | ReplacementAuthority::GladosRetirement(_)) {
            return Err(ReplacementError::AuthorizationRequired);
        }
        self.matches_target(snapshot)?;
        self.admission()?;
        let _lock = crate::owner::try_lock(
            &self.home.join(".aperture/run/team-locks"),
            &self.target.team,
        )
        .map_err(|_| ReplacementError::AuthorizationRequired)?;
        self.authority.revalidate(&self.target)?;
        let path =
            validate_component_path(&self.home.join(".aperture/teams"), &self.target.team, false)
                .map_err(|_| ReplacementError::AuthorizationRequired)?;
        let team: TeamSnapshot = read_private_json(&path.join("team.json"))
            .map_err(|_| ReplacementError::AuthorizationRequired)?;
        if team.schema_version != 1 || team.team != self.target.team {
            return Err(ReplacementError::AuthorizationRequired);
        }
        let selected = tuple(selection)?;
        let owner = OwnerStore::new(self.home.join(".aperture/run/owner"))
            .read_owner(&snapshot.seat)
            .map_err(|_| ReplacementError::AuthorizationRequired)?;
        if owner.generation != snapshot.generation || owner.state != OwnerState::Active {
            return Err(ReplacementError::GenerationMismatch);
        }
        // Tuple confirmation in a UI is intent, not a durable operator grant.
        // No native grant for changing these controls has been frozen yet.
        unchanged_execution_controls(&owner.requested, &selected)?;
        selected_in_snapshot(&team, &snapshot.seat, &selected)
    }
    fn start_fresh(
        &mut self,
        snapshot: &OwnershipSnapshot,
        selection: &StartSelection,
    ) -> Result<StartedCandidate, ReplacementError> {
        self.matches_target(snapshot)?;
        self.admission()?;
        let selected = tuple(selection)?;
        let binding = require_repository_binding(self.home, &self.target, self.attempt.budget())?;
        let checkpoint = checkpoint_for_target(
            self.home,
            &self.authority,
            &self.target,
            &binding.0,
            self.sentinels,
            self.attempt.budget(),
        )?;
        if checkpoint.worktree != self.checkpoint.worktree {
            return Err(ReplacementError::CheckpointUnavailable);
        }
        let mut plan = NativePlan::preflight_selected(
            self.home,
            &self.target.team,
            &self.target.seat,
            &selected,
            &binding.0,
            Some(checkpoint.worktree.as_str()),
            self.attempt.budget(),
        )?;
        plan.bind_recovery(checkpoint.entry.as_ref())?;
        self.plan.revalidate(self.attempt.budget())?;
        self.attempt.budget().forward(Duration::from_secs(90))?;
        self.attempt.admit_effects()?;
        let old_owner = OwnerStore::new(self.home.join(".aperture/run/owner"))
            .read_owner(&self.target.seat)
            .map_err(|_| ReplacementError::GenerationMismatch)?;
        if old_owner.generation != self.target.expected_generation {
            return Err(ReplacementError::GenerationMismatch);
        }
        // Preserve the locked process-proof boundary for both harnesses. No
        // Claude socket is expected, but its absence is not stop evidence.
        self.with_stop_guard(snapshot, |guard| match old_owner.requested.harness {
            Harness::Codex => launch::release_stopped_socket(self.home, guard),
            Harness::Claude => claude_stopped(guard.snapshot(), team_process::state),
        })?;
        {
            let _team = crate::owner::try_lock(
                &self.home.join(".aperture/run/team-locks"),
                &self.target.team,
            )
            .map_err(|_| ReplacementError::NativeFailure)?;
            self.authority.revalidate(&self.target)?;
            let store = OwnerStore::new(self.home.join(".aperture/run/owner"));
            store
                .mark_stale(
                    &AuthenticatedActor::launcher(),
                    &self.target.seat,
                    self.target.expected_generation,
                )
                .map_err(|_| ReplacementError::GenerationMismatch)?;
        }
        let started = start_native(
            self.home,
            &self.target.team,
            &self.target.seat,
            self.target.expected_generation,
            selected,
            &plan,
            &self.attempt,
            None,
        )?;
        let result = started.candidate.clone();
        self.started = Some(started);
        Ok(result)
    }
    fn activate_started(&mut self, candidate: &StartedCandidate) -> Result<(), ReplacementError> {
        let started = self
            .started
            .as_ref()
            .ok_or(ReplacementError::ModelUnverified)?;
        if started.candidate.process != candidate.process
            || started.candidate.token_id != candidate.token_id
            || started.candidate.observed.thread_id != candidate.observed.thread_id
        {
            return Err(ReplacementError::ModelUnverified);
        }
        activate_native(self.home, &self.target.team, started, &self.attempt, None)
    }
    fn abort_started(&mut self, candidate: &StartedCandidate) -> Result<(), ReplacementError> {
        let started = self
            .started
            .as_mut()
            .ok_or(ReplacementError::StartCleanupUnverified)?;
        if started.candidate.process != candidate.process
            || started.candidate.token_id != candidate.token_id
        {
            return Err(ReplacementError::StartCleanupUnverified);
        }
        cleanup_native(
            self.home,
            &self.target.team,
            &started.reservation,
            started.child.as_mut(),
            self.attempt.budget().cleanup_until(),
        )
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RevokedState {
    schema_version: u32,
    seat: String,
    revoked_through_generation: u64,
    revoked_token_ids: Vec<String>,
}
pub(crate) fn revoked_metadata(
    home: &Path,
    snapshot: &OwnershipSnapshot,
) -> Result<bool, ReplacementError> {
    let bad = || ReplacementError::RevocationUnverified;
    let owner: OwnerRecord = read_private_json(
        &home
            .join(".aperture/run/owner")
            .join(format!("{}.json", snapshot.seat)),
    )
    .map_err(|_| bad())?;
    let inc = owner.incarnation.as_ref().ok_or_else(bad)?;
    if owner.state != OwnerState::Active
        || owner.generation != snapshot.generation
        || owner.seat != snapshot.seat
        || inc.thread_id != snapshot.thread_id
    {
        return Err(bad());
    }
    let path = validate_component_path(
        &home.join(".aperture/run/revocations"),
        &format!("{}.json", snapshot.seat),
        false,
    )
    .map_err(|_| bad())?;
    let value: RevokedState = read_private_json(&path).map_err(|_| bad())?;
    let mut sorted = value.revoked_token_ids.clone();
    sorted.sort();
    sorted.dedup();
    if value.schema_version != 1
        || value.seat != snapshot.seat
        || value.revoked_through_generation != snapshot.generation
        || sorted != value.revoked_token_ids
        || sorted.len() > 4096
        || sorted.iter().any(|v| {
            v.len() != 64
                || !v
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
        || !sorted.contains(&inc.token_id)
    {
        return Err(bad());
    }
    let token = validate_component_path(
        &home.join(".aperture/run/hub-tokens"),
        &format!("{}.token", snapshot.seat),
        true,
    )
    .map_err(|_| bad())?;
    match std::fs::symlink_metadata(token) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(true),
        _ => Err(bad()),
    }
}

#[cfg(test)]
#[path = "team_replacement_native_tests.rs"]
mod tests;
