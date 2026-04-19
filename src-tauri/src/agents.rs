use crate::codex_harness;
use crate::config;
use crate::state::AppState;
use crate::tmux;
use std::fs;
use std::sync::{Arc, Mutex};

use crate::state::AgentDef;

#[tauri::command]
pub fn start_agent(name: String, state: tauri::State<'_, Arc<Mutex<AppState>>>) -> Result<(), String> {
    // Extract all needed data while holding the lock briefly, then release it
    // before doing any expensive I/O (subprocess calls, file writes). This
    // prevents the global state mutex from blocking list_agents polling and
    // other commands for the full duration of agent startup.
    let (agent, tmux_session, mcp_server_path, project_dir) = {
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
            app_state.project_dir.clone(),
        )
    }; // ← mutex released here; all I/O below is lock-free

    // Create a dedicated tmux window for this agent
    let window_id = tmux::tmux_create_window(tmux_session, name.clone())?;

    // Ensure agent's mailbox directory exists
    let mailbox_dir = format!("{}/.aperture/mailbox", std::env::var("HOME").unwrap_or_else(|_| "/tmp".into()));
    let _ = fs::create_dir_all(format!("{}/{}", mailbox_dir, name));

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
                    "BEADS_DIR": format!("{}/.aperture/.beads", std::env::var("HOME").unwrap_or_else(|_| "/tmp".into())),
                    "BD_ACTOR": &name
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
        let prompt_content = inject_skills(prompt_content, &project_dir);
        // Prepend any unread BEADS messages so Codex sees them on the first turn
        let prompt_content = codex_harness::inject_pending_messages(&name, prompt_content);
        let prompt_dest = format!("{}/prompt.md", codex_home);
        fs::write(&prompt_dest, &prompt_content).map_err(|e| e.to_string())?;

        let config_toml = format!(
            r#"model = "{bare_model}"
model_instructions_file = "{prompt_dest}"
approval_policy = "never"
sandbox_mode = "danger-full-access"

[projects."{project_dir}"]
trust_level = "trusted"

[mcp_servers.aperture-bus]
command = "node"
args = ["{mcp_server_path}"]
env = {{ AGENT_NAME = "{name}", AGENT_ROLE = "{role}", AGENT_MODEL = "{model}", APERTURE_MAILBOX = "{mailbox_dir}", BEADS_DIR = "{beads_dir}", BD_ACTOR = "{name}" }}
"#,
            bare_model = bare_model,
            prompt_dest = prompt_dest,
            project_dir = project_dir,
            mcp_server_path = mcp_server_path,
            name = name,
            role = agent.role,
            model = agent.model,
            mailbox_dir = mailbox_dir,
            beads_dir = beads_dir,
        );
        fs::write(&config_toml_path, &config_toml).map_err(|e| e.to_string())?;

        format!(
            r#"#!/bin/bash
export NVM_DIR="$HOME/.nvm"
[ -s "$NVM_DIR/nvm.sh" ] && source "$NVM_DIR/nvm.sh"
export PATH="/opt/homebrew/bin:/usr/local/bin:$HOME/.npm-global/bin:$PATH"
export CODEX_HOME="{codex_home}"
exec codex --yolo
"#,
            codex_home = codex_home,
        )
    } else {
        let config_path = format!("/tmp/aperture-mcp-{}.json", name);
        fs::write(
            &config_path,
            serde_json::to_string_pretty(&mcp_config).unwrap(),
        )
        .map_err(|e| e.to_string())?;

        format!(
            r#"#!/bin/bash
export PATH="/opt/homebrew/bin:/usr/local/bin:$PATH"
PROMPT=$(cat "{}")
exec claude --dangerously-skip-permissions --model {} --system-prompt "$PROMPT" --mcp-config {} --name {}
"#,
            agent.prompt_file, agent.model, config_path, name
        )
    };
    fs::write(&launcher_path, &launcher_script).map_err(|e| e.to_string())?;

    std::process::Command::new("chmod")
        .args(["+x", &launcher_path])
        .output()
        .map_err(|e| e.to_string())?;

    tmux::tmux_send_keys(window_id.clone(), launcher_path)?;

    // For Codex agents: start the BEADS output monitor in the background.
    // The monitor scrapes tmux pane output for @@BEADS@@ command blocks and
    // executes them on the agent's behalf, closing the outbound BEADS loop.
    if agent.model.starts_with("codex/") {
        codex_harness::start_output_monitor(window_id.clone(), name.clone());
    }

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

    // Re-acquire lock only to write the final status
    {
        let mut app_state = state.lock().map_err(|e| e.to_string())?;
        let agent_mut = app_state.agents.get_mut(&name).unwrap();
        agent_mut.tmux_window_id = Some(window_id);
        agent_mut.status = "running".into();
    }

    Ok(())
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

    Ok(app_state.agents.values().cloned().collect())
}

