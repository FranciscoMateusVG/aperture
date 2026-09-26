//! An operator attaches a TUI client to an existing managed thread. This module
//! never boots a worker, changes an owner, or delivers a prompt.
use crate::{
    journal,
    owner::{self, OwnerRecord, OwnerStore},
    state::{Harness, OwnerState},
    team_process,
    teams::{self, ManagedSeatState},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    os::unix::{fs::MetadataExt, process::CommandExt},
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};
type Result<T> = std::result::Result<T, String>;
const ERROR: &str = "E_TERMINAL_UNAVAILABLE: current managed terminal could not be verified";
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenSeatInput {
    pub team: String,
    pub seat: String,
    pub expected_generation: u64,
}
#[derive(Serialize)]
pub struct OpenSeatView {
    pub team: String,
    pub seat: String,
    pub generation: u64,
    pub window_id: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Client {
    binding: String,
    window: String,
    pid: u32,
    birth: u64,
}
#[derive(Clone, PartialEq, Eq)]
struct Binding {
    hash: String,
    thread: String,
    socket: PathBuf,
    socket_binding: SocketBinding,
    runtime: PathBuf,
    executable: PathBuf,
    executable_id: (u64, u64),
    pid: u32,
    birth: u64,
}
// Path metadata is pinned as well as the kernel peer. This is a bounded
// check/recheck, not atomic exclusion of a malicious concurrent same-UID swap.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct PathPin {
    path: PathBuf,
    dev: u64,
    ino: u64,
    uid: u32,
    mode: u32,
    links: u64,
}
impl PathPin {
    fn read(path: &Path) -> Result<Self> {
        let m = fs::symlink_metadata(path).map_err(|_| ERROR)?;
        Ok(Self {
            path: path.into(),
            dev: m.dev(),
            ino: m.ino(),
            uid: m.uid(),
            mode: m.mode(),
            // Directory child counts are not endpoint identity; unrelated
            // entries in /private/tmp must not invalidate the pin.
            links: if m.is_dir() { 0 } else { m.nlink() },
        })
    }
    fn kind(&self, kind: libc::mode_t) -> bool {
        self.mode & u32::from(libc::S_IFMT) == u32::from(kind)
    }
    fn recheck(&self) -> Result<()> {
        if Self::read(&self.path)? != *self {
            return Err(ERROR.into());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct SocketBinding {
    path: PathBuf,
    pins: Vec<PathPin>,
    link: Option<(PathBuf, PathBuf)>,
}
impl SocketBinding {
    fn recheck(&self) -> Result<()> {
        for pin in &self.pins {
            pin.recheck()?;
        }
        if let Some((link, target)) = &self.link {
            if fs::read_link(link).map_err(|_| ERROR)? != *target {
                return Err(ERROR.into());
            }
        }
        Ok(())
    }
}
fn system_tmp_pins() -> Result<Vec<PathPin>> {
    ["/", "/private", "/private/tmp"]
        .into_iter()
        .map(|p| {
            let pin = PathPin::read(Path::new(p))?;
            let safe_mode = if p == "/private/tmp" {
                pin.mode & 0o7777 == 0o1777
            } else {
                pin.mode & 0o7022 == 0
            };
            if !pin.kind(libc::S_IFDIR) || pin.uid != 0 || !safe_mode {
                return Err(ERROR.into());
            }
            Ok(pin)
        })
        .collect()
}
// Only called with the fixed native daemon directory in production. The
// separate parameter permits hermetic filesystem fixtures, never request paths.
fn pin_socket(socket: &Path, daemon: &Path, uid: u32) -> Result<SocketBinding> {
    let entry = PathPin::read(socket)?;
    if entry.uid != uid {
        return Err(ERROR.into());
    }
    let mut pins = vec![entry.clone()];
    let (path, link) = if entry.kind(libc::S_IFLNK) {
        if entry.links != 1 {
            return Err(ERROR.into());
        }
        let target = fs::read_link(socket).map_err(|_| ERROR)?;
        let name = target.file_name().and_then(|s| s.to_str()).ok_or(ERROR)?;
        // Observed 0.156.1 native basename: 64 lowercase hexadecimal bytes.
        // Exact reconstruction also rejects dot components/repeated separators.
        if name.len() != 64
            || !name
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || target.as_os_str() != daemon.join(name).as_os_str()
        {
            return Err(ERROR.into());
        }
        let dir = PathPin::read(daemon)?;
        if !dir.kind(libc::S_IFDIR) || dir.uid != uid || dir.mode & 0o7777 != 0o700 {
            return Err(ERROR.into());
        }
        pins.push(dir);
        pins.push(PathPin::read(&target)?);
        (target.clone(), Some((socket.into(), target)))
    } else {
        (socket.into(), None)
    };
    let leaf = pins.last().ok_or(ERROR)?;
    if !leaf.kind(libc::S_IFSOCK)
        || leaf.uid != uid
        || leaf.mode & 0o7777 != 0o600
        || leaf.links != 1
    {
        return Err(ERROR.into());
    }
    let result = SocketBinding { path, pins, link };
    result.recheck()?;
    Ok(result)
}
fn resolve_socket(socket: &Path) -> Result<SocketBinding> {
    let uid = unsafe { libc::geteuid() };
    let entry = PathPin::read(socket)?;
    let parents = if entry.kind(libc::S_IFLNK) {
        system_tmp_pins()?
    } else {
        vec![]
    };
    let mut binding = pin_socket(
        socket,
        &PathBuf::from(format!("/private/tmp/codex-daemon-{uid}")),
        uid,
    )?;
    if binding.pins.first() != Some(&entry) {
        return Err(ERROR.into());
    }
    binding.pins.extend(parents);
    binding.recheck()?;
    Ok(binding)
}
fn verify_socket_with(
    socket: &Path,
    pid: u32,
    resolve: impl Fn(&Path) -> Result<SocketBinding>,
    peer: impl FnOnce(&Path, u32) -> Result<()>,
) -> Result<SocketBinding> {
    let binding = resolve(socket)?;
    peer(&binding.path, pid)?;
    // Recollect link and all components after connecting, before any TUI work.
    if resolve(socket)? != binding {
        return Err(ERROR.into());
    }
    Ok(binding)
}
fn verified_socket(socket: &Path, pid: u32) -> Result<SocketBinding> {
    verify_socket_with(socket, pid, resolve_socket, socket_peer)
}
// Coordination peers are observations, never signal authority. The collector
// must prove disjoint closures independently. No caller-selected socket/seat,
// no registry override from env, no RPC, and no creation of missing paths.
pub(crate) struct CoordinationPeer {
    pub(crate) seat: String,
    pub(crate) identity: crate::team_replacement::ProcessIdentity,
    home: PathBuf,
    proof: CoordinationProof,
}
#[derive(PartialEq, Eq)]
struct CoordinationProof {
    pins: Vec<PathPin>,
    manifest: ManifestBinding,
    socket: SocketBinding,
    identity: crate::team_replacement::ProcessIdentity,
}
impl CoordinationPeer {
    pub(crate) fn recheck(&self) -> Result<()> {
        self.recheck_with(&resolve_socket, &peer_pid, &team_process::observe)
    }
    fn recheck_with(
        &self,
        resolve: &impl Fn(&Path) -> Result<SocketBinding>,
        peer: &impl Fn(&Path) -> Result<u32>,
        observe: &impl Fn(
            u32,
        ) -> std::result::Result<
            Option<team_process::ProcessMetadata>,
            crate::team_replacement::ReplacementError,
        >,
    ) -> Result<()> {
        let current = capture_coordination_peer(&self.home, &self.seat, resolve, peer, observe)?
            .ok_or(ERROR)?;
        if current.proof != self.proof || self.identity != self.proof.identity {
            return Err(ERROR.into());
        }
        Ok(())
    }
}
fn coordination_name(seat: &str) -> Result<&'static str> {
    match seat {
        "glados" => Ok("GLaDOS"),
        "peppy" => Ok("Peppy"),
        "wheatley" => Ok("Wheatley"),
        _ => Err(ERROR.into()),
    }
}
fn absent(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Ok(_) => Ok(false),
        Err(_) => Err(ERROR.into()),
    }
}
fn coordination_runtime_dirs(home: &Path) -> Result<Vec<PathPin>> {
    use std::path::Component;
    if !home.is_absolute() {
        return Err(ERROR.into());
    }
    let uid = unsafe { libc::geteuid() };
    let mut path = PathBuf::new();
    let mut pins = vec![];
    for component in home.components() {
        if !matches!(component, Component::RootDir | Component::Normal(_)) {
            return Err(ERROR.into());
        }
        path.push(component);
        let pin = PathPin::read(&path)?;
        if !pin.kind(libc::S_IFDIR)
            || ![0, uid].contains(&pin.uid)
            || pin.mode & 0o022 != 0 && !(pin.uid == 0 && pin.mode & 0o1777 == 0o1777)
        {
            return Err(ERROR.into());
        }
        pins.push(pin);
    }
    if pins.last().ok_or(ERROR)?.uid != uid {
        return Err(ERROR.into());
    }
    for relative in [".aperture", ".aperture/run"] {
        let pin = PathPin::read(&home.join(relative))?;
        if !pin.kind(libc::S_IFDIR) || pin.uid != uid || pin.mode & 0o077 != 0 {
            return Err(ERROR.into());
        }
        pins.push(pin);
    }
    Ok(pins)
}
fn coordination_registry_dirs(home: &Path, seat: &str) -> Result<Vec<PathPin>> {
    coordination_name(seat)?;
    let mut pins = vec![];
    // .claude is a user config ancestor, like HOME; dedicated Aperture
    // storage remains private. No permission changes or mkdir here.
    for (relative, private) in [
        (".claude".to_string(), false),
        (".claude/aperture".into(), true),
        (format!(".claude/aperture/{seat}"), false),
    ] {
        let pin = PathPin::read(&home.join(relative))?;
        if !pin.kind(libc::S_IFDIR)
            || pin.uid != unsafe { libc::geteuid() }
            || pin.mode & if private { 0o077 } else { 0o022 } != 0
        {
            return Err(ERROR.into());
        }
        pins.push(pin);
    }
    Ok(pins)
}
#[derive(PartialEq, Eq)]
struct ManifestBinding {
    pins: Vec<PathPin>,
    link: Option<(PathBuf, PathBuf)>,
    sha256: String,
}
fn coordination_manifest(path: &Path, seat: &str) -> Result<ManifestBinding> {
    use std::{io::Read, os::unix::fs::OpenOptionsExt, path::Component};
    let entry = PathPin::read(path)?;
    let uid = unsafe { libc::geteuid() };
    if entry.uid != uid || entry.links != 1 {
        return Err(ERROR.into());
    }
    let mut pins = vec![];
    let (target, link) = if entry.kind(libc::S_IFLNK) {
        let raw = fs::read_link(path).map_err(|_| ERROR)?;
        let target = if raw.is_absolute() {
            raw.clone()
        } else {
            path.parent().ok_or(ERROR)?.join(&raw)
        };
        // Pin each config ancestor instead of canonicalize/following an
        // arbitrary link chain. Setup's manifest symlink is explicitly allowed;
        // secondary links, traversal and writable ancestors are not.
        let mut parent = PathBuf::new();
        for component in target.parent().ok_or(ERROR)?.components() {
            if !matches!(component, Component::RootDir | Component::Normal(_)) {
                return Err(ERROR.into());
            }
            parent.push(component);
            let pin = PathPin::read(&parent)?;
            if !pin.kind(libc::S_IFDIR)
                || ![0, uid].contains(&pin.uid)
                || pin.mode & 0o022 != 0 && !(pin.uid == 0 && pin.mode & 0o1777 == 0o1777)
            {
                return Err(ERROR.into());
            }
            pins.push(pin);
        }
        pins.push(entry);
        (target, Some((path.into(), raw)))
    } else {
        (path.into(), None)
    };
    let pin = PathPin::read(&target)?;
    if !pin.kind(libc::S_IFREG) || pin.uid != uid || pin.links != 1 || pin.mode & 0o022 != 0 {
        return Err(ERROR.into());
    }
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(&target)
        .map_err(|_| ERROR)?;
    let meta = file.metadata().map_err(|_| ERROR)?;
    if meta.dev() != pin.dev
        || meta.ino() != pin.ino
        || meta.uid() != pin.uid
        || meta.mode() != pin.mode
        || meta.nlink() != 1
        || meta.len() > 65536
    {
        return Err(ERROR.into());
    }
    let mut bytes = vec![];
    file.take(65537)
        .read_to_end(&mut bytes)
        .map_err(|_| ERROR)?;
    if bytes.len() > 65536 {
        return Err(ERROR.into());
    }
    #[derive(Deserialize)]
    struct Manifest {
        name: String,
        enabled: bool,
    }
    let manifest: Manifest = serde_json::from_slice(&bytes).map_err(|_| ERROR)?;
    if !manifest.enabled || !manifest.name.eq_ignore_ascii_case(coordination_name(seat)?) {
        return Err(ERROR.into());
    }
    pins.push(pin);
    for pin in &pins {
        pin.recheck()?;
    }
    if let Some((path, raw)) = &link {
        if fs::read_link(path).map_err(|_| ERROR)? != *raw {
            return Err(ERROR.into());
        }
    }
    Ok(ManifestBinding {
        pins,
        link,
        sha256: format!("{:x}", Sha256::digest(&bytes)),
    })
}
fn capture_coordination_peer(
    home: &Path,
    seat: &str,
    resolve: &impl Fn(&Path) -> Result<SocketBinding>,
    peer: &impl Fn(&Path) -> Result<u32>,
    observe: &impl Fn(
        u32,
    ) -> std::result::Result<
        Option<team_process::ProcessMetadata>,
        crate::team_replacement::ReplacementError,
    >,
) -> Result<Option<CoordinationPeer>> {
    coordination_name(seat)?;
    // Validate run before skipping a missing endpoint; a missing/unsafe parent
    // is not proof of absence. Other registry paths are required for a peer.
    let run = home.join(".aperture/run");
    let socket_path = run.join(format!("{seat}.sock"));
    // RO equivalent of private_chain; that helper may mkdir a missing root.
    let mut pins = coordination_runtime_dirs(home)?;
    if absent(&socket_path)? {
        return Ok(None);
    }
    pins.extend(coordination_registry_dirs(home, seat)?);
    let uid = unsafe { libc::geteuid() };
    let agent = home.join(".claude/aperture").join(seat);
    if !absent(&agent.join("TEAM"))? {
        return Err(ERROR.into());
    }
    let manifest = agent.join("manifest.json");
    let manifest_binding = coordination_manifest(&manifest, seat)?;
    let socket = resolve(&socket_path)?;
    let pid = peer(&socket.path)?;
    let before = observe(pid).map_err(|_| ERROR)?.ok_or(ERROR)?;
    if pid <= 1 || before.identity.pid != pid || before.uid != uid {
        return Err(ERROR.into());
    }
    if peer(&socket.path)? != pid {
        return Err(ERROR.into());
    }
    let after = observe(pid).map_err(|_| ERROR)?.ok_or(ERROR)?;
    if before.identity != after.identity || after.uid != uid {
        return Err(ERROR.into());
    }
    if resolve(&socket_path)? != socket
        || !absent(&agent.join("TEAM"))?
        || coordination_manifest(&manifest, seat)? != manifest_binding
    {
        return Err(ERROR.into());
    }
    for pin in &pins {
        pin.recheck()?;
    }
    Ok(Some(CoordinationPeer {
        seat: seat.into(),
        identity: before.identity.clone(),
        home: home.into(),
        proof: CoordinationProof {
            pins,
            manifest: manifest_binding,
            socket,
            identity: before.identity,
        },
    }))
}
pub(crate) fn capture_coordination_peers(home: &Path) -> Result<Vec<CoordinationPeer>> {
    let mut peers = vec![];
    for seat in ["glados", "peppy", "wheatley"] {
        if let Some(peer) = capture_coordination_peer(
            home,
            seat,
            &resolve_socket,
            &peer_pid,
            &team_process::observe,
        )? {
            peers.push(peer);
        }
    }
    for peer in &peers {
        peer.recheck()?;
    }
    Ok(peers)
}
fn selectors(input: &OpenSeatInput) -> Result<()> {
    if input.team.len() > 16
        || !crate::agent_loader::is_valid_seat_name(&input.team)
        || !crate::agent_loader::is_valid_seat_name(&input.seat)
        || input.expected_generation == 0
    {
        return Err(ERROR.into());
    }
    Ok(())
}
fn owner_valid(input: &OpenSeatInput, r: &OwnerRecord) -> Result<()> {
    let i = r.incarnation.as_ref().ok_or(ERROR)?;
    if r.schema_version != 1
        || r.seat != input.seat
        || r.generation != input.expected_generation
        || r.state != OwnerState::Active
        || r.requested.harness != Harness::Codex
        || !i.observed
        || i.harness != r.requested.harness
        || i.model != r.requested.model
        || i.reasoning != r.requested.reasoning
        || i.pid <= 1
        || i.start_time == 0
        || i.thread_id.is_empty()
        || i.thread_id.len() > 128
        || !i.thread_id.as_bytes()[0].is_ascii_alphanumeric()
        || !i
            .thread_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        || !i
            .processes
            .iter()
            .any(|p| p.pid == i.pid && p.start_time == i.start_time)
    {
        return Err(ERROR.into());
    }
    Ok(())
}
fn private_chain(home: &Path, relative: &str) -> Result<PathBuf> {
    // HOME is not Aperture's private storage root: ordinary macOS homes may
    // allow group/world read or traversal. Never chmod it or require 0700.
    // Private permissions remain mandatory from .aperture downwards.
    let meta = fs::symlink_metadata(home).map_err(|_| ERROR)?;
    if !meta.is_dir()
        || meta.file_type().is_symlink()
        || meta.uid() != unsafe { libc::geteuid() }
        || meta.mode() & 0o022 != 0
    {
        return Err(ERROR.into());
    }
    let suffix = Path::new(relative)
        .strip_prefix(".aperture")
        .map_err(|_| ERROR)?
        .to_str()
        .ok_or(ERROR)?;
    let path = journal::validate_component_path(&home.join(".aperture"), suffix, false)?;
    let mut p = home.to_path_buf();
    for c in Path::new(relative).components() {
        p.push(c);
        let m = fs::symlink_metadata(&p).map_err(|_| ERROR)?;
        if !m.is_dir()
            || m.file_type().is_symlink()
            || m.uid() != unsafe { libc::geteuid() }
            || m.mode() & 0o077 != 0
        {
            return Err(ERROR.into());
        }
    }
    Ok(path)
}
fn executable(path: &Path) -> Result<(u64, u64)> {
    let m = fs::symlink_metadata(path).map_err(|_| ERROR)?;
    if !m.is_file()
        || m.file_type().is_symlink()
        || m.nlink() != 1
        || ![0, unsafe { libc::geteuid() }].contains(&m.uid())
        || m.mode() & 0o022 != 0
        || m.mode() & 0o111 == 0
    {
        return Err(ERROR.into());
    }
    Ok((m.dev(), m.ino()))
}
#[cfg(target_os = "macos")]
fn process_executable(pid: u32) -> Result<PathBuf> {
    let mut bytes = vec![0u8; 4096];
    let n =
        unsafe { libc::proc_pidpath(pid as i32, bytes.as_mut_ptr().cast(), bytes.len() as u32) };
    if n <= 0 {
        return Err(ERROR.into());
    }
    let end = bytes.iter().position(|b| *b == 0).ok_or(ERROR)?;
    Ok(PathBuf::from(
        std::str::from_utf8(&bytes[..end]).map_err(|_| ERROR)?,
    ))
}
#[cfg(not(target_os = "macos"))]
fn process_executable(_: u32) -> Result<PathBuf> {
    Err(ERROR.into())
}
fn live(pid: u32, birth: u64) -> Result<()> {
    let expected = team_process::identity_from_owner(pid, birth).map_err(|_| ERROR)?;
    let p = team_process::observe(pid)
        .map_err(|_| ERROR)?
        .ok_or(ERROR)?;
    if p.identity != expected || p.uid != unsafe { libc::geteuid() } {
        return Err(ERROR.into());
    }
    Ok(())
}
/// Check the kernel's peer PID, not just the socket pathname. No RPC or prompt.
/// Nonblocking connect/poll is bounded even if the listener is saturated.
#[cfg(target_os = "macos")]
fn peer_pid(path: &Path) -> Result<u32> {
    use std::os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::ffi::OsStrExt,
    };
    let bytes = path.as_os_str().as_bytes();
    let mut addr: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    if bytes.is_empty() || bytes.len() >= addr.sun_path.len() || bytes.contains(&0) {
        return Err(ERROR.into());
    }
    addr.sun_family = libc::AF_UNIX as libc::sa_family_t;
    addr.sun_len = (std::mem::offset_of!(libc::sockaddr_un, sun_path) + bytes.len() + 1) as u8;
    for (slot, b) in addr.sun_path.iter_mut().zip(bytes) {
        *slot = *b as libc::c_char;
    }
    let raw = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0) };
    if raw < 0 {
        return Err(ERROR.into());
    }
    // Own the descriptor immediately; all failure branches close it.
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };
    if unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } < 0
        || unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK) } < 0
    {
        return Err(ERROR.into());
    }
    let rc = unsafe {
        libc::connect(
            fd.as_raw_fd(),
            (&addr as *const libc::sockaddr_un).cast(),
            addr.sun_len as libc::socklen_t,
        )
    };
    if rc != 0 {
        if std::io::Error::last_os_error().raw_os_error() != Some(libc::EINPROGRESS) {
            return Err(ERROR.into());
        }
        let mut poll = libc::pollfd {
            fd: fd.as_raw_fd(),
            events: libc::POLLOUT,
            revents: 0,
        };
        if unsafe { libc::poll(&mut poll, 1, 1000) } != 1 || poll.revents & libc::POLLOUT == 0 {
            return Err(ERROR.into());
        }
        let mut error: libc::c_int = 0;
        let mut len = std::mem::size_of_val(&error) as libc::socklen_t;
        if unsafe {
            libc::getsockopt(
                fd.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_ERROR,
                (&mut error as *mut libc::c_int).cast(),
                &mut len,
            )
        } != 0
            || error != 0
        {
            return Err(ERROR.into());
        }
    }
    let mut peer: libc::pid_t = 0;
    let mut len = std::mem::size_of_val(&peer) as libc::socklen_t;
    if unsafe {
        libc::getsockopt(
            fd.as_raw_fd(),
            libc::SOL_LOCAL,
            libc::LOCAL_PEERPID,
            (&mut peer as *mut libc::pid_t).cast(),
            &mut len,
        )
    } != 0
        || len as usize != std::mem::size_of_val(&peer)
        || peer <= 1
    {
        return Err(ERROR.into());
    }
    Ok(peer as u32)
}
#[cfg(not(target_os = "macos"))]
fn peer_pid(_: &Path) -> Result<u32> {
    Err(ERROR.into())
}
fn socket_peer(path: &Path, expected_pid: u32) -> Result<()> {
    if peer_pid(path)? != expected_pid {
        return Err(ERROR.into());
    }
    Ok(())
}
fn binding(home: &Path, input: &OpenSeatInput, store: &OwnerStore) -> Result<Binding> {
    match teams::classify_managed_seat(home, &input.seat).map_err(|_| ERROR)? {
        Some(ManagedSeatState::Active { team, .. }) if team == input.team => {}
        _ => return Err(ERROR.into()),
    }
    let r = store.read_owner_locked(&input.seat)?;
    owner_valid(input, &r)?;
    let i = r.incarnation.as_ref().unwrap();
    live(i.pid, i.start_time)?;
    let run = private_chain(home, ".aperture/run")?;
    let runtime = private_chain(
        home,
        &format!(
            ".aperture/run/managed/{}/g{}",
            input.seat, input.expected_generation
        ),
    )?;
    let socket = run.join(format!("{}.sock", input.seat));
    let socket_binding = verified_socket(&socket, i.pid)?;
    let bin = process_executable(i.pid)?;
    let executable_id = executable(&bin)?;
    live(i.pid, i.start_time)?;
    // Carry socket/executable pins across parent-to-helper admission as well as
    // repeated checks within each process. An old owner-only receipt cannot bind
    // a newly replaced endpoint.
    let bytes = serde_json::to_vec(&(&r, &socket_binding, &runtime, &bin, executable_id))
        .map_err(|_| ERROR)?;
    Ok(Binding {
        hash: format!("{:x}", Sha256::digest(bytes)),
        thread: i.thread_id.clone(),
        socket: socket_binding.path.clone(),
        socket_binding,
        runtime,
        executable: bin,
        executable_id,
        pid: i.pid,
        birth: i.start_time,
    })
}
/// Finite tmux requests only. Never interpolates a shell program or caller path.
fn tmux(args: &[String]) -> Result<String> {
    let mut command = Command::new("/opt/homebrew/bin/tmux");
    command.args(args);
    let bytes = crate::team_replacement::repository::bounded_command(
        command,
        Instant::now() + Duration::from_secs(3),
    )
    .map_err(|_| "E_TERMINAL_UNKNOWN: tmux result unknown; refresh before retry".to_string())?;
    if bytes.len() > 8192 {
        return Err(ERROR.into());
    }
    String::from_utf8(bytes).map_err(|_| ERROR.into())
}

