use crate::codex_appserver;
use crate::config;
use crate::hub_auth;
use crate::launcher;
use crate::state::AppState;
use crate::tmux;
use std::collections::HashMap;
use std::fs;
use std::process::Command;
use std::sync::{Arc, Mutex};

use crate::state::AgentDef;

/// One assignee's resolved current-work summary (aperture-nr65b).
struct CurrentTask {
    id: String,
    title: String,
    extra_count: u32,
}

/// Resolve every agent's current-work summary from BEADS in a single `bd`
/// invocation — one process spawn per `list_agents` poll (every 3s), not one
/// per agent. Groups all `in_progress` beads by assignee, sorts each group by
/// `started_at` descending (ISO 8601 strings sort correctly as plain text),
/// and keeps the top one + a count of the rest.
///
/// Returns `None` if the query itself failed (bd not on PATH, bad JSON,
/// non-zero exit) — list_agents must never fail just because the
/// work-summary line couldn't be resolved this cycle, but it DOES need to
/// tell "query failed, no data" apart from "query succeeded, this assignee
/// simply has nothing in_progress" (the latter is a real, common state —
/// e.g. no one has anything claimed right now — and must render as "idle,"
/// not as "no data available," which is a materially different frontend
/// outcome). `Some(map)` with an assignee absent from the map means idle;
/// `None` means suppress the summary line entirely, same as before this
/// feature shipped.
fn resolve_current_tasks() -> Option<HashMap<String, CurrentTask>> {
    let output = Command::new("bd")
        .args(["list", "--status=in_progress", "--json", "--no-pager", "--limit", "0"])
        .env(
            "PATH",
            format!(
                "/opt/homebrew/bin:/usr/local/bin:{}",
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .output();

    let output = match output {
        Ok(o) if o.status.success() => o,
        Ok(o) => {
            eprintln!(
                "[aperture] warn: `bd list --status=in_progress` exited non-zero ({}): {}",
                o.status,
                String::from_utf8_lossy(&o.stderr)
            );
            return None;
        }
        Err(e) => {
            eprintln!("[aperture] warn: failed to spawn `bd` for current-task resolution: {}", e);
            return None;
        }
    };

    let issues: Vec<serde_json::Value> = match serde_json::from_slice(&output.stdout) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[aperture] warn: failed to parse `bd list --json` output: {}", e);
            return None;
        }
    };

    // Group by assignee, tracking (started_at, id, title) so we can sort
    // each group without a second pass.
    let mut by_assignee: HashMap<String, Vec<(String, String, String)>> = HashMap::new();
    for issue in &issues {
        let assignee = issue.get("assignee").and_then(|v| v.as_str());
        let id = issue.get("id").and_then(|v| v.as_str());
        let title = issue.get("title").and_then(|v| v.as_str());
        let started_at = issue.get("started_at").and_then(|v| v.as_str()).unwrap_or("");
        if let (Some(assignee), Some(id), Some(title)) = (assignee, id, title) {
            by_assignee
                .entry(assignee.to_string())
                .or_default()
                .push((started_at.to_string(), id.to_string(), title.to_string()));
        }
    }

    Some(
        by_assignee
            .into_iter()
            .map(|(assignee, mut tasks)| {
                // Most-recently-claimed first.
                tasks.sort_by(|a, b| b.0.cmp(&a.0));
                let extra_count = (tasks.len() - 1) as u32;
                let (_, id, title) = tasks.into_iter().next().expect("group is never empty");
                (assignee, CurrentTask { id, title, extra_count })
            })
            .collect(),
    )
}

#[tauri::command]
pub fn start_agent(name: String, state: tauri::State<'_, Arc<Mutex<AppState>>>) -> Result<(), String> {
    // Extract all needed data while holding the lock briefly, then release it
    // before doing any expensive I/O (subprocess calls, file writes). This
    // prevents the global state mutex from blocking list_agents polling and
    // other commands for the full duration of agent startup.
    let (agent, tmux_session, mcp_server_path, mcp_sentry_server_path, project_dir) = {
        let app_state = state.lock().map_err(|e| e.to_string())?;
        let agent = app_state
            .agents
            .get(&name)
            .ok_or(format!("Agent '{}' not found", name))?
            .clone();

        if agent.status == "running" {
            return Err(format!("Agent '{}' is already running", name));
        }

        (
            agent,
            app_state.tmux_session.clone(),
            app_state.mcp_server_path.clone(),
            app_state.mcp_sentry_server_path.clone(),
            app_state.project_dir.clone(),
        )
    }; // ← mutex released here; all I/O below is lock-free

    let window_id = boot_agent_process(
        &agent,
        tmux_session,
        mcp_server_path,
        mcp_sentry_server_path,
        project_dir,
    )?;

    // Re-acquire lock only to write the final status
    {
        let mut app_state = state.lock().map_err(|e| e.to_string())?;
        let agent_mut = app_state.agents.get_mut(&name).unwrap();
        agent_mut.tmux_window_id = Some(window_id);
        agent_mut.status = "running".into();
    }

    Ok(())
}

