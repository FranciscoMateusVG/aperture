//! Append-only authenticated validation facts. Internal collector/authority callbacks,
//! NOT a JSON command accepting caller-asserted validation or observations.
use crate::journal::{
    ensure_private_dir, read_private_json, validate_component_path, write_private_json_atomic,
};
use crate::owner::{try_lock, AdvisoryLock, OwnerRecord, OwnerStore};
use crate::state::OwnerState;
use crate::team_checkpoint::{
    ArtifactObservation, CheckpointEntry, CheckpointError, CheckpointValidation,
};
use crate::teams::{classify_managed_seat, ManagedSeatState, TeamSnapshot};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Created by authenticated control. None of these fields establishes authority
/// by itself: the current lead, owner generation, and capability are rechecked.
/// A selector may refer to an older worker generation for recovery.
pub(crate) struct ValidationContext {
    pub team: String,
    pub seat: String,
    pub generation: u64,
    pub lead_seat: String,
    pub lead_generation: u64,
}
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ValidatorKind {
    #[default]
    Lead,
    Glados,
}
impl ValidatorKind {
    fn is_lead(&self) -> bool { *self == Self::Lead }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ValidationFact {
    schema_version: u32,
    checkpoint_id: String,
    content_hash: String,
    fact_seq: u64,
    // Omitted for legacy facts, preserving their canonical serialization.
    #[serde(default, skip_serializing_if = "ValidatorKind::is_lead")]
    validator_kind: ValidatorKind,
    validated_by: String,
    validator_generation: u64,
    validated_at: u64,
    observation: ArtifactObservation,
    result: CheckpointValidation,
}
impl ValidationFact {
    pub(crate) fn result(&self) -> &CheckpointValidation { &self.result }
}
struct LockedHistory {
    // Drop seat locks before team lock. Acquire seats in lexical order.
    _seats: Vec<AdvisoryLock>,
    _team: Option<AdvisoryLock>,
    dir: PathBuf,
}
pub(crate) struct CheckpointEvidence {
    pub validated_entries: Vec<CheckpointEntry>,
    pub historical_bindings: Vec<CheckpointEntry>,
    pub sha256: String,
}
fn open(home: &Path, ctx: &ValidationContext) -> Result<LockedHistory, CheckpointError> {
    use crate::team_checkpoint::identifier;
    if !identifier(&ctx.team, 16)
        || !identifier(&ctx.seat, 31)
        || !identifier(&ctx.lead_seat, 31)
        || ctx.generation == 0
        || ctx.lead_generation == 0
    {
        return Err(CheckpointError::Generation);
    }
    let team_lock = try_lock(&home.join(".aperture/run/team-locks"), &ctx.team)
        .map_err(|_| CheckpointError::Io)?;
    for seat in [&ctx.seat, &ctx.lead_seat] {
        match classify_managed_seat(home, seat).map_err(|_| CheckpointError::Corrupt)? {
            Some(ManagedSeatState::Active { team, .. }) if team == ctx.team => {}
            _ => return Err(CheckpointError::Generation),
        }
    }
    let root = home.join(".aperture/teams");
    let team_dir =
        validate_component_path(&root, &ctx.team, false).map_err(|_| CheckpointError::Unsafe)?;
    let snapshot: TeamSnapshot =
        read_private_json(&team_dir.join("team.json")).map_err(|_| CheckpointError::Corrupt)?;
    if snapshot.lead != ctx.lead_seat {
        return Err(CheckpointError::Generation);
    }
    let owners = OwnerStore::new(home.join(".aperture/run/owner"));
    let mut names = vec![ctx.seat.as_str(), ctx.lead_seat.as_str()];
    names.sort_unstable();
    names.dedup();
    let mut locks = Vec::new();
    for name in names {
        locks.push(owners.lock(name).map_err(|_| CheckpointError::Io)?);
    }
    let lead: OwnerRecord = read_private_json(&owners.record_path(&ctx.lead_seat))
        .map_err(|_| CheckpointError::Corrupt)?;
    let target: OwnerRecord =
        read_private_json(&owners.record_path(&ctx.seat)).map_err(|_| CheckpointError::Corrupt)?;
    let actual = lead.incarnation.as_ref().ok_or(CheckpointError::Corrupt)?;
    if lead.schema_version != 1
        || target.schema_version != 1
        || lead.seat != ctx.lead_seat
        || target.seat != ctx.seat
        || lead.generation != ctx.lead_generation
        || lead.state != OwnerState::Active
        || target.generation < ctx.generation
        || actual.harness != lead.requested.harness
        || actual.model != lead.requested.model
        || actual.reasoning != lead.requested.reasoning
    {
        return Err(CheckpointError::Generation);
    }
    let dir = validate_component_path(
        &root,
        &format!("{}/checkpoints/{}", ctx.team, ctx.seat),
        false,
    )
    .map_err(|_| CheckpointError::Unsafe)?;
    Ok(LockedHistory {
        _seats: locks,
        _team: Some(team_lock),
        dir,
    })
}

fn open_prelocked(
    home: &Path,
    ctx: &ValidationContext,
    _team_lock: &AdvisoryLock,
    _seat_locks: &[AdvisoryLock],
) -> Result<LockedHistory, CheckpointError> {
    use crate::team_checkpoint::identifier;
    if !identifier(&ctx.team, 16)
        || !identifier(&ctx.seat, 31)
        || !identifier(&ctx.lead_seat, 31)
        || ctx.generation == 0
        || ctx.lead_generation == 0
    {
        return Err(CheckpointError::Generation);
    }
    let root = home.join(".aperture/teams");
    let team_dir =
        validate_component_path(&root, &ctx.team, false).map_err(|_| CheckpointError::Unsafe)?;
    let snapshot: TeamSnapshot =
        read_private_json(&team_dir.join("team.json")).map_err(|_| CheckpointError::Corrupt)?;
    if snapshot.lead != ctx.lead_seat {
        return Err(CheckpointError::Generation);
    }
    let owners = OwnerStore::new(home.join(".aperture/run/owner"));
    let lead: OwnerRecord = read_private_json(&owners.record_path(&ctx.lead_seat))
        .map_err(|_| CheckpointError::Corrupt)?;
    let target: OwnerRecord =
        read_private_json(&owners.record_path(&ctx.seat)).map_err(|_| CheckpointError::Corrupt)?;
    let actual = lead.incarnation.as_ref().ok_or(CheckpointError::Corrupt)?;
    if lead.generation != ctx.lead_generation
        || lead.state != OwnerState::Active
        || target.generation < ctx.generation
        || !actual.observed
    {
        return Err(CheckpointError::Generation);
    }
    let dir = validate_component_path(
        &root,
        &format!("{}/checkpoints/{}", ctx.team, ctx.seat),
        false,
    )
    .map_err(|_| CheckpointError::Unsafe)?;
    Ok(LockedHistory {
        _seats: vec![],
        _team: None,
        dir,
    })
}
fn checked_entries(
    history: &LockedHistory,
    ctx: &ValidationContext,
    sentinels: &[String],
) -> Result<Vec<CheckpointEntry>, CheckpointError> {
    let mut entries = super::read_raw_entries(&history.dir, ctx.generation)?;
    let mut seqs = HashSet::new();
    for entry in &entries {
        crate::team_checkpoint::validate_payload(&entry.payload, sentinels)
            .map_err(|_| CheckpointError::Corrupt)?;
        let canonical = serde_json::to_vec(&(entry.schema_version, &entry.payload))
            .map_err(|_| CheckpointError::Corrupt)?;
        let expected_validation = if entry.schema_version == 1 {
            CheckpointValidation::Pending
        } else {
            CheckpointValidation::Rejected {
                code: "E_CHECKPOINT_SCHEMA".into(),
            }
        };
        if entry.team != ctx.team
            || entry.seat != ctx.seat
            || entry.generation != ctx.generation
            || entry.seq == 0
            || !seqs.insert(entry.seq)
            || entry.checkpoint_id != format!("{}/{}/{}", ctx.seat, ctx.generation, entry.seq)
            || entry.content_hash != format!("{:x}", Sha256::digest(canonical))
            || entry.validation != expected_validation
        {
            return Err(CheckpointError::Corrupt);
        }
    }
    entries.sort_by_key(|entry| entry.seq);
    Ok(entries)
}
fn observation_is_safe(
    entry: &CheckpointEntry,
    actual: &ArtifactObservation,
    sentinels: &[String],
) -> Result<(), CheckpointError> {
    // Reuse precisely the payload allowlist for the observed fields too. No
    // stdout/stderr, provider URLs, arbitrary JSON or git error text is retained.
    let mut payload = entry.payload.clone();
    payload.head_sha = actual.head_sha.clone();
    payload.dirty_files = actual.dirty_files.clone();
    payload.open_pr = actual.open_pr.clone();
    crate::team_checkpoint::validate_payload(&payload, sentinels)
}
fn read_facts(
    history: &LockedHistory,
    ctx: &ValidationContext,
    entries: &[CheckpointEntry],
    sentinels: &[String],
) -> Result<Vec<ValidationFact>, CheckpointError> {
    let dir = history.dir.join(".validation");
    match std::fs::symlink_metadata(&dir) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(_) => return Err(CheckpointError::Io),
        Ok(_) => {}
    }
    validate_component_path(&history.dir, ".validation", false)
        .map_err(|_| CheckpointError::Unsafe)?;
    let mut facts = Vec::new();
    for file in std::fs::read_dir(&dir).map_err(|_| CheckpointError::Io)? {
        let name = file
            .map_err(|_| CheckpointError::Io)?
            .file_name()
            .into_string()
            .map_err(|_| CheckpointError::Corrupt)?;
        if name.starts_with('.') && name.ends_with(".tmp") {
            continue;
        }
        let nums = name
            .strip_suffix(".json")
            .ok_or(CheckpointError::Corrupt)?
            .split('-')
            .map(|x| x.parse::<u64>().map_err(|_| CheckpointError::Corrupt))
            .collect::<Result<Vec<_>, _>>()?;
        if nums.len() != 3
            || nums.iter().any(|v| *v == 0)
            || name != format!("{}-{}-{}.json", nums[0], nums[1], nums[2])
        {
            return Err(CheckpointError::Corrupt);
        }
        if nums[0] != ctx.generation {
            continue;
        }
        let path =
            validate_component_path(&dir, &name, false).map_err(|_| CheckpointError::Unsafe)?;
        let fact: ValidationFact =
            read_private_json(&path).map_err(|_| CheckpointError::Corrupt)?;
        let entry = entries
            .iter()
            .find(|e| e.seq == nums[1])
            .ok_or(CheckpointError::Corrupt)?;
        observation_is_safe(entry, &fact.observation, sentinels)
            .map_err(|_| CheckpointError::Corrupt)?;
        let validator_matches = match fact.validator_kind {
            ValidatorKind::Lead => fact.validated_by == ctx.lead_seat
                && fact.validator_generation > 0
                && fact.validator_generation <= ctx.lead_generation,
            ValidatorKind::Glados => fact.validated_by == "glados"
                && fact.validator_generation == ctx.generation,
        };
        if fact.schema_version != 1
            || fact.checkpoint_id != entry.checkpoint_id
            || fact.content_hash != entry.content_hash
            || fact.fact_seq != nums[2]
            || !validator_matches
            || fact.validated_at < entry.written_at
            || fact.result
                != crate::team_checkpoint::validate_against_artifacts(entry, &fact.observation)
        {
            return Err(CheckpointError::Corrupt);
        }
        facts.push(fact);
    }
    facts.sort_by_key(|f| f.fact_seq);
    // Fact sequence is global within this worker generation, assigned under the
    // owner lock. Missing/duplicated evidence is not silently projected green.
    for (i, fact) in facts.iter().enumerate() {
        if fact.fact_seq != (i as u64) + 1 {
            return Err(CheckpointError::Corrupt);
        }
    }
    Ok(facts)
}

