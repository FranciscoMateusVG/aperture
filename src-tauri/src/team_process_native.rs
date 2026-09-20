//! macOS process collector. Native evidence only, no caller ownership proofs.
//! No process signals/spawns or provider calls occur in this collector.
use super::*;
use crate::owner::{OwnerRecord, OwnerStore};
use crate::state::OwnerState;
use crate::teams::{classify_managed_seat, ManagedSeatState};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::time::{Duration, Instant};
const MAX_PIDS: usize = 65536;
const MAX_OWNED: usize = 256;
const MAX_METADATA: usize = 4096;
const MAX_ARGS: usize = 1024 * 1024;
const MAX_ARGC: usize = 4096;

// Only the native implementation reaches this private test seam. Commands,
// environment and argv bytes never leave this module; only a digest and cwd do.
trait ProcessSource {
    fn table(&mut self) -> Result<Vec<ProcessMetadata>, ReplacementError>;
    fn details(&mut self, id: &ProcessIdentity) -> Result<(String, String), ReplacementError>;
    fn observe(&mut self, id: &ProcessIdentity) -> ProcessState;
    fn control(&mut self) -> Result<ProcessIdentity, ReplacementError>;
    fn deadline(&self) -> Result<(), ReplacementError>;
}
struct NativeSource {
    deadline: Instant,
}
impl NativeSource {
    fn new() -> Self {
        Self {
            deadline: Instant::now() + Duration::from_secs(10),
        }
    }
}
impl ProcessSource for NativeSource {
    fn table(&mut self) -> Result<Vec<ProcessMetadata>, ReplacementError> {
        self.deadline()?;
        native_table(self.deadline)
    }
    fn details(&mut self, id: &ProcessIdentity) -> Result<(String, String), ReplacementError> {
        self.deadline()?;
        native_details(id)
    }
    fn observe(&mut self, id: &ProcessIdentity) -> ProcessState {
        state(id)
    }
    fn control(&mut self) -> Result<ProcessIdentity, ReplacementError> {
        observe(std::process::id())?
            .filter(|p| p.uid == unsafe { libc::geteuid() })
            .map(|p| p.identity)
            .ok_or(ReplacementError::StopUnverified)
    }
    fn deadline(&self) -> Result<(), ReplacementError> {
        if Instant::now() > self.deadline {
            Err(ReplacementError::StopUnverified)
        } else {
            Ok(())
        }
    }
}
/// Internal read-only collector; caller still must obtain persist_for_stop's
/// private CAS/lock guard before any effect. It cannot mint signal authority.
pub(crate) fn collect_native(
    home: &Path,
    team: &str,
    seat: &str,
    generation: u64,
) -> Result<OwnershipSnapshot, ReplacementError> {
    if !crate::agent_loader::is_valid_seat_name(seat)
        || !crate::agent_loader::is_valid_seat_name(team)
        || team.len() > 16
        || generation == 0
    {
        return Err(ReplacementError::InvalidSnapshot);
    }
    match classify_managed_seat(home, seat).map_err(|_| ReplacementError::InvalidSnapshot)? {
        Some(ManagedSeatState::Active { team: actual, .. }) if actual == team => {}
        _ => return Err(ReplacementError::AuthorizationRequired),
    }
    let record = OwnerStore::new(home.join(".aperture/run/owner"))
        .read_owner(seat)
        .map_err(|_| ReplacementError::InvalidSnapshot)?;
    if record.seat != seat
        || record.schema_version != 1
        || record.generation != generation
        || !matches!(record.state, OwnerState::Active | OwnerState::Starting)
    {
        return Err(ReplacementError::GenerationMismatch);
    }
    collect(&record, &mut NativeSource::new())
}
fn seeded(record: &OwnerRecord) -> Result<(ProcessIdentity, Vec<OwnedProcess>), ReplacementError> {
    let inc = record
        .incarnation
        .as_ref()
        .ok_or(ReplacementError::InvalidSnapshot)?;
    if inc.processes.is_empty() || inc.processes.len() > MAX_OWNED {
        return Err(ReplacementError::InvalidSnapshot);
    }
    let root = identity_from_owner(inc.pid, inc.start_time)?;
    let mut pids = HashSet::new();
    let mut persisted = vec![];
    for p in &inc.processes {
        if !pids.insert(p.pid) {
            return Err(ReplacementError::StopUnverified);
        }
        persisted.push(OwnedProcess {
            identity: identity_from_owner(p.pid, p.start_time)?,
            parent_pid: p.ppid,
            process_group: p.pgid,
            depth: if p.pid == inc.pid { 0 } else { 1 },
            cmdline_sha256: p.cmdline_sha256.clone(),
            cwd: p.cwd.clone(),
        });
    }
    if !persisted.iter().any(|p| p.identity == root) {
        return Err(ReplacementError::InvalidSnapshot);
    }
    Ok((root, persisted))
}
fn validate_table(table: &[ProcessMetadata]) -> Result<(), ReplacementError> {
    let mut seen = HashSet::new();
    if table.len() > MAX_PIDS
        || table.iter().any(|p| {
            p.identity.pid <= 1
                || p.identity.pid > i32::MAX as u32
                || birth_micros(&p.identity).is_err()
                || !seen.insert(p.identity.pid)
        })
    {
        return Err(ReplacementError::StopUnverified);
    }
    Ok(())
}
fn collect<S: ProcessSource>(
    record: &OwnerRecord,
    source: &mut S,
) -> Result<OwnershipSnapshot, ReplacementError> {
    let (root, persisted) = seeded(record)?;
    let inc = record.incarnation.as_ref().unwrap();
    let control = source.control()?;
    source.deadline()?;
    let first = source.table()?;
    validate_table(&first)?;
    let mut snapshot = capture_owned(
        &record.seat,
        record.generation,
        &inc.thread_id,
        &root,
        &first,
        &persisted,
        &[],
        true,
    )?;
    if snapshot.processes.len() > MAX_OWNED {
        return Err(ReplacementError::StopUnverified);
    }
    if snapshot.processes.iter().any(|p| p.identity == control) {
        return Err(ReplacementError::UnownedProcess);
    }
    // Refresh currently-live metadata privately; preserve captured metadata of
    // gone/reparented descendants, never erase their identity from stop evidence.
    for p in &mut snapshot.processes {
        source.deadline()?;
        match source.observe(&p.identity) {
            ProcessState::Same => {
                let current = first
                    .iter()
                    .find(|m| m.identity == p.identity)
                    .ok_or(ReplacementError::StopUnverified)?;
                p.parent_pid = current.ppid;
                p.process_group = current.pgid;
                let (hash, cwd) = source.details(&p.identity)?;
                if source.observe(&p.identity) != ProcessState::Same {
                    return Err(ReplacementError::StopUnverified);
                }
                p.cmdline_sha256 = hash;
                p.cwd = cwd;
            }
            ProcessState::Gone => {}
            _ => return Err(ReplacementError::StopUnverified),
        }
    }
    refresh_depths(&mut snapshot, &root)?;
    snapshot = complete_metadata(snapshot, |_| Err(ReplacementError::StopUnverified))?;
    let owned: HashSet<_> = snapshot.processes.iter().map(|p| p.identity.pid).collect();
    let uid = unsafe { libc::geteuid() };
    let mut reads = 0;
    for other in first
        .iter()
        .filter(|p| p.uid == uid && !owned.contains(&p.identity.pid))
    {
        reads += 1;
        if reads > MAX_METADATA {
            return Err(ReplacementError::StopUnverified);
        }
        source.deadline()?;
        match source.observe(&other.identity) {
            ProcessState::Gone => continue,
            ProcessState::Same => {}
            _ => return Err(ReplacementError::StopUnverified),
        }
        let (hash, cwd) = source.details(&other.identity)?;
        match source.observe(&other.identity) {
            ProcessState::Gone => continue,
            ProcessState::Same => {}
            _ => return Err(ReplacementError::StopUnverified),
        }
        if snapshot
            .processes
            .iter()
            .any(|p| p.cmdline_sha256 == hash || p.cwd == cwd)
        {
            snapshot.unowned_matches.push(other.identity.clone());
        }
    }
    // Fresh topology check is not another collection attempt. Any newly seen
    // owned descendant/group member invalidates this snapshot before signals.
    let second = source.table()?;
    validate_table(&second)?;
    let after = capture_owned(
        &record.seat,
        record.generation,
        &inc.thread_id,
        &root,
        &second,
        &snapshot.processes,
        &[],
        true,
    )?;
    if after.processes.len() != snapshot.processes.len()
        || after.processes.iter().any(|p| {
            !snapshot
                .processes
                .iter()
                .any(|old| old.identity == p.identity)
        })
    {
        return Err(ReplacementError::StopUnverified);
    }
    // New same-user outsiders were not checked for cwd/hash overlap; never
    // claim this pass covered them. A bounded caller may report uncertainty.
    if second
        .iter()
        .any(|p| p.uid == uid && !first.iter().any(|old| old.identity == p.identity))
    {
        return Err(ReplacementError::StopUnverified);
    }
    for p in &snapshot.processes {
        match source.observe(&p.identity) {
            ProcessState::Same => {
                if !second.iter().any(|m| m.identity == p.identity) {
                    return Err(ReplacementError::StopUnverified);
                }
            }
            ProcessState::Gone => {}
            _ => return Err(ReplacementError::StopUnverified),
        }
    }
    if source.observe(&control) != ProcessState::Same {
        return Err(ReplacementError::StopUnverified);
    }
    source.deadline()?;
    Ok(snapshot)
}
fn refresh_depths(
    snapshot: &mut OwnershipSnapshot,
    root: &ProcessIdentity,
) -> Result<(), ReplacementError> {
    let parents: HashMap<_, _> = snapshot
        .processes
        .iter()
        .map(|p| (p.identity.pid, p.parent_pid))
        .collect();
    for p in &mut snapshot.processes {
        if p.identity == *root {
            p.depth = 0;
            continue;
        }
        let mut current = p.identity.pid;
        let mut depth = 0;
        let mut seen = HashSet::new();
        loop {
            if !seen.insert(current) || seen.len() > MAX_OWNED {
                return Err(ReplacementError::StopUnverified);
            }
            depth += 1;
            let parent = parents
                .get(&current)
                .ok_or(ReplacementError::StopUnverified)?;
            if *parent == root.pid || !parents.contains_key(parent) {
                break;
            }
            current = *parent;
        }
        p.depth = depth;
    }
    Ok(())
}
#[cfg(target_os = "macos")]
fn native_table(deadline: Instant) -> Result<Vec<ProcessMetadata>, ReplacementError> {
    // proc_listpids returns BYTES (unlike proc_listallpids's count wrapper).
    // PROC_ALL_PIDS=1 is pinned by the macOS SDK sys/proc_info.h.
    let mut pids = vec![0i32; MAX_PIDS + 1];
    let bytes = unsafe {
        libc::proc_listpids(
            1,
            0,
            pids.as_mut_ptr().cast(),
            (pids.len() * std::mem::size_of::<i32>()) as i32,
        )
    };
    if bytes <= 0 || bytes as usize % std::mem::size_of::<i32>() != 0 {
        return Err(ReplacementError::StopUnverified);
    }
    let count = bytes as usize / std::mem::size_of::<i32>();
    if count > MAX_PIDS {
        return Err(ReplacementError::StopUnverified);
    }
    let mut out = vec![];
    let mut seen = HashSet::new();
    for pid in pids.into_iter().take(count).filter(|p| *p > 1) {
        if Instant::now() > deadline || !seen.insert(pid) {
            return Err(ReplacementError::StopUnverified);
        }
        if let Some(p) = observe(pid as u32)? {
            out.push(p);
        }
    }
    Ok(out)
}
#[cfg(not(target_os = "macos"))]
fn native_table(_: Instant) -> Result<Vec<ProcessMetadata>, ReplacementError> {
    Err(ReplacementError::StopUnverified)
}

