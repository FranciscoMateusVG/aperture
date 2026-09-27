//! Codex app-server supervisor — Comms Layer v2, Phase 2.
//!
//! Spec: docs/superpowers/specs/2026-07-19-comms-layer-v2-design.md §Protocol 2
//!
//! Per Codex agent, Tauri spawns `codex app-server --listen
//! unix://~/.aperture/run/<agent>.sock` as a supervised child (mirrors the
//! ws_hub.rs supervision pattern: respawn 2s after any exit, kill on agent
//! stop and on app exit). The agent's tmux pane then runs
//! `codex --remote unix://...` so the TUI stays fully interactive while the
//! aperture-bus codex-bridge connects to the same socket to inject
//! `turn/start` / `turn/steer` message deliveries.

use std::collections::HashMap;
use std::fs;
use std::process::{Child, Command};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

/// Identity-only C1 primitive. Registry construction is explicit caller setup;
/// this probe neither opens a registry nor creates missing paths/facts. A unit
/// result deliberately carries no reusable signal/unlink/adoption authority.
/// Legacy lifecycle functions below remain untouched and MUST NOT call this
/// observation as a substitute for the future C2/C3 operation contract.
pub(crate) fn probe_registered(
    registry: &crate::daemon_registry::Registry<'_>,
    seat: &str,
) -> Result<(), String> {
    crate::team_terminal::verify_registered_legacy_socket(registry, seat)
        .map_err(|_| "E_CODEX_UNVERIFIED: registered Unix identity could not be observed".into())
}

