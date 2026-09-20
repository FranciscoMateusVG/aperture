//! Concrete native composition, deliberately NOT registered as a command.
//! Missing repository authority fails before collection or lifecycle effects.
//! Neither project labels, worker checkpoints nor observed cwd are authority.
use super::*;
use crate::journal::{read_private_json, validate_component_path};
use crate::owner::{OwnerRecord, OwnerStore};
use crate::state::{ExecutionTuple, OwnerState};
use crate::team_auth::{AuthenticatedActor, AuthenticatedSeat};
use crate::team_process;
use crate::teams::TeamSnapshot;
use std::path::Path;
use std::time::{Duration, Instant};

/// Internal authenticated contexts only; not a command DTO or caller proof.
pub(crate) enum ReplacementAuthority<'a> {
    Operator(&'a AuthenticatedActor),
    Lead(&'a AuthenticatedSeat),
}
impl ReplacementAuthority<'_> {
    fn revalidate(&self, target: &remote::RemoteTarget) -> Result<(), ReplacementError> {
        match self {
            Self::Operator(actor) if actor.principal() == "operator" => Ok(()),
            Self::Lead(actor) if actor.team() == target.team && actor.seat() != target.seat => {
                actor
                    .revalidate_before_effect()
                    .map_err(|_| ReplacementError::AuthorizationRequired)
            }
            _ => Err(ReplacementError::AuthorizationRequired),
        }
    }
    fn remote(&self) -> remote::ResolutionAuthority<'_> {
        match self {
            Self::Operator(actor) => remote::ResolutionAuthority::Operator(actor),
            Self::Lead(actor) => remote::ResolutionAuthority::Lead(actor),
        }
    }
}

// The binding is derived from the immutable native snapshot and single catalog,
// never a caller path. Launch composition remains a distinct fail-closed gate.
struct RepositoryBinding(super::repository::BoundRepository);
fn require_repository_binding(
    home: &Path,
    target: &remote::RemoteTarget,
) -> Result<RepositoryBinding, ReplacementError> {
    super::repository::resolve_native(home, &target.team, Instant::now() + Duration::from_secs(10))
        .map(RepositoryBinding)
        .map_err(|_| ReplacementError::RepoBindingUnavailable)
}
fn require_launch_composition() -> Result<(), ReplacementError> {
    // Do not stop an old incarnation until start/observation/cleanup and the
    // durable timeout boundary are all wired. This is not a capability flag
    // supplied by a UI or caller; registration stays off as well.
    Err(ReplacementError::LaunchUnavailable)
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
/// Missing binding returns its fixed error; valid binding reaches the separate
/// uncomposed-launch error BEFORE locks, stop, revocation, stale CAS, token
/// publication or reservation. Only bounded native Git reads precede it. Capabilities
/// and command registration remain false; this is not functional P3 delivery.
pub(crate) fn replace_authorized(
    home: &Path,
    authority: ReplacementAuthority<'_>,
    target: remote::RemoteTarget,
    selection: &StartSelection,
    sentinels: &[String],
) -> Result<StartedReplacement, ReplacementError> {
    selectors(&target)?;
    let binding = require_repository_binding(home, &target)?;
    require_launch_composition()?;
    let mut runtime = NativeRuntime::new(home, authority, target, sentinels, binding)?;
    let seat = runtime.target.seat.clone();
    let generation = runtime.target.expected_generation;
    let snapshot = runtime.snapshot(&seat, generation)?;
    runtime.authorize_selection(&snapshot, selection)?;
    let prepared = prepare(
        &mut runtime,
        &seat,
        generation,
        &ReplacementPolicy::default(),
    )?;
    // Binding must also survive the stop/checkpoint/reconciliation interval.
    require_repository_binding(home, &runtime.target)?;
    start(&mut runtime, prepared, selection)
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
}
impl<'a> NativeRuntime<'a> {
    fn new(
        home: &'a Path,
        authority: ReplacementAuthority<'a>,
        target: remote::RemoteTarget,
        sentinels: &'a [String],
        binding: RepositoryBinding,
    ) -> Result<Self, ReplacementError> {
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
        };
        runtime.admission()?;
        Ok(runtime)
    }
    fn admission(&self) -> Result<(), ReplacementError> {
        self.authority.revalidate(&self.target)?;
        // Uses the SAME team -> lexical seat locks and immutable lead identity
        // as resolution. This does not accept or synthesize remote authorization.
        remote::inspect_authorized(
            self.home,
            self.authority.remote(),
            &self.target,
            self.sentinels,
        )
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
        std::thread::sleep(Duration::from_millis(ms));
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
        // None means no validated recovery admitted here, not no file exists.
        CheckpointRecovery::None
    }
    fn process_state(&mut self, process: &ProcessIdentity) -> ProcessState {
        team_process::state(process)
    }
    fn signal(
        &mut self,
        process: &ProcessIdentity,
        signal: Signal,
    ) -> Result<(), ReplacementError> {
        let snapshot = self
            .snapshot
            .as_ref()
            .ok_or(ReplacementError::InvalidSnapshot)?;
        if !snapshot.processes.iter().any(|p| p.identity == *process) {
            return Err(ReplacementError::UnownedProcess);
        }
        self.with_stop_guard(snapshot, |guard| {
            team_process::signal_recorded(guard, process, signal)
        })
    }
    fn unowned_matches(&mut self, snapshot: &OwnershipSnapshot) -> Result<bool, ReplacementError> {
        self.matches_target(snapshot)?;
        self.admission()?;
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
        let proof = self.with_stop_guard(snapshot, |guard| {
            crate::ws_hub::managed_control::revoke_stopped(self.home, guard)
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
        _snapshot: &OwnershipSnapshot,
        _selection: &StartSelection,
    ) -> Result<StartedCandidate, ReplacementError> {
        // Do not mark_stale/reserve merely because preparation once succeeded.
        // Authoritative binding and full partial-start cleanup must be wired
        // before this can create a LaunchSpec or enter generation g+1.
        require_repository_binding(self.home, &self.target)?;
        Err(ReplacementError::NativeFailure)
    }
    fn activate_started(&mut self, _candidate: &StartedCandidate) -> Result<(), ReplacementError> {
        Err(ReplacementError::ModelUnverified)
    }
    fn abort_started(&mut self, _candidate: &StartedCandidate) -> Result<(), ReplacementError> {
        // No native candidate can be minted here yet. Never claim cleanup.
        Err(ReplacementError::StartCleanupUnverified)
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
fn revoked_metadata(home: &Path, snapshot: &OwnershipSnapshot) -> Result<bool, ReplacementError> {
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
