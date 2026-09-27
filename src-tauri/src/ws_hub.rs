//! WS hub daemon supervisor — Comms Layer v2, Phase 1.
//!
//! Spec: docs/superpowers/specs/2026-07-19-comms-layer-v2-design.md
//!
//! The aperture-bus WS hub (`mcp-server/dist/ws-hub.js`) is the delivery
//! transport for Claude agents: it pushes unread BEADS message rows over
//! WebSocket (ws://127.0.0.1:4517) and replays them on reconnect. Tauri is
//! the process supervisor (spec §Architecture): we spawn `node ws-hub.js` at
//! app startup and respawn it 2s after any exit. On app exit `shutdown()`
//! kills the child (wired to `tauri::RunEvent::Exit` in lib.rs) so a stale
//! hub never squats on port 4517 across launches.

use std::process::{Child, Command};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

/// Set by `shutdown()`; tells the supervisor loop to stop respawning.
static SHUTTING_DOWN: AtomicBool = AtomicBool::new(false);

/// Handle to the currently-running hub child, shared between the supervisor
/// thread (which polls it via `try_wait`) and `shutdown()` (which kills it).
static CHILD: Mutex<Option<Child>> = Mutex::new(None);

/// The hub's listen port — kept in sync with `mcp-server/dist/ws-hub.js`.
const HUB_PORT: u16 = 4517;

/// Resolve the REAL `node` binary (aperture-256ru).
///
/// On this machine `node` on PATH is the **Volta shim**, which execs real node
/// as a CHILD and does NOT forward signals. So `Command::new("node")` +
/// `child.kill()` kills the shim while the real hub orphans (reparented to
/// PPID 1) and keeps squatting port 4517 — the next launch then hits
/// EADDRINUSE and silently respawn-loops. This was empirically reproduced in
/// prod (bead notes). Spawning the resolved real binary directly makes OUR
/// child the actual listener, so `child.kill()` reaches it.
///
/// `node -e process.execPath` runs the shim but prints the real binary path it
/// exec'd (e.g. `~/.volta/tools/image/node/<v>/bin/node`), which we spawn
/// directly. `APERTURE_NODE_BIN` overrides everything (test/ops knob, empty
/// ignored, same family as APERTURE_CODEX_BIN in codex_appserver.rs).
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

/// PID of the first process listening on `port`, via `lsof`. `None` when
/// nothing is listening (lsof exits non-zero / empty stdout on no match).
fn port_listener_pid(port: u16) -> Option<u32> {
    let out = Command::new("lsof")
        .args(["-ti", &format!("tcp:{}", port), "-sTCP:LISTEN"])
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()
        .and_then(|l| l.trim().parse::<u32>().ok())
}