// Internal C2b only. No legacy/UI/managed caller is wired to this supervisor.
pub(crate) struct NativeCodexSpec {
    pub seat: String,
    pub executable: std::path::PathBuf,
    pub codex_home: std::path::PathBuf,
    pub provenance: crate::daemon_registry::Provenance,
    #[cfg(test)]
    pub fixture: Option<(std::path::PathBuf, String)>,
    #[cfg(test)]
    pub fault: Option<String>,
}
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct CodexObservation {
    pub identity: crate::team_replacement::ProcessIdentity,
    pub created: bool,
    pub wait: &'static str,
}
pub(crate) struct NativeCodexSupervisor<'a> {
    lease: &'a crate::controller::ControllerLock,
    registry: &'a crate::daemon_registry::Registry<'a>,
    spec: NativeCodexSpec,
}
#[cfg(test)]
thread_local! { static C2_SIGNAL_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
const CODEX_UNKNOWN: &str = "E_CODEX_OPERATION_UNKNOWN";
// A path pin narrows check/use drift; it is not atomic exec identity proof.
#[derive(Debug, PartialEq, Eq)]
struct CodexExecutablePin {
    dev: u64,
    ino: u64,
    uid: u32,
    mode: u32,
    nlink: u64,
}
impl CodexExecutablePin {
    fn capture(path: &std::path::Path) -> Result<Self, String> {
        use std::os::unix::fs::MetadataExt;
        let m = fs::symlink_metadata(path).map_err(|_| CODEX_UNKNOWN)?;
        if !m.is_file()
            || m.file_type().is_symlink()
            || (m.uid() != unsafe { libc::geteuid() } && m.uid() != 0)
            || m.nlink() != 1
            || m.mode() & 0o111 == 0
            || m.mode() & 0o022 != 0
        {
            return Err(CODEX_UNKNOWN.into());
        }
        Ok(Self {
            dev: m.dev(),
            ino: m.ino(),
            uid: m.uid(),
            mode: m.mode(),
            nlink: m.nlink(),
        })
    }
}
impl<'a> NativeCodexSupervisor<'a> {
    pub(crate) fn new(
        lease: &'a crate::controller::ControllerLock,
        registry: &'a crate::daemon_registry::Registry<'a>,
        spec: NativeCodexSpec,
    ) -> Result<Self, String> {
        lease.verify_live()?;
        if !crate::agent_loader::is_valid_seat_name(&spec.seat)
            || !spec.executable.is_absolute()
            || !spec.codex_home.is_absolute()
            || spec.executable.components().any(|v| {
                !matches!(
                    v,
                    std::path::Component::RootDir | std::path::Component::Normal(_)
                )
            })
            || spec.codex_home.components().any(|v| {
                !matches!(
                    v,
                    std::path::Component::RootDir | std::path::Component::Normal(_)
                )
            })
        {
            return Err(CODEX_UNKNOWN.into());
        }
        // Constructor is read-only. Registry creation is explicit caller setup.
        registry.verify_read_context()?;
        Ok(Self {
            lease,
            registry,
            spec,
        })
    }
    fn socket_resolver(&self) -> Result<crate::team_terminal::CodexSocketResolver, String> {
        #[cfg(test)]
        if let Some((root, _)) = &self.spec.fixture {
            return crate::team_terminal::CodexSocketResolver::fixture(root);
        }
        Ok(crate::team_terminal::CodexSocketResolver::production())
    }
    fn edge(&self, point: &str) -> Result<(), String> {
        #[cfg(test)]
        if self.spec.fault.as_deref() == Some(point) {
            return Err("E_CODEX_FIXTURE_EDGE".into());
        }
        let _ = point;
        Ok(())
    }
    fn snapshot(&self) -> Result<crate::daemon_registry::CodexSnapshot, String> {
        self.registry
            .codex_snapshot(&self.spec.seat)?
            .ok_or_else(|| CODEX_UNKNOWN.into())
    }
    fn now() -> Result<u64, String> {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|t| t.as_millis() as u64)
            .map_err(|_| CODEX_UNKNOWN.into())
    }
    pub(crate) fn reconcile(&self) -> Result<CodexObservation, String> {
        use crate::daemon_registry::{CodexEventV2 as E, CodexPhaseV2 as P, Identity};
        use std::os::unix::{
            fs::{MetadataExt, OpenOptionsExt},
            process::CommandExt,
        };
        use std::process::Stdio;
        let slot = self.lease.codex_slot(&self.spec.seat)?;
        let mut op = slot.enter()?;
        self.registry.verify_operation(&op)?;
        self.registry.validate_namespace()?; // structure, NOT global readiness
        if let Some(snapshot) = self.registry.codex_snapshot(&self.spec.seat)? {
            if snapshot.phase != P::ReadyMetadataOnly || snapshot.provenance != self.spec.provenance
            {
                return Err(CODEX_UNKNOWN.into());
            }
            crate::team_terminal::codex_live_pins(
                self.registry,
                &op,
                &snapshot,
                &self.socket_resolver()?,
            )?;
            return Ok(CodexObservation {
                identity: snapshot.identity.ok_or(CODEX_UNKNOWN)?,
                created: false,
                wait: "NOT_OBSERVED",
            });
        }
        crate::team_terminal::codex_pristine_endpoint(self.registry, &op)?;
        crate::controller::private_dir_readonly(&self.spec.codex_home)?;
        let executable_pin = CodexExecutablePin::capture(&self.spec.executable)?;
        let id = self
            .registry
            .begin_codex_v2(&op, self.spec.provenance.clone(), Self::now()?)?;
        let intent = self.snapshot()?;
        self.edge("intent")?;
        #[cfg(test)]
        registered_tests::c2_fix_executable_drift(&self.spec)?;
        crate::team_terminal::codex_pristine_endpoint(self.registry, &op)?;
        self.registry.recheck_snapshot(&self.spec.seat, &intent)?;
        self.registry.verify_operation(&op)?;
        let run = self.lease.run_dir()?;
        let home = run
            .parent()
            .and_then(std::path::Path::parent)
            .ok_or(CODEX_UNKNOWN)?;
        let socket = run.join(format!("{}.sock", self.spec.seat));
        let mut command = Command::new(&self.spec.executable);
        // No ambient provider/auth/PATH/locale transport in this internal freeze.
        command.env_clear();
        command
            .args(["app-server", "--listen"])
            .arg(format!("unix://{}", socket.display()));
        #[cfg(test)]
        if let Some((root, mode)) = &self.spec.fixture {
            command = Command::new(&self.spec.executable);
            command.env_clear();
            command
                .args([
                    "--exact",
                    "codex_appserver::registered_tests::c2_inert_native_entry",
                    "--ignored",
                    "--nocapture",
                ])
                .env("APERTURE_C2_FIXTURE", root)
                .env("APERTURE_C2_MODE", mode)
                .env("APERTURE_C2_SOCKET", &socket);
        }
        command
            .env("HOME", home)
            .env("CODEX_HOME", &self.spec.codex_home)
            .stdin(Stdio::null());
        // Detached diagnostics use the existing private log convention, only
        // after durable intent. No adopted config/log is rewritten.
        let logs = home.join(".aperture/logs");
        crate::journal::ensure_private_dir(&logs)?;
        let log_path = crate::journal::validate_component_path(
            &logs,
            &format!("codex-{}.log", self.spec.seat),
            true,
        )?;
        let log = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(log_path)
            .map_err(|_| CODEX_UNKNOWN)?;
        let m = log.metadata().map_err(|_| CODEX_UNKNOWN)?;
        if !m.is_file()
            || m.uid() != unsafe { libc::geteuid() }
            || m.nlink() != 1
            || m.mode() & 0o777 != 0o600
        {
            return Err(CODEX_UNKNOWN.into());
        }
        // A FIFO swapped in after path validation must not block open. Only a
        // validated regular descriptor reaches the child, with NONBLOCK cleared.
        use std::os::fd::AsRawFd;
        let flags = unsafe { libc::fcntl(log.as_raw_fd(), libc::F_GETFL) };
        if flags < 0
            || unsafe { libc::fcntl(log.as_raw_fd(), libc::F_SETFL, flags & !libc::O_NONBLOCK) }
                != 0
            || unsafe { libc::fcntl(log.as_raw_fd(), libc::F_GETFL) } != flags & !libc::O_NONBLOCK
        {
            return Err(CODEX_UNKNOWN.into());
        }
        command
            .stdout(log.try_clone().map_err(|_| CODEX_UNKNOWN)?)
            .stderr(log);
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 {
                    Err(std::io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
        // Earliest spawn boundary: retained slot plus durable unchanged intent.
        crate::team_terminal::codex_pristine_endpoint(self.registry, &op)?;
        crate::controller::private_dir_readonly(&self.spec.codex_home)?;
        self.registry.recheck_snapshot(&self.spec.seat, &intent)?;
        self.registry.verify_operation(&op)?;
        #[cfg(test)]
        if self.spec.fault.as_deref() == Some("post-spawn-retained") {
            op.fail_next_spawn_observation();
        }
        if CodexExecutablePin::capture(&self.spec.executable)? != executable_pin {
            return Err(CODEX_UNKNOWN.into());
        }
        let identity = op
            .spawn_retained(&mut command)
            .map_err(|e| format!("{CODEX_UNKNOWN}: {e:?}"))?;
        self.edge("spawned")?;
        self.registry.append_codex_v2(
            &op,
            &id,
            E::Spawned {
                process: Identity::from_native(&identity)?,
            },
            Self::now()?,
        )?;
        self.edge("recorded")?;
        let spawned = self.snapshot()?;
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        let pins = loop {
            self.registry.recheck_snapshot(&self.spec.seat, &spawned)?;
            self.registry.verify_operation(&op)?;
            if crate::team_process::state(&identity) != crate::team_replacement::ProcessState::Same
            {
                let _ = op.try_wait();
                return Err(CODEX_UNKNOWN.into());
            }
            match crate::team_terminal::codex_live_pins(
                self.registry,
                &op,
                &spawned,
                &self.socket_resolver()?,
            ) {
                Ok(pins) => break pins,
                Err(_) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                Err(_) => return Err(CODEX_UNKNOWN.into()),
            }
        };
        self.registry
            .append_codex_v2(&op, &id, E::SocketReady { pins }, Self::now()?)?;
        self.edge("ready")?;
        self.registry.publish_codex_v2(&op, &id)?;
        self.edge("published")?;
        let current = self.snapshot()?;
        crate::team_terminal::codex_live_pins(
            self.registry,
            &op,
            &current,
            &self.socket_resolver()?,
        )?;
        Ok(CodexObservation {
            identity,
            created: true,
            wait: "DIRECT_CHILD_RETAINED",
        })
    }
    fn term_once(&self, pid: u32) -> crate::daemon_registry::TermResultV2 {
        use crate::daemon_registry::TermResultV2 as T;
        #[cfg(test)]
        match self.spec.fault.as_deref() {
            Some("term-esrch-injected") => return T::Esrch,
            Some("term-error-injected") => return T::OtherError,
            _ => {}
        }
        #[cfg(test)]
        C2_SIGNAL_CALLS.with(|v| v.set(v.get() + 1));
        let rc = unsafe { libc::kill(pid as i32, libc::SIGTERM) };
        let errno = std::io::Error::last_os_error().raw_os_error();
        if rc == 0 { T::ReturnedZero } else if errno == Some(libc::ESRCH) { T::Esrch } else { T::OtherError }
    }
    fn stop_observation(&self, op: &mut crate::controller::CodexOperation<'_>,
        identity: &crate::team_replacement::ProcessIdentity) -> Result<crate::team_replacement::ProcessState, String>
    {
        use crate::team_replacement::ProcessState as P;
        #[cfg(test)]
        match self.spec.fault.as_deref() {
            Some("term-timeout-injected") => return Ok(P::Same),
            Some("term-recycled-injected") => return Ok(P::Recycled),
            Some("term-unreadable-injected") => return Ok(P::Unreadable),
            Some("term-wait-error-injected") => return Err("E_CODEX_WAIT_UNKNOWN".into()),
            _ => {}
        }
        op.try_wait()?;
        Ok(crate::team_process::state(identity))
    }
    pub(crate) fn stop(
        &self,
        incarnation: &str,
    ) -> Result<crate::daemon_registry::StopOutcomeV2, String> {
        use crate::daemon_registry::{
            CodexEventV2 as E, CodexPhaseV2 as P, StopOutcomeV2 as O, TermResultV2 as T,
        };
        use crate::team_replacement::ProcessState;
        let slot = self.lease.codex_slot(&self.spec.seat)?;
        let mut op = slot.enter()?;
        self.registry.verify_operation(&op)?;
        self.registry.validate_namespace()?;
        let before = self.snapshot()?;
        if before.incarnation != incarnation || before.phase != P::ReadyMetadataOnly {
            return Err(CODEX_UNKNOWN.into());
        }
        crate::team_terminal::codex_live_pins(
            self.registry,
            &op,
            &before,
            &self.socket_resolver()?,
        )?;
        self.registry
            .append_codex_v2(&op, incarnation, E::StopIntent {}, Self::now()?)?;
        self.edge("stop-intent")?;
        let intent = self.snapshot()?;
        crate::team_terminal::codex_live_pins(
            self.registry,
            &op,
            &intent,
            &self.socket_resolver()?,
        )?;
        self.registry.recheck_snapshot(&self.spec.seat, &intent)?;
        self.registry.verify_operation(&op)?;
        let identity = intent.identity.as_ref().ok_or(CODEX_UNKNOWN)?;
        if crate::team_process::state(identity) != ProcessState::Same {
            return Err(CODEX_UNKNOWN.into());
        }
        let raw = self.term_once(identity.pid);
        self.edge("term-sent")?;
        self.registry.append_codex_v2(
            &op,
            incarnation,
            E::TermResult {
                result: raw.clone(),
            },
            Self::now()?,
        )?;
        let sent = self.snapshot()?;
        let mut outcome = O::Unknown;
        if raw == T::ReturnedZero {
            let wait_budget = Duration::from_secs(3);
            #[cfg(test)]
            let wait_budget = if self.spec.fault.as_deref() == Some("term-timeout-injected") { Duration::ZERO } else { wait_budget };
            let deadline = std::time::Instant::now() + wait_budget;
            loop {
                self.registry.recheck_snapshot(&self.spec.seat, &sent)?;
                let observed = match self.stop_observation(&mut op, identity) {
                    Ok(observed) => observed,
                    Err(_) => break,
                };
                match observed {
                    ProcessState::Gone => {
                        outcome = O::TermSentThenDaemonGoneDescendantsUnverified;
                        break;
                    }
                    ProcessState::Same if std::time::Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(10))
                    }
                    _ => break,
                }
            }
        }
        self.registry.append_codex_v2(
            &op,
            incarnation,
            E::StopOutcome {
                outcome: outcome.clone(),
            },
            Self::now()?,
        )?;
        Ok(outcome)
    }
    pub(crate) fn cleanup(
        &self,
        incarnation: &str,
    ) -> Result<crate::daemon_registry::CleanupOutcomeV2, String> {
        use crate::daemon_registry::{CleanupOutcomeV2 as O, CodexEventV2 as E, CodexPhaseV2 as P};
        let slot = self.lease.codex_slot(&self.spec.seat)?;
        let op = slot.enter()?;
        self.registry.verify_operation(&op)?;
        self.registry.validate_namespace()?;
        let before = self.snapshot()?;
        if before.incarnation != incarnation
            || before.phase != P::TermSentThenDaemonGoneDescendantsUnverified
        {
            return Err(CODEX_UNKNOWN.into());
        }
        self.registry
            .append_codex_v2(&op, incarnation, E::CleanupIntent {}, Self::now()?)?;
        self.edge("cleanup-intent")?;
        let intent = self.snapshot()?;
        let outcome = crate::team_terminal::codex_cleanup_fixed(
            self.registry,
            &op,
            &intent,
            &self.socket_resolver()?,
        )
        .unwrap_or(O::Unknown);
        self.edge("cleanup-effect")?;
        self.registry.append_codex_v2(
            &op,
            incarnation,
            E::CleanupOutcome {
                outcome: outcome.clone(),
            },
            Self::now()?,
        )?;
        Ok(outcome)
    }
}

