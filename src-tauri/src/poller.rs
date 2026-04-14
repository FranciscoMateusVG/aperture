use crate::codex_harness;
use crate::state::AppState;
use crate::tmux;
use crate::warroom;
use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use serde::Deserialize;

fn log_message(log_path: &str, from: &str, to: &str, content: &str, timestamp: &str) {
    let entry = serde_json::json!({
        "from": from,
        "to": to,
        "content": content,
        "timestamp": timestamp,
    });
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(log_path) {
        let _ = writeln!(file, "{}", entry.to_string());
    }
}

fn parse_filename(filepath: &str) -> (String, String) {
    let fname = std::path::Path::new(filepath)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();
    let sender = fname
        .trim_end_matches(".md")
        .split('-')
        .skip(1)
        .collect::<Vec<_>>()
        .join("-");
    let timestamp = fname.split('-').next().unwrap_or("0").to_string();
    (sender, timestamp)
}

fn scan_mailbox(path: &str) -> Vec<String> {
    match fs::read_dir(path) {
        Ok(entries) => entries
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".md"))
            .map(|e| e.path().to_string_lossy().to_string())
            .collect(),
        Err(_) => Vec::new(),
    }
}

#[derive(Debug, Deserialize)]
struct BeadsMessage {
    id: String,
    title: String,
    description: Option<String>,
}

fn bd_path() -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    // Fallback chain: prefer ~/.local/bin/bd (canonical install location),
    // then ~/go/bin/bd (go install default), then rely on PATH resolution.
    // This eliminates the manual symlink dependency.
    let candidates = [
        format!("{}/.local/bin/bd", home),
        format!("{}/go/bin/bd", home),
    ];
    for path in &candidates {
        if std::path::Path::new(path).exists() {
            return path.clone();
        }
    }
    "bd".to_string()
}

fn beads_dir() -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    format!("{}/.aperture/.beads", home)
}

fn path_env() -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let current = std::env::var("PATH").unwrap_or_default();
    format!("{}/.local/bin:/opt/homebrew/bin:/usr/local/bin:{}", home, current)
}

/// Query BEADS for unread messages destined for a specific agent.
/// Messages have title format: [sender->recipient] preview...
/// Status "open" means undelivered/unread.
fn query_unread_messages(recipient: &str) -> Vec<BeadsMessage> {
    let query = format!("type=message AND status=open AND title=\"->{recipient}]\"");
    let output = std::process::Command::new(bd_path())
        .args(["query", &query, "--json", "-n", "0", "-q"])
        .env("BEADS_DIR", beads_dir())
        .env("BD_ACTOR", "poller")
        .env("PATH", path_env())
        .output();

    match output {
        Ok(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            serde_json::from_str::<Vec<BeadsMessage>>(stdout.trim()).unwrap_or_default()
        }
        _ => Vec::new(),
    }
}

/// Mark a BEADS message as read (close it).
fn mark_message_read(message_id: &str) {
    let _ = std::process::Command::new(bd_path())
        .args(["close", message_id, "--reason", "delivered", "-q"])
        .env("BEADS_DIR", beads_dir())
        .env("BD_ACTOR", "poller")
        .env("PATH", path_env())
        .output();
}

/// Parse sender from BEADS message title: [sender->recipient] preview...
fn parse_sender_from_title(title: &str) -> String {
    if let Some(start) = title.find('[') {
        if let Some(arrow) = title.find("->") {
            return title[start + 1..arrow].to_string();
        }
    }
    "unknown".to_string()
}

