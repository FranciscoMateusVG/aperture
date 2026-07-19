mod agent_loader;
mod agents;
mod codex_appserver;
mod config;
mod launcher;
mod poller;
mod state;
mod tmux;
mod ws_hub;

use std::sync::{Arc, Mutex};

/// Returns the version metadata baked into this binary at build time.
/// Three fields: semver from Cargo.toml, short git SHA, and the UTC build
/// date. The launcher footer renders this as `vX.Y.Z · sha · YYYY-MM-DD` so
/// the operator can verify a reinstall actually picked up the latest commit.
#[tauri::command]
fn get_version() -> serde_json::Value {
    serde_json::json!({
        "semver": env!("CARGO_PKG_VERSION"),
        "sha": env!("APERTURE_GIT_SHA"),
        "built_at": env!("APERTURE_BUILD_DATE"),
    })
}

/// Headless boot entry point (aperture-syepg). Boots ONE registered agent by
/// name through the real spawn path (tmux window + launcher + Claude kickoff /
/// Codex resume-gate) with no Tauri GUI and no AppState mutex — that is what
/// makes it callable from CI. The launcher env knobs (APERTURE_CLAUDE_BIN /
/// APERTURE_CODEX_BIN / APERTURE_LAUNCHER_PATH_PREFIX) and the registry
/// override (APERTURE_AGENTS_DIR) apply identically to the GUI path.
/// APERTURE_TMUX_SESSION optionally targets an isolated tmux session (default:
/// the configured "aperture" session — which must already exist, as
/// `tmux_create_window` does not create it). Backs the boot-verification
/// harness (aperture-xt16e) and the watchdog re-kick (aperture-wul6m). Returns
/// the new tmux window id.
pub fn boot_agent_headless(name: &str) -> Result<String, String> {
    let state = config::default_state();
    let agent = state
        .agents
        .get(name)
        .ok_or_else(|| format!("agent '{}' not found in registry (check APERTURE_AGENTS_DIR)", name))?;
    let tmux_session = std::env::var("APERTURE_TMUX_SESSION")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| state.tmux_session.clone());
    agents::boot_agent_process(
        agent,
        tmux_session,
        state.mcp_server_path.clone(),
        state.mcp_sentry_server_path.clone(),
        state.project_dir.clone(),
    )
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app_state = Arc::new(Mutex::new(config::default_state()));

    // Initialize BEADS database
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let beads_dir = format!("{}/.aperture/.beads", home);
    let current_path = std::env::var("PATH").unwrap_or_default();
    let go_bin = format!("{}/go/bin", home);
    let path_env = format!("/opt/homebrew/bin:/usr/local/bin:{}:{}", go_bin, current_path);
    let bd_bin = format!("{}/go/bin/bd", home);

    // Ensure dolt is initialized in .beads dir
    if !std::path::Path::new(&format!("{}/config.json", beads_dir)).exists() {
        let _ = std::fs::create_dir_all(&beads_dir);
        let _ = std::process::Command::new("dolt")
            .arg("init")
            .current_dir(&beads_dir)
            .env("PATH", &path_env)
            .output();
    }

    // Initialize BEADS if not yet done
    // NOTE: dolt server lifecycle is owned by `bd dolt start` — Tauri no longer
    // spawns its own dolt sql-server on port 3307. This was removed to avoid
    // orphaned processes and conflicts with bd's managed server mode.
    {
        let mut cmd = std::process::Command::new(&bd_bin);
        cmd.args(["init", "--quiet"]);
        cmd.env("BEADS_DIR", &beads_dir);
        cmd.env("PATH", &path_env);
        cmd.current_dir(&app_state.lock().unwrap().project_dir);
        match cmd.output() {
            Ok(output) if output.status.success() => {
                println!("BEADS ready at {}", beads_dir);
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                if !stderr.contains("already initialized") {
                    eprintln!("BEADS init warning: {}", stderr);
                }
            }
            Err(e) => {
                eprintln!("BEADS init failed (bd not found?): {}", e);
            }
        }
    }

    // Start the operator-mailbox sweep (attention badges only — agent
    // message delivery is owned by the WS hub / codex-bridge, see poller.rs)
    let poller_state = Arc::clone(&app_state);
    std::thread::spawn(move || {
        poller::run_message_poller(poller_state);
    });

    // Start the aperture-bus WS hub daemon (Comms Layer v2, Phase 1 —
    // docs/superpowers/specs/2026-07-19-comms-layer-v2-design.md). Claude
    // message delivery now flows through this hub instead of the poller's
    // tmux injection. Skips with a warning if ws-hub.js isn't built yet.
    let project_dir = app_state.lock().unwrap().project_dir.clone();
    ws_hub::spawn_ws_hub(project_dir);

    tauri::Builder::default()
        .manage(app_state)
        .invoke_handler(tauri::generate_handler![
            // Launcher essentials — start/stop/list agents and configure model.
            agents::start_agent,
            agents::stop_agent,
            agents::list_agents,
            agents::update_agent_model,
            agents::clear_attention,
            // tmux session bootstrap (used at app startup) and window focus
            // (used by AgentCard click → switch to that agent's window).
            tmux::tmux_create_session,
            tmux::tmux_select_window,
            // Build metadata for the launcher footer (semver + git SHA + build date)
            get_version,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app_handle, event| {
            // Kill the WS hub child on app exit so it doesn't outlive the
            // launcher and hold port 4517 across restarts. Same for the
            // per-agent codex app-servers (Comms v2 Phase 2) so stale
            // processes never squat on ~/.aperture/run/*.sock.
            if let tauri::RunEvent::Exit = event {
                ws_hub::shutdown();
                codex_appserver::shutdown();
            }
        });
}