#[cfg(test)]
#[path = "codex_appserver_tests.rs"]
mod registered_tests;

/// Set by `shutdown()`; tells every supervisor loop to stop respawning.
static SHUTTING_DOWN: AtomicBool = AtomicBool::new(false);

/// One supervised app-server. `stop` is per-agent (set by `stop_app_server`);
/// `child` is shared between the supervisor thread (try_wait polling) and the
/// stop/shutdown paths (kill).
struct ServerHandle {
    stop: AtomicBool,
    child: Mutex<Option<Child>>,
}

fn servers() -> &'static Mutex<HashMap<String, Arc<ServerHandle>>> {
    static SERVERS: OnceLock<Mutex<HashMap<String, Arc<ServerHandle>>>> = OnceLock::new();
    SERVERS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn home_dir() -> String {
    std::env::var("HOME").unwrap_or_else(|_| "/tmp".into())
}

/// PATH for the spawned app-server. The Tauri app may have been launched from
/// Finder with a minimal PATH, so prepend the usual codex install locations.
fn path_env() -> String {
    let home = home_dir();
    let current = std::env::var("PATH").unwrap_or_default();
    format!(
        "{}/.npm-global/bin:{}/.local/bin:/opt/homebrew/bin:/usr/local/bin:{}",
        home, home, current
    )
}

