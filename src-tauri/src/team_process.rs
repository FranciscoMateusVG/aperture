//! Native process identity adapter for the existing macOS launcher. No name,
//! cwd or command match authorizes a signal. Unsupported/unreadable OS state
//! fails closed; no ps elapsed-time approximation is used as a birth identity.
use crate::team_replacement::{
    OwnedProcess, OwnershipSnapshot, ProcessIdentity, ProcessState, ReplacementError, Signal,
};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone)]
pub struct ProcessMetadata {
    pub identity: ProcessIdentity,
    pub ppid: u32,
    pub pgid: u32,
    pub uid: u32,
}

#[cfg(target_os = "macos")]
pub fn observe(pid: u32) -> Result<Option<ProcessMetadata>, ReplacementError> {
    if pid <= 1 || pid > i32::MAX as u32 {
        return Err(ReplacementError::StopUnverified);
    }
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let size = std::mem::size_of::<libc::proc_bsdinfo>();
    // SAFETY: pointer is correctly aligned/writable for exactly the supplied
    // struct size. We read it only after the kernel reports a complete struct.
    let count = unsafe {
        libc::proc_pidinfo(
            pid as i32,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size as i32,
        )
    };
    if count == 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
        return Ok(None);
    }
    if count != size as i32 {
        return Err(ReplacementError::StopUnverified);
    }
    let info = unsafe { info.assume_init() };
    if info.pbi_pid != pid || info.pbi_start_tvsec == 0 {
        return Err(ReplacementError::StopUnverified);
    }
    Ok(Some(ProcessMetadata {
        identity: ProcessIdentity {
            pid,
            start_time: format!("{}.{:06}", info.pbi_start_tvsec, info.pbi_start_tvusec),
        },
        ppid: info.pbi_ppid,
        pgid: info.pbi_pgid,
        uid: info.pbi_uid,
    }))
}
#[cfg(not(target_os = "macos"))]
pub fn observe(_pid: u32) -> Result<Option<ProcessMetadata>, ReplacementError> {
    Err(ReplacementError::StopUnverified)
}

pub fn state(expected: &ProcessIdentity) -> ProcessState {
    match observe(expected.pid) {
        Ok(None) => ProcessState::Gone,
        Ok(Some(p)) if p.identity == *expected && p.uid == unsafe { libc::geteuid() } => {
            ProcessState::Same
        }
        Ok(Some(_)) => ProcessState::Recycled,
        Err(_) => ProcessState::Unreadable,
    }
}
/// Caller must pass an identity already present in its durable owned snapshot.
/// Rechecking immediately before kill narrows, but cannot atomically eliminate,
/// the OS check-to-signal race; we do not claim kernel capability fencing.
pub(crate) fn signal_recorded(
    recorded: &PersistedProcessSnapshot,
    identity: &ProcessIdentity,
    signal: Signal,
) -> Result<(), ReplacementError> {
    let snapshot = &recorded.snapshot;
    if !snapshot.complete || !snapshot.processes.iter().any(|p| p.identity == *identity) {
        return Err(ReplacementError::UnownedProcess);
    }
    match state(identity) {
        ProcessState::Gone => return Ok(()),
        ProcessState::Same => {}
        _ => return Err(ReplacementError::StopUnverified),
    }
    let sig = match signal {
        Signal::Term => libc::SIGTERM,
        Signal::Kill => libc::SIGKILL,
    };
    if unsafe { libc::kill(identity.pid as i32, sig) } != 0 {
        if std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
            && state(identity) == ProcessState::Gone
        {
            return Ok(());
        }
        return Err(ReplacementError::StopUnverified);
    }
    Ok(())
}