/// GUI-free spawn core (aperture-syepg). Creates the tmux window, writes the
/// per-agent MCP config + launcher script, fires the launcher (baking the
/// static Claude kickoff / the Codex resume-gate), and returns the tmux window
/// id. Shared by the Tauri `start_agent` command and the headless
/// `aperture-boot` bin (aperture-xt16e L3 harness / watchdog aperture-wul6m).
/// Plain data in — no AppState / mutex — which is what makes it callable
/// headlessly. The env knobs (APERTURE_CLAUDE_BIN / APERTURE_CODEX_BIN /
/// APERTURE_LAUNCHER_PATH_PREFIX) are read here so the headless path honors
/// them identically to the GUI path.
pub fn boot_agent_process(
    agent: &AgentDef,
    tmux_session: String,
    mcp_server_path: String,
    mcp_sentry_server_path: String,
    project_dir: String,
) -> Result<String, String> {
    let name = agent.name.clone();

    // Create a dedicated tmux window for this agent
    let window_id = tmux::tmux_create_window(tmux_session, name.clone())?;
    // From this point onward every failure must remove the already-created
    // window. Otherwise the watchdog sees a half-booted agent and respawns it
    // repeatedly, leaving orphan panes behind.
    let boot_result = (|| -> Result<String, String> {

    // Ensure agent's mailbox directory exists
    let mailbox_dir = format!("{}/.aperture/mailbox", std::env::var("HOME").unwrap_or_else(|_| "/tmp".into()));
    let _ = fs::create_dir_all(format!("{}/{}", mailbox_dir, name));

    let home_dir = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let palace_path = format!("{}/.aperture/mempalace", home_dir);

    // aperture-ktwoy — forward the Dolt sql-server password to the MCP's `bd`
    // calls. Needed only when ~/.aperture/.beads is configured for server mode
    // (post-migration); harmless empty default while still embedded. Sourced
    // from the Tauri process env (operator exports BEADS_DOLT_PASSWORD before
    // launching Aperture). host/port/user/database live in the per-machine
    // .beads/config.yaml, not here.
    let beads_dolt_password = std::env::var("BEADS_DOLT_PASSWORD").unwrap_or_default();

    // aperture-xt16e — test/ops knobs for the generated launcher scripts.
    // Unset in normal operation, which keeps the generated output
    // byte-identical to the historical inline templates. The env reads live
    // here (side-effecting layer); the pure builders in launcher.rs only see
    // plain values.
    let claude_bin = std::env::var("APERTURE_CLAUDE_BIN").unwrap_or_else(|_| "claude".into());
    let pane_codex_bin = std::env::var("APERTURE_CODEX_BIN").unwrap_or_else(|_| "codex".into());
    let launcher_path_prefix = std::env::var("APERTURE_LAUNCHER_PATH_PREFIX").ok();
    let hub_token_path = hub_auth::provision_token(&name)?;
    let hub_token_path = hub_token_path.to_string_lossy().into_owned();

    let mcp_config = serde_json::json!({
        "mcpServers": {
            "aperture-bus": {
                "type": "stdio",
                "command": "node",
                "args": [&mcp_server_path],
                "env": {
                    "AGENT_NAME": &name,
                    "AGENT_ROLE": &agent.role,
                    "AGENT_MODEL": &agent.model,
                    "APERTURE_MAILBOX": &mailbox_dir,
                    "BEADS_DIR": format!("{}/.aperture/.beads", home_dir),
                    "BD_ACTOR": &name,
                    "BEADS_DOLT_PASSWORD": &beads_dolt_password,
                    "APERTURE_HUB_TOKEN_FILE": &hub_token_path
                }
            },
            // Sentry MCP wrap layer — enforces Cipher's 9 constraints from
            // aperture-ttzz (allowlist, mutation/attachment approval, audit
            // emission, token redaction). If mcp-server-sentry/dist is not
            // built yet, the agent's MCP client will fail to start `sentry`
            // and other tools (aperture-bus, mempalace) still work.
            "sentry": {
                "type": "stdio",
                "command": "node",
                "args": [&mcp_sentry_server_path],
                "env": {
                    "AGENT_NAME": &name,
                    "AGENT_ROLE": &agent.role,
                    "AGENT_MODEL": &agent.model,
                    "HOME": &home_dir,
                    "BEADS_DIR": format!("{}/.aperture/.beads", home_dir),
                    "BD_ACTOR": &name
                }
            },
            "mempalace": {
                "type": "stdio",
                "command": "/usr/bin/python3",
                "args": ["-m", "mempalace.mcp_server", "--palace", &palace_path],
                "env": {
                    "MEMPALACE_WING": &name
                }
            }
        }
    });

    let launcher_path = format!("/tmp/aperture-launch-{}.sh", name);
    let launcher_script = if agent.model.starts_with("codex/") {
        let bare_model = agent.model.trim_start_matches("codex/");
        let codex_home = format!("/tmp/aperture-codex-{}", name);
        let config_toml_path = format!("{}/config.toml", codex_home);

        let beads_dir = format!("{}/.aperture/.beads", std::env::var("HOME").unwrap_or_else(|_| "/tmp".into()));
        fs::create_dir_all(&codex_home).map_err(|e| e.to_string())?;

        // Codex discovers skills from $CODEX_HOME/skills. Mirror the same
        // manifest-selected runtime links that Claude Code receives under
        // ~/.claude/aperture/<agent>/skills; CODEX_HOME is per-agent and /tmp
        // is recreated after reboot, so this must happen at every launch.
        let codex_skill_count = crate::agent_loader::populate_codex_skill_home(&name, &codex_home)?;
        eprintln!(
            "[aperture] linked {} native Codex skills for '{}'",
            codex_skill_count, name
        );

        // aperture-kc7lb: Codex reads its ChatGPT login from $CODEX_HOME/auth.json.
        // The per-agent CODEX_HOME lives in /tmp (wiped on reboot) and is created
        // fresh above, so without seeding it from the operator's canonical
        // ~/.codex/auth.json every codex agent boots to the sign-in screen — and
        // a pane-side login only writes to /tmp, so it recurs after every reboot.
        // Seed when the source exists and the dest is missing or older: a fresh
        // operator login propagates on next launch, while a newer in-agent token
        // refresh isn't clobbered by a stale central file. Non-fatal on failure —
        // the agent still launches, it just shows the sign-in screen.
        let central_auth = format!("{}/.codex/auth.json", home_dir);
        let agent_auth = format!("{}/auth.json", codex_home);
        let seed_auth = match (fs::metadata(&central_auth), fs::metadata(&agent_auth)) {
            (Ok(src), Ok(dst)) => matches!(
                (src.modified(), dst.modified()),
                (Ok(s), Ok(d)) if s > d
            ),
            (Ok(_), Err(_)) => true,
            (Err(_), _) => false,
        };
        if seed_auth {
            if let Err(e) = fs::copy(&central_auth, &agent_auth) {
                eprintln!(
                    "[aperture] warning: failed to seed codex auth.json for {}: {}",
                    name, e
                );
            }
        }

        // Copy prompt into codex_home so the path is always correct.
        let prompt_content = fs::read_to_string(&agent.prompt_file)
            .map_err(|e| format!("Failed to read prompt file '{}': {}", agent.prompt_file, e))?;
        // Resident/lazy split (aperture-i7bg0): only resident.txt skills get
        // full bodies in prompt.md; the rest ride Codex's native catalog
        // populated from $CODEX_HOME/skills above.
        let prompt_content = inject_codex_skills(prompt_content, &name);
        // Codex has no SessionStart/PreCompact hook system (unlike Claude
        // Code — see .claude/settings.json), so the shared bd-memory bank
        // must be mirrored into the static prompt manually here.
        let prompt_content = inject_bd_memory(prompt_content, &beads_dir, &name);
        // Comms Layer v2 (docs/superpowers/specs/2026-07-19-comms-layer-v2-design.md):
        // unread messages are replayed by the aperture-bus codex-bridge over
        // the app-server socket; nothing is prepended to the prompt here.
        // (Historical: the pre-v2 codex_harness prompt-injection path was
        // deleted in Phase 3.)
        let prompt_dest = format!("{}/prompt.md", codex_home);
        fs::write(&prompt_dest, &prompt_content).map_err(|e| e.to_string())?;

        // Comms Layer v2, Phase 2: aperture-bus is launched through
        // mcp-server/start.sh (resolved from project_dir, same as the other
        // mcp paths in config.rs). start.sh requires AGENT_NAME in the env
        // and exec's dist/index.js, which also reads AGENT_ROLE/AGENT_MODEL/
        // APERTURE_MAILBOX plus the BEADS vars — so those are kept.
        let bus_start_sh = format!("{}/mcp-server/start.sh", project_dir);
        let config_toml = launcher::build_codex_config_toml(&launcher::CodexConfigParams {
            bare_model,
            prompt_dest: &prompt_dest,
            project_dir: &project_dir,
            bus_start_sh: &bus_start_sh,
            mcp_sentry_server_path: &mcp_sentry_server_path,
            name: &name,
            role: &agent.role,
            model: &agent.model,
            mailbox_dir: &mailbox_dir,
            beads_dir: &beads_dir,
            beads_dolt_password: &beads_dolt_password,
            home_dir: &home_dir,
            hub_token_path: &hub_token_path,
        });
        fs::write(&config_toml_path, &config_toml).map_err(|e| e.to_string())?;

        // Comms Layer v2, Phase 2 (spec §Protocol 2): spawn the supervised
        // `codex app-server --listen unix://~/.aperture/run/<name>.sock`
        // BEFORE the pane launches, so `codex --remote` has a live socket to
        // attach to. The app-server carries CODEX_HOME (model + MCP wiring
        // live in config.toml above); the pane is just the interactive TUI.
        // aperture-syepg: clear any stale exact-id handoff file from a previous
        // session before (re)spawning, so the launcher's thread-id wait gate
        // can't read a dead UUID. The codex-bridge (Rex, PR #34) re-writes it
        // (mode 0600) after it binds the fresh kickoff thread. Path mirrors the
        // socket: <run>/<name>.thread-id.
        let _ = fs::remove_file(format!("{}/.aperture/run/{}.thread-id", home_dir, name));

        let sock_path = codex_appserver::spawn_app_server(&name, &codex_home)?;

        launcher::build_codex_launcher(
            &pane_codex_bin,
            launcher_path_prefix.as_deref(),
            &codex_home,
            &sock_path,
        )
    } else {
        let config_path = format!("/tmp/aperture-mcp-{}.json", name);
        fs::write(
            &config_path,
            serde_json::to_string_pretty(&mcp_config).unwrap(),
        )
        .map_err(|e| e.to_string())?;

        // Read prompt and inject agent-specific skills.
        // Claude-backed agents inject every skills.txt body: for them,
        // skills.txt IS the resident set (aperture-auane) and lazy skills
        // ride Claude Code's native .claude/skills discovery. resident.txt
        // is a Codex-path concept (aperture-i7bg0) and is NOT consulted
        // here — warn loudly rather than let it be silently ignored.
        if crate::agent_loader::load_agent_resident_list(&name).is_some() {
            eprintln!(
                "[aperture] warning: agent '{}' has a resident.txt but runs on a Claude model; \
                 resident.txt is only honored on the Codex path. Trim agents/{}/skills.txt instead.",
                name, name
            );
        }
        let prompt_content = fs::read_to_string(&agent.prompt_file)
            .map_err(|e| format!("Failed to read prompt file '{}': {}", agent.prompt_file, e))?;
        let prompt_content = inject_skills(prompt_content, &name);
        let prompt_path = format!("/tmp/aperture-prompt-{}.md", name);
        fs::write(&prompt_path, &prompt_content).map_err(|e| e.to_string())?;

        launcher::build_claude_launcher(
            &claude_bin,
            launcher_path_prefix.as_deref(),
            &project_dir,
            &prompt_path,
            &agent.model,
            &config_path,
            &name,
            true, // fresh_session — resume support arrives with aperture-syepg
        )
    };
    fs::write(&launcher_path, &launcher_script).map_err(|e| e.to_string())?;

    std::process::Command::new("chmod")
        .args(["+x", &launcher_path])
        .output()
        .map_err(|e| e.to_string())?;

    tmux::tmux_send_keys(window_id.clone(), launcher_path)?;

    // aperture-syepg: record the kickoff-fired timestamp for Claude — the
    // kickoff positional is baked into the launcher we just fired, so this
    // instant is turn-1 fire. Feeds the presence dots (aperture-8gypy) + the
    // watchdog re-kick (aperture-wul6m). Codex writes its OWN .kickoff stamp
    // from the codex-bridge at bind time (aperture-3x136) — an earlier version
    // of this comment claimed the bridge did so via PR #34, but it never
    // actually wrote the file, which left every codex dot permanently grey.
    // Shared file: ~/.aperture/run/<name>.kickoff = unix-epoch millis (ASCII).
    if !agent.model.starts_with("codex/") {
        let millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let run_dir = format!("{}/.aperture/run", home_dir);
        let _ = fs::create_dir_all(&run_dir);
        let _ = fs::write(format!("{}/{}.kickoff", run_dir, name), millis.to_string());
    }

    // Comms Layer v2 (docs/superpowers/specs/2026-07-19-comms-layer-v2-design.md):
    // outbound Codex comms flow through the aperture-bus MCP server (wired
    // into config.toml above); inbound delivery is injected by the bus
    // codex-bridge via the app-server socket. (Historical: the pre-v2
    // codex_harness pane-scraping monitor was deleted in Phase 3.)

    // Auto-confirm the workspace trust prompt — but ONLY when the dialog is
    // actually visible. Sending Enter blindly at fixed intervals would stomp
    // on whatever the user is typing in the terminal (the agent window is
    // focused right after creation). Instead, poll pane content every 500ms
    // and send Enter exactly once when the trust prompt appears.
    let window_id_clone = window_id.clone();
    std::thread::spawn(move || {
        // Max 30 polls × 500ms = 15 seconds total timeout
        for _ in 0..30 {
            std::thread::sleep(std::time::Duration::from_millis(500));
            if let Ok(content) = tmux::tmux_capture_pane(&window_id_clone) {
                // Match the actual Claude workspace trust dialog text
                if content.contains("Do you trust the files")
                    || content.contains("Trust workspace")
                    || content.contains("trust the files in")
                {
                    let _ = tmux::tmux_send_keys(window_id_clone.clone(), "".into());
                    break; // sent exactly once — done
                }
                // Claude is already past the trust step — stop polling
                if content.contains("> ") || content.contains("claude>") || content.contains("✓") {
                    break;
                }
            }
        }
    });

    Ok(window_id.clone())
    })();

    if boot_result.is_err() {
        let _ = tmux::tmux_kill_window(window_id);
        codex_appserver::stop_app_server(&name);
    }

    boot_result
}

