//! Operator-mailbox sweep → attention badge.
//!
//! This is the poller's ONE remaining job after Comms Layer v2
//! (docs/superpowers/specs/2026-07-19-comms-layer-v2-design.md). All
//! agent-bound message delivery now flows through the aperture-bus WS hub
//! (Claude Monitor sockets) and the codex-bridge (app-server injection);
//! the poller delivers nothing to agents. The operator doorbell is
//! D keeps only a read-only attention projection over a pre-existing mailbox.
//! It does not acknowledge, delete or claim message consumption. That final
//! functionality remains a separately reviewed policy gap.

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

// Read-only mailbox projection. No mkdir/delete/ack; consumption is explicitly
// an unresolved final-functionality gap, not a new delivery/journal system.
pub(crate) fn run_message_poller(state: Arc<Mutex<AppState>>, worker: crate::daemons::WorkerContext) {
    loop {
        if worker.wait(Duration::from_secs(5)) { return; }
        if let Ok(home) = worker.home() { let _ = scan_once(&home, &state, &worker); }
    }
}
pub(crate) fn scan_once(home: &std::path::Path, state: &Arc<Mutex<AppState>>, worker: &crate::daemons::WorkerContext) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    if worker.stopped() { return Err("E_RUNTIME_CLOSING".into()); }
    let mut path = home.to_path_buf();
    let mut pins = Vec::new();
    for component in [".aperture", "mailbox", "operator"] {
        path.push(component);
        crate::controller::private_dir_readonly(&path)?;
        let m = fs::symlink_metadata(&path).map_err(|_| "E_MAILBOX_UNSAFE")?;
        pins.push((path.clone(), m.dev(), m.ino(), m.uid(), m.mode()));
    }
    let mut senders = Vec::new();
    for (count, entry) in fs::read_dir(&path).map_err(|_| "E_MAILBOX_UNAVAILABLE")?.enumerate() {
        if count >= 512 { return Err("E_MAILBOX_LIMIT".into()); }
        let entry = entry.map_err(|_| "E_MAILBOX_UNAVAILABLE")?;
        let meta = fs::symlink_metadata(entry.path()).map_err(|_| "E_MAILBOX_UNAVAILABLE")?;
        if !meta.is_file() || meta.file_type().is_symlink() || meta.uid() != unsafe { libc::geteuid() }
            || meta.mode() & 0o077 != 0 || meta.nlink() != 1 { return Err("E_MAILBOX_UNSAFE".into()); }
        if entry.file_name().to_string_lossy().ends_with(".md") {
            senders.push(parse_sender(&entry.path().to_string_lossy()));
        }
    }
    // Never read message bodies, acknowledge or remove files. Attention alone
    // is idempotent; shutdown prevents a later pass.
    if worker.stopped() { return Err("E_RUNTIME_CLOSING".into()); }
    for (path, dev, ino, uid, mode) in pins {
        crate::controller::private_dir_readonly(&path)?;
        let m = fs::symlink_metadata(path).map_err(|_| "E_MAILBOX_UNSAFE")?;
        if (m.dev(), m.ino(), m.uid(), m.mode()) != (dev, ino, uid, mode) { return Err("E_MAILBOX_UNSAFE".into()); }
    }
    let mut app = state.lock().map_err(|_| "E_MAILBOX_UNAVAILABLE")?;
    if worker.stopped() { return Err("E_RUNTIME_CLOSING".into()); }
    for sender in senders {
        if let Some(agent) = app.agents.get_mut(&sender) {
            crate::agents::light_attention(agent, crate::agents::AttentionReason::Message);
        }
    }
    Ok(())
}
