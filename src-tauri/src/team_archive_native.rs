//! Read-only native seat join for archive. Does not stop, revoke, mark owners
//! stale or mint approval. Private source facts are never accepted in a DTO.
use super::ArchiveBlocker;
use crate::journal::read_private_json;
use crate::owner::{OwnerRecord, OwnerStore};
use crate::state::OwnerState;
use crate::team_replacement::{remote, repository, ProcessState};
use crate::teams::{TeamLifecycle, TeamSnapshot, TeamStateFile};
use serde::Serialize;
use std::path::Path;
use std::time::{Duration, Instant};

/// Native-derived projection only; serialized form is a receipt, not authority.
#[derive(Serialize)]
pub(crate) struct NativeSeatView {
    pub(crate) seat: String,
    pub(crate) generation: u64,
    pub(crate) never_started: bool,
    pub(crate) owner_sha256: String,
    pub(crate) exact_processes_gone: bool,
    pub(crate) process_evidence_sha256: Option<String>,
    pub(crate) revocation_metadata_verified: bool,
    pub(crate) remote_reconciled: bool,
    pub(crate) remote_complete_observation: bool,
    pub(crate) remote_source: Option<&'static str>,
    pub(crate) remote_inventory_sha256: Option<String>,
    pub(crate) remote_evidence_sha256: Option<String>,
    pub(crate) checkpoint_evidence_sha256: Option<String>,
    pub(crate) worktree_observation_sha256: Option<String>,
    pub(crate) bound_worktree_count: usize,
    pub(crate) worktrees_clean: bool,
    pub(crate) owner_stale_verified: bool,
    pub(crate) blockers: Vec<ArchiveBlocker>,
}
/// Holds native evidence, not an approval or a lock/permission to move paths.
pub(crate) struct NativeSeatInspection {
    pub(crate) seats: Vec<NativeSeatView>,
    pub(crate) sha256: String,
}
fn blocker(code: &str, seat: &str) -> ArchiveBlocker {
    ArchiveBlocker {
        code: code.into(),
        reference: seat.into(),
    }
}
fn hash<T: Serialize>(v: &T) -> Result<String, ArchiveBlocker> {
    use sha2::{Digest, Sha256};
    Ok(format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(v).map_err(|_| blocker("E_ARCHIVE_NATIVE_INVALID", "archive"))?
        )
    ))
}
fn check_time(until: Instant) -> Result<(), ArchiveBlocker> {
    if Instant::now() >= until {
        Err(blocker("E_ARCHIVE_NATIVE_DEADLINE", "archive"))
    } else {
        Ok(())
    }
}
fn read_owner(home: &Path, seat: &str) -> Result<OwnerRecord, ArchiveBlocker> {
    OwnerStore::new(home.join(".aperture/run/owner"))
        .read_owner(seat)
        .map_err(|_| blocker("E_ARCHIVE_OWNER_INVALID", seat))
}
fn admitted_owner(r: &OwnerRecord, seat: &str) -> bool {
    r.schema_version == 1
        && r.seat == seat
        && r.generation > 0
        && r.state == OwnerState::Active
        && r.incarnation.as_ref().is_some_and(|i| {
            i.observed
                && i.harness == r.requested.harness
                && i.model == r.requested.model
                && i.reasoning == r.requested.reasoning
        })
}
fn bound_coverage(
    referenced: &std::collections::BTreeSet<String>,
    bound: &std::collections::BTreeSet<String>,
) -> bool {
    !bound.is_empty() && referenced.is_subset(bound)
}
fn processes_gone<F>(
    owner: &OwnerRecord,
    snapshot: &crate::team_replacement::OwnershipSnapshot,
    mut state: F,
) -> bool
where
    F: FnMut(&crate::team_replacement::ProcessIdentity) -> ProcessState,
{
    let Some(inc) = owner.incarnation.as_ref() else {
        return false;
    };
    snapshot.seat == owner.seat
        && snapshot.generation == owner.generation
        && snapshot.thread_id == inc.thread_id
        && snapshot.complete
        && snapshot.unowned_matches.is_empty()
        && !snapshot.processes.is_empty()
        && snapshot.processes.iter().any(|p| {
            crate::team_process::identity_from_owner(inc.pid, inc.start_time)
                .is_ok_and(|id| p.identity == id)
        })
        && inc.processes.iter().all(|p| {
            snapshot.processes.iter().any(|q| {
                crate::team_process::identity_from_owner(p.pid, p.start_time)
                    .is_ok_and(|id| q.identity == id)
            })
        })
        && snapshot
            .processes
            .iter()
            .all(|p| state(&p.identity) == ProcessState::Gone)
}
/// Absence is accepted only below an existing, validated private root. Missing
/// OwnerRecord/snapshot never uses this helper. Symlinks/errors are not absence.
fn absent_beneath(root: &Path, relative: &str) -> Result<bool, ArchiveBlocker> {
    use std::os::unix::fs::MetadataExt;
    let parts: Vec<_> = relative.split('/').collect();
    if parts.is_empty()
        || parts
            .iter()
            .any(|p| p.is_empty() || *p == "." || *p == "..")
    {
        return Err(blocker("E_ARCHIVE_NEVER_STARTED_UNVERIFIED", "archive"));
    }
    let check_dir = |p: &Path| -> Result<(), ArchiveBlocker> {
        let m = std::fs::symlink_metadata(p)
            .map_err(|_| blocker("E_ARCHIVE_NEVER_STARTED_UNVERIFIED", "archive"))?;
        if !m.is_dir()
            || m.file_type().is_symlink()
            || m.uid() != unsafe { libc::geteuid() }
            || m.mode() & 0o077 != 0
        {
            return Err(blocker("E_ARCHIVE_NEVER_STARTED_UNVERIFIED", "archive"));
        }
        Ok(())
    };
    // Caller has just read an existing private owner/team anchor via shared
    // fd-bound no-follow IO. Recheck this root and each descendant below.
    check_dir(root)?;
    let mut current = root.to_path_buf();
    for (i, part) in parts.iter().enumerate() {
        current.push(part);
        match std::fs::symlink_metadata(&current) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(true),
            Err(_) => return Err(blocker("E_ARCHIVE_NEVER_STARTED_UNVERIFIED", "archive")),
            Ok(_) if i + 1 == parts.len() => return Ok(false),
            Ok(_) => check_dir(&current)?,
        }
    }
    Ok(false)
}
fn initial_matches(snapshot: &TeamSnapshot, owner: &OwnerRecord) -> bool {
    let seats: Vec<_> = snapshot
        .seats
        .iter()
        .filter(|s| s.name == owner.seat)
        .collect();
    snapshot.schema_version == 1
        && owner.schema_version == 1
        && owner.generation == 0
        && owner.state == OwnerState::Stale
        && owner.reservation_nonce_sha256.is_none()
        && owner.provisional_token_id.is_none()
        && owner.incarnation.is_none()
        && owner.writer == "glados"
        && chrono::DateTime::parse_from_rfc3339(&owner.since).is_ok()
        && seats.len() == 1
        && owner.requested.harness == seats[0].harness
        && owner.requested.model == seats[0].model
        && owner.requested.reasoning == seats[0].reasoning
}
/// Called under actual native team->seat locks, including by the finalizer.
/// AdvisoryLock cannot be supplied/deserialized by a control request.
pub(crate) fn verify_never_started_locked(
    home: &Path,
    snapshot: &TeamSnapshot,
    state: &TeamStateFile,
    expected: &OwnerRecord,
    _team_lock: &crate::owner::AdvisoryLock,
    _seat_lock: &crate::owner::AdvisoryLock,
) -> Result<(), ArchiveBlocker> {
    let fail = || blocker("E_ARCHIVE_NEVER_STARTED_UNVERIFIED", &expected.seat);
    if !crate::agent_loader::is_valid_seat_name(&snapshot.team)
        || snapshot.team.len() > 16
        || !crate::agent_loader::is_valid_seat_name(&expected.seat)
    {
        return Err(fail());
    }
    match crate::teams::classify_managed_seat(home, &expected.seat) {
        Ok(Some(crate::teams::ManagedSeatState::Active { team, generation }))
            if team == snapshot.team && generation == state.generation => {}
        _ => return Err(fail()),
    }
    let root = home.join(".aperture/teams").join(&snapshot.team);
    let current: TeamSnapshot = read_private_json(&root.join("team.json")).map_err(|_| fail())?;
    let current_state: TeamStateFile =
        read_private_json(&root.join("state.json")).map_err(|_| fail())?;
    // Seat lock is already held: do not recursively acquire OwnerStore::lock.
    let owner: OwnerRecord = read_private_json(
        &home
            .join(".aperture/run/owner")
            .join(format!("{}.json", expected.seat)),
    )
    .map_err(|_| fail())?;
    if current != *snapshot
        || current_state != *state
        || state.schema_version != 1
        || state.generation == 0
        || state.state != TeamLifecycle::Active
        || owner != *expected
        || !initial_matches(snapshot, &owner)
    {
        return Err(fail());
    }
    for relative in [
        format!("runtime-attempts/{}", owner.seat),
        format!("checkpoints/{}", owner.seat),
    ] {
        if !absent_beneath(&root, &relative)? {
            return Err(fail());
        }
    }
    let run = home.join(".aperture/run");
    for relative in [
        format!("managed/{}", owner.seat),
        format!("hub-tokens/{}.token", owner.seat),
        format!("revocations/{}.json", owner.seat),
        format!("{}.sock", owner.seat),
    ] {
        if !absent_beneath(&run, &relative)? {
            return Err(fail());
        }
    }
    let prefix = format!("{}.g", owner.seat);
    let mut count = 0;
    for entry in std::fs::read_dir(&run).map_err(|_| fail())? {
        count += 1;
        if count > 4096 {
            return Err(fail());
        }
        let entry = entry.map_err(|_| fail())?;
        let name = entry.file_name();
        let name = name.to_str().ok_or_else(fail)?;
        if name.starts_with(&prefix) {
            return Err(fail());
        }
    }
    Ok(())
}
fn inspect_never_started(
    home: &Path,
    snapshot: &TeamSnapshot,
    state: &TeamStateFile,
    owner: &OwnerRecord,
) -> Result<(), ArchiveBlocker> {
    let team = crate::owner::try_lock(&home.join(".aperture/run/team-locks"), &snapshot.team)
        .map_err(|_| blocker("E_ARCHIVE_LOCKED", &snapshot.team))?;
    let seat = OwnerStore::new(home.join(".aperture/run/owner"))
        .lock(&owner.seat)
        .map_err(|_| blocker("E_ARCHIVE_LOCKED", &owner.seat))?;
    verify_never_started_locked(home, snapshot, state, owner, &team, &seat)
}
/// The existing prepare seam leaves stopped/revoked owners Active until final
/// CAS. We report that truth. The journal owner must revalidate and perform its
/// own final stale transition; a checklist cannot claim that it already did.
pub(crate) fn inspect_native(
    home: &Path,
    team: &str,
    generation: u64,
    sentinels: &[String],
) -> Result<NativeSeatInspection, ArchiveBlocker> {
    inspect_native_inner(home, team, generation, sentinels, None)
}