#[tauri::command]
pub fn get_recent_messages(
    _state: tauri::State<'_, Arc<Mutex<AppState>>>,
) -> Result<serde_json::Value, String> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let log_path = format!("{}/.aperture/message-log.jsonl", home);

    let content = match fs::read_to_string(&log_path) {
        Ok(c) => c,
        Err(_) => return Ok(serde_json::json!([])),
    };

    // Parse JSONL, take last 100 entries, reverse so newest first
    let messages: Vec<serde_json::Value> = content
        .lines()
        .filter(|l| !l.is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();

    let start = if messages.len() > 100 { messages.len() - 100 } else { 0 };
    let recent: Vec<serde_json::Value> = messages[start..]
        .iter()
        .rev()
        .map(|m| {
            serde_json::json!({
                "from_agent": m.get("from").and_then(|v| v.as_str()).unwrap_or("?"),
                "to_agent": m.get("to").and_then(|v| v.as_str()).unwrap_or("?"),
                "content": m.get("content").and_then(|v| v.as_str()).unwrap_or("").chars().take(200).collect::<String>(),
                "timestamp": m.get("timestamp").and_then(|v| v.as_str()).unwrap_or(""),
            })
        })
        .collect();

    Ok(serde_json::json!(recent))
}

#[tauri::command]
pub fn clear_message_history() -> Result<(), String> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let log_path = format!("{}/.aperture/message-log.jsonl", home);
    fs::write(&log_path, "").map_err(|e| e.to_string())
}

#[tauri::command]
pub fn clear_conversation_history(agent_a: String, agent_b: String) -> Result<(), String> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let log_path = format!("{}/.aperture/message-log.jsonl", home);

    let content = fs::read_to_string(&log_path).unwrap_or_default();

    let filtered: Vec<&str> = content
        .lines()
        .filter(|line| {
            if let Ok(m) = serde_json::from_str::<serde_json::Value>(line) {
                let from = m.get("from").and_then(|v| v.as_str()).unwrap_or("");
                let to = m.get("to").and_then(|v| v.as_str()).unwrap_or("");
                // Keep lines that DON'T match this conversation pair
                !((from == agent_a && to == agent_b) || (from == agent_b && to == agent_a))
            } else {
                false // drop unparseable lines
            }
        })
        .collect();

    let result = if filtered.is_empty() {
        String::new()
    } else {
        filtered.join("\n") + "\n"
    };

    fs::write(&log_path, result).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn send_chat(to_agent: String, message: String) -> Result<(), String> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let mailbox_dir = format!("{}/.aperture/mailbox/{}", home, to_agent);
    let _ = fs::create_dir_all(&mailbox_dir);

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();

    let filename = format!("{}/{}-operator.md", mailbox_dir, timestamp);
    let content = format!("# Message from the Human Operator\n\n{}\n\n---\n_Reply using: send_message(to: \"operator\", message: \"your reply\")_\n", message);
    fs::write(&filename, &content).map_err(|e| e.to_string())?;

    // Also log to chat history
    let chat_log = format!("{}/.aperture/chat-log.jsonl", home);
    let entry = serde_json::json!({
        "from": "operator",
        "to": to_agent,
        "content": message,
        "timestamp": timestamp.to_string(),
    });
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&chat_log) {
        use std::io::Write;
        let _ = writeln!(file, "{}", entry.to_string());
    }

    Ok(())
}

#[tauri::command]
pub fn get_chat_messages() -> Result<serde_json::Value, String> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let chat_log = format!("{}/.aperture/chat-log.jsonl", home);

    let content = match fs::read_to_string(&chat_log) {
        Ok(c) => c,
        Err(_) => return Ok(serde_json::json!([])),
    };

    let messages: Vec<serde_json::Value> = content
        .lines()
        .filter(|l| !l.is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();

    let start = if messages.len() > 200 { messages.len() - 200 } else { 0 };
    let recent: Vec<serde_json::Value> = messages[start..]
        .iter()
        .map(|m| {
            serde_json::json!({
                "from": m.get("from").and_then(|v| v.as_str()).unwrap_or("?"),
                "to": m.get("to").and_then(|v| v.as_str()).unwrap_or("?"),
                "content": m.get("content").and_then(|v| v.as_str()).unwrap_or(""),
                "timestamp": m.get("timestamp").and_then(|v| v.as_str()).unwrap_or(""),
            })
        })
        .collect();

    Ok(serde_json::json!(recent))
}

#[tauri::command]
pub fn update_agent_model(
    name: String,
    model: String,
    state: tauri::State<'_, Arc<Mutex<AppState>>>,
) -> Result<(), String> {
    let valid = matches!(model.as_str(), "opus" | "sonnet" | "haiku") || model.starts_with("codex/");
    if !valid {
        return Err(format!("Invalid model '{}'. Must be opus/sonnet/haiku or codex/<model>", model));
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

#[tauri::command]
pub fn clear_chat_history() -> Result<(), String> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let chat_log = format!("{}/.aperture/chat-log.jsonl", home);
    fs::write(&chat_log, "").map_err(|e| e.to_string())
}

pub fn inject_skills(mut prompt: String, project_dir: &str) -> String {
    let skills_dir = format!("{}/.claude/skills", project_dir);
    let dir = match fs::read_dir(&skills_dir) {
        Ok(d) => d,
        Err(_) => return prompt,
    };
    for entry in dir.flatten() {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let skill_name = entry.file_name().to_string_lossy().to_string();
        let base = entry.path();
        let skill_file = ["SKILL.md", "skill.md"]
            .iter()
            .map(|n| base.join(n))
            .find(|p| p.exists());
        match skill_file {
            Some(path) => match fs::read_to_string(&path) {
                Ok(content) => {
                    prompt.push_str(&format!("\n\n---\n# Skill: {}\n\n{}", skill_name, content));
                }
                Err(e) => eprintln!("[aperture] warn: could not read skill '{}': {}", skill_name, e),
            },
            None => eprintln!("[aperture] warn: skill dir '{}' has no SKILL.md", skill_name),
        }
    }
    prompt
}
