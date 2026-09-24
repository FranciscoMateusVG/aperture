//! Explicit team withdrawal, not completion of the team's task or epic.
//! Only native stop/revocation can publish the bound retirement fact.
use crate::journal::{
    read_private_json, write_private_json_atomic, ArchiveCategory, ArchiveJournalApproval,
};
use crate::owner::{try_lock, AdvisoryLock, OwnerRecord, OwnerStore};
use crate::state::{ExecutionTuple, OwnerState};
use crate::team_auth::AuthenticatedActor;
use crate::team_replacement::{CheckpointRecovery, ProcessIdentity, ProcessState};
use crate::teams::{TeamLifecycle, TeamSnapshot, TeamStateFile};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

fn fail() -> String {
    "E_ARCHIVE_RETIREMENT_UNVERIFIED: retirement facts unavailable".into()
}
fn hash<T: Serialize>(value: &T) -> Result<String, String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(value).map_err(|_| fail())?)
    ))
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct RetirementFact {
    schema_version: u32,
    team: String,
    seat: String,
    team_generation: u64,
    generation: u64,
    token_id: String,
    snapshot_sha256: String,
    owner_sha256: String,
    checkpoint_recovery: CheckpointRecovery,
    accepted_checkpoint_loss: bool,
    stopped_at_ms: i64,
    writer: String,
}
fn fact_path(home: &Path, seat: &str, generation: u64) -> PathBuf {
    home.join(".aperture/run/managed")
        .join(seat)
        .join(format!("g{generation}"))
        .join("retired.json")
}
fn token_absent(home: &Path, seat: &str) -> Result<(), String> {
    let path = crate::journal::validate_component_path(
        &home.join(".aperture/run/hub-tokens"),
        &format!("{seat}.token"),
        true,
    )
    .map_err(|_| fail())?;
    if matches!(std::fs::symlink_metadata(path), Err(e) if e.kind()==std::io::ErrorKind::NotFound) {
        Ok(())
    } else {
        Err(fail())
    }
}
fn bound_owner(
    snapshot: &TeamSnapshot,
    state: &TeamStateFile,
    owner: &OwnerRecord,
) -> Result<(), String> {
    let rows: Vec<_> = snapshot
        .seats
        .iter()
        .filter(|s| s.name == owner.seat)
        .collect();
    if snapshot.schema_version != 1
        || state.schema_version != 1
        || state.state != TeamLifecycle::Active
        || state.generation == 0
        || rows.len() != 1
        || owner.schema_version != 1
        || owner.generation == 0
        || owner.state != OwnerState::Active
        || owner.reservation_nonce_sha256.is_some()
        || owner.provisional_token_id.is_some()
    {
        return Err(fail());
    }
    let configured = ExecutionTuple {
        harness: rows[0].harness.clone(),
        model: rows[0].model.clone(),
        reasoning: rows[0].reasoning.clone(),
    };
    let i = owner.incarnation.as_ref().ok_or_else(fail)?;
    if (owner.requested != configured && !snapshot.fallbacks.contains(&owner.requested))
        || !i.observed
        || i.harness != owner.requested.harness
        || i.model != owner.requested.model
        || i.reasoning != owner.requested.reasoning
        || i.thread_id.is_empty()
        || i.processes.is_empty()
        || i.processes.len() > 256
        || !i
            .processes
            .iter()
            .any(|p| p.pid == i.pid && p.start_time == i.start_time)
    {
        return Err(fail());
    }
    Ok(())
}
fn stopped(
    home: &Path,
    owner: &OwnerRecord,
    mut state: impl FnMut(&ProcessIdentity) -> ProcessState,
) -> Result<(), String> {
    let i = owner.incarnation.as_ref().ok_or_else(fail)?;
    let mut ids = std::collections::BTreeSet::new();
    for p in &i.processes {
        if !ids.insert((p.pid, p.start_time)) {
            return Err(fail());
        }
        let id =
            crate::team_process::identity_from_owner(p.pid, p.start_time).map_err(|_| fail())?;
        if state(&id) != ProcessState::Gone {
            return Err(fail());
        }
    }
    crate::ws_hub::managed_control::verify_floor(home, &owner.seat, owner.generation, &i.token_id)
        .map_err(|_| fail())?;
    token_absent(home, &owner.seat)
}
pub(crate) fn record_stopped(
    home: &Path,
    actor: &AuthenticatedActor,
    team: &str,
    seat: &str,
    generation: u64,
    recovery: CheckpointRecovery,
    accepted_checkpoint_loss: bool,
) -> Result<(), String> {
    if !actor.is_glados() {
        return Err(fail());
    }
    actor.revalidate_before_mutation()?;
    if !crate::agent_loader::is_valid_seat_name(team)
        || team.len() > 16
        || !crate::agent_loader::is_valid_seat_name(seat)
        || generation == 0
        || (!accepted_checkpoint_loss && recovery != CheckpointRecovery::Valid)
    {
        return Err(fail());
    }
    let _team = try_lock(&home.join(".aperture/run/team-locks"), team)?;
    let store = OwnerStore::new(home.join(".aperture/run/owner"));
    let _seat = store.lock(seat)?;
    actor.revalidate_before_mutation()?;
    let snapshot: TeamSnapshot =
        read_private_json(&home.join(".aperture/teams").join(team).join("team.json"))?;
    let state: TeamStateFile =
        read_private_json(&home.join(".aperture/teams").join(team).join("state.json"))?;
    let owner = store.read_owner_locked(seat)?;
    if snapshot.team != team || owner.seat != seat || owner.generation != generation {
        return Err(fail());
    }
    bound_owner(&snapshot, &state, &owner)?;
    stopped(home, &owner, crate::team_process::state)?;
    let fact = RetirementFact {
        schema_version: 1,
        team: team.into(),
        seat: seat.into(),
        team_generation: state.generation,
        generation,
        token_id: owner
            .incarnation
            .as_ref()
            .ok_or_else(fail)?
            .token_id
            .clone(),
        snapshot_sha256: hash(&snapshot)?,
        owner_sha256: hash(&owner)?,
        checkpoint_recovery: recovery,
        accepted_checkpoint_loss,
        stopped_at_ms: chrono::Utc::now().timestamp_millis(),
        writer: "glados".into(),
    };
    actor.revalidate_before_mutation()?;
    let path = fact_path(home, seat, generation);
    write_private_json_atomic(&path, &fact, false)?;
    if read_private_json::<RetirementFact>(&path)? != fact {
        return Err(fail());
    }
    Ok(())
}
pub(crate) fn has_facts(home: &Path, snapshot: &TeamSnapshot) -> Result<bool, String> {
    let owners = OwnerStore::new(home.join(".aperture/run/owner"));
    let mut count = 0;
    for s in &snapshot.seats {
        let o = owners.read_owner(&s.name)?;
        match std::fs::symlink_metadata(fact_path(home, &s.name, o.generation)) {
            Ok(_) => count += 1,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(fail()),
        }
    }
    if count != 0 && count != snapshot.seats.len() {
        return Err(fail());
    }
    Ok(count > 0)
}
pub(crate) fn inspect(
    home: &Path,
    snapshot: &TeamSnapshot,
    state: &TeamStateFile,
) -> Result<ArchiveJournalApproval, String> {
    let team = try_lock(&home.join(".aperture/run/team-locks"), &snapshot.team)?;
    let store = OwnerStore::new(home.join(".aperture/run/owner"));
    let mut names: Vec<_> = snapshot.seats.iter().map(|s| s.name.clone()).collect();
    names.sort();
    names.dedup();
    if names.is_empty() || names.len() != snapshot.seats.len() || names.len() > 32 {
        return Err(fail());
    }
    let locks = names
        .iter()
        .map(|s| store.lock(s))
        .collect::<Result<Vec<_>, _>>()?;
    inspect_locked(home, snapshot, state, &team, &locks)
}
pub(crate) fn inspect_locked(
    home: &Path,
    snapshot: &TeamSnapshot,
    state: &TeamStateFile,
    _team: &AdvisoryLock,
    seats: &[AdvisoryLock],
) -> Result<ArchiveJournalApproval, String> {
    inspect_with(
        home,
        snapshot,
        state,
        seats.len(),
        crate::team_process::state,
    )
}
fn inspect_with(
    home: &Path,
    snapshot: &TeamSnapshot,
    state: &TeamStateFile,
    lock_count: usize,
    mut process_state: impl FnMut(&ProcessIdentity) -> ProcessState,
) -> Result<ArchiveJournalApproval, String> {
    if snapshot.team.len() > 16
        || !crate::agent_loader::is_valid_seat_name(&snapshot.team)
        || snapshot.seats.is_empty()
        || snapshot.seats.len() > 32
        || lock_count != snapshot.seats.len()
    {
        return Err(fail());
    }
    let root = home.join(".aperture/teams").join(&snapshot.team);
    if read_private_json::<TeamSnapshot>(&root.join("team.json"))? != *snapshot
        || read_private_json::<TeamStateFile>(&root.join("state.json"))? != *state
    {
        return Err(fail());
    }
    let mut names: Vec<_> = snapshot.seats.iter().map(|s| s.name.clone()).collect();
    names.sort();
    names.dedup();
    if names.len() != snapshot.seats.len() || !names.contains(&snapshot.lead) {
        return Err(fail());
    }
    let store = OwnerStore::new(home.join(".aperture/run/owner"));
    let mut owners = Vec::new();
    let mut evidence = Vec::new();
    for seat in names {
        let owner = store.read_owner_locked(&seat)?;
        bound_owner(snapshot, state, &owner)?;
        let fact: RetirementFact = read_private_json(&fact_path(home, &seat, owner.generation))?;
        let i = owner.incarnation.as_ref().ok_or_else(fail)?;
        if fact.schema_version != 1
            || fact.team != snapshot.team
            || fact.seat != seat
            || fact.team_generation != state.generation
            || fact.generation != owner.generation
            || fact.token_id != i.token_id
            || fact.snapshot_sha256 != hash(snapshot)?
            || fact.owner_sha256 != hash(&owner)?
            || fact.writer != "glados"
            || fact.stopped_at_ms <= 0
            || fact.stopped_at_ms > chrono::Utc::now().timestamp_millis()
            || (!fact.accepted_checkpoint_loss
                && fact.checkpoint_recovery != CheckpointRecovery::Valid)
        {
            return Err(fail());
        }
        stopped(home, &owner, &mut process_state)?;
        let floor: serde_json::Value = read_private_json(
            &home
                .join(".aperture/run/revocations")
                .join(format!("{seat}.json")),
        )?;
        owners.push((seat.clone(), hash(&owner)?));
        evidence.push((seat, hash(&(fact, floor))?));
    }
    Ok(ArchiveJournalApproval {
        category: ArchiveCategory::Retirement,
        generation: state.generation,
        epic_id: state
            .epic_id
            .clone()
            .filter(|v| !v.is_empty())
            .ok_or_else(fail)?,
        record_sha256: hash(&("retirement-facts-v1", &evidence))?,
        inventory_sha256: hash(&("retirement-seats-v1", snapshot, state, &owners))?,
        native_sha256: hash(&("retirement-cleanup-v1", &owners, &evidence))?,
        owner_states: owners
            .iter()
            .map(|(s, _)| (s.clone(), "active".into()))
            .collect(),
        owner_sha256: owners,
        owner_post_sha256: vec![],
        transition_at: String::new(),
        approved_by: "glados".into(),
    })
}

#[cfg(test)]
#[path = "team_archive_retirement_tests.rs"]
mod tests;