/// Pure ownership closure over a complete, scoped process-table observation.
/// Persisted records survive parent death/reparenting. Cwd/cmdline matches
/// outside this closure are returned as blockers, never added to authority.
pub fn capture_owned(
    seat: &str,
    generation: u64,
    thread_id: &str,
    pane: &ProcessIdentity,
    table: &[ProcessMetadata],
    persisted: &[OwnedProcess],
    matching: &[ProcessIdentity],
    table_complete: bool,
) -> Result<OwnershipSnapshot, ReplacementError> {
    if !table_complete || table.len() > 65536 {
        return Err(ReplacementError::StopUnverified);
    }
    let mut by_pid = HashMap::new();
    for p in table {
        if by_pid.insert(p.identity.pid, p).is_some() {
            return Err(ReplacementError::StopUnverified);
        }
    }
    let mut owned: HashMap<u32, OwnedProcess> = HashMap::new();
    for p in persisted {
        if p.identity.pid <= 1 || owned.insert(p.identity.pid, p.clone()).is_some() {
            return Err(ReplacementError::InvalidSnapshot);
        }
        if let Some(actual) = by_pid.get(&p.identity.pid) {
            if actual.identity != p.identity {
                return Err(ReplacementError::StopUnverified);
            }
        }
    }
    let root = by_pid.get(&pane.pid);
    if let Some(root) = root {
        if root.identity != *pane {
            return Err(ReplacementError::StopUnverified);
        }
        owned.entry(pane.pid).or_insert(OwnedProcess {
            identity: pane.clone(),
            parent_pid: root.ppid,
            process_group: root.pgid,
            depth: 0,
            cmdline_sha256: String::new(),
            cwd: String::new(),
        });
        // The pane's process group is authority only when the pane is its group
        // leader. A shared parent/tmux group is never swept by PGID alone.
        if root.pgid == root.identity.pid {
            for p in table.iter().filter(|p| p.pgid == root.pgid) {
                owned.entry(p.identity.pid).or_insert(OwnedProcess {
                    identity: p.identity.clone(),
                    parent_pid: p.ppid,
                    process_group: p.pgid,
                    depth: 1,
                    cmdline_sha256: String::new(),
                    cwd: String::new(),
                });
            }
        }
    } else if persisted.is_empty() {
        return Err(ReplacementError::StopUnverified);
    }
    for _ in 0..table.len() {
        let additions: Vec<_> = table
            .iter()
            .filter(|p| !owned.contains_key(&p.identity.pid) && owned.contains_key(&p.ppid))
            .map(|p| (p.clone(), owned[&p.ppid].depth.saturating_add(1)))
            .collect();
        if additions.is_empty() {
            break;
        }
        for (p, depth) in additions {
            owned.insert(
                p.identity.pid,
                OwnedProcess {
                    identity: p.identity,
                    parent_pid: p.ppid,
                    process_group: p.pgid,
                    depth,
                    cmdline_sha256: String::new(),
                    cwd: String::new(),
                },
            );
        }
    }
    let uid = unsafe { libc::geteuid() };
    if owned
        .keys()
        .filter_map(|id| by_pid.get(id))
        .any(|p| p.uid != uid)
    {
        return Err(ReplacementError::StopUnverified);
    }
    let mut unowned = Vec::new();
    let mut seen = HashSet::new();
    for m in matching {
        if !owned.get(&m.pid).map(|p| p.identity == *m).unwrap_or(false) && seen.insert(m.pid) {
            unowned.push(m.clone())
        }
    }
    let mut processes: Vec<_> = owned.into_values().collect();
    processes.sort_by_key(|p| p.identity.pid);
    Ok(OwnershipSnapshot {
        seat: seat.into(),
        generation,
        thread_id: thread_id.into(),
        processes,
        complete: false,
        unowned_matches: unowned,
    })
}

/// Capture is intentionally incomplete until metadata is hydrated by the native
/// collector. Read metadata only for exact owned identities, hash command bytes
/// privately, and recheck birth identity around that read. This callback must
/// not use cwd/command similarity as evidence of ownership.
pub fn complete_metadata<F>(
    mut snapshot: OwnershipSnapshot,
    mut read: F,
) -> Result<OwnershipSnapshot, ReplacementError>
where
    F: FnMut(&ProcessIdentity) -> Result<(String, String), ReplacementError>,
{
    if snapshot.processes.is_empty() {
        return Err(ReplacementError::InvalidSnapshot);
    }
    for process in &mut snapshot.processes {
        if process.cmdline_sha256.is_empty() || process.cwd.is_empty() {
            let (hash, cwd) = read(&process.identity)?;
            process.cmdline_sha256 = hash;
            process.cwd = cwd;
        }
        if process.cmdline_sha256.len() != 64
            || !process
                .cmdline_sha256
                .bytes()
                .all(|b| b.is_ascii_hexdigit())
            || !process.cwd.starts_with('/')
            || process.cwd.chars().any(char::is_control)
        {
            return Err(ReplacementError::InvalidSnapshot);
        }
    }
    snapshot.complete = true;
    Ok(snapshot)
}