/// Private bounded buffer for KERN_PROCARGS2. That OS record also contains
/// environment bytes. They are NEVER hashed, interpreted, logged or returned;
/// best-effort wipe covers the full allocation even on a parse error.
struct PrivateArgs(Vec<u8>);
impl Drop for PrivateArgs {
    fn drop(&mut self) {
        self.0.fill(0);
    }
}
fn argv_digest(bytes: &[u8]) -> Result<String, ReplacementError> {
    if bytes.len() < 5 || bytes.len() > MAX_ARGS {
        return Err(ReplacementError::StopUnverified);
    }
    let argc = i32::from_ne_bytes(bytes[..4].try_into().unwrap());
    if argc <= 0 || argc as usize > MAX_ARGC {
        return Err(ReplacementError::StopUnverified);
    }
    let mut offset = 4;
    let executable_end = bytes[offset..]
        .iter()
        .position(|b| *b == 0)
        .ok_or(ReplacementError::StopUnverified)?
        + offset;
    if executable_end == offset {
        return Err(ReplacementError::StopUnverified);
    }
    offset = executable_end + 1;
    while offset < bytes.len() && bytes[offset] == 0 {
        offset += 1;
    }
    let mut hash = Sha256::new();
    hash.update((argc as u64).to_be_bytes());
    for _ in 0..argc {
        if offset >= bytes.len() {
            return Err(ReplacementError::StopUnverified);
        }
        let end = bytes[offset..]
            .iter()
            .position(|b| *b == 0)
            .ok_or(ReplacementError::StopUnverified)?
            + offset;
        hash.update(((end - offset) as u64).to_be_bytes());
        hash.update(&bytes[offset..end]);
        offset = end + 1;
    }
    Ok(format!("{:x}", hash.finalize()))
}
#[cfg(target_os = "macos")]
fn native_details(id: &ProcessIdentity) -> Result<(String, String), ReplacementError> {
    if state(id) != ProcessState::Same {
        return Err(ReplacementError::StopUnverified);
    }
    let mut vnode = std::mem::MaybeUninit::<libc::proc_vnodepathinfo>::zeroed();
    let size = std::mem::size_of::<libc::proc_vnodepathinfo>();
    let got = unsafe {
        libc::proc_pidinfo(
            id.pid as i32,
            libc::PROC_PIDVNODEPATHINFO,
            0,
            vnode.as_mut_ptr().cast(),
            size as i32,
        )
    };
    if got != size as i32 {
        return Err(ReplacementError::StopUnverified);
    }
    let vnode = unsafe { vnode.assume_init() };
    let raw: Vec<u8> = vnode
        .pvi_cdir
        .vip_path
        .iter()
        .flatten()
        .map(|c| *c as u8)
        .collect();
    let end = raw
        .iter()
        .position(|b| *b == 0)
        .ok_or(ReplacementError::StopUnverified)?;
    let cwd =
        String::from_utf8(raw[..end].to_vec()).map_err(|_| ReplacementError::StopUnverified)?;
    if !cwd.starts_with('/') || cwd.chars().any(char::is_control) {
        return Err(ReplacementError::StopUnverified);
    }
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, id.pid as i32];
    let mut args = PrivateArgs(vec![0; MAX_ARGS]);
    let mut len = args.0.len();
    let rc = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            args.0.as_mut_ptr().cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc != 0 || len == 0 || len >= MAX_ARGS {
        return Err(ReplacementError::StopUnverified);
    }
    let hash = argv_digest(&args.0[..len])?;
    if state(id) != ProcessState::Same {
        return Err(ReplacementError::StopUnverified);
    }
    Ok((hash, cwd))
}
#[cfg(not(target_os = "macos"))]
fn native_details(_: &ProcessIdentity) -> Result<(String, String), ReplacementError> {
    Err(ReplacementError::StopUnverified)
}

#[cfg(test)]
#[path = "team_process_native_tests.rs"]
mod tests;
