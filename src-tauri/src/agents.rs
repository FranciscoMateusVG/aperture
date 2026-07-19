use crate::codex_appserver;
use crate::config;
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
                    "BEADS_DOLT_PASSWORD": &beads_dolt_password
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

        // Copy prompt into codex_home so the path is always correct.
        let prompt_content = fs::read_to_string(&agent.prompt_file)
            .map_err(|e| format!("Failed to read prompt file '{}': {}", agent.prompt_file, e))?;
        let prompt_content = inject_skills(prompt_content, &name);
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

        // Read prompt and inject agent-specific skills
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
    // watchdog re-kick (aperture-wul6m). Codex records its own at bridge-inject
    // time (PR #34). Shared file: ~/.aperture/run/<name>.kickoff = unix-epoch
    // millis (ASCII).
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

    Ok(window_id)
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

    if let Some(window_id) = window_id_opt {
        let _ = tmux::tmux_send_keys(window_id.clone(), "C-c".into());
        std::thread::sleep(std::time::Duration::from_millis(500));
        let _ = tmux::tmux_send_keys(window_id.clone(), "/exit".into());
        std::thread::sleep(std::time::Duration::from_millis(500));
        let _ = tmux::tmux_kill_window(window_id);
    }

    // Comms Layer v2, Phase 2: kill this agent's supervised codex app-server
    // (no-op for Claude agents, which never register one).
    codex_appserver::stop_app_server(&name);

    // Re-acquire to update status
    {
        let mut app_state = state.lock().map_err(|e| e.to_string())?;
        let agent_mut = app_state.agents.get_mut(&name).unwrap();
        agent_mut.tmux_window_id = None;
        agent_mut.status = "stopped".into();
    }

    Ok(())
}

#[tauri::command]
pub fn list_agents(state: tauri::State<'_, Arc<Mutex<AppState>>>) -> Result<Vec<AgentDef>, String> {
    let mut app_state = state.lock().map_err(|e| e.to_string())?;

    // Cross-reference with actual tmux windows to detect agents started outside the UI
    if let Ok(windows) = tmux::tmux_list_windows(app_state.tmux_session.clone()) {
        for agent in app_state.agents.values_mut() {
            let running_window = windows.iter().find(|window| {
                window.name == agent.name
                    && (window.command == "claude"
                        || window.command.contains("claude")
                        || window.command == "codex"
                        || window.command.contains("codex")
                        || window.command == "node")
            });

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

#[tauri::command]
pub fn clear_attention(
    name: String,
    state: tauri::State<'_, Arc<Mutex<AppState>>>,
) -> Result<(), String> {
    let mut app_state = state.lock().map_err(|e| e.to_string())?;
    if let Some(agent) = app_state.agents.get_mut(&name) {
        agent.attention = false;
    }
    Ok(())
}

#[tauri::command]
pub fn update_agent_model(
    name: String,
    model: String,
    state: tauri::State<'_, Arc<Mutex<AppState>>>,
) -> Result<(), String> {
    let valid = matches!(model.as_str(), "opus" | "sonnet" | "haiku" | "fable") || model.starts_with("codex/");
    if !valid {
        return Err(format!("Invalid model '{}'. Must be opus/sonnet/haiku/fable or codex/<model>", model));
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
pub fn inject_skills(mut prompt: String, agent_name: &str) -> String {
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
    for (skill_name, content) in skills {
        prompt.push_str(&format!("\n\n---\n# Skill: {}\n\n{}", skill_name, content));
    }
    prompt
}
