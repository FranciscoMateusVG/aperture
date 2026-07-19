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
/// install locations first, then fall back to PATH resolution.
fn codex_bin() -> String {
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
