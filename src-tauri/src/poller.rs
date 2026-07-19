use crate::codex_harness;
use crate::state::AppState;
use crate::tmux;
use std::collections::{HashMap, HashSet};
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

// NOTE(Comms v2 Phase 1): mark_message_read() was deleted. The poller must
// never mark messages read — read state is owned by the recipient's explicit
// mark_as_read (spec Q4), and the WS hub relies on rows staying open to
// replay them.

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

    // Ensure operator mailbox exists
    let _ = fs::create_dir_all(format!("{}/operator", mailbox_base));

    let mut notified: HashSet<String> = HashSet::new();

    loop {
        std::thread::sleep(Duration::from_secs(5));

        // ── Handle operator-bound messages (agent → human) ──
        //
        // The chat panel is gone. When an agent calls
        //   send_message(to: "operator", message: "...")
        // the MCP server still writes a file to mailbox/operator/. We consume
        // it here, set the attention badge on the sending agent, and delete
        // the file. The actual message body lives in the agent's tmux
        // scrollback (via Claude Code's normal tool-call rendering) — the
        // operator clicks the agent in the launcher to review it there.
        let operator_path = format!("{}/operator", mailbox_base);
        let operator_files = scan_mailbox(&operator_path);
        for filepath in &operator_files {
            let (sender, _timestamp) = parse_filename(filepath);
            if !sender.is_empty() {
                if let Ok(mut app_state) = state.lock() {
                    if let Some(agent) = app_state.agents.get_mut(&sender) {
                        agent.attention = true;
                    }
                }
            }
            let _ = fs::remove_file(filepath);
        }

        // ── Handle agent-bound messages via BEADS message bus ──
        // Each tuple is (agent_name, window_id, is_codex).
        // Codex agents get messages buffered to a pending file instead of
        // tmux-injected shell commands (which Codex's loop ignores).
        let agents: Vec<(String, String, bool)> = {
            let Ok(app_state) = state.lock() else {
                continue;
            };

            // Resolve live permanent-agent windows from tmux every cycle rather
            // than relying on cached AppState window IDs. This self-heals after
            // external restarts and ignores stale shell windows left behind by
            // prior sessions with the same agent name.
            let running_windows: HashMap<String, String> =
                match tmux::tmux_list_windows(app_state.tmux_session.clone()) {
                    Ok(windows) => windows
                        .into_iter()
                        .filter(|w| {
                            w.command == "claude"
                                || w.command.contains("claude")
                                || w.command == "codex"
                                || w.command.contains("codex")
                                || w.command == "node"
                        })
                        .map(|w| (w.name, w.window_id))
                        .collect(),
                    Err(_) => HashMap::new(),
                };

            app_state
                .agents
                .values()
                .filter_map(|a| {
                    running_windows.get(&a.name).map(|wid| {
                        let is_codex = a.model.starts_with("codex/");
                        if is_codex {
                            codex_harness::ensure_output_monitor(wid.clone(), a.name.clone());
                        }
                        (a.name.clone(), wid.clone(), is_codex)
                    })
                })
                .collect()
        };

        for (agent_name, window_id, is_codex) in &agents {
            // ── Comms Layer v2, Phase 1 ──
            // (docs/superpowers/specs/2026-07-19-comms-layer-v2-design.md)
            //
            // Claude agents no longer get poller delivery. The old path wrote
            // /tmp/aperture-msg-<id>.md and tmux-injected `cat` into the pane,
            // then marked the message read on the delivery *attempt*. That is
            // replaced by the WS hub (mcp-server/dist/ws-hub.js), which pushes
            // unread BEADS rows to the agent's Monitor socket and replays them
            // on reconnect. We deliberately skip Claude agents here, leaving
            // their messages open (unread) in BEADS for the hub.
            //
            // Codex agents keep the pending-buffer path below until Phase 2
            // (app-server injection).
            if *is_codex {
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

                    // Codex agents: MCP tools are confirmed working (Failure Mode A).
                    // The agent calls get_messages() itself to read unread messages.
                    // Do NOT mark as read here — let the MCP call handle read state
                    // so the message remains visible when the agent polls BEADS.
                    //
                    // We still buffer to pending-msgs.md as a secondary delivery path
                    // (for the monitor thread's flush prompt), but the primary channel
                    // is the agent's own MCP get_messages call.
                    codex_harness::buffer_pending_message(agent_name, &formatted);

                    notified.insert(msg.id.clone());
                }
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