fn project_evidence(
    history: &LockedHistory,
    ctx: &ValidationContext,
    sentinels: &[String],
) -> Result<CheckpointEvidence, CheckpointError> {
    let entries = checked_entries(history, ctx, sentinels)?;
    let facts = read_facts(history, ctx, &entries, sentinels)?;
    // Bind the immutable checkpoint entries and every validated fact field,
    // not only the latest projected result. A later Ok fact with different
    // observation/provenance is archive evidence even when the projection is
    // still Ok.
    let canonical = serde_json::to_vec(&(
        "aperture.checkpoint-evidence.v1",
        ctx.generation,
        &entries,
        &facts,
    ))
    .map_err(|_| CheckpointError::Corrupt)?;
    let sha256 = format!("{:x}", Sha256::digest(canonical));
    let historical_bindings = entries
        .iter()
        .filter(|entry| {
            facts.iter().any(|fact| {
                fact.checkpoint_id == entry.checkpoint_id
                    && fact.content_hash == entry.content_hash
                    && fact.result == CheckpointValidation::Ok
            })
        })
        .cloned()
        .collect();
    let mut validated_entries = entries;
    for fact in facts {
        let entry = validated_entries
            .iter_mut()
            .find(|entry| entry.checkpoint_id == fact.checkpoint_id)
            .ok_or(CheckpointError::Corrupt)?;
        entry.validation = fact.result;
    }
    Ok(CheckpointEvidence {
        validated_entries,
        historical_bindings,
        sha256,
    })
}

