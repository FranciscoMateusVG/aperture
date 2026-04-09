use crate::state::AppState;
use crate::tmux;
use std::fs;
use std::sync::{Arc, Mutex};

use crate::state::AgentDef;

#[tauri::command]
pub fn start_agent(name: String, state: tauri::State<'_, Arc<Mutex<AppState>>>) -> Result<(), String> {
    let mut app_state = state.lock().map_err(|e| e.to_string())?;
    let agent = app_state
        .agents
        .get(&name)
        .ok_or(format!("Agent '{}' not found", name))?
        .clone();

    if agent.status == "running" {
        return Err(format!("Agent '{}' is already running", name));
    }

    // Create a dedicated tmux window for this agent
    let window_id = tmux::tmux_create_window(
        app_state.tmux_session.clone(),
        name.clone(),
    )?;

    // Ensure agent's mailbox directory exists
    let mailbox_dir = format!("{}/.aperture/mailbox", std::env::var("HOME").unwrap_or_else(|_| "/tmp".into()));
    let _ = fs::create_dir_all(format!("{}/{}", mailbox_dir, name));

    let mcp_config = serde_json::json!({
        "mcpServers": {
            "aperture-bus": {
                "type": "stdio",
                "command": "node",
                "args": [&app_state.mcp_server_path],
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

    let config_path = format!("/tmp/aperture-mcp-{}.json", name);
    fs::write(
        &config_path,
        serde_json::to_string_pretty(&mcp_config).unwrap(),
    )
    .map_err(|e| e.to_string())?;

    // Write a launcher script that reads the prompt from file
    let launcher_path = format!("/tmp/aperture-launch-{}.sh", name);
    let launcher_script = format!(
        r#"#!/bin/bash
export PATH="/opt/homebrew/bin:/usr/local/bin:$PATH"
PROMPT=$(cat "{}")
exec claude --dangerously-skip-permissions --model {} --system-prompt "$PROMPT" --mcp-config {} --name {}
"#,
        agent.prompt_file, agent.model, config_path, name
    );
    fs::write(&launcher_path, &launcher_script).map_err(|e| e.to_string())?;

    std::process::Command::new("chmod")
        .args(["+x", &launcher_path])
        .output()
        .map_err(|e| e.to_string())?;

    tmux::tmux_send_keys(window_id.clone(), launcher_path)?;

    // Auto-confirm the workspace trust prompt
    let window_id_clone = window_id.clone();
    std::thread::spawn(move || {
        for _ in 0..3 {
            std::thread::sleep(std::time::Duration::from_secs(2));
            let _ = tmux::tmux_send_keys(window_id_clone.clone(), "".into());
        }
    });

    let agent_mut = app_state.agents.get_mut(&name).unwrap();
    agent_mut.tmux_window_id = Some(window_id);
    agent_mut.status = "running".into();

    Ok(())
}

#[tauri::command]
pub fn stop_agent(name: String, state: tauri::State<'_, Arc<Mutex<AppState>>>) -> Result<(), String> {
    let mut app_state = state.lock().map_err(|e| e.to_string())?;
    let agent = app_state
        .agents
        .get(&name)
        .ok_or(format!("Agent '{}' not found", name))?
        .clone();

    if agent.status != "running" {
        return Err(format!("Agent '{}' is not running", name));
    }

    if let Some(ref window_id) = agent.tmux_window_id {
        let _ = tmux::tmux_send_keys(window_id.clone(), "C-c".into());
        std::thread::sleep(std::time::Duration::from_millis(500));
        let _ = tmux::tmux_send_keys(window_id.clone(), "/exit".into());
        std::thread::sleep(std::time::Duration::from_millis(500));
        let _ = tmux::tmux_kill_window(window_id.clone());
    }

    let agent_mut = app_state.agents.get_mut(&name).unwrap();
    agent_mut.tmux_window_id = None;
    agent_mut.status = "stopped".into();

    Ok(())
}

#[tauri::command]
pub fn list_agents(state: tauri::State<'_, Arc<Mutex<AppState>>>) -> Result<Vec<AgentDef>, String> {
    let mut app_state = state.lock().map_err(|e| e.to_string())?;

    // Cross-reference with actual tmux windows to detect agents started outside the UI
    if let Ok(windows) = tmux::tmux_list_windows(app_state.tmux_session.clone()) {
        for window in &windows {
            if let Some(agent) = app_state.agents.get_mut(&window.name) {
                if window.command == "claude" || window.command.contains("claude") {
                    if agent.status != "running" {
                        agent.status = "running".into();
                        agent.tmux_window_id = Some(window.window_id.clone());
                    }
                } else if agent.tmux_window_id.as_deref() == Some(&window.window_id) {
                    // Window exists but claude isn't running in it
                    agent.status = "stopped".into();
                    agent.tmux_window_id = None;
                }
            }
        }

        // Also mark agents as stopped if their window is gone entirely
        let window_names: Vec<String> = windows.iter().map(|w| w.name.clone()).collect();
        for agent in app_state.agents.values_mut() {
            if agent.status == "running" && !window_names.contains(&agent.name) {
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
pub fn clear_chat_history() -> Result<(), String> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let chat_log = format!("{}/.aperture/chat-log.jsonl", home);
    fs::write(&chat_log, "").map_err(|e| e.to_string())
}
