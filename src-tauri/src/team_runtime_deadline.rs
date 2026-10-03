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
    #[cfg(test)]
    pub(super) fn fixture_elapsed(elapsed: Duration) -> Self {
        Self { started: Instant::now() - elapsed }
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
    SmokeCleaned,
    StoppedReconciled,
}

/// Read-only admission for reconciling an already stopped ordinary attempt.
/// This grants no renewed RuntimeAttempt and never rewrites UNKNOWN. Call only
/// under the target's team/seat locks; stopped/revoked proof is separate.
pub(crate) fn expired_unknown_stop_locked(
    home: &Path, team: &str, seat: &str, generation: u64,
) -> Result<(), ReplacementError> {
    let deny = || ReplacementError::OutcomeUnknown;
    if generation == 0 { return Err(deny()); }
    let dir = home.join(".aperture/teams").join(team).join("runtime-attempts")
        .join(seat).join(format!("g{generation}"));
    // Ordinary UNKNOWN stays on the original exact-three-fact path. Only the
    // canonical retirement child selects the separate closed history grammar.
    match std::fs::symlink_metadata(dir.join("retirement")) {
        Ok(_) => return expired_retirement_stop_locked(&dir, team, seat, generation),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
        Err(_) => return Err(deny()),
    }
    let a: Admission = read_private_json(&dir.join("admitted.json")).map_err(|_| deny())?;
    let e: Fact = read_private_json(&dir.join("effects.json")).map_err(|_| deny())?;
    let t: Fact = read_private_json(&dir.join("terminal.json")).map_err(|_| deny())?;
    let now = chrono::Utc::now().timestamp_millis();
    if a.schema_version != 1 || a.team != team || a.seat != seat || a.old_generation != generation
        || !crate::team_claude_launch::canonical_uuid(&a.attempt_id)
        || a.native_budget_ms != TOTAL.as_millis() as u64
        || a.cleanup_reserve_ms != CLEANUP.as_millis() as u64
        || a.admitted_at_ms <= 0
        || a.admitted_at_ms.checked_add(TOTAL.as_millis() as i64 + 10_000).is_none_or(|end| now <= end)
        || e.schema_version != 1 || e.attempt_id != a.attempt_id || e.kind != FactKind::EffectsMayHaveOccurred
        || t.schema_version != 1 || t.attempt_id != a.attempt_id || t.kind != FactKind::Unknown
    { return Err(deny()); }
    // Do not reinterpret prepared, nested/retried, bootstrap reconciliation or
    // retirement state. The narrow ordinary UNKNOWN case has these three files.
    let names = std::fs::read_dir(&dir).map_err(|_| deny())?.take(4)
        .map(|entry| entry.map(|entry| entry.file_name()).map_err(|_| deny()))
        .collect::<Result<Vec<_>, _>>()?;
    if names.len() != 3 || names.iter().any(|name| !["admitted.json", "effects.json", "terminal.json"]
        .iter().any(|expected| name == expected)) { return Err(deny()); }
    Ok(())
}