/// Blocking teardown shared by `stop_agent` and `restart_agent`. Call with NO
/// AppState lock held (sleeps ~1s). With a window id: interrupt, `/exit`, kill
/// the window. Always: stop any supervised codex app-server, remove the
/// kickoff file, and clear the watchdog's in-memory state — see the comments
/// inline. Does NOT touch AppState; callers write `status`/`tmux_window_id`
/// themselves under a fresh lock.
fn teardown_agent(name: &str, window_id: Option<String>) {
    if let Some(window_id) = window_id {
        let _ = tmux::tmux_send_keys(window_id.clone(), "C-c".into());
        std::thread::sleep(std::time::Duration::from_millis(500));
        let _ = tmux::tmux_send_keys(window_id.clone(), "/exit".into());
        std::thread::sleep(std::time::Duration::from_millis(500));
        let _ = tmux::tmux_kill_window(window_id);
    }

    // Comms Layer v2, Phase 2: kill this agent's supervised codex app-server
    // (no-op for Claude agents, which never register one).
    codex_appserver::stop_app_server(name);

    // aperture-wul6m: clear watchdog eligibility for a DELIBERATE stop so it is
    // never fought. Removing the kickoff file drops the agent below the
    // "expected-present" gate (eligibility = running window + kickoff file);
    // on_agent_stopped also clears any in-memory presence/attempt state so a
    // later restart begins from a clean slate.
    {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        let _ = fs::remove_file(format!("{}/.aperture/run/{}.kickoff", home, name));
        crate::watchdog::on_agent_stopped(name);
    }
}

