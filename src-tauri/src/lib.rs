mod local_package;
mod runtime_release;
mod controller;
mod daemon_registry;
mod daemons;
mod web_auth;
pub mod web_server;
mod team_claude_launch;
mod team_claude_inbox;
mod team_claude_kickoff;
mod team_claude_observation;
mod agent_loader;
mod agents;
mod codex_appserver;
mod config;
mod launcher;
mod hub_auth;
mod journal;
mod owner;
mod team_auth;
mod teams;
mod team_checkpoint;
mod team_replacement;
mod team_process;
mod team_terminal;
mod team_archive;
mod team_archive_finalize;
mod poller;
mod state;
mod tmux;
mod watchdog;
mod ws_hub;

use std::sync::{Arc, Mutex};

/// Private helper entrypoints: identity and authority are derived by the native
/// gate/writer from the current owner, never from argv or environment claims.
pub fn managed_claude_gate(team: &str, seat: &str, generation: u64) -> Result<(), String> {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from)
        .ok_or_else(|| "E_CLAUDE_RUNTIME_IO".to_string())?;
    team_claude_launch::gate_native(&home, team, seat, generation)
        .map_err(|error| error.code().to_string())
}

pub fn managed_claude_observe(team: &str, seat: &str) -> Result<(), String> {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from)
        .ok_or_else(|| "E_CLAUDE_RUNTIME_IO".to_string())?;
    team_claude_observation::write_startup_observation(&home, team, seat, std::io::stdin().lock())
        .map_err(|error| error.code().to_string())
}

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

/// Attach only a terminal client to an existing managed worker.
pub use team_terminal::attach_existing as attach_managed_terminal;

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
    agents::require_legacy_lifecycle(name)?;
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from).ok_or("home unavailable")?;
    let mut initial = config::default_state();
    if initial.agents.get(name).is_some_and(|a| a.model.starts_with("codex/")) {
        return Err(agents::LifecycleRefusal::InputsUnverified.code().into());
    }
    if let Some(session) = std::env::var("APERTURE_TMUX_SESSION").ok().filter(|s| !s.is_empty()) { initial.tmux_session = session; }
    let state = Arc::new(Mutex::new(initial));
    boot_agent_headless_external(&home, name, &state)
}
// Same synchronous external path used by the public binary wrapper and inert
// fixtures. No production executable/environment override is accepted here.
pub(crate) fn boot_agent_headless_external(
    home: &std::path::Path, name: &str, state: &Arc<Mutex<state::AppState>>,
) -> Result<String, String> {
    // Deny Codex input before acquiring/creating controller files when possible.
    let agent = state.lock().map_err(|e| e.to_string())?.agents.get(name).ok_or("agent not found")?.clone();
    if agent.model.starts_with("codex/") {
        return Err(agents::LifecycleRefusal::InputsUnverified.code().into());
    }
    let lease = controller::ControllerLock::acquire(home)?;
    let tools=daemons::LocalTools::resolve(home)?;
    let runtime = daemons::RuntimeOwner::local(lease,tools)?;
    let result = {
        let work = runtime.admit(Some(name))?;
        let _body = work.body()?;
        let context = work.lifecycle(name)?;
        agents::start_agent_shared(name.to_string(), state, &context)?;
        state.lock().map_err(|e| e.to_string())?.agents.get(name)
            .and_then(|v| v.tmux_window_id.clone()).ok_or_else(|| "boot has no window".into())
    };
    runtime.close()?;
    result
}

/// Authenticated, headless V4 team activation. The request carries selectors
/// only; teams.rs derives authority from the fixed canonical GLaDOS bearer.
pub fn team_control_json(input: &str) -> Result<String, String> {
    teams::team_control_headless(input)
        .and_then(|response| serde_json::to_string(&response).map_err(|_| teams::TeamError {
            code: "E_STAGING_IO".into(),
            message: "team control result serialization failed".into(),
        }))
        .map_err(|error| serde_json::to_string(&error).unwrap_or_else(|_| "{\"code\":\"E_STAGING_IO\",\"message\":\"team control failed\"}".into()))
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
    // The fallback GUI must acquire the same lease before any initialization.
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from).expect("home unavailable");
    let _controller = match controller::ControllerLock::acquire(&home) {
        Ok(lease) => lease,
        Err(error) => { eprintln!("[aperture] {error}"); return; }
    };
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

    let tools=match daemons::LocalTools::resolve(std::path::Path::new(&home)){Ok(t)=>t,Err(e)=>{eprintln!("[aperture] local tools: {e}");return;}};
    let runtime = match daemons::RuntimeOwner::local(_controller,tools){Ok(r)=>Arc::new(r),Err(e)=>{eprintln!("[aperture] local runtime: {e}");return;}};
    if let Err(error) = runtime.start(Arc::clone(&app_state)) {
        eprintln!("[aperture] daemon startup refused: {error}");
        return;
    }

    tauri::Builder::default()
        .manage(app_state)
        .manage(runtime)
        .invoke_handler(tauri::generate_handler![
            // Launcher essentials — start/stop/list agents and configure model.
            agents::start_agent,
            agents::stop_agent,
            // Stop-if-alive then boot; works on crashed agents too (aperture-ull4y).
            agents::restart_agent,
            agents::list_agents,
            agents::update_agent_model,
            agents::clear_attention,
            // tmux session bootstrap (used at app startup) and window focus
            // (used by AgentCard click → switch to that agent's window).
            tmux::tmux_create_session,
            tmux::tmux_select_window,
            // Build metadata for the launcher footer (semver + git SHA + build date)
            get_version,
            // Aperture V4 P1 team lifecycle. Activation is intentionally not
            // exposed to Tauri; authenticated GLaDOS control uses the common
            // headless engine in teams.rs.
            teams::team_get_catalog,
            teams::team_list_presets,
            teams::team_save_preset,
            teams::team_create,
            teams::team_list,
            teams::team_cancel_pending,
            teams::team_bootstrap_seat,
            team_terminal::team_open_seat,
            teams::team_prepare_replacement,
            teams::team_start_replacement,
            teams::team_archive,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            // Detach only; D still owes admission closure and task joins.
            // No daemon/port kill or unlink is authority granted by GUI exit.
            if let tauri::RunEvent::ExitRequested { api, .. } = &event {
                use tauri::Manager;
                let runtime = app_handle.state::<Arc<daemons::RuntimeOwner>>();
                if runtime.close().is_err() { api.prevent_exit(); }
            }
            if let tauri::RunEvent::Exit = event {
                use tauri::Manager;
                let runtime = app_handle.state::<Arc<daemons::RuntimeOwner>>();
                if let Err(error) = runtime.close() { eprintln!("[aperture] runtime drain incomplete: {error}"); }
                ws_hub::shutdown();
                codex_appserver::shutdown();
            }
        });
}