// Read-only, closed retirement grammar. No RuntimeAttempt/UUID, writer or
// effect authority is created. Callers retain the team/seat locks throughout.
fn expired_retirement_stop_locked(
    root: &Path, team: &str, seat: &str, generation: u64,
) -> Result<(), ReplacementError> {
    use std::os::unix::fs::MetadataExt;
    let deny = || ReplacementError::OutcomeUnknown;
    // Keep directory identities/names and exact file bytes for a final recheck.
    // This detects drift; it is not an atomic filesystem transaction.
    let mut directories = Vec::new();
    let mut facts = Vec::new();
    let mut seen_ids = std::collections::HashSet::new();
    let now = chrono::Utc::now().timestamp_millis();
    let mut previous_time = 0;
    let mut dir = root.to_path_buf();
    let mut found_unknown = false;
    for level in 0..=32 {
        let meta = std::fs::symlink_metadata(&dir).map_err(|_| deny())?;
        if !meta.is_dir() || meta.file_type().is_symlink()
            || meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o077 != 0
        { return Err(deny()); }
        let mut names = std::fs::read_dir(&dir).map_err(|_| deny())?.take(5)
            .map(|e| e.map(|e| e.file_name()).map_err(|_| deny()))
            .collect::<Result<Vec<_>, _>>()?;
        names.sort();
        let matches = |expected: &[&str]| names.len() == expected.len()
            && expected.iter().all(|n| names.iter().any(|actual| actual == n));
        directories.push((dir.clone(), meta.dev(), meta.ino(), meta.mtime(), meta.mtime_nsec(),
            meta.ctime(), meta.ctime_nsec(), names.clone()));
        if level == 0 && matches(&["retirement"]) {
            dir = dir.join("retirement");
            continue;
        }
        let child = if level == 0 { "retirement" } else { "reprepare" };
        let failed = matches(&["admitted.json", "terminal.json", child]);
        let unknown = level > 0 && matches(&["admitted.json", "effects.json", "terminal.json"]);
        if !failed && !unknown { return Err(deny()); }
        let mut read = |name: &str| -> Result<Vec<u8>, ReplacementError> {
            use std::io::Read;
            let path = dir.join(name);
            // Existing nofollow private-file/ancestor validation; facts are
            // bounded independently of the maximum private JSON record size.
            let mut file = crate::journal::open_private_file_nofollow(&path).map_err(|_| deny())?;
            let before = file.metadata().map_err(|_| deny())?;
            let mut bytes = Vec::new();
            (&mut file).take(16 * 1024 + 1).read_to_end(&mut bytes).map_err(|_| deny())?;
            let after = file.metadata().map_err(|_| deny())?;
            if bytes.len() > 16 * 1024 || before.len() != bytes.len() as u64
                || history_pin(&before) != history_pin(&after) { return Err(deny()); }
            facts.push((path, history_pin(&after), bytes.clone()));
            Ok(bytes)
        };
        let a: Admission = serde_json::from_slice(&read("admitted.json")?).map_err(|_| deny())?;
        let t: Fact = serde_json::from_slice(&read("terminal.json")?).map_err(|_| deny())?;
        if a.schema_version != 1 || a.team != team || a.seat != seat || a.old_generation != generation
            || !crate::team_claude_launch::canonical_uuid(&a.attempt_id) || !seen_ids.insert(a.attempt_id.clone()) || seen_ids.len() > 32
            || a.native_budget_ms != TOTAL.as_millis() as u64 || a.cleanup_reserve_ms != CLEANUP.as_millis() as u64
            || a.admitted_at_ms <= 0 || a.admitted_at_ms > now || a.admitted_at_ms < previous_time
            || t.schema_version != 1 || t.attempt_id != a.attempt_id
        { return Err(deny()); }
        previous_time = a.admitted_at_ms;
        if unknown {
            let e: Fact = serde_json::from_slice(&read("effects.json")?).map_err(|_| deny())?;
            if t.kind != FactKind::Unknown || e.schema_version != 1 || e.attempt_id != a.attempt_id
                || e.kind != FactKind::EffectsMayHaveOccurred
                || a.admitted_at_ms.checked_add(TOTAL.as_millis() as i64 + 10_000).is_none_or(|end| now <= end)
            { return Err(deny()); }
            found_unknown = true;
            break;
        }
        if t.kind != FactKind::Failed { return Err(deny()); }
        dir = dir.join(child);
    }
    if !found_unknown { return Err(deny()); }
    for (path, pin, bytes) in facts {
        use std::io::Read;
        let mut file = crate::journal::open_private_file_nofollow(&path).map_err(|_| deny())?;
        if history_pin(&file.metadata().map_err(|_| deny())?) != pin { return Err(deny()); }
        let mut actual = Vec::new();
        (&mut file).take(16 * 1024 + 1).read_to_end(&mut actual).map_err(|_| deny())?;
        if actual != bytes || history_pin(&file.metadata().map_err(|_| deny())?) != pin { return Err(deny()); }
    }
    for (path, dev, ino, mt, mn, ct, cn, names) in directories {
        let m = std::fs::symlink_metadata(&path).map_err(|_| deny())?;
        let mut actual = std::fs::read_dir(&path).map_err(|_| deny())?.take(5)
            .map(|e| e.map(|e| e.file_name()).map_err(|_| deny())).collect::<Result<Vec<_>, _>>()?;
        actual.sort();
        if !m.is_dir() || m.file_type().is_symlink() || m.uid() != unsafe { libc::geteuid() }
            || m.mode() & 0o077 != 0 || (m.dev(),m.ino(),m.mtime(),m.mtime_nsec(),m.ctime(),m.ctime_nsec()) != (dev,ino,mt,mn,ct,cn)
            || actual != names { return Err(deny()); }
    }
    Ok(())
}
fn history_pin(m: &std::fs::Metadata) -> (u64,u64,u32,u32,u64,u64,i64,i64,i64,i64) {
    use std::os::unix::fs::MetadataExt;
    (m.dev(),m.ino(),m.uid(),m.mode(),m.nlink(),m.len(),m.mtime(),m.mtime_nsec(),m.ctime(),m.ctime_nsec())
}

