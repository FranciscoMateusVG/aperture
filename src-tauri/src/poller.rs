use crate::state::AppState;
use crate::tmux;
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Duration;

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

// NOTE(Comms v2 Phase 1): mark_message_read() was deleted. The poller must
// never mark messages read — read state is owned by the recipient's explicit
// mark_as_read (spec Q4), and the WS hub relies on rows staying open to
// replay them.
//
// NOTE(Comms v2 Phase 2): query_unread_messages(), BeadsMessage, and
// parse_sender_from_title() were deleted along with the codex
// buffer_pending_message delivery path — the aperture-bus codex-bridge now
// owns Codex delivery over the app-server socket. See the comment in the
// main loop below.

pub fn run_message_poller(state: Arc<Mutex<AppState>>) {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let mailbox_base = format!("{}/.aperture/mailbox", home);
    let message_log = format!("{}/.aperture/message-log.jsonl", home);

    // Ensure operator mailbox exists
    let _ = fs::create_dir_all(format!("{}/operator", mailbox_base));

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

        // ── Handle agent-bound messages ──
        // Each tuple is (agent_name, window_id). Since Comms v2 Phase 2 the
        // poller delivers NO BEADS messages for either protocol; only the
        // legacy file-based mailbox sweep below remains (Phase 3 removes it).
        let agents: Vec<(String, String)> = {
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
                    running_windows
                        .get(&a.name)
                        .map(|wid| (a.name.clone(), wid.clone()))
                })
                .collect()
        };

        for (agent_name, window_id) in &agents {
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
            // ── Comms Layer v2, Phase 2 ──
            //
            // The Codex pending-buffer path (codex_harness::
            // buffer_pending_message + ensure_output_monitor) is disabled the
            // same way: the hub's codex-bridge now injects deliveries over the
            // per-agent app-server socket and replays unread rows itself. The
            // poller delivers no BEADS messages at all anymore; this loop
            // skeleton survives only for the legacy mailbox sweep below and is
            // deleted wholesale in Phase 3.

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
