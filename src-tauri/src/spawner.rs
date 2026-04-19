use crate::state::{AppState, SpiderlingDef};
use crate::tmux;
use regex::Regex;
use std::fs;
use std::sync::{Arc, Mutex};

const PERMANENT_AGENTS: &[&str] = &["glados", "wheatley", "peppy", "izzy", "vance", "rex", "scout", "cipher", "sage", "atlas", "sentinel", "sterling", "planner", "operator", "warroom"];

fn home() -> String {
    std::env::var("HOME").unwrap_or_else(|_| "/tmp".into())
}

fn path_env() -> String {
    let current = std::env::var("PATH").unwrap_or_default();
    format!("/opt/homebrew/bin:/usr/local/bin:{}", current)
}

fn active_spiderlings_path() -> String {
    format!("{}/.aperture/active-spiderlings.json", home())
}

pub fn write_active_spiderlings(state: &AppState) {
    let spiderlings: Vec<&SpiderlingDef> = state.spiderlings.values().collect();
    if let Ok(json) = serde_json::to_string_pretty(&spiderlings) {
        let _ = fs::write(active_spiderlings_path(), json);
    }
}

fn validate_name(name: &str, state: &AppState) -> Result<(), String> {
    let re = Regex::new(r"^[a-z0-9][a-z0-9-]{0,30}$").unwrap();
    if !re.is_match(name) {
        return Err(format!(
            "Invalid spiderling name '{}'. Must match [a-z0-9][a-z0-9-]{{0,30}}",
            name
        ));
    }
    if PERMANENT_AGENTS.contains(&name) || state.agents.contains_key(name) {
        return Err(format!("Name '{}' conflicts with a permanent agent", name));
    }
    if state.spiderlings.contains_key(name) {
        return Err(format!("Spiderling '{}' already exists", name));
    }
    Ok(())
}

pub fn spawn_spiderling(
    name: String,
    task_id: String,
    prompt: String,
    requested_by: String,
    project_path: Option<String>,
    app_state: &mut AppState,
) -> Result<String, String> {
    validate_name(&name, app_state)?;

    let home = home();
    let mcp_server_path = app_state.mcp_server_path.clone();
    let tmux_session = app_state.tmux_session.clone();

    // Resolve the repo to create the worktree from:
    // If project_path is given, use that project's repo; otherwise fall back to Aperture
    let repo_dir = match &project_path {
        Some(p) => {
            let expanded = if p.starts_with("~/") {
                format!("{}/{}", home, &p[2..])
            } else {
                p.clone()
            };
            // Verify it's a git repo
            let check = std::process::Command::new("git")
                .args(["rev-parse", "--git-dir"])
                .current_dir(&expanded)
                .env("PATH", &path_env())
                .output();
            match check {
                Ok(out) if out.status.success() => expanded,
                _ => return Err(format!("'{}' is not a valid git repository", expanded)),
            }
        }
        None => app_state.project_dir.clone(),
    };

    // Create git worktree
    let worktree_dir = format!("{}/.aperture/worktrees", home);
    let _ = fs::create_dir_all(&worktree_dir);
    let worktree_path = format!("{}/{}", worktree_dir, name);
    let branch_name = name.clone();

    // Try creating worktree with new branch
    let output = std::process::Command::new("git")
        .args(["worktree", "add", "-b", &branch_name, &worktree_path])
        .current_dir(&repo_dir)
        .env("PATH", &path_env())
        .output()
        .map_err(|e| format!("Failed to create worktree: {}", e))?;

    if !output.status.success() {
        // Branch might already exist, try without -b
        let output2 = std::process::Command::new("git")
            .args(["worktree", "add", &worktree_path, &branch_name])
            .current_dir(&repo_dir)
            .env("PATH", &path_env())
            .output()
            .map_err(|e| format!("Failed to create worktree: {}", e))?;

        if !output2.status.success() {
            return Err(format!(
                "Failed to create git worktree: {}",
                String::from_utf8_lossy(&output2.stderr)
            ));
        }
    }

    // Create tmux window in the main session
    let window_id = tmux::tmux_create_window(tmux_session, name.clone())?;

    // Ensure spiderling mailbox
    let mailbox_dir = format!("{}/.aperture/mailbox/{}", home, name);
    let _ = fs::create_dir_all(&mailbox_dir);

    // Write MCP config
    let mcp_config = serde_json::json!({
        "mcpServers": {
            "aperture-bus": {
                "type": "stdio",
                "command": "node",
                "args": [&mcp_server_path],
                "env": {
                    "AGENT_NAME": &name,
                    "AGENT_ROLE": "spiderling",
                    "AGENT_MODEL": "sonnet",
                    "APERTURE_MAILBOX": format!("{}/.aperture/mailbox", home),
                    "BEADS_DIR": format!("{}/.aperture/.beads", home),
                    "BD_ACTOR": &name
                }
            }
        }
    });

    let config_path = format!("/tmp/aperture-mcp-{}.json", name);
    fs::write(
        &config_path,
        serde_json::to_string_pretty(&mcp_config).unwrap(),
    )
    .map_err(|e| e.to_string())?;

    // Write prompt to file to avoid shell escaping issues
    let launcher_dir = format!("{}/.aperture/launchers", home);
    let _ = fs::create_dir_all(&launcher_dir);

    let system_prompt = format!(
        "You are a spiderling named {name}, working for GLaDOS in the Aperture system.\n\
         Your task is tracked in BEADS issue {task_id}.\n\
         Work in this git worktree at {worktree_path} — do NOT switch branches or leave this directory.\n\n\
         ## Communication — use BEADS, not send_message\n\
         Do NOT use send_message(to: 'glados') — those messages get lost.\n\
         Instead, communicate through BEADS task updates:\n\
         - Progress updates: update_task(id: '{task_id}', notes: 'what you found/did')\n\
         - Store deliverables: store_artifact(task_id: '{task_id}', type: 'file'|'note', value: '...')\n\
         - When done: update_task(id: '{task_id}', status: 'done', notes: 'summary of what was done')\n\
         GLaDOS polls BEADS — your updates will be seen reliably.\n\n\
         ## War Room\n\
         If you receive a War Room context (starts with '# WAR ROOM'), you MUST:\n\
         1. Pause your current work\n\
         2. Read the transcript carefully\n\
         3. Respond using: send_message(to: 'warroom', message: 'your contribution')\n\
         4. Do NOT reply in the terminal — use the send_message MCP tool with to='warroom'\n\
         5. Return to your task after responding\n\n\
         TASK:\n{prompt}",
        name = name,
        task_id = task_id,
        worktree_path = worktree_path,
        prompt = prompt,
    );

    let system_prompt = crate::agents::inject_skills(system_prompt, &repo_dir);

    let prompt_path = format!("{}/{}-prompt.txt", launcher_dir, name);
    fs::write(&prompt_path, &system_prompt).map_err(|e| e.to_string())?;

    let launcher_path = format!("{}/{}.sh", launcher_dir, name);
    let launcher_script = format!(
        r#"#!/bin/bash
export PATH="/opt/homebrew/bin:/usr/local/bin:$PATH"
cd "{worktree_path}"
PROMPT=$(cat "{prompt_path}")
exec claude --dangerously-skip-permissions --model sonnet --system-prompt "$PROMPT" --mcp-config {config_path} --name {name}
"#,
        worktree_path = worktree_path,
        prompt_path = prompt_path,
        config_path = config_path,
        name = name,
    );

    fs::write(&launcher_path, &launcher_script).map_err(|e| e.to_string())?;
    std::process::Command::new("chmod")
        .args(["+x", &launcher_path])
        .output()
        .map_err(|e| e.to_string())?;

    // Launch in tmux
    tmux::tmux_send_keys(window_id.clone(), launcher_path)?;

    // Auto-confirm workspace trust, then send initial task message
    let window_id_clone = window_id.clone();
    std::thread::spawn(move || {
        // Press enter a few times to confirm workspace trust prompts
        for _ in 0..3 {
            std::thread::sleep(std::time::Duration::from_secs(2));
            let _ = tmux::tmux_send_keys(window_id_clone.clone(), "".into());
        }
        // Wait for Claude to fully boot, then send the initial task prompt
        std::thread::sleep(std::time::Duration::from_secs(3));
        let _ = tmux::tmux_send_keys(
            window_id_clone.clone(),
            "Begin your task now. Read your system prompt carefully for full instructions.".into(),
        );
    });

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .to_string();

    let spiderling = SpiderlingDef {
        name: name.clone(),
        task_id,
        tmux_window_id: Some(window_id),
        worktree_path,
        worktree_branch: branch_name,
        source_repo: Some(repo_dir),
        requested_by,
        status: "working".into(),
        spawned_at: timestamp,
    };

    app_state.spiderlings.insert(name.clone(), spiderling);
    write_active_spiderlings(app_state);

    Ok(name)
}

