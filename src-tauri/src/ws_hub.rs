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

    std::thread::spawn(move || loop {
        if SHUTTING_DOWN.load(Ordering::SeqCst) {
            break;
        }

        match Command::new("node").arg(&hub_path).spawn() {
            Ok(child) => {
                println!("[aperture] ws-hub started (pid {})", child.id());
                if let Ok(mut guard) = CHILD.lock() {
                    *guard = Some(child);
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
}