fn args(xs: &[&str]) -> Vec<String> {
    xs.iter().map(|s| (*s).into()).collect()
}
fn window_id(s: &str) -> bool {
    s.starts_with('@') && s.len() > 1 && s.len() < 24 && s[1..].bytes().all(|b| b.is_ascii_digit())
}
fn pane(window: &str) -> Result<(u32, bool)> {
    if !window_id(window) {
        return Err(ERROR.into());
    }
    let out = tmux(&args(&[
        "list-panes",
        "-t",
        window,
        "-F",
        "#{pane_pid}|#{pane_dead}",
    ]))?;
    if out.lines().count() != 1 {
        return Err(ERROR.into());
    }
    let (pid, dead) = out.trim().split_once('|').ok_or(ERROR)?;
    Ok((
        pid.parse().map_err(|_| ERROR)?,
        match dead {
            "0" => false,
            "1" => true,
            _ => return Err(ERROR.into()),
        },
    ))
}
fn client_path(home: &Path, input: &OpenSeatInput) -> PathBuf {
    home.join(".aperture/run/terminals").join(format!(
        "{}-g{}.json",
        input.seat, input.expected_generation
    ))
}
fn reusable(c: &Client, b: &Binding, pid: u32, dead: bool) -> bool {
    c.binding == b.hash && c.pid == pid && !dead && live(c.pid, c.birth).is_ok()
}
fn tui_args(thread: &str, socket: &Path) -> Vec<String> {
    vec![
        "resume".into(),
        thread.into(),
        "--remote".into(),
        format!("unix://{}", socket.display()),
    ]
}
fn boot_helper() -> Result<PathBuf> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/release/aperture-boot");
    executable(&p)?;
    Ok(p)
}
/// Called only by the GUI's operator command. Selectors are never shell input.
pub(crate) fn open(home: &Path, input: OpenSeatInput) -> Result<OpenSeatView> {
    selectors(&input)?;
    let _team = owner::try_lock(&home.join(".aperture/run/team-locks"), &input.team)?;
    let store = OwnerStore::new(home.join(".aperture/run/owner"));
    let _seat = store.lock(&input.seat)?;
    // A Claude worker already runs inside its own launch window. Open selects
    // that exact window; the Codex client path below is untouched.
    if store.read_owner_locked(&input.seat)?.requested.harness == Harness::Claude {
        return open_claude_with(
            &mut NativeClaude {
                home,
                input: &input,
                store: &store,
            },
            &input,
        );
    }
    let before = binding(home, &input, &store)?;
    let dir = home.join(".aperture/run/terminals");
    journal::ensure_private_dir(&dir)?;
    let path = client_path(home, &input);
    let old = match fs::symlink_metadata(&path) {
        Ok(_) => Some(journal::read_private_json::<Client>(&path)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => return Err(ERROR.into()),
    };
    if let Some(c) = old.as_ref() {
        // A dead/missing client can be replaced. A live but differently bound
        // client is never silently reused or killed.
        if let Ok((pid, dead)) = pane(&c.window) {
            if reusable(c, &before, pid, dead) {
                if binding(home, &input, &store)? != before {
                    return Err(ERROR.into());
                }
                tmux(&args(&["select-window", "-t", &c.window]))?;
                return Ok(OpenSeatView {
                    team: input.team,
                    seat: input.seat,
                    generation: input.expected_generation,
                    window_id: c.window.clone(),
                });
            }
            if !dead && live(c.pid, c.birth).is_ok() {
                return Err(ERROR.into());
            }
        } else if live(c.pid, c.birth).is_ok() {
            return Err(ERROR.into());
        }
    }
    let helper = boot_helper()?;
    if binding(home, &input, &store)? != before {
        return Err(ERROR.into());
    }
    let out = tmux(&vec![
        "new-window".into(),
        "-d".into(),
        "-t".into(),
        "aperture".into(),
        "-n".into(),
        format!("{}-g{}", input.seat, input.expected_generation),
        "-P".into(),
        "-F".into(),
        "#{window_id}".into(),
        helper.to_string_lossy().into(),
        "--attach-managed".into(),
        input.team.clone(),
        input.seat.clone(),
        input.expected_generation.to_string(),
        before.hash.clone(),
    ])?;
    let window = out.trim().to_string();
    let (pid, dead) = pane(&window)?;
    if dead {
        return Err(ERROR.into());
    }
    let p = team_process::observe(pid)
        .map_err(|_| ERROR)?
        .ok_or(ERROR)?;
    let birth = team_process::birth_micros(&p.identity).map_err(|_| ERROR)?;
    let c = Client {
        binding: before.hash,
        window: window.clone(),
        pid,
        birth,
    };
    journal::write_private_json_atomic(&path, &c, old.is_some())?;
    // Child is waiting for these locks. It validates this receipt and all native
    // evidence again before exec. No failed path launches a second worker.
    tmux(&args(&["select-window", "-t", &window]))?;
    Ok(OpenSeatView {
        team: input.team,
        seat: input.seat,
        generation: input.expected_generation,
        window_id: window,
    })
}
/// Tmux executes this helper, not a worker launcher. It reacquires the same
/// locks after the GUI publishes its client receipt, then execs only the TUI.
pub fn attach_existing(
    team: String,
    seat: String,
    generation: u64,
    expected_hash: String,
) -> Result<()> {
    let input = OpenSeatInput {
        team,
        seat,
        expected_generation: generation,
    };
    selectors(&input)?;
    if expected_hash.len() != 64 || !expected_hash.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err(ERROR.into());
    }
    let home = PathBuf::from(std::env::var_os("HOME").ok_or(ERROR)?);
    let until = Instant::now() + Duration::from_secs(5);
    let _team = loop {
        match owner::try_lock(&home.join(".aperture/run/team-locks"), &input.team) {
            Ok(l) => break l,
            Err(e) if e.starts_with("E_LOCK_HELD") && Instant::now() < until => {
                std::thread::sleep(Duration::from_millis(20))
            }
            Err(e) => return Err(e),
        }
    };
    let store = OwnerStore::new(home.join(".aperture/run/owner"));
    let _seat = store.lock(&input.seat)?;
    let b = binding(&home, &input, &store)?;
    if b.hash != expected_hash {
        return Err(ERROR.into());
    }
    let c: Client = journal::read_private_json(&client_path(&home, &input))?;
    if c.pid != std::process::id() || c.binding != b.hash {
        return Err(ERROR.into());
    }
    live(c.pid, c.birth)?;
    let (pid, dead) = pane(&c.window)?;
    if pid != c.pid || dead {
        return Err(ERROR.into());
    }
    if binding(&home, &input, &store)? != b {
        return Err(ERROR.into());
    }
    let error = Command::new(&b.executable)
        .args(tui_args(&b.thread, &b.socket))
        .env("CODEX_HOME", &b.runtime)
        .env_remove("CODEX_THREAD_ID")
        .current_dir(&b.runtime)
        .exec();
    Err(format!(
        "E_TERMINAL_UNAVAILABLE: TUI exec failed ({:?})",
        error.kind()
    ))
}
#[tauri::command]
pub async fn team_open_seat(
    input: OpenSeatInput,
) -> std::result::Result<OpenSeatView, teams::TeamError> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| teams::TeamError {
            code: "E_TERMINAL_UNAVAILABLE".into(),
            message: "Home unavailable".into(),
        })?;
    tauri::async_runtime::spawn_blocking(move || open(&home, input))
        .await
        .map_err(|_| teams::TeamError {
            code: "E_TERMINAL_UNKNOWN".into(),
            message: "Terminal operation interrupted".into(),
        })?
        .map_err(|e| teams::TeamError {
            code: e
                .split(':')
                .next()
                .unwrap_or("E_TERMINAL_UNAVAILABLE")
                .into(),
            message: "Could not attach the current managed terminal; refresh before retry".into(),
        })
}