/// Lossless adapter to the shared OwnerRecord convention: checked Unix epoch
/// microseconds, identical for root and descendants. Never elapsed-time text.
pub(crate) fn birth_micros(identity: &ProcessIdentity) -> Result<u64, ReplacementError> {
    let (seconds, fraction) = identity
        .start_time
        .split_once('.')
        .ok_or(ReplacementError::InvalidSnapshot)?;
    if seconds.is_empty()
        || !seconds.bytes().all(|b| b.is_ascii_digit())
        || fraction.len() != 6
        || !fraction.bytes().all(|b| b.is_ascii_digit())
    {
        return Err(ReplacementError::InvalidSnapshot);
    }
    let seconds: u64 = seconds
        .parse()
        .map_err(|_| ReplacementError::InvalidSnapshot)?;
    let fraction: u64 = fraction
        .parse()
        .map_err(|_| ReplacementError::InvalidSnapshot)?;
    let micros = seconds
        .checked_mul(1_000_000)
        .and_then(|s| s.checked_add(fraction))
        .filter(|m| *m > 0)
        .ok_or(ReplacementError::InvalidSnapshot)?;
    if identity.start_time != format!("{}.{:06}", micros / 1_000_000, micros % 1_000_000) {
        return Err(ReplacementError::InvalidSnapshot);
    }
    Ok(micros)
}
pub(crate) fn identity_from_owner(
    pid: u32,
    micros: u64,
) -> Result<ProcessIdentity, ReplacementError> {
    if pid <= 1 || pid > i32::MAX as u32 || micros == 0 {
        return Err(ReplacementError::InvalidSnapshot);
    }
    Ok(ProcessIdentity {
        pid,
        start_time: format!("{}.{:06}", micros / 1_000_000, micros % 1_000_000),
    })
}

/// Private stop authority. Not Deserialize/Clone: only the persisted, owner-CAS
/// path can construct it. Both locks remain held through signals/verification.
/// Drop this guard before a subsequent owner transition re-acquires its lock.
pub(crate) struct PersistedProcessSnapshot {
    _seat_lock: crate::owner::AdvisoryLock,
    _team_lock: crate::owner::AdvisoryLock,
    snapshot: OwnershipSnapshot,
}
impl PersistedProcessSnapshot {
    pub(crate) fn snapshot(&self) -> &OwnershipSnapshot {
        &self.snapshot
    }
}

pub(crate) fn persist_for_stop(
    home: &std::path::Path,
    team: &str,
    actor: &crate::team_auth::AuthenticatedActor,
    snapshot: OwnershipSnapshot,
) -> Result<PersistedProcessSnapshot, ReplacementError> {
    persist_for_stop_checked(home, team, actor, snapshot, state)
}

