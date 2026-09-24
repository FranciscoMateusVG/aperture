//! Diagnostic retirement is not mission completion. Only a fully bound, cleaned
//! native pre-input launch can bypass the mission/epic reconciliation collector.
//! No request can supply this category, a process proof, or an approval hash.
use crate::journal::{read_private_json, ArchiveCategory, ArchiveJournalApproval};
use crate::owner::{try_lock, AdvisoryLock, OwnerRecord, OwnerStore};
use crate::state::{ExecutionTuple, Harness, OwnerState};
use crate::team_claude_launch::{
    canonical_uuid, ClaudeAttempt, ClaudeLaunchMode, ClaudeLaunchPlan, MODEL,
};
use crate::team_replacement::{ProcessIdentity, ProcessState};
use crate::teams::{TeamLifecycle, TeamSnapshot, TeamStateFile};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn fail() -> String {
    "E_ARCHIVE_DIAGNOSTIC_UNVERIFIED: native diagnostic retirement evidence unavailable".into()
}
fn hash<T: Serialize>(value: &T) -> Result<String, String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(value).map_err(|_| fail())?)
    ))
}
fn read<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    read_private_json(path).map_err(|_| fail())
}
fn hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|v| v.is_ascii_digit() || (b'a'..=b'f').contains(&v))
}
fn diagnostic(mode: &ClaudeLaunchMode) -> bool {
    *mode == ClaudeLaunchMode::DiagnosticPreinput
}

// Frozen native schemas. Unknown fields fail closed; serializers retain the
// producer field order because release hashes bind these exact native records.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Launch {
    schema_version: u32,
    team: String,
    seat: String,
    generation: u64,
    session_id: String,
    nonce_sha256: String,
    token_id: String,
    snapshot_sha256: String,
    executable: PathBuf,
    helper: PathBuf,
    node: PathBuf,
    tmux: PathBuf,
    cwd: PathBuf,
    cwd_identity: (u64, u64),
    worktree: Option<String>,
    args: Vec<String>,
    pins: BTreeMap<PathBuf, String>,
    private_pins: BTreeMap<PathBuf, String>,
    #[serde(default, skip_serializing_if = "diagnostic")]
    mode: ClaudeLaunchMode,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Release {
    schema_version: u32,
    launch_sha256: String,
    attempt_sha256: String,
    root_pid: u32,
    root_start_time_us: u64,
}
#[derive(Serialize, Deserialize)]
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
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Fact {
    schema_version: u32,
    attempt_id: String,
    kind: String,
}
fn fact(f: &Fact, a: &Admission, kind: &str) -> bool {
    f.schema_version == 1 && f.attempt_id == a.attempt_id && f.kind == kind
}
fn optional_fact(path: &Path) -> Result<Option<Fact>, String> {
    match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Ok(_) => read(path).map(Some),
        Err(_) => Err(fail()),
    }
}

fn token_absent(home: &Path, seat: &str) -> Result<(), String> {
    // Parent run tree has already been fd-bound validated by owner/floor reads.
    // No read of token contents and no creation of missing roots.
    let dir = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY)
        .open(home.join(".aperture/run/hub-tokens"))
        .map_err(|_| fail())?;
    let m = dir.metadata().map_err(|_| fail())?;
    if !m.is_dir() || m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o077 != 0 {
        return Err(fail());
    }
    use std::os::fd::AsRawFd;
    let name = std::ffi::CString::new(format!("{seat}.token")).map_err(|_| fail())?;
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    let result = unsafe {
        libc::fstatat(
            dir.as_raw_fd(),
            name.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if result == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ENOENT) {
        Ok(())
    } else {
        Err(fail())
    }
}

pub(crate) fn inspect(
    home: &Path,
    snapshot: &TeamSnapshot,
    state: &TeamStateFile,
) -> Result<ArchiveJournalApproval, String> {
    let team_lock = try_lock(&home.join(".aperture/run/team-locks"), &snapshot.team)?;
    let owners = OwnerStore::new(home.join(".aperture/run/owner"));
    let mut names: Vec<_> = snapshot.seats.iter().map(|s| s.name.clone()).collect();
    names.sort();
    if names.is_empty() || names.len() > 32 || names.windows(2).any(|v| v[0] == v[1]) {
        return Err(fail());
    }
    let locks = names
        .iter()
        .map(|s| owners.lock(s))
        .collect::<Result<Vec<_>, _>>()?;
    inspect_locked(home, snapshot, state, &team_lock, &locks)
}