/// Resolve the codex binary the same way poller.rs resolves `bd`: known
/// install locations first, then fall back to PATH resolution. The
/// APERTURE_CODEX_BIN env var (aperture-xt16e) overrides everything and is
/// used verbatim — no existence probe.
fn codex_bin() -> String {
    if let Ok(bin) = std::env::var("APERTURE_CODEX_BIN") {
        if !bin.is_empty() {
            return bin;
        }
    }
    let home = home_dir();
    let candidates = [
        format!("{}/.npm-global/bin/codex", home),
        "/opt/homebrew/bin/codex".to_string(),
        "/usr/local/bin/codex".to_string(),
    ];
    for path in &candidates {
        if std::path::Path::new(path).exists() {
            return path.clone();
        }
    }
    "codex".to_string()
}

/// Unix socket the app-server listens on and the TUI/bridge connect to.
pub fn socket_path(agent_name: &str) -> String {
    format!("{}/.aperture/run/{}.sock", home_dir(), agent_name)
}

/// Spawn (or reuse) the supervised app-server for `agent_name`. Returns the
/// socket path for the pane's `codex --remote unix://...` command.
///
/// `codex_home` is the per-agent /tmp/aperture-codex-<name> dir whose
/// config.toml carries model, approval policy, and MCP wiring — the
/// app-server (the actual engine behind `--remote`) reads it via CODEX_HOME.
pub fn spawn_app_server(agent_name: &str, codex_home: &str) -> Result<String, String> {
    let sock = socket_path(agent_name);
    let run_dir = format!("{}/.aperture/run", home_dir());
    fs::create_dir_all(&run_dir)
        .map_err(|e| format!("Failed to create {}: {}", run_dir, e))?;

    // Cross-process spawn-if-absent (aperture-r8n62): the in-process `servers()`
    // map is always empty in a fresh `aperture-boot` process, so a stale map is
    // no evidence that the socket is free. Probe the sock directly — if a
    // healthy app-server (from a prior boot or the running launcher) already
    // owns it, reuse it instead of deleting the sock and stacking an orphan.
    if std::os::unix::net::UnixStream::connect(&sock).is_ok() {
        println!(
            "[aperture] codex app-server for '{}' already live on {} — reusing (no respawn)",
            agent_name, sock
        );
        return Ok(sock);
    }

    let handle = {
        let mut map = servers().lock().map_err(|e| e.to_string())?;
        if let Some(existing) = map.get(&agent_name.to_string()) {
            if !existing.stop.load(Ordering::SeqCst) {
                // Supervisor already running for this agent — reuse it.
                return Ok(sock);
            }
            // Stale stopped handle (shouldn't normally persist) — replace it.
            map.remove(agent_name);
        }
        let handle = Arc::new(ServerHandle {
            stop: AtomicBool::new(false),
            child: Mutex::new(None),
        });
        map.insert(agent_name.to_string(), Arc::clone(&handle));
        handle
    };

    let name = agent_name.to_string();
    let codex_home = codex_home.to_string();
    let sock_for_thread = sock.clone();

    std::thread::spawn(move || {
        loop {
            if SHUTTING_DOWN.load(Ordering::SeqCst) || handle.stop.load(Ordering::SeqCst) {
                break;
            }

            // Delete any stale socket file before (re)spawning — a leftover
            // sock from a crashed app-server would make --listen fail.
            let _ = fs::remove_file(&sock_for_thread);

            match Command::new(codex_bin())
                .args(["app-server", "--listen", &format!("unix://{}", sock_for_thread)])
                .env("CODEX_HOME", &codex_home)
                .env("PATH", path_env())
                .spawn()
            {
                Ok(child) => {
                    println!(
                        "[aperture] codex app-server for '{}' started (pid {}) on {}",
                        name,
                        child.id(),
                        sock_for_thread
                    );
                    if let Ok(mut guard) = handle.child.lock() {
                        *guard = Some(child);
                    }
                }
                Err(e) => {
                    eprintln!(
                        "[aperture] warn: failed to spawn codex app-server for '{}' ({})",
                        name, e
                    );
                }
            }

            // Poll the child until it exits (or is taken by stop/shutdown).
            // Same try_wait-on-interval pattern as ws_hub.rs — we can't block
            // in Child::wait() while the handle sits in the mutex.
            loop {
                std::thread::sleep(Duration::from_millis(500));
                let Ok(mut guard) = handle.child.lock() else { break };
                match guard.as_mut() {
                    Some(child) => match child.try_wait() {
                        Ok(Some(status)) => {
                            eprintln!(
                                "[aperture] codex app-server for '{}' exited ({}) — respawning in 2s",
                                name, status
                            );
                            *guard = None;
                            break;
                        }
                        Ok(None) => {} // still running
                        Err(e) => {
                            eprintln!(
                                "[aperture] codex app-server for '{}' wait error ({}) — respawning in 2s",
                                name, e
                            );
                            *guard = None;
                            break;
                        }
                    },
                    // stop_app_server()/shutdown() took and killed the child.
                    None => break,
                }
            }

            if SHUTTING_DOWN.load(Ordering::SeqCst) || handle.stop.load(Ordering::SeqCst) {
                break;
            }
            std::thread::sleep(Duration::from_secs(2));
        }

        let _ = fs::remove_file(&sock_for_thread);
    });

    Ok(sock)
}