/// Spawn the WS hub under a supervisor thread. Mirrors the poller-thread
/// pattern in `lib.rs::run()`. If the compiled hub doesn't exist yet (e.g.
/// `just build-mcp` hasn't been run since ws-hub.ts landed), log a warning
/// and skip — the app must not crash over a missing optional daemon.
pub fn spawn_ws_hub(project_dir: String) {
    let hub_path = format!("{}/mcp-server/dist/ws-hub.js", project_dir);
    if !std::path::Path::new(&hub_path).exists() {
        eprintln!(
            "[aperture] warn: WS hub not found at {} — skipping spawn. \
             Run `just build-mcp` to compile mcp-server (including ws-hub.js).",
            hub_path
        );
        return;
    }

    // Resolve the real node binary ONCE (aperture-256ru) — never spawn via the
    // PATH shim, whose SIGKILL orphans the real hub onto port 4517.
    let node_bin = resolve_node();
    println!("[aperture] ws-hub: resolved node binary → {}", node_bin);

    std::thread::spawn(move || loop {
        if SHUTTING_DOWN.load(Ordering::SeqCst) {
            break;
        }

        // aperture-mler9: capture the hub's structured stderr/stdout to a log
        // file. A GUI-launched app inherits /dev/null stdio, which silently
        // discarded every hub diagnostic (notify_forwarded / notify_offline /
        // bad_hello / replay …) — the 2026-07-19 comms investigation had to
        // reconstruct hub behaviour from probes because the logs were gone.
        let mut cmd = Command::new(&node_bin);
        cmd.arg(&hub_path);
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        let log_dir = format!("{}/.aperture/logs", home);
        let _ = std::fs::create_dir_all(&log_dir);
        if let Ok(log_file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(format!("{}/ws-hub.log", log_dir))
        {
            if let Ok(clone) = log_file.try_clone() {
                cmd.stdout(clone);
            }
            cmd.stderr(log_file);
        } // else: fall back to inherited stdio — never block the spawn on logging.

        match cmd.spawn() {
            Ok(child) => {
                let pid = child.id();
                println!("[aperture] ws-hub started (pid {})", pid);
                if let Ok(mut guard) = CHILD.lock() {
                    *guard = Some(child);
                }
                // aperture-256ru: confirm OUR child actually owns the port. If
                // a stale/orphan hub is squatting 4517, this spawn fails to bind
                // while a DIFFERENT pid keeps serving (possibly stale) — turn
                // that silent EADDRINUSE respawn into a loud error.
                std::thread::sleep(Duration::from_millis(600));
                match port_listener_pid(HUB_PORT) {
                    Some(listener) if listener == pid => {
                        println!("[aperture] ws-hub bound {} (pid {} verified)", HUB_PORT, pid);
                    }
                    Some(listener) => {
                        // aperture-3x136: a DIFFERENT pid holds the port — a
                        // stale/orphan hub (e.g. after a `tauri dev` hot-restart,
                        // which re-execs the binary without firing the clean-exit
                        // residual-kill sweep). Previously we only logged and told
                        // the operator to kill it by hand. Since 4517 is OUR hub
                        // port exclusively, any squatter is always a stale aperture
                        // hub — kill it so the respawn loop can bind. Our own child
                        // (which lost the bind race) already exited(1) on EADDRINUSE
                        // (ws-hub.ts), so the next spawn 2s later takes the freed
                        // port cleanly. Self-heals what used to need a manual kill.
                        eprintln!(
                            "[aperture] ws-hub: pid {} lost the bind race; port {} squatted by stale \
                             hub pid {} (aperture-256ru) — killing the squatter so the respawn binds.",
                            pid, HUB_PORT, listener
                        );
                        let _ = Command::new("kill").args(["-9", &listener.to_string()]).status();
                    }
                    None => {
                        eprintln!(
                            "[aperture] warn: ws-hub pid {} spawned but nothing is listening on {} yet",
                            pid, HUB_PORT
                        );
                    }
                }
            }
            Err(e) => {
                eprintln!("[aperture] warn: failed to spawn ws-hub ({})", e);
            }
        }

        // Poll the child until it exits (or is taken by shutdown()). We can't
        // block in Child::wait() while the handle sits in the mutex, so we
        // try_wait on a short interval instead.
        loop {
            std::thread::sleep(Duration::from_millis(500));
            let Ok(mut guard) = CHILD.lock() else { break };
            match guard.as_mut() {
                Some(child) => match child.try_wait() {
                    Ok(Some(status)) => {
                        eprintln!("[aperture] ws-hub exited ({}) — respawning in 2s", status);
                        *guard = None;
                        break;
                    }
                    Ok(None) => {} // still running
                    Err(e) => {
                        eprintln!("[aperture] ws-hub wait error ({}) — respawning in 2s", e);
                        *guard = None;
                        break;
                    }
                },
                // shutdown() took and killed the child.
                None => break,
            }
        }

        if SHUTTING_DOWN.load(Ordering::SeqCst) {
            break;
        }
        std::thread::sleep(Duration::from_secs(2));
    });
}

/// Kill the hub child and stop the supervisor loop. Called from the
/// `tauri::RunEvent::Exit` handler in `lib.rs` so the hub doesn't outlive
/// the app and hold port 4517 hostage for the next launch.
pub fn shutdown() {
    SHUTTING_DOWN.store(true, Ordering::SeqCst);
    if let Ok(mut guard) = CHILD.lock() {
        if let Some(mut child) = guard.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
    // aperture-256ru: belt-and-suspenders. With the resolved-real-node spawn,
    // child.kill() already reaches the real listener — but a prior orphan (from
    // a pre-fix launch) or a race could still squat the port. Kill whatever
    // holds it so the next launch binds cleanly (lsof empty on 4517 post-exit).
    if let Some(pid) = port_listener_pid(HUB_PORT) {
        eprintln!(
            "[aperture] ws-hub: killing residual listener on {} (pid {})",
            HUB_PORT, pid
        );
        let _ = Command::new("kill").args(["-9", &pid.to_string()]).status();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_node_avoids_the_volta_shim() {
        let n = resolve_node();
        assert!(!n.is_empty(), "resolve_node must never return empty");
        // On a machine with node installed the resolved path must exist and must
        // NOT be a Volta shim (shims live under `.volta/bin/`). On a CI box with
        // no node, resolve_node falls back to the bare "node" string, which we
        // can't validate — skip the existence/shim checks in that case.
        if n != "node" {
            assert!(
                std::path::Path::new(&n).exists(),
                "resolved node binary must exist: {}",
                n
            );
            assert!(
                !n.contains("/.volta/bin/"),
                "resolved node must be the real binary, not the Volta shim: {}",
                n
            );
        }
    }
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