pub(crate) fn inspect_native_locked(
    home: &Path,
    team: &str,
    generation: u64,
    sentinels: &[String],
    team_lock: &crate::owner::AdvisoryLock,
    seat_locks: &[crate::owner::AdvisoryLock],
) -> Result<NativeSeatInspection, ArchiveBlocker> {
    inspect_native_inner(
        home,
        team,
        generation,
        sentinels,
        Some((team_lock, seat_locks)),
    )
}

fn inspect_native_inner(
    home: &Path,
    team: &str,
    generation: u64,
    sentinels: &[String],
    held: Option<(&crate::owner::AdvisoryLock, &[crate::owner::AdvisoryLock])>,
) -> Result<NativeSeatInspection, ArchiveBlocker> {
    if !crate::agent_loader::is_valid_seat_name(team) || team.len() > 16 || generation == 0 {
        return Err(blocker("E_ARCHIVE_BINDING", "archive"));
    }
    let until = Instant::now() + Duration::from_secs(20);
    let root = home.join(".aperture/teams").join(team);
    let snapshot: TeamSnapshot = read_private_json(&root.join("team.json"))
        .map_err(|_| blocker("E_ARCHIVE_BINDING", team))?;
    let state: TeamStateFile = read_private_json(&root.join("state.json"))
        .map_err(|_| blocker("E_ARCHIVE_BINDING", team))?;
    if snapshot.schema_version != 1
        || state.schema_version != 1
        || snapshot.team != team
        || snapshot.seats.is_empty()
        || snapshot.seats.len() > 32
        || state.state != TeamLifecycle::Active
        || state.generation != generation
    {
        return Err(blocker("E_ARCHIVE_BINDING", team));
    }
    let mut names: Vec<_> = snapshot.seats.iter().map(|s| s.name.clone()).collect();
    names.sort();
    if names.windows(2).any(|v| v[0] == v[1])
        || !names.contains(&snapshot.lead)
        || names
            .iter()
            .any(|s| !crate::agent_loader::is_valid_seat_name(s))
    {
        return Err(blocker("E_ARCHIVE_BINDING", team));
    }
    let repo = repository::resolve_native(home, team, until)
        .map_err(|_| blocker("E_REPO_BINDING_UNAVAILABLE", team))?;
    if held.is_some_and(|(_, locks)| locks.len() != names.len()) {
        return Err(blocker("E_ARCHIVE_LOCKED", team));
    }
    let before: Vec<_> = names
        .iter()
        .map(|n| {
            if held.is_some() {
                read_private_json(&home.join(".aperture/run/owner").join(format!("{n}.json")))
                    .map_err(|_| blocker("E_ARCHIVE_OWNER_INVALID", n))
            } else {
                read_owner(home, n)
            }
        })
        .collect::<Result<_, _>>()?;
    let lead = before
        .iter()
        .find(|r| r.seat == snapshot.lead)
        .ok_or_else(|| blocker("E_ARCHIVE_OWNER_INVALID", team))?;
    let mut views = vec![];
    for (owner_index, owner) in before.iter().enumerate() {
        check_time(until)?;
        let mut view = NativeSeatView {
            seat: owner.seat.clone(),
            generation: owner.generation,
            never_started: false,
            owner_sha256: hash(owner)?,
            exact_processes_gone: false,
            process_evidence_sha256: None,
            revocation_metadata_verified: false,
            remote_reconciled: false,
            remote_complete_observation: false,
            remote_source: None,
            remote_inventory_sha256: None,
            remote_evidence_sha256: None,
            checkpoint_evidence_sha256: None,
            worktree_observation_sha256: None,
            bound_worktree_count: 0,
            worktrees_clean: false,
            owner_stale_verified: owner.state == OwnerState::Stale,
            blockers: vec![],
        };
        if owner.generation == 0 {
            let never_started = match held {
                Some((team_lock, seat_locks)) => verify_never_started_locked(
                    home,
                    &snapshot,
                    &state,
                    owner,
                    team_lock,
                    &seat_locks[owner_index],
                ),
                None => inspect_never_started(home, &snapshot, &state, owner),
            };
            match never_started {
                Ok(()) => {
                    view.never_started = true;
                }
                Err(e) => view.blockers.push(e),
            }
            // No invented process observation, revocation, Git-clean or zero
            // external-effects facts. Finalizer uses this explicit lifecycle
            // category and still requires all BEADS reconciliation/reviews.
            views.push(view);
            continue;
        }
        if !admitted_owner(owner, &owner.seat) {
            view.blockers
                .push(blocker("E_ARCHIVE_OWNER_UNSUPPORTED", &owner.seat));
            views.push(view);
            continue;
        }
        let process_result = if held.is_some() {
            crate::team_process::native::collect_native_until_locked(owner, until)
        } else {
            crate::team_process::native::collect_native_until(
                home,
                team,
                &owner.seat,
                owner.generation,
                until,
            )
        };
        match process_result {
            Ok(processes) => {
                view.exact_processes_gone =
                    processes_gone(owner, &processes, crate::team_process::state);
                view.process_evidence_sha256 = Some(hash(&processes)?);
                if view.exact_processes_gone {
                    view.revocation_metadata_verified =
                        crate::team_replacement::native::revoked_metadata(home, &processes)
                            .unwrap_or(false);
                }
            }
            Err(_) => {}
        }
        if !view.exact_processes_gone {
            view.blockers
                .push(blocker("E_STOP_UNVERIFIED", &owner.seat));
        }
        if !view.revocation_metadata_verified {
            view.blockers
                .push(blocker("E_REVOCATION_UNVERIFIED", &owner.seat));
        }
        check_time(until)?;
        let remote_target = remote::RemoteTarget {
            team: team.into(),
            seat: owner.seat.clone(),
            expected_generation: owner.generation,
        };
        let remote_result = match held {
            Some((team_lock, seat_locks)) => remote::project_native_locked(
                home,
                &remote_target,
                sentinels,
                team_lock,
                seat_locks,
            ),
            None => remote::project_native(home, &remote_target, sentinels),
        };
        match remote_result {
            Ok(p) => {
                view.remote_reconciled = p.may_proceed;
                view.remote_complete_observation = p.inventory.complete_observation;
                view.remote_source = p.source;
                view.remote_inventory_sha256 = Some(p.inventory.inventory_hash);
                view.remote_evidence_sha256 = Some(p.evidence_sha256);
            }
            Err(_) => {}
        }
        if !view.remote_reconciled {
            view.blockers
                .push(blocker("E_REMOTE_UNCERTAIN", &owner.seat));
        }
        // Inspect every historically authenticated worktree in each retained
        // generation. A pending/absent binding is not permission to inspect cwd
        // from payload/process or fall back to the repository root.
        let mut bound = std::collections::BTreeMap::new();
        let mut referenced = std::collections::BTreeSet::new();
        let mut checkpoint_evidence = Vec::new();
        let mut valid = owner.generation <= 128 && admitted_owner(lead, &snapshot.lead);
        if valid {
            for g in 1..=owner.generation {
                check_time(until)?;
                let ctx = crate::team_checkpoint::native::validation::ValidationContext {
                    team: team.into(),
                    seat: owner.seat.clone(),
                    generation: g,
                    lead_seat: snapshot.lead.clone(),
                    lead_generation: lead.generation,
                };
                let evidence_result = match held {
                    Some((team_lock, seat_locks)) => crate::team_checkpoint::native::validation::checkpoint_evidence_native_locked(
                        home, &ctx, sentinels, team_lock, seat_locks,
                    ),
                    None => crate::team_checkpoint::native::validation::checkpoint_evidence_native(
                        home, &ctx, sentinels, || Ok(()),
                    ),
                };
                match evidence_result {
                    Ok(evidence) => {
                        checkpoint_evidence.push((g, evidence.sha256));
                        for entry in evidence.validated_entries {
                            referenced.insert(entry.payload.worktree);
                        }
                        for e in evidence.historical_bindings {
                            bound.insert(e.payload.worktree.clone(), e);
                        }
                    }
                    Err(_) => {
                        valid = false;
                        break;
                    }
                }
                if bound.len() > 256 {
                    valid = false;
                    break;
                }
            }
        }
        view.bound_worktree_count = bound.len();
        if valid {
            view.checkpoint_evidence_sha256 = Some(hash(&(
                "aperture.archive.checkpoint-evidence.v1",
                &checkpoint_evidence,
            ))?);
        }
        view.worktrees_clean =
            valid && bound_coverage(&referenced, &bound.keys().cloned().collect());
        let mut worktree_observations = Vec::new();
        for entry in bound.values() {
            check_time(until)?;
            match repository::collect_native(&repo, entry, until) {
                Ok(actual) if actual.dirty_files.is_empty() => {
                    worktree_observations.push((entry.payload.worktree.clone(), actual));
                }
                _ => {
                    view.worktrees_clean = false;
                    break;
                }
            }
        }
        if view.worktrees_clean {
            view.worktree_observation_sha256 = Some(hash(&worktree_observations)?);
        }
        if !view.worktrees_clean {
            view.blockers
                .push(blocker("E_WORKTREE_UNPROTECTED", &owner.seat));
        }
        views.push(view);
    }
    check_time(until)?;
    for owner in &before {
        let current = if held.is_some() {
            read_private_json(
                &home
                    .join(".aperture/run/owner")
                    .join(format!("{}.json", owner.seat)),
            )
            .map_err(|_| blocker("E_ARCHIVE_OWNER_INVALID", &owner.seat))?
        } else {
            read_owner(home, &owner.seat)?
        };
        if current != *owner {
            return Err(blocker("E_ARCHIVE_NATIVE_DRIFT", &owner.seat));
        }
    }
    let current: TeamSnapshot = read_private_json(&root.join("team.json"))
        .map_err(|_| blocker("E_ARCHIVE_BINDING", team))?;
    let current_state: TeamStateFile = read_private_json(&root.join("state.json"))
        .map_err(|_| blocker("E_ARCHIVE_BINDING", team))?;
    if current != snapshot || current_state != state {
        return Err(blocker("E_ARCHIVE_NATIVE_DRIFT", team));
    }
    let sha256 = hash(&(&snapshot, &state, &views))?;
    Ok(NativeSeatInspection {
        seats: views,
        sha256,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::team_replacement::{OwnedProcess, OwnershipSnapshot, ProcessIdentity};
    fn owner() -> OwnerRecord {
        serde_json::from_value(serde_json::json!({
            "schema_version":1,"seat":"t1-worker","generation":1,"state":"active",
            "requested":{"harness":"codex","model":"gpt-6-astra","reasoning":"high"},
            "incarnation":{"pid":900001,"start_time":42,"thread_id":"synthetic-private-thread","token_id":"a".repeat(64),
                "harness":"codex","model":"gpt-6-astra","reasoning":"high","observed":true,
                "processes":[{"pid":900001,"start_time":42,"ppid":1,"pgid":900001,"cmdline_sha256":"b".repeat(64),"cwd":"private"},
                    {"pid":900002,"start_time":43,"ppid":1,"pgid":900001,"cmdline_sha256":"b".repeat(64),"cwd":"private"}]},
            "reservation_nonce_sha256":null,"provisional_token_id":null,
            "since":"2026-09-20T00:00:00Z","writer":"launcher"})).unwrap()
    }
    fn snapshot() -> OwnershipSnapshot {
        OwnershipSnapshot {
            seat: "t1-worker".into(),
            generation: 1,
            thread_id: "synthetic-private-thread".into(),
            complete: true,
            unowned_matches: vec![],
            processes: [(900001, "0.000042"), (900002, "0.000043")]
                .iter()
                .map(|(pid, birth)| OwnedProcess {
                    identity: ProcessIdentity {
                        pid: *pid,
                        start_time: birth.to_string(),
                    },
                    parent_pid: 1,
                    process_group: 900001,
                    depth: 1,
                    cmdline_sha256: "b".repeat(64),
                    cwd: "private".into(),
                })
                .collect(),
        }
    }
    #[test]
    fn process_proof_requires_owner_union_exact_birth_and_fresh_absence() {
        assert!(processes_gone(&owner(), &snapshot(), |_| {
            ProcessState::Gone
        }));
        for state in [
            ProcessState::Same,
            ProcessState::Recycled,
            ProcessState::Unreadable,
        ] {
            assert!(!processes_gone(&owner(), &snapshot(), |_| state));
        }
        for mode in 0..6 {
            let mut p = snapshot();
            match mode {
                0 => p.generation = 2,
                1 => p.thread_id = "different".into(),
                2 => {
                    p.processes.pop();
                }
                3 => p.processes[0].identity.start_time = "99".into(),
                4 => p.complete = false,
                _ => p.unowned_matches.push(p.processes[0].identity.clone()),
            }
            assert!(!processes_gone(&owner(), &p, |_| ProcessState::Gone));
        }
    }
    #[test]
    fn starting_stale_quarantined_g0_and_unobserved_are_not_fabricated_active_evidence() {
        assert!(admitted_owner(&owner(), "t1-worker"));
        for state in [
            OwnerState::Starting,
            OwnerState::Stale,
            OwnerState::Quarantined,
        ] {
            let mut o = owner();
            o.state = state;
            assert!(!admitted_owner(&o, "t1-worker"));
        }
        let mut o = owner();
        o.generation = 0;
        assert!(!admitted_owner(&o, "t1-worker"));
        let mut o = owner();
        o.incarnation.as_mut().unwrap().observed = false;
        assert!(!admitted_owner(&o, "t1-worker"));
        let mut o = owner();
        o.incarnation.as_mut().unwrap().model = "other".into();
        assert!(!admitted_owner(&o, "t1-worker"));
    }
    #[test]
    fn pending_unbound_worktree_and_absence_do_not_become_clean_empty_inventory() {
        let empty = std::collections::BTreeSet::new();
        let bound = std::collections::BTreeSet::from(["task-a".into()]);
        let requested = std::collections::BTreeSet::from(["task-a".into(), "untrusted-b".into()]);
        assert!(!bound_coverage(&empty, &empty));
        assert!(!bound_coverage(&requested, &bound));
        assert!(bound_coverage(&bound, &bound));
    }
    #[test]
    fn invalid_selector_and_absent_private_binding_stop_without_subprocess() {
        for (team, g) in [("../outside", 1), ("t1", 0)] {
            assert!(
                matches!(inspect_native(Path::new("/nonexistent-synthetic-home"),team,g,&[]),Err(e) if e.code=="E_ARCHIVE_BINDING")
            );
        }
        assert!(
            matches!(inspect_native(Path::new("/nonexistent-synthetic-home"),"t1",1,&[]),Err(e) if e.code=="E_ARCHIVE_BINDING")
        );
        assert!(check_time(Instant::now() - Duration::from_secs(1)).is_err());
    }
}

#[cfg(test)]
mod never_started_tests {
    use super::*;
    use crate::journal::{ensure_private_dir, write_private_json_atomic};
    use crate::state::{ExecutionTuple, Harness, ReasoningEffort};
    use std::os::unix::fs::{symlink, PermissionsExt};
    struct Fixture {
        home: std::path::PathBuf,
        snapshot: TeamSnapshot,
        state: TeamStateFile,
        owner: OwnerRecord,
    }
    impl Fixture {
        fn new() -> Self {
            let home =
                std::env::temp_dir().join(format!("aperture-archive-g0-{}", uuid::Uuid::new_v4()));
            ensure_private_dir(&home).unwrap();
            let snapshot:TeamSnapshot=serde_json::from_value(serde_json::json!({
                "schema_version":1,"team":"t1","project":"project:aperture","repo":"aperture","mission":"fixture","acceptance":"fixture",
                "preset":{"id":null,"sha256":null},"lead":"t1-worker","seats":[{"name":"t1-worker","role":"backend","harness":"codex","model":"gpt-6-astra","reasoning":"high"}],
                "fallbacks":[],"grants":[],"created_at":"2026-09-20T00:00:00Z","creation_request_id":uuid::Uuid::new_v4().to_string(),"staging_uuid":uuid::Uuid::new_v4().to_string()})).unwrap();
            let state = TeamStateFile {
                schema_version: 1,
                state: TeamLifecycle::Active,
                generation: 1,
                epic_id: Some("aperture-epic".into()),
                failure: None,
                updated_at: "2026-09-20T00:00:00Z".into(),
            };
            let owner:OwnerRecord=serde_json::from_value(serde_json::json!({"schema_version":1,"seat":"t1-worker","generation":0,"state":"stale",
                "reservation_nonce_sha256":null,"provisional_token_id":null,"incarnation":null,"writer":"glados","since":"2026-09-20T00:00:00Z",
                "requested":{"harness":"codex","model":"gpt-6-astra","reasoning":"high"}})).unwrap();
            let f = Self {
                home,
                snapshot,
                state,
                owner,
            };
            f.write(".aperture/teams/t1/team.json", &f.snapshot);
            f.write(".aperture/teams/t1/state.json", &f.state);
            f.write(".aperture/run/owner/t1-worker.json", &f.owner);
            for marker in ["TEAM", ".complete"] {
                f.write(
                    &format!(".claude/aperture/t1-worker/{marker}"),
                    &serde_json::json!({}),
                );
            }
            f
        }
        fn write<T: Serialize>(&self, relative: &str, value: &T) {
            let p = self.home.join(relative);
            ensure_private_dir(p.parent().unwrap()).unwrap();
            write_private_json_atomic(&p, value, true).unwrap();
        }
        fn check(&self) -> Result<(), ArchiveBlocker> {
            inspect_never_started(&self.home, &self.snapshot, &self.state, &self.owner)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.home);
        }
    }
    #[test]
    fn exact_private_initial_state_can_be_archived_without_launching_it() {
        let f = Fixture::new();
        f.check().unwrap();
        assert!(!f.home.join(".aperture/run/managed").exists());
        assert_eq!(read_owner(&f.home, "t1-worker").unwrap(), f.owner);
        // No process/remote "zero effects" or Git-clean fact is constructed.
    }
    #[test]
    fn any_partial_attempt_token_cleanup_revoke_or_observation_blocks() {
        for path in [
            ".aperture/teams/t1/journal.json",
            ".aperture/teams/t1/runtime-attempts/t1-worker/g0/admitted.json",
            ".aperture/teams/t1/runtime-attempts/t1-worker/g0/terminal.json",
            ".aperture/teams/t1/checkpoints/t1-worker/1-1.json",
            ".aperture/run/hub-tokens/t1-worker.token",
            ".aperture/run/managed/t1-worker/g1/prepared.json",
            ".aperture/run/revocations/t1-worker.json",
            ".aperture/run/t1-worker.g1.managed-start-attempt.json",
            ".aperture/run/t1-worker.g1.managed-observation.json",
            ".aperture/run/t1-worker.sock",
        ] {
            let f = Fixture::new();
            f.write(path, &serde_json::json!({"synthetic":"partial"}));
            assert!(f.check().is_err(), "{path}");
        }
    }
    #[test]
    fn missing_corrupt_unsafe_owner_and_symlink_parents_are_not_absence() {
        for mode in 0..7 {
            let f = Fixture::new();
            let p = f.home.join(".aperture/run/owner/t1-worker.json");
            match mode {
                0 => std::fs::remove_file(&p).unwrap(),
                1 => std::fs::write(&p, b"not-json").unwrap(),
                2 => std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap(),
                3 => {
                    let root = f.home.join(".aperture/run/managed");
                    symlink("/nonexistent-fixture", root).unwrap();
                }
                5 => {
                    std::fs::hard_link(&p, f.home.join("owner-hardlink.json")).unwrap();
                }
                6 => {
                    std::fs::remove_file(f.home.join(".claude/aperture/t1-worker/.complete"))
                        .unwrap();
                }
                _ => {
                    ensure_private_dir(&f.home.join(".aperture/run/managed")).unwrap();
                    std::fs::set_permissions(
                        f.home.join(".aperture/run/managed"),
                        std::fs::Permissions::from_mode(0o755),
                    )
                    .unwrap();
                }
            }
            assert!(f.check().is_err());
        }
    }
    #[test]
    fn snapshot_and_reservation_drift_are_no_effect_denials() {
        let f = Fixture::new();
        let mut s = f.snapshot.clone();
        s.seats[0].model = "other".into();
        f.write(".aperture/teams/t1/team.json", &s);
        assert!(f.check().is_err());
        let f = Fixture::new();
        let mut o = f.owner.clone();
        o.reservation_nonce_sha256 = Some("a".repeat(64));
        f.write(".aperture/run/owner/t1-worker.json", &o);
        assert!(f.check().is_err());
        let f = Fixture::new();
        let mut o = f.owner.clone();
        o.provisional_token_id = Some("a".repeat(64));
        f.write(".aperture/run/owner/t1-worker.json", &o);
        assert!(f.check().is_err());
    }
    #[test]
    fn archive_lock_excludes_reservation_and_later_reservation_invalidates_proof() {
        let f = Fixture::new();
        let store = OwnerStore::new(f.home.join(".aperture/run/owner"));
        let team = crate::owner::try_lock(&f.home.join(".aperture/run/team-locks"), "t1").unwrap();
        let seat = store.lock("t1-worker").unwrap();
        verify_never_started_locked(&f.home, &f.snapshot, &f.state, &f.owner, &team, &seat)
            .unwrap();
        let tuple = ExecutionTuple {
            harness: Harness::Codex,
            model: "gpt-6-astra".into(),
            reasoning: Some(ReasoningEffort::High),
        };
        let contender = store.clone();
        let selected = tuple.clone();
        let raced = std::thread::spawn(move || {
            contender.reserve_start(
                &crate::team_auth::AuthenticatedActor::launcher(),
                "t1-worker",
                0,
                selected,
            )
        });
        assert!(raced.join().unwrap().is_err());
        drop(seat);
        drop(team);
        store
            .reserve_start(
                &crate::team_auth::AuthenticatedActor::launcher(),
                "t1-worker",
                0,
                tuple,
            )
            .unwrap();
        assert!(f.check().is_err());
        assert_eq!(store.read_owner("t1-worker").unwrap().generation, 1);
        // Actual journal-terminal versus reservation race belongs to finalizer;
        // this oracle proves the same owner lock and reread used by its seam.
    }
}