#[tauri::command]
pub fn stop_agent(name: String, state: tauri::State<'_, Arc<Mutex<AppState>>>) -> Result<(), String> {
    // Extract needed data and release the lock before the blocking sleep calls
    let (window_id_opt, is_running) = {
        let app_state = state.lock().map_err(|e| e.to_string())?;
        let agent = app_state
            .agents
            .get(&name)
            .ok_or(format!("Agent '{}' not found", name))?;

        (agent.tmux_window_id.clone(), agent.status == "running")
    }; // ← mutex released here

    if !is_running {
        return Err(format!("Agent '{}' is not running", name));
    }

    teardown_agent(&name, window_id_opt);

    // Re-acquire to update status
    {
        let mut app_state = state.lock().map_err(|e| e.to_string())?;
        let agent_mut = app_state.agents.get_mut(&name).unwrap();
        agent_mut.tmux_window_id = None;
        agent_mut.status = "stopped".into();
    }

    Ok(())
}

/// Restart an agent regardless of whether it is currently alive
/// (aperture-ull4y). The gap this closes: `stop_agent` errors with "not
/// running" on a crashed/exited agent and `start_agent` errors with "already
/// running" on a live one, so the launcher had no single action for "bring
/// this agent back." Liveness is decided by the SAME tmux probe `list_agents`
/// uses (a real window running claude/codex/node), not the cached `status`,
/// which can lag a crash by up to one poll cycle.
///
/// Running → full stop sequence (C-c, /exit, kill window) then boot.
/// Not running → skip the tmux teardown WITHOUT erroring, still run the
/// idempotent cleanup (codex app-server, kickoff file, watchdog state) so a
/// crash's leftovers can't leak into the fresh boot, then boot.
///
/// Lock discipline mirrors `start_agent`: snapshot under the lock, release for
/// every blocking step (tmux probe, teardown sleeps, boot), re-lock only to
/// write the outcome.
#[tauri::command]
pub fn restart_agent(name: String, state: tauri::State<'_, Arc<Mutex<AppState>>>) -> Result<(), String> {
    let (agent, tmux_session, mcp_server_path, mcp_sentry_server_path, project_dir) = {
        let app_state = state.lock().map_err(|e| e.to_string())?;
        let agent = app_state
            .agents
            .get(&name)
            .ok_or(format!("Agent '{}' not found", name))?
            .clone();
        (
            agent,
            app_state.tmux_session.clone(),
            app_state.mcp_server_path.clone(),
            app_state.mcp_sentry_server_path.clone(),
            app_state.project_dir.clone(),
        )
    }; // ← mutex released here; all I/O below is lock-free

    // Same liveness probe as list_agents. If tmux itself can't be listed,
    // fall back to the cached status/window id rather than refusing to act.
    let live_window: Option<String> = match tmux::tmux_list_windows(tmux_session.clone()) {
        Ok(windows) => find_running_window(&windows, &name).map(|w| w.window_id.clone()),
        Err(_) => agent.tmux_window_id.clone().filter(|_| agent.status == "running"),
    };

    if live_window.is_some() {
        teardown_agent(&name, live_window);
    } else {
        eprintln!("[aperture] restart_agent: '{}' is not running — skipping stop, booting fresh", name);
        teardown_agent(&name, None);
    }

    // Reflect the stopped state before the (possibly failing) boot so a boot
    // failure never leaves a phantom "running" with a dead window id.
    {
        let mut app_state = state.lock().map_err(|e| e.to_string())?;
        if let Some(agent_mut) = app_state.agents.get_mut(&name) {
            agent_mut.tmux_window_id = None;
            agent_mut.status = "stopped".into();
        }
    }

    let window_id = boot_agent_process(
        &agent,
        tmux_session,
        mcp_server_path,
        mcp_sentry_server_path,
        project_dir,
    )?;

    {
        let mut app_state = state.lock().map_err(|e| e.to_string())?;
        let agent_mut = app_state
            .agents
            .get_mut(&name)
            .ok_or(format!("Agent '{}' not found", name))?;
        agent_mut.tmux_window_id = Some(window_id);
        agent_mut.status = "running".into();
    }

    Ok(())
}