pub(crate) fn inspect_locked(
    home: &Path,
    snapshot: &TeamSnapshot,
    state: &TeamStateFile,
    team_lock: &AdvisoryLock,
    seat_locks: &[AdvisoryLock],
) -> Result<ArchiveJournalApproval, String> {
    inspect_with(
        home,
        snapshot,
        state,
        team_lock,
        seat_locks,
        crate::team_process::state,
    )
}

fn inspect_with<F: FnMut(&ProcessIdentity) -> ProcessState>(
    home: &Path,
    snapshot: &TeamSnapshot,
    state: &TeamStateFile,
    _team_lock: &AdvisoryLock,
    seat_locks: &[AdvisoryLock],
    mut process_state: F,
) -> Result<ArchiveJournalApproval, String> {
    let until = Instant::now() + Duration::from_secs(20);
    let team = &snapshot.team;
    if snapshot.schema_version != 1
        || state.schema_version != 1
        || team.len() > 16
        || !crate::agent_loader::is_valid_seat_name(team)
        || state.state != TeamLifecycle::Active
        || state.generation == 0
        || snapshot.seats.is_empty()
        || snapshot.seats.len() > 32
        || seat_locks.len() != snapshot.seats.len()
    {
        return Err(fail());
    }
    let root = home.join(".aperture/teams").join(team);
    // The observation producer binds the exact persisted bytes, while the
    // launch producer binds typed serialization. They are distinct contracts.
    let snapshot_file =
        crate::journal::open_private_file_nofollow(&root.join("team.json")).map_err(|_| fail())?;
    let mut snapshot_bytes = Vec::new();
    snapshot_file
        .take(1_048_577)
        .read_to_end(&mut snapshot_bytes)
        .map_err(|_| fail())?;
    if snapshot_bytes.len() > 1_048_576 {
        return Err(fail());
    }
    let persisted: TeamSnapshot = serde_json::from_slice(&snapshot_bytes).map_err(|_| fail())?;
    if persisted != *snapshot || read::<TeamStateFile>(&root.join("state.json"))? != *state {
        return Err(fail());
    }
    let snapshot_hash = hash(snapshot)?;
    let observation_snapshot_hash = format!("{:x}", Sha256::digest(&snapshot_bytes));
    let mut names: Vec<_> = snapshot.seats.iter().map(|s| s.name.clone()).collect();
    names.sort();
    if names.windows(2).any(|v| v[0] == v[1]) || !names.contains(&snapshot.lead) {
        return Err(fail());
    }
    let mut owners = Vec::new();
    let mut evidence = Vec::new();
    for seat in names {
        if Instant::now() >= until || !crate::agent_loader::is_valid_seat_name(&seat) {
            return Err(fail());
        }
        let o: OwnerRecord = read(
            &home
                .join(".aperture/run/owner")
                .join(format!("{seat}.json")),
        )?;
        let configured = snapshot
            .seats
            .iter()
            .find(|s| s.name == seat)
            .ok_or_else(fail)?;
        let tuple = ExecutionTuple {
            harness: Harness::Claude,
            model: MODEL.into(),
            reasoning: None,
        };
        if o.schema_version != 1
            || o.seat != seat
            || o.generation != 1
            || o.state != OwnerState::Quarantined
            || o.reservation_nonce_sha256.is_some()
            || o.provisional_token_id.is_some()
            || o.requested != tuple
            || configured.harness != tuple.harness
            || configured.model != tuple.model
            || configured.reasoning.is_some()
        {
            return Err(fail());
        }
        let i = o.incarnation.as_ref().ok_or_else(fail)?;
        let a: ClaudeAttempt = read(
            &home
                .join(".aperture/run")
                .join(format!("{seat}.g1.claude-attempt.json")),
        )?;
        let base = home.join(".aperture/run/managed").join(&seat).join("g1");
        let launch: Launch = read(&base.join("claude-launch.json"))?;
        let release: Release = read(&base.join("claude-release.json"))?;
        let dir = root.join("runtime-attempts").join(&seat).join("g0");
        let admitted: Admission = read(&dir.join("admitted.json"))?;
        let effects: Fact = read(&dir.join("effects.json"))?;
        let terminal = optional_fact(&dir.join("terminal.json"))?;
        let reconciled = optional_fact(&dir.join("reconciled.json"))?;
        if a.schema_version != 1
            || a.team != *team
            || a.seat != seat
            || a.generation != o.generation
            || a.team_generation != state.generation
            || a.snapshot_sha256 != observation_snapshot_hash
            || a.mode != ClaudeLaunchMode::DiagnosticPreinput
            || !canonical_uuid(&a.session_id)
            || !hex(&a.reservation_nonce_sha256)
            || !hex(&a.token_id)
            || a.requested_model != MODEL
            || a.root_pid != i.pid
            || a.root_start_time_us != i.start_time
            || a.token_id != i.token_id
            || i.harness != tuple.harness
            || i.model != tuple.model
            || i.reasoning.is_some()
            || (i.observed && i.thread_id != a.session_id)
            || (!i.observed && !i.thread_id.is_empty())
            || launch.schema_version != 1
            || launch.mode != ClaudeLaunchMode::DiagnosticPreinput
            || launch.team != a.team
            || launch.seat != a.seat
            || launch.generation != a.generation
            || launch.session_id != a.session_id
            || launch.nonce_sha256 != a.reservation_nonce_sha256
            || launch.token_id != a.token_id
            || launch.snapshot_sha256 != snapshot_hash
            || launch.helper != home.join(".aperture/bin/aperture-boot")
            || launch.worktree.is_some()
            || release.schema_version != 1
            || release.root_pid != i.pid
            || release.root_start_time_us != i.start_time
            || release.launch_sha256 != hash(&launch)?
            || release.attempt_sha256 != hash(&a)?
            || admitted.schema_version != 1
            || admitted.team != *team
            || admitted.seat != seat
            || admitted.old_generation != 0
            || !canonical_uuid(&admitted.attempt_id)
            || admitted.admitted_at_ms <= 0
            || a.created_at_ms < admitted.admitted_at_ms
            || admitted.native_budget_ms != 170_000
            || admitted.cleanup_reserve_ms != 40_000
            || !fact(&effects, &admitted, "effects_may_have_occurred")
        {
            return Err(fail());
        }
        // Rebuild the actual diagnostic argv: absent/default mode alone is not
        // enough if a positional mission or permission-bypass flag was inserted.
        let mut plan =
            ClaudeLaunchPlan::new(home, team, &seat, o.generation, &tuple, &launch.helper)
                .map_err(|_| fail())?;
        plan.argv[3] = a.session_id.clone();
        plan.argv.extend([
            "--append-system-prompt-file".into(),
            base.join("prompt.md").to_str().ok_or_else(fail)?.into(),
        ]);
        if launch.args != plan.argv {
            return Err(fail());
        }
        let cleaned = terminal
            .as_ref()
            .is_some_and(|f| fact(f, &admitted, "smoke_cleaned"))
            && reconciled.is_none();
        let recovered = reconciled
            .as_ref()
            .is_some_and(|f| fact(f, &admitted, "stopped_reconciled"))
            && terminal
                .as_ref()
                .is_none_or(|f| fact(f, &admitted, "unknown"));
        if !cleaned && !recovered {
            return Err(fail());
        }
        if i.processes.is_empty()
            || i.processes.len() > 1024
            || !i
                .processes
                .iter()
                .any(|p| p.pid == i.pid && p.start_time == i.start_time)
        {
            return Err(fail());
        }
        let mut identities = std::collections::BTreeSet::new();
        for p in &i.processes {
            if Instant::now() >= until || !identities.insert((p.pid, p.start_time)) {
                return Err(fail());
            }
            let id = crate::team_process::identity_from_owner(p.pid, p.start_time)
                .map_err(|_| fail())?;
            if process_state(&id) != ProcessState::Gone {
                return Err(fail());
            }
        }
        crate::ws_hub::managed_control::verify_floor(home, &seat, o.generation, &i.token_id)
            .map_err(|_| fail())?;
        token_absent(home, &seat)?;
        let floor: serde_json::Value = read(
            &home
                .join(".aperture/run/revocations")
                .join(format!("{seat}.json")),
        )?;
        owners.push((seat.clone(), hash(&o)?));
        evidence.push((
            seat,
            hash(&(
                a, launch, release, admitted, effects, terminal, reconciled, floor, &o,
            ))?,
        ));
    }
    Ok(ArchiveJournalApproval {
        category: ArchiveCategory::DiagnosticRetirement,
        generation: state.generation,
        epic_id: state
            .epic_id
            .clone()
            .filter(|v| !v.is_empty())
            .ok_or_else(fail)?,
        record_sha256: hash(&("diagnostic-facts-v1", &snapshot_hash, &evidence))?,
        inventory_sha256: hash(&("diagnostic-seats-v1", snapshot, state, &owners))?,
        native_sha256: hash(&("diagnostic-cleanup-v1", &evidence, &owners))?,
        owner_states: owners
            .iter()
            .map(|(s, _)| (s.clone(), "quarantined".into()))
            .collect(),
        owner_sha256: owners,
        owner_post_sha256: vec![],
        transition_at: String::new(),
        approved_by: "glados".into(),
    })
}

#[cfg(test)]
#[path = "team_archive_diagnostic_tests.rs"]
mod tests;