fn persist_for_stop_checked<F>(
    home: &std::path::Path,
    team: &str,
    actor: &crate::team_auth::AuthenticatedActor,
    snapshot: OwnershipSnapshot,
    mut observe: F,
) -> Result<PersistedProcessSnapshot, ReplacementError>
where
    F: FnMut(&ProcessIdentity) -> ProcessState,
{
    use crate::owner::{try_lock, OwnerRecord, OwnerStore};
    use crate::state::OwnerState;
    use crate::teams::{classify_managed_seat, ManagedSeatState};
    if !actor.is_launcher() {
        return Err(ReplacementError::AuthorizationRequired);
    }
    if !crate::agent_loader::is_valid_seat_name(&snapshot.seat)
        || team.len() > 16
        || !crate::agent_loader::is_valid_seat_name(team)
        || snapshot.generation == 0
        || !snapshot.complete
        || snapshot.processes.is_empty()
        || snapshot.processes.len() > 256
    {
        return Err(ReplacementError::InvalidSnapshot);
    }
    if !snapshot.unowned_matches.is_empty() {
        return Err(ReplacementError::UnownedProcess);
    }
    let team_lock = try_lock(&home.join(".aperture/run/team-locks"), team)
        .map_err(|_| ReplacementError::NativeFailure)?;
    match classify_managed_seat(home, &snapshot.seat)
        .map_err(|_| ReplacementError::InvalidSnapshot)?
    {
        Some(ManagedSeatState::Active { team: actual, .. }) if actual == team => {}
        _ => return Err(ReplacementError::AuthorizationRequired),
    }
    let store = OwnerStore::new(home.join(".aperture/run/owner"));
    let before = store
        .read_owner(&snapshot.seat)
        .map_err(|_| ReplacementError::InvalidSnapshot)?;
    if before.generation != snapshot.generation {
        return Err(ReplacementError::GenerationMismatch);
    }
    if !matches!(before.state, OwnerState::Starting | OwnerState::Active) {
        return Err(ReplacementError::StopUnverified);
    }
    let incarnation = before
        .incarnation
        .as_ref()
        .ok_or(ReplacementError::InvalidSnapshot)?;
    if incarnation.thread_id != snapshot.thread_id {
        return Err(ReplacementError::InvalidSnapshot);
    }
    let root = identity_from_owner(incarnation.pid, incarnation.start_time)?;
    let mut pids = HashSet::new();
    let mut identities = Vec::new();
    for p in &snapshot.processes {
        if p.identity.pid <= 1
            || p.identity.pid > i32::MAX as u32
            || !pids.insert(p.identity.pid)
            || p.cmdline_sha256.len() != 64
            || !p
                .cmdline_sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || p.cwd.len() > 4096
            || !p.cwd.starts_with('/')
            || p.cwd.chars().any(char::is_control)
        {
            return Err(ReplacementError::InvalidSnapshot);
        }
        let micros = birth_micros(&p.identity)?;
        match observe(&p.identity) {
            ProcessState::Same | ProcessState::Gone => {}
            _ => return Err(ReplacementError::StopUnverified),
        }
        identities.push(crate::owner::ProcessIdentity {
            pid: p.identity.pid,
            start_time: micros,
            ppid: p.parent_pid,
            pgid: p.process_group,
            cmdline_sha256: p.cmdline_sha256.clone(),
            cwd: p.cwd.clone(),
        });
    }
    if !snapshot.processes.iter().any(|p| p.identity == root)
        || incarnation.processes.iter().any(|old| {
            !identities
                .iter()
                .any(|new| new.pid == old.pid && new.start_time == old.start_time)
        })
    {
        // Persisted orphaned descendants cannot be silently discarded merely
        // because they no longer appear under the pane's current parent tree.
        return Err(ReplacementError::InvalidSnapshot);
    }
    let written = store
        .record_process_snapshot(
            actor,
            &snapshot.seat,
            snapshot.generation,
            incarnation.pid,
            incarnation.start_time,
            identities,
        )
        .map_err(|_| ReplacementError::StopUnverified)?;
    // The shared owner seam may retain an identity added concurrently. Such a
    // union must not silently grant this adapter an incomplete stop set.
    let persisted = written
        .incarnation
        .as_ref()
        .ok_or(ReplacementError::StopUnverified)?;
    if persisted.processes.len() != snapshot.processes.len()
        || persisted.processes.iter().any(|stored| {
            !snapshot.processes.iter().any(|p| {
                p.identity.pid == stored.pid
                    && birth_micros(&p.identity).ok() == Some(stored.start_time)
            })
        })
    {
        return Err(ReplacementError::StopUnverified);
    }
    // The owner API performs its own lock/CAS. Reacquire and compare the exact
    // written record before returning signal authority; any intervening drift
    // is terminal, with the snapshot retained and zero signal from this path.
    let seat_lock = store
        .lock(&snapshot.seat)
        .map_err(|_| ReplacementError::StopUnverified)?;
    let actual: OwnerRecord = crate::journal::read_private_json(&store.record_path(&snapshot.seat))
        .map_err(|_| ReplacementError::StopUnverified)?;
    if actual != written {
        return Err(ReplacementError::StopUnverified);
    }
    Ok(PersistedProcessSnapshot {
        _seat_lock: seat_lock,
        _team_lock: team_lock,
        snapshot,
    })
}

#[cfg(test)]
#[path = "team_process_tests.rs"]
mod persisted_tests;
