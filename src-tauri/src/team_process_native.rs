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
    collect_native_until(
        home,
        team,
        seat,
        generation,
        Instant::now() + Duration::from_secs(10),
    )
}
pub(crate) fn collect_native_until(
    home: &Path,
    team: &str,
    seat: &str,
    generation: u64,
    until: Instant,
) -> Result<OwnershipSnapshot, ReplacementError> {
    if Instant::now() >= until {
        return Err(ReplacementError::Deadline);
    }
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
    collect(
        &record,
        &mut NativeSource {
            deadline: until.min(Instant::now() + Duration::from_secs(10)),
        },
    )
}

/// Read-only process observation when the archive finalizer already holds the
/// canonical team and seat locks. The caller supplies the exact locked owner
/// record, so this seam never reacquires a lock or rereads a different owner.
pub(crate) fn collect_native_until_locked(
    record: &OwnerRecord,
    until: Instant,
) -> Result<OwnershipSnapshot, ReplacementError> {
    if Instant::now() >= until
        || record.schema_version != 1
        || record.generation == 0
        || record.seat.is_empty()
        || !matches!(record.state, OwnerState::Active | OwnerState::Starting)
    {
        return Err(ReplacementError::InvalidSnapshot);
    }
    collect(
        record,
        &mut NativeSource {
            deadline: until.min(Instant::now() + Duration::from_secs(10)),
        },
    )
}
/// Metadata for the exact child held by the native pipe gate, before any
/// harness exec. A caller PID/path cannot construct PendingChild. This observes
/// no process table and grants no signal authority by itself.
pub(crate) fn capture_gated_child(
    child: &crate::team_replacement::launch_gate::PendingChild,
) -> Result<crate::owner::ProcessIdentity, ReplacementError> {
    let id = child.identity();
    let before = observe(id.pid)?.ok_or(ReplacementError::StopUnverified)?;
    if before.identity != *id
        || before.uid != unsafe { libc::geteuid() }
        || before.pgid != id.pid
        || before.ppid != std::process::id()
    {
        return Err(ReplacementError::StopUnverified);
    }
    let (cmdline_sha256, cwd) = native_details(id)?;
    let after = observe(id.pid)?.ok_or(ReplacementError::StopUnverified)?;
    if after.identity != before.identity
        || after.ppid != before.ppid
        || after.pgid != before.pgid
        || after.uid != before.uid
    {
        return Err(ReplacementError::StopUnverified);
    }
    Ok(crate::owner::ProcessIdentity {
        pid: id.pid,
        start_time: birth_micros(id)?,
        ppid: before.ppid,
        pgid: before.pgid,
        cmdline_sha256,
        cwd,
    })
}

/// Capture a natively obtained tmux gate identity without assuming its parent
/// is the GUI (tmux owns it). This grants no stop authority; the caller must
/// persist it in OwnerStore before release and never accept a caller PID.
pub(crate) fn capture_gated_identity(id: &ProcessIdentity) -> Result<crate::owner::ProcessIdentity, ReplacementError> {
    let before=observe(id.pid)?.ok_or(ReplacementError::StopUnverified)?;
    if before.identity!=*id || before.uid!=unsafe{libc::geteuid()} {return Err(ReplacementError::StopUnverified);}
    let (cmdline_sha256,cwd)=native_details(id)?;
    let after=observe(id.pid)?.ok_or(ReplacementError::StopUnverified)?;
    if after.identity!=before.identity||after.ppid!=before.ppid||after.pgid!=before.pgid||after.uid!=before.uid {return Err(ReplacementError::StopUnverified);}
    Ok(crate::owner::ProcessIdentity{pid:id.pid,start_time:birth_micros(id)?,ppid:before.ppid,pgid:before.pgid,cmdline_sha256,cwd})
}
#[cfg(all(test,target_os="macos"))]
#[test]
fn gated_identity_fixture_uses_native_details_and_rejects_wrong_birth() {
    let id=observe(std::process::id()).unwrap().unwrap().identity;
    let captured=capture_gated_identity(&id).unwrap();
    assert_eq!(captured.pid,id.pid);assert_eq!(captured.start_time,birth_micros(&id).unwrap());
    assert_eq!(captured.cmdline_sha256.len(),64);assert!(std::path::Path::new(&captured.cwd).is_absolute());
    let mut wrong=id;wrong.start_time="1.000001".into();assert!(capture_gated_identity(&wrong).is_err());
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
// Darwin LP64 kinfo_proc ABI from SDK sys/sysctl.h + sys/proc.h. These
// offsets are verified against the installed SDK by the C layout oracle in
// tests. Decode only metadata; never expose comm/login/kernel pointer fields.
// Unknown architectures fail closed instead of guessing another ABI.
#[cfg(all(target_os = "macos", target_pointer_width = "64", target_endian = "little",
    any(target_arch = "aarch64", target_arch = "x86_64")))]