// ---------------------------------------------------------------------------
// Claude seats: the worker IS the tmux window created at launch (window name
// `<seat>-g<gen>`, pane root process == owner incarnation pid). Open therefore
// only locates and selects that window. No client receipt, no new-window, no
// helper, no worker/thread, no keys. Every negative returns before `select`.
// ---------------------------------------------------------------------------
const CLAUDE_PANE_FORMAT: &str =
    "#{session_name}|#{window_id}|#{pane_id}|#{pane_pid}|#{pane_dead}|#{window_name}";
/// Private injection boundary for hermetic ordering tests only. The public
/// entrypoint always uses `NativeClaude`; no DTO supplies any of these facts.
trait ClaudeWindowIo {
    fn owner(&mut self) -> Result<OwnerRecord>;
    fn live(&mut self, pid: u32, birth: u64) -> Result<()>;
    fn panes(&mut self) -> Result<String>;
    fn select(&mut self, window: &str) -> Result<()>;
}
struct NativeClaude<'a> {
    home: &'a Path,
    input: &'a OpenSeatInput,
    store: &'a OwnerStore,
}
impl ClaudeWindowIo for NativeClaude<'_> {
    fn owner(&mut self) -> Result<OwnerRecord> {
        match teams::classify_managed_seat(self.home, &self.input.seat).map_err(|_| ERROR)? {
            Some(ManagedSeatState::Active { team, .. }) if team == self.input.team => {}
            _ => return Err(ERROR.into()),
        }
        let r = self.store.read_owner_locked(&self.input.seat)?;
        claude_owner_valid(self.input, &r)?;
        Ok(r)
    }
    fn live(&mut self, pid: u32, birth: u64) -> Result<()> {
        live(pid, birth)
    }
    fn panes(&mut self) -> Result<String> {
        tmux(&args(&[
            "list-panes",
            "-s",
            "-t",
            "aperture",
            "-F",
            CLAUDE_PANE_FORMAT,
        ]))
    }
    fn select(&mut self, window: &str) -> Result<()> {
        if !window_id(window) {
            return Err(ERROR.into());
        }
        tmux(&args(&["select-window", "-t", window])).map(|_| ())
    }
}
/// Exact Active Claude owner: approved exact Claude literal / reasoning None
/// (see `team_claude_launch::CLAUDE_MODELS`), observed session id equal to the
/// requested one, no pending reservation, and the pane root process recorded.
fn claude_owner_valid(input: &OpenSeatInput, r: &OwnerRecord) -> Result<()> {
    let i = r.incarnation.as_ref().ok_or(ERROR)?;
    if r.schema_version != 1
        || r.seat != input.seat
        || r.generation != input.expected_generation
        || r.generation == 0
        || r.state != OwnerState::Active
        || r.requested.harness != Harness::Claude
        || !crate::team_claude_launch::is_exact_claude_model(&r.requested.model)
        || r.requested.reasoning.is_some()
        || r.reservation_nonce_sha256.is_some()
        || r.provisional_token_id.is_some()
        || !i.observed
        || i.harness != r.requested.harness
        || i.model != r.requested.model
        || i.reasoning != r.requested.reasoning
        || i.pid <= 1
        || i.start_time == 0
        || !crate::team_claude_launch::canonical_uuid(&i.thread_id)
        || !i
            .processes
            .iter()
            .any(|p| p.pid == i.pid && p.start_time == i.start_time)
    {
        return Err(ERROR.into());
    }
    Ok(())
}
fn pane_id(s: &str) -> bool {
    s.starts_with('%') && s.len() > 1 && s.len() < 24 && s[1..].bytes().all(|b| b.is_ascii_digit())
}
/// Exactly one live pane in session `aperture` whose root pid is the owner pid
/// and whose window carries the launch name. Malformed rows fail closed.
fn claude_window(out: &str, pid: u32, name: &str) -> Result<String> {
    if out.len() > 8192 {
        return Err(ERROR.into());
    }
    let mut found: Option<String> = None;
    for (n, line) in out.lines().enumerate() {
        if n >= 4096 {
            return Err(ERROR.into());
        }
        let p: Vec<&str> = line.splitn(6, '|').collect();
        if p.len() != 6 || !window_id(p[1]) || !pane_id(p[2]) {
            return Err(ERROR.into());
        }
        let row_pid: u32 = p[3].parse().map_err(|_| ERROR)?;
        if row_pid != pid {
            continue;
        }
        if found.is_some() || p[0] != "aperture" || p[4] != "0" || p[5] != name {
            return Err(ERROR.into());
        }
        found = Some(p[1].to_string());
    }
    found.ok_or_else(|| ERROR.into())
}
/// Production sequence: owner -> live -> panes -> live -> owner -> select.
/// The only mutating verb runs after the second live/owner recheck.
fn open_claude_with(io: &mut impl ClaudeWindowIo, input: &OpenSeatInput) -> Result<OpenSeatView> {
    let owner = io.owner()?;
    let (pid, birth) = {
        let i = owner.incarnation.as_ref().ok_or(ERROR)?;
        (i.pid, i.start_time)
    };
    io.live(pid, birth)?;
    let name = format!("{}-g{}", input.seat, input.expected_generation);
    let window = claude_window(&io.panes()?, pid, &name)?;
    io.live(pid, birth)?;
    if io.owner()? != owner {
        return Err(ERROR.into());
    }
    io.select(&window)?;
    Ok(OpenSeatView {
        team: input.team.clone(),
        seat: input.seat.clone(),
        generation: input.expected_generation,
        window_id: window,
    })
}
#[cfg(test)]
#[path = "team_terminal_tests.rs"]
mod tests;
