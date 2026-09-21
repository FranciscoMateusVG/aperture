//! Concrete native composition, deliberately NOT registered as a command.
//! Missing repository authority fails before collection or lifecycle effects.
//! Neither project labels, worker checkpoints nor observed cwd are authority.
use super::*;
use crate::journal::{read_private_json, validate_component_path};
use crate::owner::{Incarnation, OwnerRecord, OwnerStore, StartReservation};
use crate::state::{ExecutionTuple, OwnerState};
use crate::team_auth::{AuthenticatedActor, AuthenticatedSeat};
use crate::team_process;
use crate::teams::TeamSnapshot;
use std::path::Path;
use std::time::{Duration, Instant};

struct NativeStarted {
    reservation: StartReservation,
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
    if expected_generation != 0 {
        return Err(ReplacementError::GenerationMismatch);
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
        launch::NativeLaunchBinding::preflight(home, team, seat, &selected, &repo, None, &budget)?;
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

fn start_native(
    home: &Path,
    team: &str,
    seat: &str,
    generation: u64,
    selected: ExecutionTuple,
    plan: &launch::NativeLaunchBinding,
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
        if let Some(actor) = bootstrap_actor { authorize_bootstrap(actor)?; }
        store
            .reserve_start(&launcher, seat, generation, selected.clone())
            .map_err(|_| ReplacementError::GenerationMismatch)?
    };
    let mut child = None;
    let result = (|| {
        let token = crate::hub_auth::managed::provision(home, team, &launcher, &reservation)
            .map_err(|_| ReplacementError::NativeFailure)?;
        let spec = plan.publish(&reservation, &token, attempt.budget())?;
        attempt.budget().forward(Duration::from_secs(85))?;
        if let Some(actor) = bootstrap_actor { authorize_bootstrap(actor)?; }
        let pending = launch_gate::spawn(spec).map_err(|_| ReplacementError::OutcomeUnknown)?;
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
        let until = attempt.budget().forward_until(Duration::from_secs(85))?;
        let observation = loop {
            attempt.budget().forward(Duration::ZERO)?;
            if Instant::now() >= until {
                return Err(ReplacementError::ModelUnverified);
            }
            if child
                .as_mut()
                .unwrap()
                .try_wait()
                .map_err(|_| ReplacementError::StartCleanupUnverified)?
                .is_some()
            {
                return Err(ReplacementError::ModelUnverified);
            }
            match model_observation::read_native(home, team, &reservation) {
                Ok(v) => break v.into_runtime_observation(),
                Err(model_observation::ObservationError::Missing) => {
                    std::thread::sleep(Duration::from_millis(25))
                }
                Err(_) => return Err(ReplacementError::ModelUnverified),
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
            actual_harness: Some("codex".into()),
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
    attempt.budget().forward(Duration::from_secs(1))?;
    let observation = model_observation::read_native(home, team, &started.reservation)
        .map_err(|_| ReplacementError::ModelUnverified)?;
    let store = OwnerStore::new(home.join(".aperture/run/owner"));
    let launcher = AuthenticatedActor::launcher();
    store
        .record_runtime_observation(
            &launcher,
            &started.reservation,
            observation.into_runtime_observation(),
        )
        .map_err(|_| ReplacementError::ModelUnverified)?;
    let _team = lock_activation(home, team, bootstrap_actor)?;
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
    let budget = deadline::Deadline::new();
    selectors(&target)?;
    let binding = require_repository_binding(home, &target, &budget)?;
    authority.revalidate(&target)?;
    let checkpoint =
        checkpoint_for_target(home, &authority, &target, &binding.0, sentinels, &budget)?;
    let mut plan = launch::NativeLaunchBinding::preflight_selected(
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
    plan: launch::NativeLaunchBinding,
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
    let target = remote::RemoteTarget {
        team: team.into(),
        seat: seat.into(),
        expected_generation,
    };
    selectors(&target)?;
    let budget = deadline::Deadline::new();
    let authority = ReplacementAuthority::Operator(actor);
    let binding = require_repository_binding(home, &target, &budget)?;
    authority.revalidate(&target)?;
    let owner = OwnerStore::new(home.join(".aperture/run/owner"))
        .read_owner(seat)
        .map_err(|_| ReplacementError::GenerationMismatch)?;
    if owner.generation != expected_generation || owner.state != OwnerState::Active {
        return Err(ReplacementError::GenerationMismatch);
    }
    let checkpoint =
        checkpoint_for_target(home, &authority, &target, &binding.0, sentinels, &budget)?;
    let mut plan = launch::NativeLaunchBinding::preflight_selected(
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
    let prepared = match prepare(
        &mut runtime,
        seat,
        expected_generation,
        &ReplacementPolicy::default(),
    ) {
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
    plan: launch::NativeLaunchBinding,
    checkpoint: RecoveryContext,
    started: Option<NativeStarted>,
}
impl<'a> NativeRuntime<'a> {
    fn new(
        home: &'a Path,
        authority: ReplacementAuthority<'a>,
        target: remote::RemoteTarget,
        sentinels: &'a [String],
        binding: RepositoryBinding,
        budget: deadline::Deadline,
        plan: launch::NativeLaunchBinding,
        checkpoint: RecoveryContext,
    ) -> Result<Self, ReplacementError> {
        authority.revalidate(&target)?;
        remote::inspect_authorized(home, authority.remote(), &target, sentinels)
            .map_err(|_| ReplacementError::AuthorizationRequired)?;
        let attempt = deadline::RuntimeAttempt::begin(
            home,
            &AuthenticatedActor::launcher(),
            &target.team,
            &target.seat,
            target.expected_generation,
            budget,
        )?;
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
        };
        runtime.admission()?;
        Ok(runtime)
    }
    fn admission(&self) -> Result<(), ReplacementError> {
        self.attempt.budget().forward(Duration::ZERO)?;
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
        })
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
        let mut plan = launch::NativeLaunchBinding::preflight_selected(
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
        self.with_stop_guard(snapshot, |guard| {
            launch::release_stopped_socket(self.home, guard)
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
