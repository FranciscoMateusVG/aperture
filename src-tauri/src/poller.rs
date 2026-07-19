//! Operator-mailbox sweep → attention badge.
//!
//! This is the poller's ONE remaining job after Comms Layer v2
//! (docs/superpowers/specs/2026-07-19-comms-layer-v2-design.md). All
//! agent-bound message delivery now flows through the aperture-bus WS hub
//! (Claude Monitor sockets) and the codex-bridge (app-server injection);
//! the poller delivers nothing to agents. The operator doorbell is
//! unchanged by the spec: when an agent calls
//! `send_message(to: "operator", ...)` the MCP server drops a file into
//! `~/.aperture/mailbox/operator/`, and this sweep consumes it, lights the
//! attention badge on the sending agent's launcher card, and deletes the
//! file. The message body itself lives in the agent's tmux scrollback —
//! the operator clicks the badged agent to review it there.

use crate::state::AppState;
use std::fs;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Extract the sending agent's name from a mailbox filename of the form
/// `<timestamp>-<sender>.md` (sender may itself contain hyphens).
fn parse_sender(filepath: &str) -> String {
    let fname = std::path::Path::new(filepath)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();
    fname
        .trim_end_matches(".md")
        .split('-')
        .skip(1)
        .collect::<Vec<_>>()
        .join("-")
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

pub fn run_message_poller(state: Arc<Mutex<AppState>>) {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let operator_path = format!("{}/.aperture/mailbox/operator", home);

    // Ensure operator mailbox exists
    let _ = fs::create_dir_all(&operator_path);

    loop {
        std::thread::sleep(Duration::from_secs(5));

        for filepath in scan_mailbox(&operator_path) {
            let sender = parse_sender(&filepath);
            if !sender.is_empty() {
                if let Ok(mut app_state) = state.lock() {
                    if let Some(agent) = app_state.agents.get_mut(&sender) {
                        agent.attention = true;
                    }
                }
            }
            let _ = fs::remove_file(&filepath);
        }
    }
}