pub(crate) fn checkpoint_evidence_native<F>(
    home: &Path,
    ctx: &ValidationContext,
    sentinels: &[String],
    mut revalidate: F,
) -> Result<CheckpointEvidence, CheckpointError>
where
    F: FnMut() -> Result<(), CheckpointError>,
{
    revalidate()?;
    let history = open(home, ctx)?;
    revalidate()?;
    let evidence = project_evidence(&history, ctx, sentinels)?;
    revalidate()?;
    Ok(evidence)
}

pub(crate) fn checkpoint_evidence_native_locked(
    home: &Path,
    ctx: &ValidationContext,
    sentinels: &[String],
    team_lock: &AdvisoryLock,
    seat_locks: &[AdvisoryLock],
) -> Result<CheckpointEvidence, CheckpointError> {
    let history = open_prelocked(home, ctx, team_lock, seat_locks)?;
    project_evidence(&history, ctx, sentinels)
}

/// The native control adapter supplies a lead capability revalidator AND a
/// bounded git/gh collector; observations cannot arrive in a worker/UI DTO.
/// No public command is registered until those two seams are wired.
pub(crate) fn validate_native<F, C>(
    home: &Path,
    ctx: &ValidationContext,
    seq: u64,
    now: u64,
    sentinels: &[String],
    revalidate: F,
    collect: C,
) -> Result<ValidationFact, CheckpointError>
where
    F: FnMut() -> Result<(), CheckpointError>,
    C: FnMut(&CheckpointEntry) -> Result<ArtifactObservation, CheckpointError>,
{
    validate_as(home, ctx, seq, now, sentinels, ValidatorKind::Lead, revalidate, collect)
}