/// The one liveness probe: an agent is running iff its tmux session has a
/// window named after it whose foreground command is claude/codex/node.
/// Shared by `list_agents` (every 3s poll) and `restart_agent`.
fn find_running_window<'a>(windows: &'a [tmux::WindowInfo], agent_name: &str) -> Option<&'a tmux::WindowInfo> {
    windows.iter().find(|window| {
        window.name == agent_name
            && (window.command == "claude"
                || window.command.contains("claude")
                || window.command == "codex"
                || window.command.contains("codex")
                || window.command == "node")
    })
}

#[tauri::command]
pub fn list_agents(state: tauri::State<'_, Arc<Mutex<AppState>>>) -> Result<Vec<AgentDef>, String> {
    let mut app_state = state.lock().map_err(|e| e.to_string())?;

    // Cross-reference with actual tmux windows to detect agents started outside the UI
    if let Ok(windows) = tmux::tmux_list_windows(app_state.tmux_session.clone()) {
        for agent in app_state.agents.values_mut() {
            let running_window = find_running_window(&windows, &agent.name);

            if let Some(window) = running_window {
                agent.status = "running".into();
                agent.tmux_window_id = Some(window.window_id.clone());
            } else {
                agent.status = "stopped".into();
                agent.tmux_window_id = None;
            }
        }
    }

    // Current-work summary line (aperture-nr65b). One `bd` spawn per poll
    // cycle, not one per agent — see resolve_current_tasks. Only meaningful
    // for a running agent; a stopped agent's fields are cleared rather than
    // left showing stale work from before it stopped.
    //
    // resolve_current_tasks distinguishes "query failed" (None — suppress
    // the summary line entirely, same as before this feature shipped) from
    // "query succeeded, this assignee just has nothing in_progress" (a real,
    // common state that must render as "idle," not as missing data). The
    // sentinel for idle is current_task_id = Some("") with no title — see
    // the doc comment on AgentDef::current_task_id in state.rs.
    let current_tasks = resolve_current_tasks();
    for agent in app_state.agents.values_mut() {
        if agent.status != "running" {
            agent.current_task_id = None;
            agent.current_task_title = None;
            agent.current_task_extra_count = None;
            continue;
        }
        match &current_tasks {
            None => {
                // bd query failed this cycle — no data, render nothing extra.
                agent.current_task_id = None;
                agent.current_task_title = None;
                agent.current_task_extra_count = None;
            }
            Some(tasks) => match tasks.get(&agent.name) {
                Some(task) => {
                    agent.current_task_id = Some(task.id.clone());
                    agent.current_task_title = Some(task.title.clone());
                    agent.current_task_extra_count = Some(task.extra_count);
                }
                // Query succeeded; this agent has no in_progress bead — idle.
                None => {
                    agent.current_task_id = Some(String::new());
                    agent.current_task_title = None;
                    agent.current_task_extra_count = Some(0);
                }
            },
        }
    }

    Ok(app_state.agents.values().cloned().collect())
}