/// Read-only handle to an EXPIRED bootstrap, never a renewed RuntimeAttempt.
/// Recovery preserves the original terminal (including UNKNOWN) and cannot
/// mint an observation PASS or another start permit.
pub(crate) struct UnfinishedBootstrap { dir: PathBuf, admission: Admission }
impl UnfinishedBootstrap {
    /// The historical smoke/reconcile handle: g0 ONLY. Diagnostic recovery
    /// (`reconcile_stopped_claude_smoke`) is never widened to a later generation.
    pub(crate) fn read_locked(home: &Path, team: &str, seat: &str) -> Result<Self, ReplacementError> {
        Self::read_locked_at(home, team, seat, 0)
    }
    /// Private parameterized reader: the expired bootstrap admission at
    /// `runtime-attempts/<seat>/g<old_generation>` (a first start at g0, or the
    /// explicit recovery admission at g1). Same facts for both: bootstrap
    /// budget, effects, terminal `unknown` or absent-expired, never reconciled.
    fn read_locked_at(home: &Path, team: &str, seat: &str, old_generation: u64) -> Result<Self, ReplacementError> {
        let dir = home.join(".aperture/teams").join(team).join("runtime-attempts").join(seat).join(format!("g{old_generation}"));
        let a: Admission = read_private_json(&dir.join("admitted.json")).map_err(|_| ReplacementError::OutcomeUnknown)?;
        let f: Fact = read_private_json(&dir.join("effects.json")).map_err(|_| ReplacementError::OutcomeUnknown)?;
        let now = chrono::Utc::now().timestamp_millis();
        if a.schema_version != 1 || a.team != team || a.seat != seat || a.old_generation != old_generation
            || !crate::team_claude_launch::canonical_uuid(&a.attempt_id)
            || a.native_budget_ms != 170_000 || a.cleanup_reserve_ms != 40_000
            || a.admitted_at_ms <= 0 || a.admitted_at_ms.checked_add(180_000).is_none_or(|end| now <= end)
            || f.schema_version != 1 || f.attempt_id != a.attempt_id || f.kind != FactKind::EffectsMayHaveOccurred {
            return Err(ReplacementError::OutcomeUnknown);
        }
        match std::fs::symlink_metadata(dir.join("terminal.json")) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
            Ok(_) => {
                let t: Fact = read_private_json(&dir.join("terminal.json")).map_err(|_| ReplacementError::OutcomeUnknown)?;
                if t.schema_version != 1 || t.attempt_id != a.attempt_id || t.kind != FactKind::Unknown {
                    return Err(ReplacementError::OutcomeUnknown);
                }
            },
            _ => return Err(ReplacementError::OutcomeUnknown),
        }
        if !matches!(std::fs::symlink_metadata(dir.join("reconciled.json")), Err(e) if e.kind()==std::io::ErrorKind::NotFound) {
            return Err(ReplacementError::OutcomeUnknown);
        }
        Ok(Self {dir, admission:a})
    }
    pub(crate) fn revalidate_locked(&self, home: &Path) -> Result<(), ReplacementError> {
        let current = Self::read_locked(home, &self.admission.team, &self.admission.seat)?;
        if current.admission != self.admission || current.dir != self.dir { return Err(ReplacementError::OutcomeUnknown); }
        Ok(())
    }
    pub(crate) fn record_locked(&self, home: &Path) -> Result<(), ReplacementError> {
        self.revalidate_locked(home)?;
        let fact = Fact {schema_version:1, attempt_id:self.admission.attempt_id.clone(), kind:FactKind::StoppedReconciled};
        write_private_json_atomic(&self.dir.join("reconciled.json"), &fact, false).map_err(|_| ReplacementError::OutcomeUnknown)?;
        if read_private_json::<Fact>(&self.dir.join("reconciled.json")).map_err(|_| ReplacementError::OutcomeUnknown)? != fact { return Err(ReplacementError::OutcomeUnknown); }
        Ok(())
    }
}
/// Read-only proof that the PREVIOUS bootstrap admission of an explicit
/// recovery expired without a verdict: `old_generation` 0 (the first start,
/// before a g1 recovery) or 1 (the g1 recovery admission, before a g2
/// recovery). No handle is returned, so nothing can be recorded or reconciled
/// through it; the smoke/reconcile handle above stays g0-only.
pub(crate) fn expired_bootstrap_locked(home: &Path, team: &str, seat: &str, old_generation: u64) -> Result<(), ReplacementError> {
    if old_generation > 1 { return Err(ReplacementError::GenerationMismatch); }
    UnfinishedBootstrap::read_locked_at(home, team, seat, old_generation).map(|_| ())
}
/// Pre-reserve check for the explicit bootstrap recovery: the CURRENT
/// attempt's `g<old_generation>` admission is open (admitted and effects bound
/// to exactly `attempt_id`, no terminal) and nothing else lives there. Another
/// attempt, a finished one, or any prepared/retirement material refuses.
/// Read-only; `old_generation` is the quarantined generation being recovered.
pub(crate) fn recovery_attempt_open_locked(home: &Path, team: &str, seat: &str, old_generation: u64, attempt_id: &str) -> Result<(), ReplacementError> {
    if old_generation != 1 && old_generation != 2 { return Err(ReplacementError::GenerationMismatch); }
    let dir = home.join(".aperture/teams").join(team).join("runtime-attempts").join(seat).join(format!("g{old_generation}"));
    let a: Admission = read_private_json(&dir.join("admitted.json")).map_err(|_| ReplacementError::OutcomeUnknown)?;
    let f: Fact = read_private_json(&dir.join("effects.json")).map_err(|_| ReplacementError::OutcomeUnknown)?;
    if !crate::team_claude_launch::canonical_uuid(attempt_id)
        || a.schema_version != 1 || a.attempt_id != attempt_id || a.team != team || a.seat != seat || a.old_generation != old_generation
        || f.schema_version != 1 || f.attempt_id != attempt_id || f.kind != FactKind::EffectsMayHaveOccurred {
        return Err(ReplacementError::OutcomeUnknown);
    }
    for name in ["terminal.json", "reconciled.json", "prepared.json", "reprepare", "start", "retirement"] {
        if !matches!(std::fs::symlink_metadata(dir.join(name)), Err(e) if e.kind() == std::io::ErrorKind::NotFound) {
            return Err(ReplacementError::OutcomeUnknown);
        }
    }
    Ok(())
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Fact {
    schema_version: u32,
    attempt_id: String,
    kind: FactKind,
}

/// Durable Ready facts are evidence, never a reconstructed preparation permit.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PriorReady {
    schema_version: u32,
    attempt_id: String,
    seat: String,
    generation: u64,
    owner_identity_sha256: String,
    revocation: super::RevocationProof,
}
impl PriorReady {
    fn owner_hash(owner: &OwnerRecord) -> Result<String, ReplacementError> {
        use sha2::{Digest, Sha256};
        let i = owner
            .incarnation
            .as_ref()
            .ok_or(ReplacementError::PreparationExpired)?;
        if owner.state != OwnerState::Active
            || !i.observed
            || i.model != owner.requested.model
            || i.harness != owner.requested.harness
            || i.reasoning != owner.requested.reasoning
        {
            return Err(ReplacementError::PreparationExpired);
        }
        Ok(format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_vec(&(
                    &owner.seat,
                    owner.generation,
                    &owner.requested,
                    i.pid,
                    i.start_time,
                    &i.thread_id,
                    &i.token_id
                ))
                .map_err(|_| ReplacementError::PreparationExpired)?
            )
        ))
    }
    pub(crate) fn verify_owner(&self, owner: &OwnerRecord) -> Result<(), ReplacementError> {
        let p = &self.revocation;
        if self.schema_version != 1
            || self.seat != owner.seat
            || self.generation != owner.generation
            || self.owner_identity_sha256 != Self::owner_hash(owner)?
            || p.generation != owner.generation
            || !p.durable
            || !p.sockets_closed
            || !p.token_deleted
            || p.close_code != 4001
            || p.reconnect_code != 4003
            || p.close_elapsed_ms > 1000
        {
            return Err(ReplacementError::PreparationExpired);
        }
        Ok(())
    }
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
    prior_ready: Option<PriorReady>,
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
        if !matches!(std::fs::symlink_metadata(self.dir.join("reprepare")),Err(e) if e.kind()==std::io::ErrorKind::NotFound)
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
// An explicit new call may follow only a proved pre-effect rejection. Never
// reuse or overwrite the old admission/terminal, and never retry inside a call.
fn failed_before_effects(dir:&Path,team:&str,seat:&str,generation:u64)->Result<(),ReplacementError> {
    let a:Admission=read_private_json(&dir.join("admitted.json")).map_err(|_|ReplacementError::OutcomeUnknown)?;
    let f:Fact=read_private_json(&dir.join("terminal.json")).map_err(|_|ReplacementError::OutcomeUnknown)?;
    if a.schema_version!=1 || a.team!=team || a.seat!=seat || a.old_generation!=generation
        || f.schema_version!=1 || f.attempt_id!=a.attempt_id || f.kind!=FactKind::Failed
        || !matches!(std::fs::symlink_metadata(dir.join("effects.json")),Err(e) if e.kind()==std::io::ErrorKind::NotFound) {
        return Err(ReplacementError::OutcomeUnknown);
    }
    Ok(())
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
        Self::begin_checked(home, actor, team, seat, generation, budget, false, false)
    }
    pub(crate) fn begin_retirement(
        home: &Path, actor: &AuthenticatedActor, team: &str, seat: &str,
        generation: u64, budget: Deadline,
    ) -> Result<Self, ReplacementError> {
        if generation == 0 { return Err(ReplacementError::GenerationMismatch); }
        Self::begin_checked(home, actor, team, seat, generation, budget, false, true)
    }
    pub(crate) fn begin_bootstrap(
        home: &Path,
        actor: &AuthenticatedActor,
        team: &str,
        seat: &str,
        budget: Deadline,
    ) -> Result<Self, ReplacementError> {
        Self::begin_checked(home, actor, team, seat, 0, budget, true, false)
    }
    /// Explicit bootstrap recovery: the owner must be exactly the quarantined,
    /// unobserved `generation` (1 = the first bootstrap, 2 = a first recovery
    /// that failed the same way; nothing later). The attempt lands in
    /// `runtime-attempts/<seat>/g<generation>`; earlier facts are never opened
    /// for writing, and an existing directory refuses (one admission, no retry).
    pub(crate) fn begin_bootstrap_recovery(
        home: &Path,
        actor: &AuthenticatedActor,
        team: &str,
        seat: &str,
        generation: u64,
        budget: Deadline,
    ) -> Result<Self, ReplacementError> {
        if generation != 1 && generation != 2 { return Err(ReplacementError::GenerationMismatch); }
        Self::begin_checked(home, actor, team, seat, generation, budget, true, false)
    }
    fn begin_checked(
        home: &Path,
        actor: &AuthenticatedActor,
        team: &str,
        seat: &str,
        generation: u64,
        budget: Deadline,
        bootstrap: bool,
        retirement: bool,
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
        let state_ok = if bootstrap && (generation == 1 || generation == 2) {
            // Explicit recovery (g1, or a g2 that failed the same way): quarantined,
            // never observed, thread never bound, no nonce; a retained provisional
            // id must be the incarnation's own token id (nothing is cleaned to fit).
            owner.state == OwnerState::Quarantined
                && owner.reservation_nonce_sha256.is_none()
                && owner.incarnation.as_ref().is_some_and(|i| {
                    !i.observed
                        && i.thread_id.is_empty()
                        && owner.provisional_token_id.as_deref().is_none_or(|p| p == i.token_id)
                })
        } else if bootstrap {
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
        let mut dir = validate_component_path(&seat_dir, &format!("g{generation}"), true)
            .map_err(|_| ReplacementError::NativeFailure)?;
        if retirement {
            // The parent is either an ordinary pre-effect Failed, or only the
            // container created by a previous explicit retirement admission.
            match std::fs::symlink_metadata(&dir) {
                Ok(_) => match std::fs::symlink_metadata(dir.join("admitted.json")) {
                    Ok(_) => failed_before_effects(&dir,team,seat,generation)?,
                    Err(e) if e.kind()==std::io::ErrorKind::NotFound => {
                        let names=std::fs::read_dir(&dir).map_err(|_|ReplacementError::OutcomeUnknown)?
                            .take(2).map(|v|v.map(|v|v.file_name())).collect::<Result<Vec<_>,_>>()
                            .map_err(|_|ReplacementError::OutcomeUnknown)?;
                        if names.len()!=1 || names[0]!="retirement" {return Err(ReplacementError::OutcomeUnknown);}
                    }
                    _=>return Err(ReplacementError::OutcomeUnknown),
                },
                Err(e) if e.kind()==std::io::ErrorKind::NotFound=>{},
                _=>return Err(ReplacementError::OutcomeUnknown),
            }
            ensure_private_dir(&dir).map_err(|_| ReplacementError::OutcomeUnknown)?;
            dir = validate_component_path(&dir, "retirement", true)
                .map_err(|_| ReplacementError::OutcomeUnknown)?;
        }
        let mut prior_ready = None;
        // Ordinary preparation supersedes only Ready. Explicit retirement may
        // traverse only pre-effect Failed facts. No live/uncertain work retries.
        for ordinal in 0..32 {
            match std::fs::symlink_metadata(&dir) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
                Ok(_) if retirement && ordinal<31 => {
                    failed_before_effects(&dir,team,seat,generation)?;
                    dir=validate_component_path(&dir,"reprepare",true)
                        .map_err(|_|ReplacementError::OutcomeUnknown)?;
                }
                Ok(_) if !bootstrap && !retirement && ordinal < 31 => {
                    validate_component_path(
                        &seat_dir,
                        dir.strip_prefix(&seat_dir)
                            .ok()
                            .and_then(|p| p.to_str())
                            .ok_or(ReplacementError::OutcomeUnknown)?,
                        false,
                    )
                    .map_err(|_| ReplacementError::OutcomeUnknown)?;
                    let a: Admission = read_private_json(&dir.join("admitted.json"))
                        .map_err(|_| ReplacementError::OutcomeUnknown)?;
                    let fact: Fact = read_private_json(&dir.join("terminal.json"))
                        .map_err(|_| ReplacementError::OutcomeUnknown)?;
                    let evidence: PriorReady = read_private_json(&dir.join("prepared.json"))
                        .map_err(|_| ReplacementError::OutcomeUnknown)?;
                    if a.schema_version != 1
                        || a.team != team
                        || a.seat != seat
                        || a.old_generation != generation
                        || fact.attempt_id != a.attempt_id
                        || fact.kind != FactKind::Ready
                        || fact.schema_version != 1
                        || evidence.attempt_id != a.attempt_id
                    {
                        return Err(ReplacementError::OutcomeUnknown);
                    }
                    evidence.verify_owner(&owner)?;
                    let start = dir.join("start");
                    if !matches!(std::fs::symlink_metadata(&start),Err(e) if e.kind()==std::io::ErrorKind::NotFound)
                    {
                        let terminal: Fact = read_private_json(&start.join("terminal.json"))
                            .map_err(|_| ReplacementError::OutcomeUnknown)?;
                        let started: Admission = read_private_json(&start.join("admitted.json"))
                            .map_err(|_| ReplacementError::OutcomeUnknown)?;
                        if terminal.schema_version != 1
                            || terminal.kind != FactKind::Failed
                            || terminal.attempt_id != started.attempt_id
                            || started.team != team
                            || started.seat != seat
                            || started.old_generation != generation
                            || !matches!(std::fs::symlink_metadata(start.join("effects.json")),Err(e) if e.kind()==std::io::ErrorKind::NotFound)
                        {
                            return Err(ReplacementError::OutcomeUnknown);
                        }
                    }
                    prior_ready = Some(evidence);
                    dir = validate_component_path(&dir, "reprepare", true)
                        .map_err(|_| ReplacementError::OutcomeUnknown)?;
                }
                _ => return Err(ReplacementError::OutcomeUnknown),
            }
        }
        ensure_private_dir(&dir).map_err(|_| ReplacementError::OutcomeUnknown)?;
        let mut attempt = Self::publish(
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
        )?;
        attempt.prior_ready = prior_ready;
        Ok(attempt)
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
            prior_ready: None,
        })
    }
    /// Human prepare is terminal and factual; no clock runs while a person
    /// reads the dialog. The retained native permit is consumed by Start.
    pub(crate) fn finish_ready(
        mut self,
        revocation: &super::RevocationProof,
    ) -> Result<ReadyAttempt, ReplacementError> {
        self.budget.forward(Duration::ZERO)?;
        if self.terminal {
            return Err(ReplacementError::OutcomeUnknown);
        }
        {
            let _team = try_lock(
                &self.home.join(".aperture/run/team-locks"),
                &self.admitted.team,
            )
            .map_err(|_| ReplacementError::OutcomeUnknown)?;
            let store = OwnerStore::new(self.home.join(".aperture/run/owner"));
            let _seat = store
                .lock(&self.admitted.seat)
                .map_err(|_| ReplacementError::OutcomeUnknown)?;
            let owner: OwnerRecord = read_private_json(&store.record_path(&self.admitted.seat))
                .map_err(|_| ReplacementError::OutcomeUnknown)?;
            let evidence = PriorReady {
                schema_version: 1,
                attempt_id: self.id().into(),
                seat: self.admitted.seat.clone(),
                generation: self.admitted.old_generation,
                owner_identity_sha256: PriorReady::owner_hash(&owner)?,
                revocation: revocation.clone(),
            };
            evidence.verify_owner(&owner)?;
            write_private_json_atomic(&self.dir.join("prepared.json"), &evidence, false)
                .map_err(|_| ReplacementError::OutcomeUnknown)?;
        }
        self.fact("terminal.json", FactKind::Ready)?;
        self.terminal = true;
        Ok(ReadyAttempt {
            home: self.home,
            dir: self.dir,
            admitted: self.admitted,
        })
    }
    pub(crate) fn prior_ready(&self) -> Option<&PriorReady> {
        self.prior_ready.as_ref()
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
    /// A native-only proof, rechecked under team/seat locks, is the sole path
    /// to this terminal. It never makes a bootstrap attempt eligible for retry.
    pub(crate) fn finish_smoke_cleaned(
        &mut self, proof: super::native::SmokeCleanupProof, actor: &AuthenticatedActor,
    ) -> Result<(), ReplacementError> {
        if self.terminal || !self.effects_admitted || self.admitted.old_generation != 0 {
            return Err(ReplacementError::OutcomeUnknown);
        }
        proof.with_revalidated(&self.home, &self.admitted.team, &self.admitted.seat,
            self.admitted.old_generation, self.id(), &self.budget, actor, || {
                let actual: Admission = read_private_json(&self.dir.join("admitted.json"))
                    .map_err(|_| ReplacementError::OutcomeUnknown)?;
                let effects: Fact = read_private_json(&self.dir.join("effects.json"))
                    .map_err(|_| ReplacementError::OutcomeUnknown)?;
                if actual != self.admitted || effects.schema_version != 1
                    || effects.attempt_id != self.id() || effects.kind != FactKind::EffectsMayHaveOccurred {
                    return Err(ReplacementError::OutcomeUnknown);
                }
                if Instant::now() >= self.budget.cleanup_until() { return Err(ReplacementError::Deadline); }
                let fact = Fact { schema_version: 1, attempt_id: self.id().into(), kind: FactKind::SmokeCleaned };
                write_private_json_atomic(&self.dir.join("terminal.json"), &fact, false)
                    .map_err(|_| ReplacementError::OutcomeUnknown)?;
                if read_private_json::<Fact>(&self.dir.join("terminal.json"))
                    .map_err(|_| ReplacementError::OutcomeUnknown)? != fact { return Err(ReplacementError::OutcomeUnknown); }
                Ok(())
            })?;
        self.terminal = true;
        Ok(())
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

/// Explicit recovery uses the SAME private append-only substrate. An intent is
/// never a success receipt. A second invocation may reconcile a completed native
/// postcondition, but cannot repeat an uncertain signal/revoke/spawn.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct RecoveryPhaseFact {
    version: u8,
    operation_id: String,
    step: String,
    completed: bool,
    reconciled: bool,
}
pub(crate) struct RecoveryJournal {
    dir: PathBuf,
    operation_id: String,
}
impl RecoveryJournal {
    pub(crate) fn open(dir: PathBuf, operation_id: &str) -> Result<Self, ReplacementError> {
        if !crate::team_claude_launch::canonical_uuid(operation_id) { return Err(ReplacementError::AuthorizationRequired); }
        ensure_private_dir(&dir).map_err(|_| ReplacementError::OutcomeUnknown)?;
        Ok(Self {dir, operation_id:operation_id.into()})
    }
    pub(crate) fn read(dir:PathBuf,operation_id:&str)->Result<Self,ReplacementError> {
        use std::os::unix::fs::MetadataExt;
        if !crate::team_claude_launch::canonical_uuid(operation_id) {return Err(ReplacementError::OutcomeUnknown);}
        let parent=dir.parent().ok_or(ReplacementError::OutcomeUnknown)?;
        let name=dir.file_name().and_then(|s|s.to_str()).ok_or(ReplacementError::OutcomeUnknown)?;
        validate_component_path(parent,name,false).map_err(|_|ReplacementError::OutcomeUnknown)?;
        let m=std::fs::symlink_metadata(&dir).map_err(|_|ReplacementError::OutcomeUnknown)?;
        if !m.is_dir() || m.uid()!=unsafe{libc::geteuid()} || m.mode()&0o777!=0o700 {return Err(ReplacementError::OutcomeUnknown);}
        Ok(Self {dir,operation_id:operation_id.into()})
    }
    fn path(&self, step: &str, completed: bool) -> Result<PathBuf, ReplacementError> {
        if step.is_empty() || step.len()>96 || !step.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
            return Err(ReplacementError::OutcomeUnknown);
        }
        Ok(self.dir.join(format!("{step}.{}.json", if completed {"done"} else {"intent"})))
    }
    pub(crate) fn has(&self, step: &str, completed: bool) -> Result<bool, ReplacementError> {
        let path = self.path(step, completed)?;
        match std::fs::symlink_metadata(&path) {
            Err(e) if e.kind()==std::io::ErrorKind::NotFound => Ok(false),
            Err(_) => Err(ReplacementError::OutcomeUnknown),
            Ok(_) => {
                let f: RecoveryPhaseFact = read_private_json(&path).map_err(|_| ReplacementError::OutcomeUnknown)?;
                if f.version!=1 || f.operation_id!=self.operation_id || f.step!=step || f.completed!=completed
                    || (!completed && f.reconciled) { return Err(ReplacementError::OutcomeUnknown); }
                Ok(true)
            }
        }
    }
    fn write(&self, step: &str, completed: bool, reconciled: bool) -> Result<(), ReplacementError> {
        let path = self.path(step, completed)?;
        let f = RecoveryPhaseFact {version:1, operation_id:self.operation_id.clone(), step:step.into(), completed, reconciled};
        write_private_json_atomic(&path,&f,false).map_err(|_|ReplacementError::OutcomeUnknown)?;
        if read_private_json::<RecoveryPhaseFact>(&path).map_err(|_|ReplacementError::OutcomeUnknown)? != f {
            return Err(ReplacementError::OutcomeUnknown);
        }
        Ok(())
    }
    /// `known` is the real native postcondition, never caller-supplied proof.
    /// `effect` returns only after its own native ACK/postcondition is checked.
    pub(crate) fn phase(&self, step: &str,
        known: impl FnOnce()->Result<bool,ReplacementError>,
        effect: impl FnOnce()->Result<(),ReplacementError>,
    ) -> Result<(), ReplacementError> {
        if self.has(step,true)? { return Ok(()); }
        let intent = self.has(step,false)?;
        if known()? {
            if !intent { self.write(step,false,false)?; }
            return self.write(step,true,true);
        }
        if intent { return Err(ReplacementError::OutcomeUnknown); }
        self.write(step,false,false)?;
        effect()?;
        self.write(step,true,false)
    }
}