/// Root authority is a real native capability, never a worker-provided kind/name.
/// Selecting an immutable checkpoint explicitly approves its mission binding;
/// its artifact validity is still computed by the native repository collector.
pub(crate) fn validate_glados_native<C>(
    home: &Path,
    actor: &crate::team_auth::AuthenticatedActor,
    ctx: &ValidationContext,
    seq: u64,
    now: u64,
    sentinels: &[String],
    collect: C,
) -> Result<ValidationFact, CheckpointError>
where C: FnMut(&CheckpointEntry) -> Result<ArtifactObservation, CheckpointError>,
{
    let revalidate = || {
        if !actor.is_glados() { return Err(CheckpointError::Generation); }
        actor.revalidate_before_mutation().map_err(|_| CheckpointError::Generation)
    };
    validate_as(home, ctx, seq, now, sentinels, ValidatorKind::Glados, revalidate, collect)
}

fn validate_as<F, C>(
    home: &Path, ctx: &ValidationContext, seq: u64, now: u64,
    sentinels: &[String], validator: ValidatorKind,
    mut revalidate: F, mut collect: C,
) -> Result<ValidationFact, CheckpointError>
where
    F: FnMut() -> Result<(), CheckpointError>,
    C: FnMut(&CheckpointEntry) -> Result<ArtifactObservation, CheckpointError>,
{
    revalidate()?;
    let history = open(home, ctx)?;
    revalidate()?;
    let entries = checked_entries(&history, ctx, sentinels)?;
    let entry = entries
        .iter()
        .find(|e| e.seq == seq)
        .ok_or(CheckpointError::Invalid)?;
    if now < entry.written_at {
        return Err(CheckpointError::Invalid);
    }
    let facts = read_facts(&history, ctx, &entries, sentinels)?;
    if facts.iter().any(|f| f.validated_at > now) {
        return Err(CheckpointError::Corrupt);
    }
    // Unknown schema is already durably rejected; do not invoke a collector.
    if entry.schema_version != 1 {
        return Err(CheckpointError::Invalid);
    }
    let observation = collect(entry)?;
    observation_is_safe(entry, &observation, sentinels)?;
    let fact = ValidationFact {
        schema_version: 1,
        checkpoint_id: entry.checkpoint_id.clone(),
        content_hash: entry.content_hash.clone(),
        fact_seq: (facts.len() as u64)
            .checked_add(1)
            .ok_or(CheckpointError::Corrupt)?,
        validator_kind: validator,
        validated_by: if validator.is_lead() { ctx.lead_seat.clone() } else { "glados".into() },
        validator_generation: if validator.is_lead() { ctx.lead_generation } else { ctx.generation },
        validated_at: now,
        result: crate::team_checkpoint::validate_against_artifacts(entry, &observation),
        observation,
    };
    revalidate()?;
    let dir = history.dir.join(".validation");
    ensure_private_dir(&dir).map_err(|_| CheckpointError::Unsafe)?;
    let path = validate_component_path(
        &dir,
        &format!("{}-{}-{}.json", ctx.generation, seq, fact.fact_seq),
        true,
    )
    .map_err(|_| CheckpointError::Unsafe)?;
    revalidate()?;
    write_private_json_atomic(&path, &fact, false).map_err(|_| CheckpointError::Io)?;
    Ok(fact)
}
pub(crate) fn validated_entries_native<F>(
    home: &Path,
    ctx: &ValidationContext,
    sentinels: &[String],
    mut revalidate: F,
) -> Result<Vec<CheckpointEntry>, CheckpointError>
where
    F: FnMut() -> Result<(), CheckpointError>,
{
    revalidate()?;
    let history = open(home, ctx)?;
    revalidate()?;
    let mut entries = checked_entries(&history, ctx, sentinels)?;
    let facts = read_facts(&history, ctx, &entries, sentinels)?;
    for fact in facts {
        let entry = entries
            .iter_mut()
            .find(|e| e.checkpoint_id == fact.checkpoint_id)
            .ok_or(CheckpointError::Corrupt)?;
        entry.validation = fact.result;
    }
    revalidate()?;
    Ok(entries)
}

/// Historical authentication is path-binding evidence, NOT current artifact
/// validity. Entries retain their raw Pending validation; callers must perform
/// fresh native collection before treating recovery contents as current.
pub(crate) fn historical_bindings_native<F>(
    home: &Path,
    ctx: &ValidationContext,
    sentinels: &[String],
    mut revalidate: F,
) -> Result<Vec<CheckpointEntry>, CheckpointError>
where
    F: FnMut() -> Result<(), CheckpointError>,
{
    revalidate()?;
    let history = open(home, ctx)?;
    revalidate()?;
    let entries = checked_entries(&history, ctx, sentinels)?;
    let facts = read_facts(&history, ctx, &entries, sentinels)?;
    let bound = entries
        .into_iter()
        .filter(|e| {
            facts.iter().any(|f| {
                f.checkpoint_id == e.checkpoint_id
                    && f.content_hash == e.content_hash
                    && f.result == CheckpointValidation::Ok
            })
        })
        .collect();
    revalidate()?;
    Ok(bound)
}