/// Why an attention badge is being lit (aperture-ull4y). Serialized onto
/// `AgentDef.attention_reason` as `"message"` / `"crash"`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum AttentionReason {
    /// The agent rang the operator doorbell (`send_message(to: "operator")`).
    Message,
    /// The watchdog latched red after exhausting its re-kick budget.
    Crash,
}

impl AttentionReason {
    fn as_str(self) -> &'static str {
        match self {
            AttentionReason::Message => "message",
            AttentionReason::Crash => "crash",
        }
    }
}

/// Light the attention badge with a reason. Precedence rule: `crash` always
/// wins — a crash latch overwrites a lit `message` badge, but a later message
/// never downgrades a standing `crash` (the operator must still see that the
/// agent is dead, and the doorbell text lives in scrollback regardless).
/// Callers: poller.rs (message), watchdog.rs (crash). `clear_attention` is the
/// only thing that resets it.
pub fn light_attention(agent: &mut AgentDef, reason: AttentionReason) {
    agent.attention = true;
    let already_crash = agent.attention_reason.as_deref() == Some(AttentionReason::Crash.as_str());
    if reason == AttentionReason::Crash || !already_crash {
        agent.attention_reason = Some(reason.as_str().to_string());
    }
}

#[tauri::command]
pub fn clear_attention(
    name: String,
    state: tauri::State<'_, Arc<Mutex<AppState>>>,
) -> Result<(), String> {
    let mut app_state = state.lock().map_err(|e| e.to_string())?;
    if let Some(agent) = app_state.agents.get_mut(&name) {
        agent.attention = false;
        agent.attention_reason = None;
    }
    Ok(())
}

/// Claude aliases the launcher's model picker offers. The frontend catalog
/// (src/components/AgentConfigModal.ts `CLAUDE_MODELS`) is the source of
/// truth for what the UI shows; this array must match it exactly, and the
/// `picker_and_validator_agree` test below parses that file to enforce it.
/// Codex models are accepted by prefix (`codex/<anything non-empty>`) — the
/// picker lists a curated few, the validator deliberately allows the whole
/// family. A bare `codex/` is rejected: it would boot `codex --model ""`.
const CLAUDE_MODEL_ALIASES: [&str; 4] = ["opus", "sonnet", "haiku", "fable"];

pub fn is_valid_model(model: &str) -> bool {
    CLAUDE_MODEL_ALIASES.contains(&model)
        || model.strip_prefix("codex/").is_some_and(|m| !m.is_empty())
}

