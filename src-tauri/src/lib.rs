mod agent_loader;
mod agents;
mod codex_appserver;
mod config;
mod launcher;
mod hub_auth;
mod poller;
mod state;
mod tmux;
mod watchdog;
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

/// aperture-3x136: GUI-launched apps inherit launchd's minimal PATH
/// (/usr/bin:/bin:/usr/sbin:/sbin) — no volta, no homebrew, no npm-global.
/// Every subprocess resolution downstream then degrades to hardcoded
/// candidate lists that are machine-specific and incomplete: on a volta-only
/// machine `node` resolves nowhere, so the WS hub silently ENOENT-looped and
/// comms died fleet-wide (2026-07-19). Ask the user's login shell for its
/// PATH once at startup and prepend it, so the app resolves binaries exactly
/// like a terminal launch. Bounded by a 3s watchdog so a hung rc file cannot
/// wedge app startup; on any failure we keep the inherited PATH.
fn repair_gui_path() {
    use std::io::Read;
    use std::process::{Command, Stdio};
    let mut child = match Command::new("/bin/zsh")
        .args(["-ilc", "printf %s \"$PATH\""])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[aperture] warn: PATH repair skipped (zsh spawn failed: {e})");
            return;
        }
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                eprintln!("[aperture] warn: PATH repair skipped (login shell timed out)");
                return;
            }
        }
    }
    let mut out = String::new();
    if let Some(mut stdout) = child.stdout.take() {
        let _ = stdout.read_to_string(&mut out);
    }
    let shell_path = out.trim();
    if shell_path.is_empty() {
        eprintln!("[aperture] warn: PATH repair skipped (login shell returned empty PATH)");
        return;
    }
    let current = std::env::var("PATH").unwrap_or_default();
    let mut merged: Vec<&str> = shell_path.split(':').filter(|p| !p.is_empty()).collect();
    for p in current.split(':').filter(|p| !p.is_empty()) {
        if !merged.contains(&p) {
            merged.push(p);
        }
    }
    std::env::set_var("PATH", merged.join(":"));
    println!(
        "[aperture] PATH repaired from login shell ({} entries)",
        merged.len()
    );
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Must run before anything that spawns subprocesses (BEADS init, poller,
    // WS hub, codex app-servers) — they all resolve binaries via PATH.
    repair_gui_path();

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

    if let Err(e) = hub_auth::provision_token("watchdog") {
        eprintln!("[aperture] fatal: cannot provision hub watchdog token: {e}");
        return;
    }

    // Start the aperture-bus WS hub daemon (Comms Layer v2, Phase 1 —
    // docs/superpowers/specs/2026-07-19-comms-layer-v2-design.md). Claude
    // message delivery now flows through this hub instead of the poller's
    // tmux injection. Skips with a warning if ws-hub.js isn't built yet.
    let project_dir = app_state.lock().unwrap().project_dir.clone();
    ws_hub::spawn_ws_hub(project_dir);

    // Start the liveness watchdog (aperture-wul6m) — the agent-side half of
    // comms-v2 reliability. Subscribes to the hub's presence stream and re-kicks
    // any expected-present agent that goes silent past the 60s deadline (a hub
    // bounce killed its exit-on-drop inbox monitor and it couldn't self-heal).
    // Also computes the presence-dot state the launcher polls via list_agents.
    watchdog::spawn_watchdog(Arc::clone(&app_state));

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