mod process_table {
    use super::*;
    pub(super) const ROW_SIZE: usize = 648;
    pub(super) const PID: usize = 40;
    pub(super) const START_SEC: usize = 0;
    pub(super) const START_USEC: usize = 8;
    pub(super) const UID: usize = 420;
    pub(super) const PPID: usize = 560;
    pub(super) const PGID: usize = 564;

    pub(super) fn decode(bytes: &[u8], capacity: usize, deadline: Instant)
        -> Result<Vec<ProcessMetadata>, ReplacementError>
    {
        if bytes.is_empty() || bytes.len() >= capacity || bytes.len() % ROW_SIZE != 0
            || bytes.len() / ROW_SIZE > MAX_PIDS {
            return Err(ReplacementError::StopUnverified);
        }
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        for row in bytes.chunks_exact(ROW_SIZE) {
            if Instant::now() >= deadline { return Err(ReplacementError::StopUnverified); }
            let word = |at| u32::from_ne_bytes(row[at..at+4].try_into().unwrap());
            let pid = word(PID);
            if pid > i32::MAX as u32 || !seen.insert(pid) {
                return Err(ReplacementError::StopUnverified);
            }
            // Kernel and launchd are not signal candidates, as in the original table.
            if pid <= 1 { continue; }
            let sec = i64::from_ne_bytes(row[START_SEC..START_SEC+8].try_into().unwrap());
            let usec = word(START_USEC) as i32;
            let ppid = word(PPID);
            let pgid = word(PGID);
            if sec <= 0 || !(0..1_000_000).contains(&usec)
                || (sec as u64).checked_mul(1_000_000).and_then(|v| v.checked_add(usec as u64)).is_none()
                || ppid > i32::MAX as u32 || pgid > i32::MAX as u32 {
                return Err(ReplacementError::StopUnverified);
            }
            out.push(ProcessMetadata {
                identity: ProcessIdentity { pid, start_time: format!("{}.{:06}", sec, usec) },
                ppid, pgid, uid: word(UID),
            });
        }
        Ok(out)
    }

    pub(super) fn read(deadline: Instant) -> Result<Vec<ProcessMetadata>, ReplacementError> {
        if Instant::now() >= deadline { return Err(ReplacementError::StopUnverified); }
        let mut mib = [libc::CTL_KERN, libc::KERN_PROC, libc::KERN_PROC_ALL, 0];
        let mut needed: usize = 0;
        // Size probe plus ONE bounded read; growth/ENOMEM is uncertainty, not a
        // partial table or an automatic retry. No UID/EPERM-based exclusions.
        let rc = unsafe { libc::sysctl(mib.as_mut_ptr(), 4, std::ptr::null_mut(),
            &mut needed, std::ptr::null_mut(), 0) };
        if rc != 0 || needed == 0 || needed > MAX_PIDS * ROW_SIZE
            || Instant::now() >= deadline {
            return Err(ReplacementError::StopUnverified);
        }
        let capacity = (needed + 64 * ROW_SIZE).min((MAX_PIDS + 1) * ROW_SIZE);
        let mut storage = vec![0u64; capacity.div_ceil(8)];
        let mut len = capacity;
        let rc = unsafe { libc::sysctl(mib.as_mut_ptr(), 4, storage.as_mut_ptr().cast(),
            &mut len, std::ptr::null_mut(), 0) };
        if rc != 0 || len >= capacity { return Err(ReplacementError::StopUnverified); }
        // Aligned initialized allocation; len bounded above, fixed byte decoder
        // avoids creating a Rust struct from the kernel's padding/pointer fields.
        let bytes = unsafe { std::slice::from_raw_parts(storage.as_ptr().cast::<u8>(), len) };
        decode(bytes, capacity, deadline)
    }
}
#[cfg(all(target_os = "macos", target_pointer_width = "64", target_endian = "little",
    any(target_arch = "aarch64", target_arch = "x86_64")))]
fn native_table(deadline: Instant) -> Result<Vec<ProcessMetadata>, ReplacementError> {
    process_table::read(deadline)
}
#[cfg(not(all(target_os = "macos", target_pointer_width = "64", target_endian = "little",
    any(target_arch = "aarch64", target_arch = "x86_64"))))]
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