#[tauri::command]
pub fn update_agent_model(
    name: String,
    model: String,
    state: tauri::State<'_, Arc<Mutex<AppState>>>,
) -> Result<(), String> {
    if !is_valid_model(&model) {
        return Err(format!(
            "Invalid model '{}'. Must be one of {} or codex/<model>",
            model,
            CLAUDE_MODEL_ALIASES.join("/")
        ));
    }

    let mut app_state = state.lock().map_err(|e| e.to_string())?;
    let agent = app_state
        .agents
        .get_mut(&name)
        .ok_or(format!("Agent '{}' not found", name))?;

    agent.model = model.clone();

    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    config::save_agent_override(&home, &name, &model);

    Ok(())
}

/// Read every skill under `~/.claude/aperture/<agent>/skills/` and append its
/// contents to the prompt. Skills are loaded in deterministic alphabetical
/// order (see `agent_loader::load_agent_skills`). The on-disk layout is built
/// by `just setup`; the canonical sources live in the repo at
/// `agents/<name>/skills.txt` and `.claude/skills/<name>/`.
pub fn inject_skills(prompt: String, agent_name: &str) -> String {
    let skills = crate::agent_loader::load_agent_skills(agent_name);
    if skills.is_empty() {
        eprintln!(
            "[aperture] warn: no skills found for agent '{}' under \
             ~/.claude/aperture/{}/skills/ — did you run `just setup`?",
            agent_name, agent_name
        );
        return prompt;
    }
    let names: Vec<&str> = skills.iter().map(|(n, _)| n.as_str()).collect();
    eprintln!(
        "[aperture] loading {} skills for '{}': {:?}",
        skills.len(),
        agent_name,
        names
    );
    append_skill_bodies(prompt, skills)
}

/// Codex variant of `inject_skills`, honoring the optional resident/lazy
/// split (aperture-i7bg0). Codex natively surfaces a lazy `## Skills`
/// catalog from `$CODEX_HOME/skills` — the same directory
/// `populate_codex_skill_home` links on every launch — and reads a skill's
/// full SKILL.md on demand when a task matches its description. So full-body
/// prompt injection is only needed for the small "resident" subset of
/// always-active behavioral norms listed in
/// `~/.claude/aperture/<agent>/resident.txt`.
///
/// When resident.txt is absent, ALL skill bodies are injected exactly as
/// before — rollout is opt-in per agent, zero behavior change without the
/// file. Resident names with no matching skill dir are warned about and
/// skipped (warn-don't-fail). Claude agents never take this path; their lazy
/// pool is Claude Code's own `.claude/skills` discovery.
pub fn inject_codex_skills(prompt: String, agent_name: &str) -> String {
    let Some(resident) = crate::agent_loader::load_agent_resident_list(agent_name) else {
        // No resident.txt — inject every skill body, today's behavior.
        return inject_skills(prompt, agent_name);
    };
    let skills = crate::agent_loader::load_agent_skills(agent_name);
    for name in &resident {
        if !skills.iter().any(|(n, _)| n == name) {
            eprintln!(
                "[aperture] warn: resident.txt for '{}' names unknown skill \
                 '{}' (not under ~/.claude/aperture/{}/skills/) — skipped",
                agent_name, name, agent_name
            );
        }
    }
    let total = skills.len();
    let resident_skills: Vec<(String, String)> = skills
        .into_iter()
        .filter(|(name, _)| resident.iter().any(|r| r == name))
        .collect();
    eprintln!(
        "[aperture] codex skills for '{}': {} resident injected, {} lazy (native catalog)",
        agent_name,
        resident_skills.len(),
        total - resident_skills.len()
    );
    append_skill_bodies(prompt, resident_skills)
}

fn append_skill_bodies(mut prompt: String, skills: Vec<(String, String)>) -> String {
    for (skill_name, content) in skills {
        prompt.push_str(&format!("\n\n---\n# Skill: {}\n\n{}", skill_name, content));
    }
    prompt
}

