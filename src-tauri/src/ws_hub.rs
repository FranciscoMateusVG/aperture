//! F2-B2 synchronous, lease-borrowed native hub supervisor. No kill-by-port,
//! blind respawn loop or detached authority thread. Aggregate startup stays
//! fenced in daemons.rs until C/D; protocol completion alone is not identity.
use crate::{
    controller::ControllerLock,
    daemon_registry::{Endpoint, Provenance, Registry},
    team_process,
    team_replacement::{ProcessIdentity, ProcessState},
};
use std::os::{
    fd::AsRawFd,
    unix::{
        fs::{MetadataExt, OpenOptionsExt},
        process::CommandExt,
    },
};
use std::{
    io::{Read, Write},
    net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

const HUB_PORT: u16 = 4517;
const UNVERIFIED: &str = "E_HUB_UNVERIFIED";

fn resolve_node() -> String {
    if let Ok(bin) = std::env::var("APERTURE_NODE_BIN") {
        if !bin.is_empty() {
            return bin;
        }
    }
    if let Ok(out) = Command::new("node")
        .args(["-e", "process.stdout.write(process.execPath)"])
        .output()
    {
        if out.status.success() {
            let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !path.is_empty() && std::path::Path::new(&path).exists() {
                return path;
            }
        }
    }
    for candidate in ["/opt/homebrew/bin/node", "/usr/local/bin/node"] {
        if std::path::Path::new(candidate).exists() {
            return candidate.to_string();
        }
    }
    "node".to_string()
}

/// Fixed native launch parameters. Production and inert fixtures use the same
/// supervisor, spawn/record and pre-bearer proof path, not copied algorithms.
pub(crate) struct HubSpec {
    pub command: PathBuf,
    pub args: Vec<std::ffi::OsString>,
    pub endpoint: SocketAddr,
    pub home: PathBuf,
    pub provenance: Provenance,
    #[cfg(test)]
    pub fault: Option<String>,
    #[cfg(test)]
    pub fixture_mode: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HubObservation {
    pub identity: ProcessIdentity,
    pub spawned: bool,
    // Adoption has no waitpid authority/exit code. Never fabricate exit zero.
    pub exit_observation: &'static str,
}
pub(crate) struct Supervisor<'a> {
    lease: &'a ControllerLock,
    spec: HubSpec,
}
impl<'a> Supervisor<'a> {
    pub(crate) fn new(lease: &'a ControllerLock, spec: HubSpec) -> Result<Self, String> {
        lease.verify_live()?;
        if spec.endpoint.ip() != Ipv4Addr::LOCALHOST
            || spec.endpoint.port() == 0
            || spec.home.join(".aperture/run") != lease.run_dir()?
            || !spec.command.is_absolute()
        {
            return Err(UNVERIFIED.into());
        }
        Ok(Self { lease, spec })
    }
    fn edge(&self, point: &str) -> Result<(), String> {
        #[cfg(test)]
        if self.spec.fault.as_deref() == Some(point) {
            return Err("E_HUB_FIXTURE_CRASH_EDGE".into());
        }
        let _ = point;
        Ok(())
    }
    pub(crate) fn reconcile(&mut self) -> Result<HubObservation, String> {
        let _flight = self.lease.hub_transition()?;
        let registry = Registry::open(self.lease)?;
        registry.validate_namespace()?; // namespace structure is not target adoption
        // Reap only our direct child, without signals. Error is not Gone.
        let mut held_child = self.lease.hub_child()?;
        if let Some(child) = held_child.as_mut() {
            if child.try_wait().map_err(|_| UNVERIFIED)?.is_some() {
                *held_child = None;
            }
        }
        if let Some(current) = registry.current("hub")? {
            if current.endpoint()
                != &(Endpoint::Hub {
                    port: self.spec.endpoint.port(),
                })
            {
                return Err(UNVERIFIED.into());
            }
            let expected = current.identity();
            match team_process::state(&expected) {
                ProcessState::Same => {
                    probe(self.lease, self.spec.endpoint, &expected)?;
                    return Ok(HubObservation {
                        identity: expected,
                        spawned: false,
                        exit_observation: "NOT_OBSERVED",
                    });
                }
                ProcessState::Gone => {} // Separate guarded transition below, never adoption.
                _ => return Err(UNVERIFIED.into()),
            }
        }
        if held_child.is_some() {
            return Err("E_HUB_CHILD_UNRECONCILED".into());
        }
        registry.capacity("hub")?;
        // Absence metadata is not absence of a listener. A bind without REUSE
        // denies any occupant. Releasing it is NOT kill authority; a subsequent
        // race leaves the durable reservation and cannot trigger a blind retry.
        let free =
            TcpListener::bind(self.spec.endpoint).map_err(|_| "E_HUB_OCCUPIED_OR_UNVERIFIED")?;
        let reservation = registry.reserve(
            Endpoint::Hub {
                port: self.spec.endpoint.port(),
            },
            self.spec.provenance.clone(),
            now_ms()?,
        )?;
        self.edge("reserved")?;
        self.lease.verify_live()?;
        let mut command = Command::new(&self.spec.command);
        command
            .args(&self.spec.args)
            .env("HOME", &self.spec.home)
            .env("APERTURE_RUN_DIR", self.lease.run_dir()?)
            .env(
                "APERTURE_HUB_TOKEN_DIR",
                self.lease.run_dir()?.join("hub-tokens"),
            )
            .env("APERTURE_WS_PORT", self.spec.endpoint.port().to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // Retain the existing hub diagnostic destination, without inherited
        // pipe handles that die with the controller. Failure stays Unknown;
        // never silently discard daemon diagnostics or retry this reservation.
        let log_dir = self.spec.home.join(".aperture/logs");
        crate::journal::ensure_private_dir(&log_dir)?;
        let log_path = crate::journal::validate_component_path(&log_dir, "ws-hub.log", true)?;
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(log_path)
            .map_err(|_| "E_HUB_LOG_UNAVAILABLE")?;
        let meta = log.metadata().map_err(|_| "E_HUB_LOG_UNAVAILABLE")?;
        if !meta.is_file()
            || meta.uid() != unsafe { libc::geteuid() }
            || meta.nlink() != 1
            || meta.mode() & 0o077 != 0
        {
            return Err("E_HUB_LOG_UNSAFE".into());
        }
        command
            .stdout(log.try_clone().map_err(|_| "E_HUB_LOG_UNAVAILABLE")?)
            .stderr(log);
        #[cfg(test)]
        command.env("APERTURE_FIXTURE_MODE", &self.spec.fixture_mode);
        // SAFETY: only async-signal-safe setsid in the post-fork child. Failure
        // aborts exec. No captured locks, allocations or parent signal groups.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    Err(std::io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
        drop(free);
        let child = command.spawn().map_err(|_| "E_HUB_SPAWN_UNKNOWN")?;
        let pid = child.id();
        *held_child = Some(child); // Lease retains wait authority, never kill on error/drop.
        self.edge("spawned")?;
        let native = team_process::observe(pid)
            .map_err(|_| UNVERIFIED)?
            .ok_or(UNVERIFIED)?;
        if native.pgid != pid {
            return Err(UNVERIFIED.into());
        }
        let record = registry.record(&reservation, &native.identity, now_ms()?)?;
        self.edge("recorded")?;
        registry.publish_current(&record)?;
        let until = Instant::now() + SNAPSHOT_DEADLINE;
        // Bounded startup wait for this ONE recorded child, not spawn/protocol
        // retries. Failure preserves all intent/history and never signals.
        while presence(self.lease, &native.identity).is_err() {
            if Instant::now() >= until
                || team_process::state(&native.identity) != ProcessState::Same
            {
                return Err(UNVERIFIED.into());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        // No startup retry hides failed proof. Even if bind/presence is delayed,
        // the next explicit reconciliation can only probe this recorded process.
        probe(self.lease, self.spec.endpoint, &native.identity)?;
        Ok(HubObservation {
            identity: native.identity,
            spawned: true,
            exit_observation: "NOT_OBSERVED",
        })
    }
}
// No Supervisor Drop effect or detached authority thread. All mutations are
// synchronous under the borrowed lease; direct-child handles live in that lease.
fn now_ms() -> Result<u64, String> {
    u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| UNVERIFIED)?
            .as_millis(),
    )
    .map_err(|_| UNVERIFIED.into())
}
/// Called only inside the still-fenced aggregate composition. Production uses
/// the identical Supervisor implementation as the inert process fixtures.
pub(crate) fn spawn_ws_hub(lease: &ControllerLock, project_dir: String) -> Result<(), String> {
    let home = lease
        .run_dir()?
        .parent()
        .and_then(Path::parent)
        .ok_or(UNVERIFIED)?
        .to_path_buf();
    let spec = HubSpec {
        command: PathBuf::from(resolve_node()),
        args: vec![Path::new(&project_dir)
            .join("mcp-server/dist/ws-hub.js")
            .into_os_string()],
        endpoint: SocketAddr::from((Ipv4Addr::LOCALHOST, HUB_PORT)),
        home,
        provenance: Provenance::LegacyUnknown,
        #[cfg(test)]
        fault: None,
        #[cfg(test)]
        fixture_mode: String::new(),
    };
    Supervisor::new(lease, spec)?.reconcile().map(|_| ())
}
/// Kept for existing Tauri/server exit callers. No global child/authority thread
/// remains, no daemon signal/unlink/port sweep occurs. Aggregate startup fenced.
pub fn shutdown() {}

fn private_bytes(run: &Path, relative: &str, max: usize) -> Result<Vec<u8>, String> {
    let path =
        crate::journal::validate_component_path(run, relative, false).map_err(|_| UNVERIFIED)?;
    crate::journal::validate_private_file(&path).map_err(|_| UNVERIFIED)?;
    let f = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(|_| UNVERIFIED)?;
    let m = f.metadata().map_err(|_| UNVERIFIED)?;
    if !m.is_file()
        || m.uid() != unsafe { libc::geteuid() }
        || m.nlink() != 1
        || m.mode() & 0o077 != 0
        || m.len() > max as u64
    {
        return Err(UNVERIFIED.into());
    }
    let mut bytes = Vec::new();
    f.take((max + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| UNVERIFIED)?;
    if bytes.len() > max {
        return Err(UNVERIFIED.into());
    }
    Ok(bytes)
}
fn presence(lease: &ControllerLock, expected: &ProcessIdentity) -> Result<(), String> {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Presence {
        hub_pid: u32,
        updated_at: String,
        agents: std::collections::BTreeMap<String, serde_json::Value>,
    }
    let p: Presence = serde_json::from_slice(&private_bytes(
        lease.run_dir()?,
        "presence.json",
        SNAPSHOT_TOTAL_BYTES,
    )?)
    .map_err(|_| UNVERIFIED)?;
    if p.hub_pid != expected.pid
        || p.agents.len() > SNAPSHOT_ENTRIES
        || chrono::DateTime::parse_from_rfc3339(&p.updated_at).is_err()
        || team_process::state(expected) != ProcessState::Same
    {
        return Err(UNVERIFIED.into());
    }
    Ok(())
}
/// Caps bytes before tungstenite allocates handshake/frame storage and applies
/// an absolute deadline to EVERY syscall (slow trickle cannot reset timeout).
const HEADER_BYTES: usize = 8192;
#[derive(Default)]
struct HeaderBuffer {
    bytes: Vec<u8>,
    delivered: usize,
    complete: bool,
}
impl HeaderBuffer {
    // Framing only: tungstenite remains the sole HTTP/upgrade parser. Buffer
    // one finite header so its small-packet AttackCheck stays enabled without
    // mistaking our adapter's artificial one-byte reads for an attack.
    fn push(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        if self.complete || bytes.len() > HEADER_BYTES.saturating_sub(self.bytes.len()) {
            return Err(std::io::ErrorKind::InvalidData.into());
        }
        self.bytes.extend_from_slice(bytes);
        if let Some(at) = self.bytes.windows(4).position(|v| v == b"\r\n\r\n") {
            if at + 4 != self.bytes.len() {
                return Err(std::io::ErrorKind::InvalidData.into());
            }
            self.complete = true;
        } else if self.bytes.len() == HEADER_BYTES {
            return Err(std::io::ErrorKind::InvalidData.into());
        }
        Ok(())
    }
    fn copy_to(&mut self, out: &mut [u8]) -> usize {
        let n = out.len().min(self.bytes.len() - self.delivered);
        out[..n].copy_from_slice(&self.bytes[self.delivered..self.delivered + n]);
        self.delivered += n;
        n
    }
}
fn remaining_time(until: Instant, now: Instant) -> std::io::Result<Duration> {
    until
        .checked_duration_since(now)
        .filter(|v| !v.is_zero())
        .ok_or_else(|| std::io::ErrorKind::TimedOut.into())
}
struct BoundedIo {
    tcp: TcpStream,
    until: Instant,
    remaining: usize,
    handshake: bool,
    header: HeaderBuffer,
}
impl BoundedIo {
    fn timeout(&self) -> std::io::Result<()> {
        let left = remaining_time(self.until, Instant::now())?;
        self.tcp.set_read_timeout(Some(left))?;
        self.tcp.set_write_timeout(Some(left))
    }
}
impl Read for BoundedIo {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        self.timeout()?;
        if b.is_empty() {
            return Ok(0);
        }
        if self.handshake {
            while !self.header.complete {
                self.timeout()?; // The same absolute deadline, not a fresh one.
                let mut chunk = [0; 4096];
                let max = chunk.len().min(HEADER_BYTES - self.header.bytes.len());
                let got = self.tcp.read(&mut chunk[..max])?;
                if got == 0 {
                    return Err(std::io::ErrorKind::UnexpectedEof.into());
                }
                self.header.push(&chunk[..got])?;
            }
            self.timeout()?;
            return Ok(self.header.copy_to(b));
        }
        if self.remaining == 0 {
            return Err(std::io::ErrorKind::InvalidData.into());
        }
        let n = b.len().min(self.remaining);
        let got = self.tcp.read(&mut b[..n])?;
        self.remaining -= got;
        Ok(got)
    }
}
impl Write for BoundedIo {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.timeout()?;
        self.tcp.write(b)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.timeout()?;
        self.tcp.flush()
    }
}
fn no_pending_bytes(stream: &TcpStream) -> Result<(), String> {
    let mut b = 0u8;
    let n = unsafe {
        libc::recv(
            stream.as_raw_fd(),
            (&mut b as *mut u8).cast(),
            1,
            libc::MSG_PEEK | libc::MSG_DONTWAIT,
        )
    };
    if n == -1 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::WouldBlock {
        Ok(())
    } else {
        Err(UNVERIFIED.into())
    }
}
fn probe(
    lease: &ControllerLock,
    endpoint: SocketAddr,
    expected: &ProcessIdentity,
) -> Result<(), String> {
    presence(lease, expected).map_err(|_| "E_HUB_PRESENCE")?;
    let started = Instant::now();
    let tcp =
        TcpStream::connect_timeout(&endpoint, SNAPSHOT_DEADLINE).map_err(|_| "E_HUB_CONNECT")?;
    let io = BoundedIo {
        tcp,
        until: started + SNAPSHOT_DEADLINE,
        remaining: 8192,
        handshake: true,
        header: HeaderBuffer::default(),
    };
    // Fixed request without Authorization/cookie/query/hello. No redirect/TLS or
    // HTTP client that might inject ambient credentials or follow a proxy.
    let config = tungstenite::protocol::WebSocketConfig {
        max_message_size: Some(SNAPSHOT_FRAME_BYTES),
        max_frame_size: Some(SNAPSHOT_FRAME_BYTES),
        write_buffer_size: 0,
        max_write_buffer_size: 8192,
        ..Default::default()
    };
    let (mut ws, _) =
        tungstenite::client::client_with_config(format!("ws://{endpoint}/"), io, Some(config))
            .map_err(|_| "E_HUB_UPGRADE")?;
    if !ws.get_ref().header.complete
        || ws.get_ref().header.delivered != ws.get_ref().header.bytes.len()
    {
        return Err("E_HUB_UPGRADE_BUFFER".into());
    }
    team_process::verify_tcp_server_binding(expected, endpoint, &ws.get_ref().tcp)?;
    no_pending_bytes(&ws.get_ref().tcp).map_err(|_| "E_HUB_EARLY_OR_CLOSED")?;
    // Gate above is before even reading the bearer, not only before sending it.
    let token = private_bytes(lease.run_dir()?, "hub-tokens/watchdog.token", 64)
        .map_err(|_| "E_HUB_TOKEN_UNAVAILABLE")?;
    if token.len() != 64 || !token.iter().all(u8::is_ascii_hexdigit) {
        return Err(UNVERIFIED.into());
    }
    let hello = serde_json::json!({"type":"hello","role":"subscriber","agent":"watchdog","token":String::from_utf8(token).map_err(|_| UNVERIFIED)?});
    ws.get_mut().handshake = false;
    ws.get_mut().remaining = SNAPSHOT_TOTAL_BYTES + SNAPSHOT_FRAMES * 14;
    ws.send(tungstenite::Message::Text(hello.to_string()))
        .map_err(|_| UNVERIFIED)?;
    let mut decoder = SnapshotDecoder::new(started);
    loop {
        decoder
            .check_deadline(Instant::now())
            .map_err(|_| UNVERIFIED)?;
        let frame = ws.read().map_err(|_| "E_HUB_SNAPSHOT_READ")?;
        if let Some(done) = decoder
            .feed(&frame, Instant::now())
            .map_err(|_| UNVERIFIED)?
        {
            if done.claimed_hub_pid != expected.pid {
                return Err(UNVERIFIED.into());
            }
            break;
        }
    }
    // Reject buffered duplicate/unsolicited frames and close before post-check.
    // Live presence racing here is conservatively Unverified, never authority.
    ws.get_mut()
        .tcp
        .set_nonblocking(true)
        .map_err(|_| UNVERIFIED)?;
    match ws.read() {
        Err(tungstenite::Error::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => {}
        _ => return Err("E_HUB_POST_FRAME_OR_CLOSED".into()),
    }
    team_process::verify_tcp_server_binding(expected, endpoint, &ws.get_ref().tcp)?;
    presence(lease, expected).map_err(|_| "E_HUB_PRESENCE")?;
    lease.verify_live()?;
    Ok(())
}

// Native replacement consumes the existing watchdog control seam only.
#[path = "team_revoke_native.rs"]
pub(crate) mod managed_control;

// B1 protocol completion only. Deliberately not wired into legacy supervision:
// server identity/adoption needs the independent B2 binding contract.
const SNAPSHOT_FRAME_BYTES: usize = 4096;
const SNAPSHOT_TOTAL_BYTES: usize = 256 * 1024;
const SNAPSHOT_ENTRIES: usize = 256;
const SNAPSHOT_FRAMES: usize = 512;
const SNAPSHOT_DEADLINE: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SnapshotCompletion {
    pub claimed_hub_pid: u32,
    pub entries: usize,
}
#[derive(serde::Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
enum SnapshotFrame {
    #[serde(rename = "presence")]
    Presence {
        agent: String,
        event: String,
        ts: String,
    },
    #[serde(rename = "subscriber_snapshot_end")]
    End {
        protocol_version: u32,
        hub_pid: u32,
        snapshot_count: usize,
    },
}

/// One initial snapshot on one fresh connection. Callers must register their
/// frame consumer BEFORE sending hello. Output is an untrusted PID claim, not
/// proof of peer identity, authentication, readiness, or adoption authority.
pub(crate) struct SnapshotDecoder {
    started: std::time::Instant,
    bytes: usize,
    frames: usize,
    agents: std::collections::HashSet<String>,
    complete: bool,
    failed: Option<&'static str>,
}
impl SnapshotDecoder {
    pub(crate) fn new(started: std::time::Instant) -> Self {
        Self {
            started,
            bytes: 0,
            frames: 0,
            agents: Default::default(),
            complete: false,
            failed: None,
        }
    }
    /// Time check for an otherwise silent peer. No frame, timeout, or successful
    /// hello send can synthesize SnapshotCompletion.
    pub(crate) fn check_deadline(&mut self, now: std::time::Instant) -> Result<(), &'static str> {
        if let Some(error) = self.failed {
            return Err(error);
        }
        if !self.complete
            && (now < self.started || now.duration_since(self.started) >= SNAPSHOT_DEADLINE)
        {
            self.failed = Some("E_HUB_SNAPSHOT_TIMEOUT");
            return Err("E_HUB_SNAPSHOT_TIMEOUT");
        }
        Ok(())
    }
    pub(crate) fn feed(
        &mut self,
        message: &tungstenite::Message,
        now: std::time::Instant,
    ) -> Result<Option<SnapshotCompletion>, &'static str> {
        let result = self.decode(message, now);
        if let Err(error) = result {
            self.failed = Some(error);
        }
        result
    }
    fn decode(
        &mut self,
        message: &tungstenite::Message,
        now: std::time::Instant,
    ) -> Result<Option<SnapshotCompletion>, &'static str> {
        self.check_deadline(now)?;
        if self.complete {
            return Err("E_HUB_SNAPSHOT_AFTER_END");
        }
        let bytes = match message {
            tungstenite::Message::Text(v) => v.len(),
            tungstenite::Message::Ping(v) | tungstenite::Message::Pong(v) => v.len(),
            _ => return Err("E_HUB_SNAPSHOT_FRAME"),
        };
        // Includes control messages: an endless ping stream cannot evade budget.
        if bytes > SNAPSHOT_FRAME_BYTES
            || self.frames >= SNAPSHOT_FRAMES
            || self
                .bytes
                .checked_add(bytes)
                .is_none_or(|n| n > SNAPSHOT_TOTAL_BYTES)
        {
            return Err("E_HUB_SNAPSHOT_LIMIT");
        }
        self.frames += 1;
        self.bytes += bytes;
        let tungstenite::Message::Text(text) = message else {
            return Ok(None);
        };
        let frame: SnapshotFrame =
            serde_json::from_str(text).map_err(|_| "E_HUB_SNAPSHOT_FRAME")?;
        match frame {
            SnapshotFrame::Presence { agent, event, ts } => {
                if agent.is_empty()
                    || agent.len() > 128
                    || agent.chars().any(char::is_control)
                    || !matches!(event.as_str(), "join" | "busy" | "idle")
                    || ts.len() > 64
                    || chrono::DateTime::parse_from_rfc3339(&ts).is_err()
                    || self.agents.len() >= SNAPSHOT_ENTRIES
                    || !self.agents.insert(agent)
                {
                    return Err("E_HUB_SNAPSHOT_ENTRY");
                }
                Ok(None)
            }
            SnapshotFrame::End {
                protocol_version,
                hub_pid,
                snapshot_count,
            } => {
                if protocol_version != 1 {
                    return Err("E_HUB_SNAPSHOT_VERSION");
                }
                if hub_pid <= 1 || hub_pid > i32::MAX as u32 || snapshot_count != self.agents.len()
                {
                    return Err("E_HUB_SNAPSHOT_END");
                }
                self.complete = true;
                Ok(Some(SnapshotCompletion {
                    claimed_hub_pid: hub_pid,
                    entries: snapshot_count,
                }))
            }
        }
    }
}

#[cfg(test)]
#[path = "ws_hub_tests.rs"]
mod snapshot_tests;
