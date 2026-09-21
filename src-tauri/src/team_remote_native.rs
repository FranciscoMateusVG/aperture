//! Native remote uncertainty inventory and append-only authorization facts.
//! A decision is NOT provider observation. No provider calls or public transport.
use crate::journal::{
    ensure_private_dir, read_private_json, validate_component_path, write_private_json_atomic,
};
use crate::owner::{try_lock, AdvisoryLock, OwnerRecord, OwnerStore};
use crate::state::OwnerState;
use crate::team_auth::{AuthenticatedActor, AuthenticatedSeat};
use crate::team_checkpoint::{CheckpointEntry, CheckpointValidation};
use crate::teams::{classify_managed_seat, ManagedSeatState, TeamSnapshot};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};

const MAX_ENTRIES: usize = 1024;
const MAX_BYTES: u64 = 16 * 1024 * 1024;
const MAX_FACTS: usize = 256;
const MAX_REFS: usize = 64;
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RemoteError {
    Invalid,
    Authority,
    Generation,
    StaleInventory,
    Conflict,
    Corrupt,
    Unsafe,
    Limit,
    Io,
}
impl RemoteError {
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::Authority => "E_CONTROL_UNAUTHORIZED",
            Self::Generation => "E_GENERATION_MISMATCH",
            Self::Invalid => "E_REMOTE_RESOLUTION_INVALID",
            Self::Conflict => "E_REMOTE_RESOLUTION_CONFLICT",
            _ => "E_REMOTE_UNCERTAIN",
        }
    }
}
/// Selectors, constructed by existing authenticated control; no authority.
pub(crate) struct RemoteTarget {
    pub team: String,
    pub seat: String,
    pub expected_generation: u64,
}
/// Not deserializable; actual authority remains the existing authenticated seam.
pub(crate) enum ResolutionAuthority<'a> {
    Operator(&'a AuthenticatedActor),
    Lead(&'a AuthenticatedSeat),
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ResolutionScope {
    EffectResolution,
    InventoryRiskAcceptance,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ResolutionDecision {
    Finished,
    Cancelled,
    ProceedWithUnobservedEffects,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ResolutionRequest {
    pub expected_inventory_hash: String,
    pub scope: ResolutionScope,
    pub reference: Option<String>,
    pub decision: ResolutionDecision,
    pub evidence_ref: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ObservedState {
    Unknown,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct InventoryEffect {
    pub reference: String,
    observed_state: ObservedState,
}
/// Native derivation only. There are no fully instrumented effect adapters in
/// this runtime: even an empty checkpoint history leaves observation incomplete.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct RemoteInventoryView {
    pub target_generation: u64,
    pub complete_observation: bool,
    pub effects: Vec<InventoryEffect>,
    pub inventory_hash: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Principal {
    Operator,
    Lead { seat: String, generation: u64 },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum FactSource {
    AuthorizedDecision,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RemoteResolutionAuthorizationFact {
    schema_version: u32,
    fact_seq: u64,
    team: String,
    target_seat: String,
    target_generation: u64,
    inventory_hash: String,
    scope: ResolutionScope,
    reference: Option<String>,
    decision: ResolutionDecision,
    evidence_ref: String,
    resolver: Principal,
    written_at: u64,
    source: FactSource,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct ResolutionReceipt {
    pub fact_id: String,
    pub inventory_hash: String,
    pub source: &'static str,
    pub complete_observation: bool,
    pub replay: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct RemoteProjection {
    pub inventory: RemoteInventoryView,
    pub inventory_risk_accepted: bool,
    pub authorized_references: Vec<String>,
    pub source: Option<&'static str>,
    pub may_proceed: bool,
    pub evidence_sha256: String,
}
impl RemoteProjection {
    pub(super) fn permits(
        &self,
        generation: u64,
        effects: &[super::RemoteEffect],
        complete: bool,
    ) -> bool {
        if generation != self.inventory.target_generation
            || complete != self.inventory.complete_observation
            || effects.len() != self.inventory.effects.len()
            || effects
                .iter()
                .any(|e| e.resolution != super::RemoteResolution::Unknown)
        {
            return false;
        }
        let mut refs: Vec<_> = effects.iter().map(|e| e.reference.as_str()).collect();
        refs.sort_unstable();
        let expected: Vec<_> = self
            .inventory
            .effects
            .iter()
            .map(|e| e.reference.as_str())
            .collect();
        self.may_proceed && refs == expected
    }
    pub(crate) fn into_core(self) -> super::RemoteInventory {
        super::RemoteInventory {
            effects: self
                .inventory
                .effects
                .iter()
                .map(|e| super::RemoteEffect {
                    reference: e.reference.clone(),
                    resolution: super::RemoteResolution::Unknown,
                })
                .collect(),
            complete_observation: self.inventory.complete_observation,
            explicit_resolution: Some(self),
        }
    }
}
struct Locked {
    _seats: Vec<AdvisoryLock>,
    _team: Option<AdvisoryLock>,
    team_dir: PathBuf,
    checkpoints: PathBuf,
    lead: String,
}
fn ident(s: &str, max: usize) -> bool {
    !s.is_empty()
        && s.len() <= max
        && s.as_bytes()[0].is_ascii_alphanumeric()
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"-_".contains(&b))
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn hash_valid(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn reference_valid(s: &str, sentinels: &[String]) -> bool {
    !s.is_empty()
        && s.len() <= 200
        && !s.starts_with('/')
        && !s.contains("..")
        && !s.contains("://")
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_:./#".contains(&b))
        && ![
            "bearer",
            "sk_live_",
            "sk_test_",
            "rk_live_",
            "password",
            "process.env",
            "secret=",
            "token=",
        ]
        .iter()
        .any(|p| s.to_ascii_lowercase().contains(p))
        && !sentinels
            .iter()
            .filter(|s| !s.is_empty())
            .any(|v| s.contains(v))
}
fn request_valid(r: &ResolutionRequest, sentinels: &[String]) -> Result<(), RemoteError> {
    if !hash_valid(&r.expected_inventory_hash) || !reference_valid(&r.evidence_ref, sentinels) {
        return Err(RemoteError::Invalid);
    }
    match (&r.scope, &r.reference, &r.decision) {
        (
            ResolutionScope::EffectResolution,
            Some(s),
            ResolutionDecision::Finished | ResolutionDecision::Cancelled,
        ) if reference_valid(s, sentinels) => Ok(()),
        (
            ResolutionScope::InventoryRiskAcceptance,
            None,
            ResolutionDecision::ProceedWithUnobservedEffects,
        ) => Ok(()),
        _ => Err(RemoteError::Invalid),
    }
}
fn owner(home: &Path, seat: &str, generation: u64) -> Result<OwnerRecord, RemoteError> {
    let value: OwnerRecord = read_private_json(
        &home
            .join(".aperture/run/owner")
            .join(format!("{seat}.json")),
    )
    .map_err(|_| RemoteError::Corrupt)?;
    let actual = value.incarnation.as_ref().ok_or(RemoteError::Corrupt)?;
    if value.schema_version != 1
        || value.seat != seat
        || value.generation != generation
        || value.state != OwnerState::Active
    {
        return Err(RemoteError::Generation);
    }
    if !actual.observed
        || actual.harness != value.requested.harness
        || actual.model != value.requested.model
        || actual.reasoning != value.requested.reasoning
    {
        return Err(RemoteError::Corrupt);
    }
    Ok(value)
}
fn open(
    home: &Path,
    t: &RemoteTarget,
    principal: Option<&Principal>,
) -> Result<Locked, RemoteError> {
    if !ident(&t.team, 16) || !ident(&t.seat, 31) || t.expected_generation == 0 {
        return Err(RemoteError::Invalid);
    }
    let team_lock =
        try_lock(&home.join(".aperture/run/team-locks"), &t.team).map_err(|_| RemoteError::Io)?;
    let team_dir = validate_component_path(&home.join(".aperture/teams"), &t.team, false)
        .map_err(|_| RemoteError::Unsafe)?;
    let snapshot: TeamSnapshot =
        read_private_json(&team_dir.join("team.json")).map_err(|_| RemoteError::Corrupt)?;
    if snapshot.schema_version != 1
        || snapshot.team != t.team
        || !snapshot.seats.iter().any(|s| s.name == t.seat)
    {
        return Err(RemoteError::Generation);
    }
    let mut seats = vec![t.seat.as_str()];
    if let Some(Principal::Lead { seat, generation }) = principal {
        if !ident(seat, 31) || *generation == 0 || *seat == t.seat || snapshot.lead != *seat {
            return Err(RemoteError::Authority);
        }
        seats.push(seat);
    }
    seats.sort_unstable();
    seats.dedup();
    let owners = OwnerStore::new(home.join(".aperture/run/owner"));
    let mut locks = vec![];
    for seat in seats {
        match classify_managed_seat(home, seat).map_err(|_| RemoteError::Corrupt)? {
            Some(ManagedSeatState::Active { team, .. }) if team == t.team => {}
            _ => return Err(RemoteError::Generation),
        }
        locks.push(owners.lock(seat).map_err(|_| RemoteError::Io)?);
    }
    owner(home, &t.seat, t.expected_generation)?;
    if let Some(Principal::Lead { seat, generation }) = principal {
        owner(home, seat, *generation)?;
    }
    Ok(Locked {
        _seats: locks,
        _team: Some(team_lock),
        checkpoints: team_dir.join("checkpoints").join(&t.seat),
        team_dir,
        lead: snapshot.lead,
    })
}

fn open_prelocked(
    home: &Path,
    t: &RemoteTarget,
    _team_lock: &AdvisoryLock,
    _seat_locks: &[AdvisoryLock],
) -> Result<Locked, RemoteError> {
    if !ident(&t.team, 16) || !ident(&t.seat, 31) || t.expected_generation == 0 {
        return Err(RemoteError::Invalid);
    }
    let team_dir = validate_component_path(&home.join(".aperture/teams"), &t.team, false)
        .map_err(|_| RemoteError::Unsafe)?;
    let snapshot: TeamSnapshot =
        read_private_json(&team_dir.join("team.json")).map_err(|_| RemoteError::Corrupt)?;
    if snapshot.schema_version != 1
        || snapshot.team != t.team
        || !snapshot.seats.iter().any(|s| s.name == t.seat)
    {
        return Err(RemoteError::Generation);
    }
    match classify_managed_seat(home, &t.seat).map_err(|_| RemoteError::Corrupt)? {
        Some(ManagedSeatState::Active { team, .. }) if team == t.team => {}
        _ => return Err(RemoteError::Generation),
    }
    owner(home, &t.seat, t.expected_generation)?;
    Ok(Locked {
        _seats: vec![],
        _team: None,
        checkpoints: team_dir.join("checkpoints").join(&t.seat),
        team_dir,
        lead: snapshot.lead,
    })
}
fn existing_dir(root: &Path, relative: &str) -> Result<Option<PathBuf>, RemoteError> {
    let path = root.join(relative);
    // Validate every existing parent even when the leaf is absent.
    let checked = validate_component_path(root, relative, true).map_err(|_| RemoteError::Unsafe)?;
    match std::fs::symlink_metadata(&path) {
        Ok(m) if m.is_dir() => {
            ensure_private_dir(&checked).map_err(|_| RemoteError::Unsafe)?;
            Ok(Some(checked))
        }
        Ok(_) => Err(RemoteError::Unsafe),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(RemoteError::Io),
    }
}
fn inventory(
    locked: &Locked,
    t: &RemoteTarget,
    sentinels: &[String],
) -> Result<RemoteInventoryView, RemoteError> {
    let mut refs = BTreeSet::new();
    let mut seqs = BTreeSet::new();
    let mut total_bytes = 0u64;
    // No checkpoints directory is a normal first-turn state, not zero effects.
    let base = existing_dir(&locked.team_dir, "checkpoints")?;
    if let Some(base) = base {
        if let Some(dir) = existing_dir(&base, &t.seat)? {
            for (i, file) in std::fs::read_dir(&dir)
                .map_err(|_| RemoteError::Io)?
                .enumerate()
            {
                if i >= MAX_ENTRIES {
                    return Err(RemoteError::Limit);
                }
                let file = file.map_err(|_| RemoteError::Io)?;
                let name = file
                    .file_name()
                    .into_string()
                    .map_err(|_| RemoteError::Corrupt)?;
                if name == ".validation" || name == ".remote-resolution" {
                    existing_dir(&dir, &name)?;
                    continue;
                }
                // Crash residue is unresolved evidence, not silently omitted.
                let (g, s) = name
                    .strip_suffix(".json")
                    .and_then(|v| v.split_once('-'))
                    .ok_or(RemoteError::Corrupt)?;
                let g: u64 = g.parse().map_err(|_| RemoteError::Corrupt)?;
                let seq: u64 = s.parse().map_err(|_| RemoteError::Corrupt)?;
                if g == 0 || seq == 0 || name != format!("{g}-{seq}.json") {
                    return Err(RemoteError::Corrupt);
                }
                if g != t.expected_generation {
                    continue;
                }
                let path =
                    validate_component_path(&dir, &name, false).map_err(|_| RemoteError::Unsafe)?;
                let bytes = std::fs::symlink_metadata(&path)
                    .map_err(|_| RemoteError::Io)?
                    .len();
                total_bytes = total_bytes.checked_add(bytes).ok_or(RemoteError::Limit)?;
                if bytes > 128 * 1024 || total_bytes > MAX_BYTES {
                    return Err(RemoteError::Limit);
                }
                let entry: CheckpointEntry =
                    read_private_json(&path).map_err(|_| RemoteError::Corrupt)?;
                if entry.schema_version != 1
                    || entry.team != t.team
                    || entry.seat != t.seat
                    || entry.generation != g
                    || entry.seq != seq
                    || entry.checkpoint_id != format!("{}/{g}/{seq}", t.seat)
                    || entry.validation != CheckpointValidation::Pending
                {
                    return Err(RemoteError::Corrupt);
                }
                crate::team_checkpoint::validate_payload(&entry.payload, sentinels)
                    .map_err(|_| RemoteError::Corrupt)?;
                let canonical = serde_json::to_vec(&(entry.schema_version, &entry.payload))
                    .map_err(|_| RemoteError::Corrupt)?;
                if entry.content_hash != digest(&canonical) || !seqs.insert(seq) {
                    return Err(RemoteError::Corrupt);
                }
                for declared in entry.payload.remote_effects {
                    // A worker's finished/cancelled field is a declaration, NOT
                    // authenticated external observation. Retain dropped refs.
                    refs.insert(declared.reference);
                    if refs.len() > MAX_REFS {
                        return Err(RemoteError::Limit);
                    }
                }
            }
        }
    }
    for (i, seq) in seqs.iter().enumerate() {
        if *seq != i as u64 + 1 {
            return Err(RemoteError::Corrupt);
        }
    }
    let effects: Vec<_> = refs
        .into_iter()
        .map(|reference| InventoryEffect {
            reference,
            observed_state: ObservedState::Unknown,
        })
        .collect();
    let hash = digest(
        &serde_json::to_vec(&(t.expected_generation, false, &effects))
            .map_err(|_| RemoteError::Corrupt)?,
    );
    Ok(RemoteInventoryView {
        target_generation: t.expected_generation,
        complete_observation: false,
        effects,
        inventory_hash: hash,
    })
}
fn facts(
    locked: &Locked,
    t: &RemoteTarget,
    sentinels: &[String],
) -> Result<Vec<RemoteResolutionAuthorizationFact>, RemoteError> {
    // First-time inspection never creates fact/checkpoint storage.
    let Some(base) = existing_dir(&locked.team_dir, "checkpoints")? else {
        return Ok(vec![]);
    };
    let Some(seat) = existing_dir(&base, &t.seat)? else {
        return Ok(vec![]);
    };
    let Some(dir) = existing_dir(&seat, ".remote-resolution")? else {
        return Ok(vec![]);
    };
    let mut out = vec![];
    let mut scopes = HashSet::new();
    for (i, file) in std::fs::read_dir(&dir)
        .map_err(|_| RemoteError::Io)?
        .enumerate()
    {
        if i >= MAX_FACTS {
            return Err(RemoteError::Limit);
        }
        let file = file.map_err(|_| RemoteError::Io)?;
        let name = file
            .file_name()
            .into_string()
            .map_err(|_| RemoteError::Corrupt)?;
        let n: u64 = name
            .strip_suffix(".json")
            .ok_or(RemoteError::Corrupt)?
            .parse()
            .map_err(|_| RemoteError::Corrupt)?;
        if n == 0 || name != format!("{n}.json") {
            return Err(RemoteError::Corrupt);
        }
        let path = validate_component_path(&dir, &name, false).map_err(|_| RemoteError::Unsafe)?;
        if std::fs::symlink_metadata(&path)
            .map_err(|_| RemoteError::Io)?
            .len()
            > 16 * 1024
        {
            return Err(RemoteError::Limit);
        }
        let fact: RemoteResolutionAuthorizationFact =
            read_private_json(&path).map_err(|_| RemoteError::Corrupt)?;
        request_valid(
            &ResolutionRequest {
                expected_inventory_hash: fact.inventory_hash.clone(),
                scope: fact.scope.clone(),
                reference: fact.reference.clone(),
                decision: fact.decision.clone(),
                evidence_ref: fact.evidence_ref.clone(),
            },
            sentinels,
        )
        .map_err(|_| RemoteError::Corrupt)?;
        if fact.schema_version != 1
            || fact.fact_seq != n
            || fact.team != t.team
            || fact.target_seat != t.seat
            || fact.target_generation == 0
            || fact.target_generation > t.expected_generation
            || fact.written_at == 0
        {
            return Err(RemoteError::Corrupt);
        }
        if let Principal::Lead { seat, generation } = &fact.resolver {
            if !ident(seat, 31)
                || *generation == 0
                || *seat != locked.lead
                || *seat == t.seat
                || fact.scope != ResolutionScope::EffectResolution
            {
                return Err(RemoteError::Corrupt);
            }
        }
        // No latest-wins. Even duplicate/conflicting on-disk facts are invalid;
        // idempotent replay must have returned the original without appending.
        let key = (
            fact.target_generation,
            fact.inventory_hash.clone(),
            serde_json::to_string(&fact.scope).map_err(|_| RemoteError::Corrupt)?,
            fact.reference.clone(),
        );
        if !scopes.insert(key) {
            return Err(RemoteError::Conflict);
        }
        out.push(fact);
    }
    out.sort_by_key(|f| f.fact_seq);
    for (i, f) in out.iter().enumerate() {
        if f.fact_seq != i as u64 + 1 {
            return Err(RemoteError::Corrupt);
        }
    }
    Ok(out)
}
fn projection(
    view: RemoteInventoryView,
    all: &[RemoteResolutionAuthorizationFact],
) -> Result<RemoteProjection, RemoteError> {
    let matching: Vec<_> = all
        .iter()
        .filter(|f| {
            f.target_generation == view.target_generation && f.inventory_hash == view.inventory_hash
        })
        .collect();
    let risk = matching.iter().any(|f| {
        f.scope == ResolutionScope::InventoryRiskAcceptance && f.resolver == Principal::Operator
    });
    let refs: Vec<_> = view
        .effects
        .iter()
        .filter(|e| {
            matching.iter().any(|f| {
                f.scope == ResolutionScope::EffectResolution
                    && f.reference.as_ref() == Some(&e.reference)
            })
        })
        .map(|e| e.reference.clone())
        .collect();
    let may_proceed = (view.complete_observation || risk) && refs.len() == view.effects.len();
    let evidence_sha256 =
        digest(&serde_json::to_vec(&(all, &view)).map_err(|_| RemoteError::Corrupt)?);
    Ok(RemoteProjection {
        inventory: view,
        inventory_risk_accepted: risk,
        authorized_references: refs,
        source: if matching.is_empty() {
            None
        } else {
            Some("authorized_decision")
        },
        may_proceed,
        evidence_sha256,
    })
}
/// Authenticated inventory read for the existing control action. Lead may
/// inspect only another seat of its current immutable team, under the same
/// target/lead locks used for resolution. No inventory leaks before validation.
pub(crate) fn inspect_authorized(
    home: &Path,
    auth: ResolutionAuthority<'_>,
    t: &RemoteTarget,
    sentinels: &[String],
) -> Result<RemoteInventoryView, RemoteError> {
    let who = principal(&auth, t)?;
    let locked = open(home, t, Some(&who))?;
    if principal(&auth, t)? != who {
        return Err(RemoteError::Authority);
    }
    let view = inventory(&locked, t, sentinels)?;
    if principal(&auth, t)? != who {
        return Err(RemoteError::Authority);
    }
    owner(home, &t.seat, t.expected_generation)?;
    if let Principal::Lead { seat, generation } = &who {
        owner(home, seat, *generation)?;
    }
    Ok(view)
}
pub(crate) fn inspect_native(
    home: &Path,
    t: &RemoteTarget,
    sentinels: &[String],
) -> Result<RemoteInventoryView, RemoteError> {
    let locked = open(home, t, None)?;
    inventory(&locked, t, sentinels)
}
pub(crate) fn project_native(
    home: &Path,
    t: &RemoteTarget,
    sentinels: &[String],
) -> Result<RemoteProjection, RemoteError> {
    let locked = open(home, t, None)?;
    let view = inventory(&locked, t, sentinels)?;
    let all = facts(&locked, t, sentinels)?;
    projection(view, &all)
}

pub(crate) fn project_native_locked(
    home: &Path,
    t: &RemoteTarget,
    sentinels: &[String],
    team_lock: &AdvisoryLock,
    seat_locks: &[AdvisoryLock],
) -> Result<RemoteProjection, RemoteError> {
    let locked = open_prelocked(home, t, team_lock, seat_locks)?;
    let view = inventory(&locked, t, sentinels)?;
    let all = facts(&locked, t, sentinels)?;
    projection(view, &all)
}
fn principal(auth: &ResolutionAuthority<'_>, t: &RemoteTarget) -> Result<Principal, RemoteError> {
    match auth {
        ResolutionAuthority::Operator(actor) if actor.principal() == "operator" => {
            Ok(Principal::Operator)
        }
        ResolutionAuthority::Lead(actor) if actor.team() == t.team && actor.seat() != t.seat => {
            actor
                .revalidate_before_effect()
                .map_err(|_| RemoteError::Authority)?;
            Ok(Principal::Lead {
                seat: actor.seat().into(),
                generation: actor.generation(),
            })
        }
        _ => Err(RemoteError::Authority),
    }
}
pub(crate) fn resolve_native(
    home: &Path,
    auth: ResolutionAuthority<'_>,
    t: &RemoteTarget,
    request: &ResolutionRequest,
    sentinels: &[String],
) -> Result<ResolutionReceipt, RemoteError> {
    let who = principal(&auth, t)?;
    resolve_checked(home, &who, t, request, sentinels, || {
        if principal(&auth, t)? != who {
            return Err(RemoteError::Authority);
        }
        Ok(())
    })
}
// Private test seam only: production callers cannot supply a Principal or an
// authority boolean. Native inventory/clock/storage are never injected.
fn resolve_checked<F: FnMut() -> Result<(), RemoteError>>(
    home: &Path,
    who: &Principal,
    t: &RemoteTarget,
    r: &ResolutionRequest,
    sentinels: &[String],
    mut revalidate: F,
) -> Result<ResolutionReceipt, RemoteError> {
    request_valid(r, sentinels)?;
    if r.scope == ResolutionScope::InventoryRiskAcceptance && *who != Principal::Operator {
        return Err(RemoteError::Authority);
    }
    revalidate()?;
    let locked = open(home, t, Some(who))?;
    revalidate()?;
    let before = inventory(&locked, t, sentinels)?;
    if before.inventory_hash != r.expected_inventory_hash {
        return Err(RemoteError::StaleInventory);
    }
    if let Some(reference) = &r.reference {
        if !before.effects.iter().any(|e| e.reference == *reference) {
            return Err(RemoteError::Invalid);
        }
    }
    let all = facts(&locked, t, sentinels)?;
    if let Some(existing) = all.iter().find(|f| {
        f.target_generation == t.expected_generation
            && f.inventory_hash == before.inventory_hash
            && f.scope == r.scope
            && f.reference == r.reference
    }) {
        if existing.decision != r.decision
            || existing.evidence_ref != r.evidence_ref
            || existing.resolver != *who
        {
            return Err(RemoteError::Conflict);
        }
        revalidate()?;
        return Ok(ResolutionReceipt {
            fact_id: format!(
                "{}/{}/remote/{}",
                t.seat, t.expected_generation, existing.fact_seq
            ),
            inventory_hash: before.inventory_hash,
            source: "authorized_decision",
            complete_observation: before.complete_observation,
            replay: true,
        });
    }
    if all.len() >= MAX_FACTS {
        return Err(RemoteError::Limit);
    }
    let now: u64 = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| RemoteError::Io)?
        .as_millis()
        .try_into()
        .map_err(|_| RemoteError::Io)?;
    if all.iter().any(|f| f.written_at > now) {
        return Err(RemoteError::Corrupt);
    }
    let fact = RemoteResolutionAuthorizationFact {
        schema_version: 1,
        fact_seq: all.len() as u64 + 1,
        team: t.team.clone(),
        target_seat: t.seat.clone(),
        target_generation: t.expected_generation,
        inventory_hash: before.inventory_hash.clone(),
        scope: r.scope.clone(),
        reference: r.reference.clone(),
        decision: r.decision.clone(),
        evidence_ref: r.evidence_ref.clone(),
        resolver: who.clone(),
        written_at: now,
        source: FactSource::AuthorizedDecision,
    };
    revalidate()?;
    owner(home, &t.seat, t.expected_generation)?;
    if let Principal::Lead { seat, generation } = who {
        owner(home, seat, *generation)?;
    }
    if inventory(&locked, t, sentinels)? != before {
        return Err(RemoteError::StaleInventory);
    }
    let dir = locked.checkpoints.join(".remote-resolution");
    ensure_private_dir(&dir).map_err(|_| RemoteError::Unsafe)?;
    let path = validate_component_path(&dir, &format!("{}.json", fact.fact_seq), true)
        .map_err(|_| RemoteError::Unsafe)?;
    revalidate()?;
    owner(home, &t.seat, t.expected_generation)?;
    if let Principal::Lead { seat, generation } = who {
        owner(home, seat, *generation)?;
    }
    if inventory(&locked, t, sentinels)? != before {
        return Err(RemoteError::StaleInventory);
    }
    write_private_json_atomic(&path, &fact, false).map_err(|_| RemoteError::Io)?;
    let actual: RemoteResolutionAuthorizationFact =
        read_private_json(&path).map_err(|_| RemoteError::Corrupt)?;
    if actual != fact {
        return Err(RemoteError::Corrupt);
    }
    // This readback proves durability only; it does not turn Unknown into an
    // observation or pretend arbitrary shell/SSH work has been instrumented.
    Ok(ResolutionReceipt {
        fact_id: format!(
            "{}/{}/remote/{}",
            t.seat, t.expected_generation, fact.fact_seq
        ),
        inventory_hash: before.inventory_hash,
        source: "authorized_decision",
        complete_observation: before.complete_observation,
        replay: false,
    })
}

#[cfg(test)]
#[path = "team_remote_native_tests.rs"]
mod tests;