pub fn kill_spiderling(name: String, app_state: &mut AppState) -> Result<(), String> {
    let spiderling = app_state
        .spiderlings
        .get(&name)
        .ok_or(format!("Spiderling '{}' not found", name))?
        .clone();

    if let Some(ref window_id) = spiderling.tmux_window_id {
        let _ = tmux::tmux_send_keys(window_id.clone(), "C-c".into());
        std::thread::sleep(std::time::Duration::from_millis(500));
        let _ = tmux::tmux_send_keys(window_id.clone(), "/exit".into());
        std::thread::sleep(std::time::Duration::from_millis(500));
        let _ = tmux::tmux_kill_window(window_id.clone());
    }

    // Remove worktree (preserve branch for merging)
    // Use source_repo if available (target project), otherwise fall back to Aperture
    let repo_dir = spiderling
        .source_repo
        .as_deref()
        .unwrap_or(&app_state.project_dir);
    let _ = std::process::Command::new("git")
        .args(["worktree", "remove", "--force", &spiderling.worktree_path])
        .current_dir(repo_dir)
        .env("PATH", &path_env())
        .output();

    // Clean up launcher and config files
    let home = home();
    let _ = fs::remove_file(format!("{}/.aperture/launchers/{}.sh", home, name));
    let _ = fs::remove_file(format!("{}/.aperture/launchers/{}-prompt.txt", home, name));
    let _ = fs::remove_file(format!("/tmp/aperture-mcp-{}.json", name));

    app_state.spiderlings.remove(&name);
    write_active_spiderlings(app_state);

    Ok(())
}

#[tauri::command]
pub fn list_spiderlings(
    state: tauri::State<'_, Arc<Mutex<AppState>>>,
) -> Result<Vec<SpiderlingDef>, String> {
    let app_state = state.lock().map_err(|e| e.to_string())?;
    Ok(app_state.spiderlings.values().cloned().collect())
}

#[tauri::command]
pub fn kill_spiderling_cmd(
    name: String,
    state: tauri::State<'_, Arc<Mutex<AppState>>>,
) -> Result<(), String> {
    let mut app_state = state.lock().map_err(|e| e.to_string())?;
    kill_spiderling(name, &mut app_state)
}