impl RuntimeAttempt {
    /// Called only with a native stopped/revoked/quarantined recovery proof.
    /// This is a fresh admission, not a reconstruction of an old start nonce.
    pub(crate) fn begin_readmission(home: &Path, actor: &AuthenticatedActor,
        proof: &super::native::ReadmissionProof, budget: Deadline,
    ) -> Result<Self, ReplacementError> {
        proof.verify(home, actor)?;
        let dir = proof.directory().join("start");
        ensure_private_dir(&dir).map_err(|_| ReplacementError::OutcomeUnknown)?;
        budget.forward(Duration::from_secs(90))?;
        // Re-entry is permitted only by the opaque proof that the original
        // quarantined gN is still exact: reserve_start has not happened. A new
        // UUID/second spawn is never inferred from a missing completion fact.
        let mut attempt=if dir.join("admitted.json").exists() {
            let admitted:Admission=read_private_json(&dir.join("admitted.json")).map_err(|_|ReplacementError::OutcomeUnknown)?;
            if admitted.team!=proof.team() || admitted.seat!=proof.seat() || admitted.old_generation!=proof.generation()
                || admitted.schema_version!=1 || admitted.native_budget_ms!=170_000 || admitted.cleanup_reserve_ms!=40_000
                || !crate::team_claude_launch::canonical_uuid(&admitted.attempt_id)
                || dir.join("terminal.json").exists() {return Err(ReplacementError::OutcomeUnknown);}
            Self {home:home.into(),dir,admitted,budget,effects_admitted:false,terminal:false,prior_ready:None}
        } else {Self::publish(home, dir, Admission {
            schema_version:1, attempt_id:uuid::Uuid::new_v4().to_string(),
            team:proof.team().into(), seat:proof.seat().into(), old_generation:proof.generation(),
            admitted_at_ms:chrono::Utc::now().timestamp_millis(), native_budget_ms:170_000, cleanup_reserve_ms:40_000,
        }, budget)?};
        if attempt.dir.join("effects.json").exists() {
            verify_readmission(proof.directory(),proof.team(),proof.seat(),proof.generation())?;
        } else {attempt.fact("effects.json",FactKind::EffectsMayHaveOccurred)?;}
        attempt.effects_admitted=true;
        Ok(attempt)
    }
}