pub fn run_message_poller(state: Arc<Mutex<AppState>>) {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let mailbox_base = format!("{}/.aperture/mailbox", home);
    let message_log = format!("{}/.aperture/message-log.jsonl", home);
    let chat_log = format!("{}/.aperture/chat-log.jsonl", home);

    // Ensure operator mailbox exists
    let _ = fs::create_dir_all(format!("{}/operator", mailbox_base));

    let mut notified: HashSet<String> = HashSet::new();
    let mut warroom_notified: HashSet<String> = HashSet::new();

    loop {
        std::thread::sleep(Duration::from_secs(5));

        // ── Handle spawn requests ──
        {
            let spawn_dir = format!("{}/_spawn", mailbox_base);
            let _ = fs::create_dir_all(&spawn_dir);

            if let Ok(entries) = fs::read_dir(&spawn_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().and_then(|e| e.to_str()) != Some("json") {
                        continue;
                    }
                    if let Ok(content) = fs::read_to_string(&path) {
                        if let Ok(req) = serde_json::from_str::<serde_json::Value>(&content) {
                            let name = req
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let task_id = req
                                .get("task_id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let prompt = req
                                .get("prompt")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let requested_by = req
                                .get("requested_by")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let project_path = req
                                .get("project_path")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string());

                            if !name.is_empty() {
                                let mut app_state = match state.lock() {
                                    Ok(s) => s,
                                    Err(_) => continue,
                                };
                                match crate::spawner::spawn_spiderling(
                                    name.clone(),
                                    task_id,
                                    prompt,
                                    requested_by,
                                    project_path,
                                    &mut app_state,
                                ) {
                                    Ok(_) => println!("Spawned spiderling: {}", name),
                                    Err(e) => eprintln!("Failed to spawn {}: {}", name, e),
                                }
                            }
                        }
                    }
                    let _ = fs::remove_file(&path);
                }
            }
        }

        // ── Handle kill requests ──
        {
            let kill_dir = format!("{}/_kill", mailbox_base);
            let _ = fs::create_dir_all(&kill_dir);

            if let Ok(entries) = fs::read_dir(&kill_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if let Ok(content) = fs::read_to_string(&path) {
                        let name = content.trim().to_string();
                        if !name.is_empty() {
                            let mut app_state = match state.lock() {
                                Ok(s) => s,
                                Err(_) => continue,
                            };
                            match crate::spawner::kill_spiderling(name.clone(), &mut app_state) {
                                Ok(_) => println!("Killed spiderling: {}", name),
                                Err(e) => eprintln!("Failed to kill {}: {}", name, e),
                            }
                        }
                    }
                    let _ = fs::remove_file(&path);
                }
            }
        }

        // ── Handle war room messages ──
        {
            let warroom_mailbox = format!("{}/warroom", mailbox_base);
            let _ = fs::create_dir_all(&warroom_mailbox);

            let wr_state_path = format!("{}/.aperture/warroom/state.json", home);
            if let Ok(wr_data) = fs::read_to_string(&wr_state_path) {
                if wr_data.contains("\"active\"") {
                    let wr_files = scan_mailbox(&warroom_mailbox);
                    warroom_notified.retain(|f| wr_files.contains(f));

                    let new_wr_files: Vec<&String> = wr_files
                        .iter()
                        .filter(|f| !warroom_notified.contains(*f))
                        .collect();

                    for filepath in &new_wr_files {
                        if let Ok(content) = fs::read_to_string(filepath) {
                            let (sender, _timestamp) = parse_filename(filepath);
                            match warroom::handle_warroom_message(&sender, &content, &state) {
                                Ok(()) => {
                                    let _ = fs::remove_file(filepath);
                                }
                                Err(_e) => {
                                    let _ = fs::remove_file(filepath);
                                }
                            }
                        }
                        warroom_notified.insert((*filepath).clone());
                    }
                }
            }
        }

        // ── Handle operator-bound messages (agent → human) ──
        let operator_path = format!("{}/operator", mailbox_base);
        let operator_files = scan_mailbox(&operator_path);
        for filepath in &operator_files {
            if notified.contains(filepath) {
                continue;
            }
            if let Ok(content) = fs::read_to_string(filepath) {
                let (sender, timestamp) = parse_filename(filepath);
                log_message(&chat_log, &sender, "operator", &content, &timestamp);
                let _ = fs::remove_file(filepath);
            }
            notified.insert(filepath.clone());
        }
        notified.retain(|f| !f.starts_with(&operator_path) || operator_files.contains(f));

        // ── Handle agent-bound messages via BEADS message bus ──
        // Each tuple is (agent_name, window_id, is_codex).
        // Codex agents get messages buffered to a pending file instead of
        // tmux-injected shell commands (which Codex's loop ignores).
        let agents: Vec<(String, String, bool)> = {
            let Ok(app_state) = state.lock() else {
                continue;
            };

            let named: Vec<(String, String, bool)> = app_state
                .agents
                .values()
                .filter(|a| a.status == "running")
                .filter_map(|a| {
                    a.tmux_window_id
                        .as_ref()
                        .map(|wid| (a.name.clone(), wid.clone(), a.model.starts_with("codex/")))
                })
                .collect();

            // Spiderlings are always Claude — never Codex
            let spiderlings: Vec<(String, String, bool)> = app_state
                .spiderlings
                .values()
                .filter(|s| s.status == "working")
                .filter_map(|s| {
                    s.tmux_window_id
                        .as_ref()
                        .map(|wid| (s.name.clone(), wid.clone(), false))
                })
                .collect();

            named.into_iter().chain(spiderlings).collect()
        };

        for (agent_name, window_id, is_codex) in &agents {
            // Query BEADS for unread messages addressed to this agent
            let messages = query_unread_messages(agent_name);

            for msg in &messages {
                // Skip if we already tried delivering this message this cycle
                if notified.contains(&msg.id) {
                    continue;
                }

                let sender = parse_sender_from_title(&msg.title);
                let content = msg.description.as_deref().unwrap_or("(no content)");
                let timestamp = &msg.id; // use message ID as reference

                // Format as markdown (same format agents expect)
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis();
                let formatted = format!(
                    "# Message from {}\n_{}_\n\n{}\n",
                    sender,
                    now,
                    content
                );

                // Log the message
                log_message(&message_log, &sender, agent_name, &formatted, timestamp);

                if *is_codex {
                    // Codex agents: MCP tools are confirmed working (Failure Mode A).
                    // The agent calls get_messages() itself to read unread messages.
                    // Do NOT mark as read here — let the MCP call handle read state
                    // so the message remains visible when the agent polls BEADS.
                    //
                    // We still buffer to pending-msgs.md as a secondary delivery path
                    // (for the monitor thread's flush prompt), but the primary channel
                    // is the agent's own MCP get_messages call.
                    codex_harness::buffer_pending_message(agent_name, &formatted);
                    // Do NOT call mark_message_read — intentionally omitted for Codex.
                } else {
                    // Claude / spiderling agents: write to temp file and inject via tmux
                    let tmp_path = format!("/tmp/aperture-msg-{}.md", msg.id);
                    if fs::write(&tmp_path, &formatted).is_ok() {
                        let cmd = format!("cat '{}' && rm '{}'", tmp_path, tmp_path);
                        let _ = tmux::tmux_send_keys(window_id.clone(), cmd);
                    }
                    // Mark as read immediately after tmux delivery
                    mark_message_read(&msg.id);
                }
                notified.insert(msg.id.clone());
            }

            // Also handle any legacy file-based messages still in mailbox
            let mailbox_path = format!("{}/{}", mailbox_base, agent_name);
            let files = scan_mailbox(&mailbox_path);
            if !files.is_empty() {
                for filepath in &files {
                    if let Ok(file_content) = fs::read_to_string(filepath) {
                        let (sender, ts) = parse_filename(filepath);
                        log_message(&message_log, &sender, agent_name, &file_content, &ts);
                    }
                }
                let cmd = format!(
                    "for f in '{}'/*.md; do [ -f \"$f\" ] && cat \"$f\" && rm \"$f\"; done",
                    mailbox_path
                );
                let _ = tmux::tmux_send_keys(window_id.clone(), cmd);
            }
        }
    }
}
