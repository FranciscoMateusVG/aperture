//! Fixed native replace budget and durable admission evidence. This uses the
//! existing private append-only IO, NOT a second journal for owner/team moves.
//! A missing terminal fact is UNKNOWN, never permission to repeat an attempt.
use super::ReplacementError;
use crate::journal::{
    ensure_private_dir, read_private_json, validate_component_path, write_private_json_atomic,
};
use crate::owner::{try_lock, OwnerRecord, OwnerStore};
use crate::state::OwnerState;
use crate::team_auth::AuthenticatedActor;
use crate::teams::{classify_managed_seat, ManagedSeatState};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const TOTAL: Duration = Duration::from_secs(170);
// Two native collectors10+10, TERM10/KILL1, hub3, bounded metadata margin6.
const CLEANUP: Duration = Duration::from_secs(40);
// Stop11 + post-stop collection10 + revoke3 + launch/observe90 + metadata2.
const FORWARD_AFTER_FIRST_EFFECT: Duration = Duration::from_secs(116);

pub(crate) struct Deadline {
    started: Instant,
}
impl Deadline {
    pub(crate) fn new() -> Self {
        Self {
            started: Instant::now(),
        }
    }
    fn remaining_at(&self, now: Instant) -> Duration {
        TOTAL.saturating_sub(now.saturating_duration_since(self.started))
    }
    pub(crate) fn remaining(&self) -> Duration {
        self.remaining_at(Instant::now())
    }
    pub(crate) fn forward(&self, required: Duration) -> Result<(), ReplacementError> {
        self.forward_at(required, Instant::now())
    }
    fn forward_at(&self, required: Duration, now: Instant) -> Result<(), ReplacementError> {
        if self.remaining_at(now) <= CLEANUP.saturating_add(required) {
            Err(ReplacementError::Deadline)
        } else {
            Ok(())
        }
    }
    /// Native subprocesses share this deadline, never a fresh per-call 170s.
    pub(crate) fn forward_until(&self, cap: Duration) -> Result<Instant, ReplacementError> {
        self.forward(Duration::ZERO)?;
        Ok((Instant::now() + cap).min(self.started + TOTAL - CLEANUP))
    }
    pub(crate) fn cleanup_until(&self) -> Instant {
        self.started + TOTAL
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Admission {
    schema_version: u32,
    attempt_id: String,
    team: String,
    seat: String,
    old_generation: u64,
    admitted_at_ms: i64,
    native_budget_ms: u64,
    cleanup_reserve_ms: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum FactKind {
    EffectsMayHaveOccurred,
    Ready,
    Active,
    Failed,
    Unknown,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Fact {
    schema_version: u32,
    attempt_id: String,
    kind: FactKind,
}

/// Unforgeable in callers: no Deserialize/Clone, native constructor only.
/// Drop does not write success, remove evidence or retry cleanup.
pub(crate) struct RuntimeAttempt {
    home: PathBuf,
    dir: PathBuf,
    admitted: Admission,
    budget: Deadline,
    effects_admitted: bool,
    terminal: bool,
}
/// Clock-free, opaque, one-use human preparation authority. No caller fields.
pub(crate) struct ReadyAttempt {
    home: PathBuf,
    dir: PathBuf,
    admitted: Admission,
}
impl ReadyAttempt {
    pub(crate) fn start(self, budget: Deadline) -> Result<RuntimeAttempt, ReplacementError> {
        let expired = || ReplacementError::PreparationExpired;
        budget.forward(Duration::ZERO)?;
        let _team = try_lock(
            &self.home.join(".aperture/run/team-locks"),
            &self.admitted.team,
        )
        .map_err(|_| expired())?;
        let store = OwnerStore::new(self.home.join(".aperture/run/owner"));
        let _seat = store.lock(&self.admitted.seat).map_err(|_| expired())?;
        let current: Admission =
            read_private_json(&self.dir.join("admitted.json")).map_err(|_| expired())?;
        let terminal: Fact =
            read_private_json(&self.dir.join("terminal.json")).map_err(|_| expired())?;
        if current != self.admitted
            || terminal
                != (Fact {
                    schema_version: 1,
                    attempt_id: self.admitted.attempt_id.clone(),
                    kind: FactKind::Ready,
                })
        {
            return Err(expired());
        }
        match classify_managed_seat(&self.home, &self.admitted.seat).map_err(|_| expired())? {
            Some(ManagedSeatState::Active { team, .. }) if team == self.admitted.team => {}
            _ => return Err(expired()),
        }
        let owner: OwnerRecord =
            read_private_json(&store.record_path(&self.admitted.seat)).map_err(|_| expired())?;
        if owner.schema_version != 1
            || owner.seat != self.admitted.seat
            || owner.generation != self.admitted.old_generation
            || owner.state != OwnerState::Active
            || owner.provisional_token_id.is_some()
            || !owner.incarnation.as_ref().is_some_and(|i| {
                i.observed
                    && i.model == owner.requested.model
                    && i.harness == owner.requested.harness
                    && i.reasoning == owner.requested.reasoning
            })
        {
            return Err(expired());
        }
        let dir = validate_component_path(&self.dir, "start", true).map_err(|_| expired())?;
        if !matches!(std::fs::symlink_metadata(&dir),Err(e) if e.kind()==std::io::ErrorKind::NotFound)
        {
            return Err(expired());
        }
        ensure_private_dir(&dir).map_err(|_| expired())?;
        let mut admitted = self.admitted.clone();
        admitted.attempt_id = uuid::Uuid::new_v4().to_string();
        admitted.admitted_at_ms = chrono::Utc::now().timestamp_millis();
        RuntimeAttempt::publish(&self.home, dir, admitted, budget)
    }
}
impl RuntimeAttempt {
    /// Called only after native action authority, repo and launch preflight.
    /// Launcher identity and current managed owner are rechecked under locks.
    pub(crate) fn begin(
        home: &Path,
        actor: &AuthenticatedActor,
        team: &str,
        seat: &str,
        generation: u64,
        budget: Deadline,
    ) -> Result<Self, ReplacementError> {
        if generation == 0 {
            return Err(ReplacementError::GenerationMismatch);
        }
        Self::begin_checked(home, actor, team, seat, generation, budget, false)
    }
    pub(crate) fn begin_bootstrap(
        home: &Path,
        actor: &AuthenticatedActor,
        team: &str,
        seat: &str,
        budget: Deadline,
    ) -> Result<Self, ReplacementError> {
        Self::begin_checked(home, actor, team, seat, 0, budget, true)
    }
    fn begin_checked(
        home: &Path,
        actor: &AuthenticatedActor,
        team: &str,
        seat: &str,
        generation: u64,
        budget: Deadline,
        bootstrap: bool,
    ) -> Result<Self, ReplacementError> {
        if !actor.is_launcher()
            || team.len() > 16
            || !crate::agent_loader::is_valid_seat_name(team)
            || !crate::agent_loader::is_valid_seat_name(seat)
        {
            return Err(ReplacementError::AuthorizationRequired);
        }
        budget.forward(Duration::ZERO)?;
        let _team = try_lock(&home.join(".aperture/run/team-locks"), team)
            .map_err(|_| ReplacementError::NativeFailure)?;
        match classify_managed_seat(home, seat)
            .map_err(|_| ReplacementError::AuthorizationRequired)?
        {
            Some(ManagedSeatState::Active { team: actual, .. }) if actual == team => {}
            _ => return Err(ReplacementError::AuthorizationRequired),
        }
        let store = OwnerStore::new(home.join(".aperture/run/owner"));
        let _seat = store
            .lock(seat)
            .map_err(|_| ReplacementError::NativeFailure)?;
        let owner: OwnerRecord = read_private_json(&store.record_path(seat))
            .map_err(|_| ReplacementError::GenerationMismatch)?;
        let state_ok = if bootstrap {
            owner.generation == 0
                && owner.state == OwnerState::Stale
                && owner.incarnation.is_none()
                && owner.reservation_nonce_sha256.is_none()
                && owner.provisional_token_id.is_none()
        } else {
            owner.state == OwnerState::Active
                && owner.provisional_token_id.is_none()
                && owner.incarnation.as_ref().is_some_and(|i| i.observed)
        };
        if owner.schema_version != 1
            || owner.seat != seat
            || owner.generation != generation
            || !state_ok
        {
            return Err(ReplacementError::GenerationMismatch);
        }
        if bootstrap {
            let snapshot: crate::teams::TeamSnapshot =
                read_private_json(&home.join(".aperture/teams").join(team).join("team.json"))
                    .map_err(|_| ReplacementError::GenerationMismatch)?;
            let seats: Vec<_> = snapshot.seats.iter().filter(|s| s.name == seat).collect();
            if snapshot.team != team
                || seats.len() != 1
                || owner.requested.harness != seats[0].harness
                || owner.requested.model != seats[0].model
                || owner.requested.reasoning != seats[0].reasoning
            {
                return Err(ReplacementError::GenerationMismatch);
            }
        }
        let team_dir = validate_component_path(&home.join(".aperture/teams"), team, false)
            .map_err(|_| ReplacementError::NativeFailure)?;
        let attempts = validate_component_path(&team_dir, "runtime-attempts", true)
            .map_err(|_| ReplacementError::NativeFailure)?;
        ensure_private_dir(&attempts).map_err(|_| ReplacementError::NativeFailure)?;
        let seat_dir = validate_component_path(&attempts, seat, true)
            .map_err(|_| ReplacementError::NativeFailure)?;
        ensure_private_dir(&seat_dir).map_err(|_| ReplacementError::NativeFailure)?;
        let dir = validate_component_path(&seat_dir, &format!("g{generation}"), true)
            .map_err(|_| ReplacementError::NativeFailure)?;
        // Even an incomplete directory left before admission is retained and
        // blocked. No blind resume/retry, no deletion of crash evidence.
        match std::fs::symlink_metadata(&dir) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            _ => return Err(ReplacementError::OutcomeUnknown),
        }
        ensure_private_dir(&dir).map_err(|_| ReplacementError::OutcomeUnknown)?;
        Self::publish(
            home,
            dir,
            Admission {
                schema_version: 1,
                attempt_id: uuid::Uuid::new_v4().to_string(),
                team: team.into(),
                seat: seat.into(),
                old_generation: generation,
                admitted_at_ms: chrono::Utc::now().timestamp_millis(),
                native_budget_ms: 170_000,
                cleanup_reserve_ms: CLEANUP.as_millis() as u64,
            },
            budget,
        )
    }
    fn publish(
        home: &Path,
        dir: PathBuf,
        admitted: Admission,
        budget: Deadline,
    ) -> Result<Self, ReplacementError> {
        write_private_json_atomic(&dir.join("admitted.json"), &admitted, false)
            .map_err(|_| ReplacementError::OutcomeUnknown)?;
        let actual: Admission = read_private_json(&dir.join("admitted.json"))
            .map_err(|_| ReplacementError::OutcomeUnknown)?;
        if actual != admitted {
            return Err(ReplacementError::OutcomeUnknown);
        }
        Ok(Self {
            home: home.into(),
            dir,
            admitted,
            budget,
            effects_admitted: false,
            terminal: false,
        })
    }
    /// Human prepare is terminal and factual; no clock runs while a person
    /// reads the dialog. The retained native permit is consumed by Start.
    pub(crate) fn finish_ready(mut self) -> Result<ReadyAttempt, ReplacementError> {
        self.budget.forward(Duration::ZERO)?;
        if self.terminal {
            return Err(ReplacementError::OutcomeUnknown);
        }
        self.fact("terminal.json", FactKind::Ready)?;
        self.terminal = true;
        Ok(ReadyAttempt {
            home: self.home,
            dir: self.dir,
            admitted: self.admitted,
        })
    }
    pub(crate) fn effects_admitted(&self) -> bool {
        self.effects_admitted
    }
    pub(crate) fn budget(&self) -> &Deadline {
        &self.budget
    }
    pub(crate) fn id(&self) -> &str {
        &self.admitted.attempt_id
    }
    fn fact(&self, name: &str, kind: FactKind) -> Result<(), ReplacementError> {
        let _team = try_lock(
            &self.home.join(".aperture/run/team-locks"),
            &self.admitted.team,
        )
        .map_err(|_| ReplacementError::OutcomeUnknown)?;
        let store = OwnerStore::new(self.home.join(".aperture/run/owner"));
        let _seat = store
            .lock(&self.admitted.seat)
            .map_err(|_| ReplacementError::OutcomeUnknown)?;
        let actual: Admission = read_private_json(&self.dir.join("admitted.json"))
            .map_err(|_| ReplacementError::OutcomeUnknown)?;
        if actual != self.admitted {
            return Err(ReplacementError::OutcomeUnknown);
        }
        if matches!(kind, FactKind::Active | FactKind::Ready) {
            let owner: OwnerRecord = read_private_json(&store.record_path(&self.admitted.seat))
                .map_err(|_| ReplacementError::OutcomeUnknown)?;
            if owner.schema_version != 1
                || owner.seat != self.admitted.seat
                || owner.generation
                    != self
                        .admitted
                        .old_generation
                        .checked_add(if kind == FactKind::Active { 1 } else { 0 })
                        .ok_or(ReplacementError::OutcomeUnknown)?
                || owner.state != OwnerState::Active
                || owner.provisional_token_id.is_some()
                || !owner.incarnation.as_ref().is_some_and(|i| {
                    i.observed
                        && i.harness == owner.requested.harness
                        && i.model == owner.requested.model
                        && i.reasoning == owner.requested.reasoning
                })
            {
                return Err(ReplacementError::OutcomeUnknown);
            }
            self.budget.forward(Duration::ZERO)?;
        }
        let fact = Fact {
            schema_version: 1,
            attempt_id: self.id().into(),
            kind,
        };
        write_private_json_atomic(&self.dir.join(name), &fact, false)
            .map_err(|_| ReplacementError::OutcomeUnknown)?;
        if read_private_json::<Fact>(&self.dir.join(name))
            .map_err(|_| ReplacementError::OutcomeUnknown)?
            != fact
        {
            return Err(ReplacementError::OutcomeUnknown);
        }
        Ok(())
    }
    /// Persist before the FIRST signal/reservation/token/launch effect. Repeated
    /// calls only enforce the shared deadline; they never renew the budget.
    pub(crate) fn admit_effects(&mut self) -> Result<(), ReplacementError> {
        if self.terminal {
            return Err(ReplacementError::OutcomeUnknown);
        }
        if !self.effects_admitted {
            self.budget.forward(FORWARD_AFTER_FIRST_EFFECT)?;
            self.effects_admitted = true;
            self.fact("effects.json", FactKind::EffectsMayHaveOccurred)?;
        }
        self.budget.forward(Duration::ZERO)
    }
    /// Call after native owner Active readback, not based on an RPC promise.
    pub(crate) fn finish_active(&mut self) -> Result<(), ReplacementError> {
        self.budget.forward(Duration::ZERO)?;
        if !self.effects_admitted {
            return Err(ReplacementError::OutcomeUnknown);
        }
        self.finish(FactKind::Active)
    }
    /// Only a pre-effect rejection can be completed without a native cleanup
    /// proof. Post-effect failures stay UNKNOWN until reconciliation is wired.
    pub(crate) fn finish_failed(&mut self) -> Result<(), ReplacementError> {
        if self.effects_admitted || self.budget.remaining().is_zero() {
            return self.finish_unknown();
        }
        self.finish(FactKind::Failed)
    }
    pub(crate) fn finish_unknown(&mut self) -> Result<(), ReplacementError> {
        self.finish(FactKind::Unknown)?;
        Err(ReplacementError::OutcomeUnknown)
    }
    fn finish(&mut self, kind: FactKind) -> Result<(), ReplacementError> {
        if self.terminal {
            return Err(ReplacementError::OutcomeUnknown);
        }
        self.fact("terminal.json", kind)?;
        self.terminal = true;
        Ok(())
    }
}

#[cfg(test)]
#[path = "team_runtime_deadline_tests.rs"]
mod tests;
