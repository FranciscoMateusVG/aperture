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
    os::unix::{
        fs::{FileTypeExt, MetadataExt},
        process::CommandExt,
    },
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
    socket_id: (u64, u64),
    runtime: PathBuf,
    executable: PathBuf,
    executable_id: (u64, u64),
    pid: u32,
    birth: u64,
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
    let path = journal::validate_component_path(home, relative, false)?;
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
fn socket_peer(path: &Path, expected_pid: u32) -> Result<()> {
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
        || peer as u32 != expected_pid
    {
        return Err(ERROR.into());
    }
    Ok(())
}
#[cfg(not(target_os = "macos"))]
fn socket_peer(_: &Path, _: u32) -> Result<()> {
    Err(ERROR.into())
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
    let m = fs::symlink_metadata(&socket).map_err(|_| ERROR)?;
    if !m.file_type().is_socket() || m.uid() != unsafe { libc::geteuid() } {
        return Err(ERROR.into());
    }
    socket_peer(&socket, i.pid)?;
    let bin = process_executable(i.pid)?;
    let executable_id = executable(&bin)?;
    live(i.pid, i.start_time)?;
    let bytes = serde_json::to_vec(&r).map_err(|_| ERROR)?;
    Ok(Binding {
        hash: format!("{:x}", Sha256::digest(bytes)),
        thread: i.thread_id.clone(),
        socket,
        socket_id: (m.dev(), m.ino()),
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
/// Exact Active Claude owner: approved Sonnet 5 / reasoning None, observed
/// session id, no pending reservation, and the pane root process recorded.
fn claude_owner_valid(input: &OpenSeatInput, r: &OwnerRecord) -> Result<()> {
    let i = r.incarnation.as_ref().ok_or(ERROR)?;
    if r.schema_version != 1
        || r.seat != input.seat
        || r.generation != input.expected_generation
        || r.generation == 0
        || r.state != OwnerState::Active
        || r.requested.harness != Harness::Claude
        || r.requested.model != crate::team_claude_launch::MODEL
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