/// Kill the app-server for one agent and stop its supervisor loop. Called
/// from `agents.rs::stop_agent()`; a no-op for agents without an app-server.
pub fn stop_app_server(agent_name: &str) {
    let handle = {
        let Ok(mut map) = servers().lock() else { return };
        map.remove(agent_name)
    };
    if let Some(handle) = handle {
        handle.stop.store(true, Ordering::SeqCst);
        if let Ok(mut guard) = handle.child.lock() {
            if let Some(mut child) = guard.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
        let _ = fs::remove_file(socket_path(agent_name));
    }
}

/// Kill every app-server and stop all supervisor loops. Called from the
/// `tauri::RunEvent::Exit` handler in lib.rs (alongside ws_hub::shutdown) so
/// app-servers never outlive the launcher and squat on their sockets.
pub fn shutdown() {
    SHUTTING_DOWN.store(true, Ordering::SeqCst);
    let Ok(mut map) = servers().lock() else { return };
    for (name, handle) in map.drain() {
        handle.stop.store(true, Ordering::SeqCst);
        if let Ok(mut guard) = handle.child.lock() {
            if let Some(mut child) = guard.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
        let _ = fs::remove_file(socket_path(&name));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    /// aperture-r8n62 regression: when a healthy app-server already owns the
    /// socket, `spawn_app_server` must reuse it — return the sock path WITHOUT
    /// deleting the sock or spawning a duplicate. We stand in for the live
    /// app-server with a bound `UnixListener`; if the reuse path fired, the
    /// listener's socket file survives the call (the spawn path would have
    /// `fs::remove_file`d it). Full spawn-reuse across `aperture-boot` restarts
    /// is integration-tested via the aperture-boot-twice scenario.
    ///
    /// NOTE: `spawn_app_server` derives the sock path from `$HOME`, so this test
    /// mutates the process-global `HOME`. It is the only HOME-mutating test in
    /// this module; keep it that way to avoid cross-test races.
    #[test]
    fn spawn_app_server_reuses_live_socket_without_respawn() {
        // Keep the path short: unix socket paths must fit in SUN_LEN (~104 on
        // macOS), so we can't nest under the long system temp dir.
        let tmp = std::path::PathBuf::from(format!("/tmp/ap-r8n62-{}", std::process::id()));
        let run = tmp.join(".aperture/run");
        fs::create_dir_all(&run).expect("create temp run dir");

        let prev_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", &tmp);

        let agent = "tc";
        let sock = socket_path(agent);

        // Stand in for a healthy, listening app-server.
        let _listener = UnixListener::bind(&sock).expect("bind fake app-server socket");

        let result = spawn_app_server(agent, "/tmp/does-not-matter");

        // Restore HOME before asserting so a failure can't leak it.
        match prev_home {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }

        assert_eq!(result.as_deref(), Ok(sock.as_str()), "reuse must return the live sock path");
        assert!(
            std::path::Path::new(&sock).exists(),
            "reuse path must NOT delete the live socket"
        );
        // Reuse returns before touching the in-process map — no supervisor registered.
        assert!(
            servers().lock().unwrap().get(agent).is_none(),
            "reuse path must not register a supervisor handle"
        );

        drop(_listener);
        let _ = fs::remove_dir_all(&tmp);
    }
}