/// Exact start admission/effects pair for recovery observation access; no nonce
/// reconstruction, renewed budget or inferred zero-effect status.
pub(crate) fn verify_readmission(dir:&Path,team:&str,seat:&str,generation:u64)->Result<(),ReplacementError> {
    let a:Admission=read_private_json(&dir.join("start/admitted.json")).map_err(|_|ReplacementError::OutcomeUnknown)?;
    let e:Fact=read_private_json(&dir.join("start/effects.json")).map_err(|_|ReplacementError::OutcomeUnknown)?;
    if a.schema_version!=1 || a.team!=team || a.seat!=seat || a.old_generation!=generation
        || a.native_budget_ms!=170_000 || a.cleanup_reserve_ms!=40_000
        || !crate::team_claude_launch::canonical_uuid(&a.attempt_id)
        || e.schema_version!=1 || e.attempt_id!=a.attempt_id || e.kind!=FactKind::EffectsMayHaveOccurred {
        return Err(ReplacementError::OutcomeUnknown);
    }
    Ok(())
}

impl RecoveryJournal {
    /// Typed re-entry of the CAS only. No generic phase retry, signal, token,
    /// or spawn callback: an exact Active postimage proves CAS not committed.
    /// Quarantined postimages use ordinary postcondition reconciliation.
    pub(crate) fn quarantine_stopped(&self,proof:&super::native::StoppedCodexRecoveryProof,
        actor:&AuthenticatedActor)->Result<(),ReplacementError> {
        proof.verify_quarantine_journal(&self.dir,&self.operation_id,actor)?;
        if self.has("quarantine",true)? {return Err(ReplacementError::OutcomeUnknown);}
        if !self.has("quarantine",false)? {self.write("quarantine",false,false)?;}
        proof.verify_quarantine_journal(&self.dir,&self.operation_id,actor)?;
        proof.commit_quarantine(actor)?;
        self.write("quarantine",true,false)
    }
    /// This exception is typed: exact old quarantined owner proves native
    /// reserve_start (the first start effect) has not committed. No generic
    /// retry/boolean flag can re-enter a signal or a new-generation candidate.
    pub(crate) fn start_unreserved(&self,proof:&super::native::ReadmissionProof,
        home:&Path,actor:&AuthenticatedActor,effect:impl FnOnce()->Result<(),ReplacementError>)->Result<(),ReplacementError> {
        proof.verify(home,actor)?;
        if self.dir!=proof.directory() || self.has("start",true)? {return Err(ReplacementError::OutcomeUnknown);}
        if !self.has("start",false)? {self.write("start",false,false)?;}
        proof.verify(home,actor)?;
        effect()?;
        self.write("start",true,false)
    }
}