/// Claude Code agents get the shared `bd remember` memory bank for free via
/// the `SessionStart`/`PreCompact` hooks in `.claude/settings.json`, which
/// run `bd prime` and inject its output into context automatically. Codex
/// has no equivalent hook system — it only reads a static
/// `model_instructions_file` written once at boot — so without this, every
/// Codex-backed agent (Rex/Scout/Cipher as of 2026-07) boots with zero
/// memory context even though `bd` itself is fully wired for them (same
/// BEADS_DIR/BD_ACTOR env as Claude agents get). Manually shell out to the
/// same `bd prime` command and append its output, so both backends see the
/// identical memory bank at boot. Failure here must not fail agent boot —
/// worst case the agent starts without memories, same as before this fix.
pub fn inject_bd_memory(mut prompt: String, beads_dir: &str, agent_name: &str) -> String {
    let output = std::process::Command::new("bd")
        .arg("prime")
        .env("BEADS_DIR", beads_dir)
        .env("BD_ACTOR", agent_name)
        .output();
    match output {
        Ok(out) if out.status.success() => {
            let text = String::from_utf8_lossy(&out.stdout);
            if text.trim().is_empty() {
                eprintln!(
                    "[aperture] warning: `bd prime` returned empty output for '{}'",
                    agent_name
                );
            } else {
                eprintln!(
                    "[aperture] injected bd prime memory bank ({} bytes) for '{}'",
                    text.len(),
                    agent_name
                );
                prompt.push_str(&format!(
                    "\n\n---\n# Beads Memory Bank (bd prime, mirrored for Codex — no hook system)\n\n{}",
                    text
                ));
            }
        }
        Ok(out) => {
            eprintln!(
                "[aperture] warning: `bd prime` exited non-zero for '{}': {}",
                agent_name,
                String::from_utf8_lossy(&out.stderr)
            );
        }
        Err(e) => {
            eprintln!(
                "[aperture] warning: failed to run `bd prime` for '{}': {}",
                agent_name, e
            );
        }
    }
    prompt
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent() -> AgentDef {
        AgentDef {
            name: "vance".into(),
            model: "fable".into(),
            role: "builder".into(),
            prompt_file: String::new(),
            tmux_window_id: None,
            status: "running".into(),
            emoji: None,
            attention: false,
            attention_reason: None,
            turn_state: None,
            current_task_id: None,
            current_task_title: None,
            current_task_extra_count: None,
            dot_state: None,
            dot_state_since: None,
            kickoff_fired_at: None,
        }
    }

    // ---- aperture-84bby: picker <-> validator alignment ----

    /// The Claude aliases the picker offers, parsed from the frontend source
    /// at test time: every `value: "<alias>"` inside the `CLAUDE_MODELS`
    /// literal (stops at the first `]`). Codex entries live in a separate
    /// literal and are covered by the prefix rule, not by this list.
    fn picker_claude_aliases() -> Vec<String> {
        let src = include_str!("../../src/components/AgentConfigModal.ts");
        let start = src
            .find("const CLAUDE_MODELS = [")
            .expect("AgentConfigModal.ts: CLAUDE_MODELS literal not found");
        let body = &src[start..];
        let end = body.find(']').expect("CLAUDE_MODELS literal not closed");
        body[..end]
            .split("value: \"")
            .skip(1)
            .map(|rest| rest.split('"').next().unwrap().to_string())
            .collect()
    }

    #[test]
    fn picker_and_validator_agree() {
        let mut picker = picker_claude_aliases();
        assert!(!picker.is_empty(), "picker parse returned nothing");
        let mut validator: Vec<String> = CLAUDE_MODEL_ALIASES.iter().map(|s| s.to_string()).collect();
        picker.sort();
        validator.sort();
        assert_eq!(
            picker, validator,
            "AgentConfigModal.ts CLAUDE_MODELS and agents.rs CLAUDE_MODEL_ALIASES drifted"
        );
    }

    #[test]
    fn validator_accepts_every_picker_alias_and_codex_prefix() {
        for alias in picker_claude_aliases() {
            assert!(is_valid_model(&alias), "picker offers {alias} but validator rejects it");
        }
        assert!(is_valid_model("codex/gpt-5.6-sol"));
        assert!(is_valid_model("codex/anything-new"));
        assert!(!is_valid_model("codex/"));
        assert!(!is_valid_model("gpt-5.6-sol"));
        assert!(!is_valid_model("claude-opus-4"));
        assert!(!is_valid_model(""));
    }

    // ---- aperture-ull4y: attention_reason precedence ----

    #[test]
    fn attention_reason_message_lights_badge() {
        let mut a = agent();
        light_attention(&mut a, AttentionReason::Message);
        assert!(a.attention);
        assert_eq!(a.attention_reason.as_deref(), Some("message"));
    }

    #[test]
    fn attention_reason_crash_overwrites_message() {
        let mut a = agent();
        light_attention(&mut a, AttentionReason::Message);
        light_attention(&mut a, AttentionReason::Crash);
        assert!(a.attention);
        assert_eq!(a.attention_reason.as_deref(), Some("crash"));
    }

    #[test]
    fn attention_reason_message_never_downgrades_crash() {
        let mut a = agent();
        light_attention(&mut a, AttentionReason::Crash);
        light_attention(&mut a, AttentionReason::Message);
        assert!(a.attention);
        assert_eq!(a.attention_reason.as_deref(), Some("crash"));
    }

    #[test]
    fn attention_clear_resets_both_fields() {
        // Mirrors clear_attention's body (the command itself needs a Tauri
        // State handle); after a clear, a fresh message lights "message"
        // again — no stale crash precedence survives the clear.
        let mut a = agent();
        light_attention(&mut a, AttentionReason::Crash);
        a.attention = false;
        a.attention_reason = None;
        light_attention(&mut a, AttentionReason::Message);
        assert_eq!(a.attention_reason.as_deref(), Some("message"));
    }

    #[test]
    fn find_running_window_matches_agent_shell_commands_only() {
        let w = |name: &str, command: &str| tmux::WindowInfo {
            window_id: format!("@{}", name),
            name: name.into(),
            command: command.into(),
        };
        let windows = vec![w("vance", "zsh"), w("rex", "codex"), w("izzy", "node"), w("scout", "claude")];
        assert!(find_running_window(&windows, "vance").is_none(), "a bare shell is a dead agent");
        assert_eq!(find_running_window(&windows, "rex").unwrap().window_id, "@rex");
        assert_eq!(find_running_window(&windows, "izzy").unwrap().window_id, "@izzy");
        assert_eq!(find_running_window(&windows, "scout").unwrap().window_id, "@scout");
        assert!(find_running_window(&windows, "ghost").is_none());
    }
}
