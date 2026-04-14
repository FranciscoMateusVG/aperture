//! Codex BEADS Bridge — Harness
//!
//! This module bridges Tauri's Codex agent lifecycle with BEADS. Three jobs:
//!
//! 1. **Pre-prompt injection** [`inject_pending_messages`] — at agent startup,
//!    query BEADS for unread messages and prepend them to the system prompt
//!    before writing `prompt.md`. Codex sees its messages on the first turn.
//!
//! 2. **Message buffering** [`buffer_pending_message`] — for already-running
//!    Codex agents, write incoming messages to a pending file instead of
//!    tmux-injecting a shell command (which Codex's interactive loop ignores).
//!
//! 3. **Output monitoring** [`start_output_monitor`] — a background thread per
//!    Codex agent that polls the tmux pane, scans for `@@BEADS@@` command blocks,
//!    and executes them on the agent's behalf. Also flushes buffered messages
//!    into the session each cycle.

use crate::beads_parser::{parse_beads_blocks, BeadsCommand};
use crate::tmux;
use std::collections::HashSet;
use std::fs;
use std::time::Duration;

// ─────────────────────────────────────────────────────────────────────────────
// CLI helpers
// ─────────────────────────────────────────────────────────────────────────────

fn bd_path() -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    // Fallback chain: prefer ~/.local/bin/bd, then ~/go/bin/bd, then PATH.
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

fn pending_msgs_path(agent_name: &str) -> String {
    format!("/tmp/aperture-codex-{}/pending-msgs.md", agent_name)
}

// ─────────────────────────────────────────────────────────────────────────────
// BEADS message query types
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, serde::Deserialize)]
struct BeadsMessage {
    id: String,
    title: String,
    description: Option<String>,
}

// ─────────────────────────────────────────────────────────────────────────────
// 1. Pre-prompt injection
// ─────────────────────────────────────────────────────────────────────────────

/// Query BEADS for unread messages for `agent_name`, format them, and prepend
/// to `prompt`. Messages are marked as read immediately after injection so they
/// are not re-delivered on the next startup.
///
/// Called from `agents.rs` `start_agent()` in the Codex branch, after
/// `inject_skills()` and before writing `prompt.md`.
///
/// Returns the (possibly modified) prompt. If there are no unread messages,
/// returns `prompt` unchanged — zero overhead.
pub fn inject_pending_messages(agent_name: &str, prompt: String) -> String {
    let messages = query_unread(agent_name);
    if messages.is_empty() {
        return prompt;
    }

    let mut header = String::from(
        "--- BEADS MESSAGES (unread at startup) ---\n\
         The following messages were waiting for you. Read them carefully.\n\
         Respond using @@BEADS send_message@@ blocks where replies are needed.\n\n",
    );

    for msg in &messages {
        let sender = parse_sender(&msg.title);
        let content = msg.description.as_deref().unwrap_or("(no content)");
        header.push_str(&format!("From: {}\n{}\n\n", sender, content));
        mark_read(&msg.id);
    }

    header.push_str("--- END BEADS MESSAGES ---\n\n");

    // Prepend so the agent reads messages before its standing instructions
    format!("{}{}", header, prompt)
}

// ─────────────────────────────────────────────────────────────────────────────
// 2. Message buffering (for running agents)
// ─────────────────────────────────────────────────────────────────────────────

/// Append a formatted message to the agent's pending-msgs file.
///
/// Called from `poller.rs` instead of `tmux_send_keys` for Codex agents.
/// The output monitor flushes this file into the live session each poll cycle.
pub fn buffer_pending_message(agent_name: &str, formatted: &str) {
    use std::io::Write;
    let path = pending_msgs_path(agent_name);
    // Ensure the parent directory exists — Codex agents that were never started
    // via start_agent() won't have their /tmp/aperture-codex-{name}/ dir yet.
    let parent = std::path::Path::new(&path).parent().unwrap_or(std::path::Path::new("/tmp"));
    if let Err(e) = fs::create_dir_all(parent) {
        eprintln!(
            "[codex_harness] warn: could not create pending-msgs dir for '{}': {}",
            agent_name, e
        );
        return;
    }
    match fs::OpenOptions::new().create(true).append(true).open(&path) {
        Ok(mut f) => {
            let _ = writeln!(f, "{}\n---", formatted);
        }
        Err(e) => {
            eprintln!(
                "[codex_harness] warn: could not buffer message for '{}': {}",
                agent_name, e
            );
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 3. Output monitor
// ─────────────────────────────────────────────────────────────────────────────

/// Spawn a background thread that monitors `window_id` for `@@BEADS@@` blocks
/// and executes them as `agent_name`.
///
/// Also flushes any buffered pending messages into the session via tmux each
/// poll cycle (best-effort — Codex may or may not incorporate them depending
/// on its interactive interface).
///
/// The thread exits cleanly when the tmux window disappears (agent stopped).
pub fn start_output_monitor(window_id: String, agent_name: String) {
    std::thread::spawn(move || {
        // Give Codex time to fully boot before we start scraping output
        std::thread::sleep(Duration::from_secs(8));
        monitor_loop(window_id, agent_name);
    });
}

fn monitor_loop(window_id: String, agent_name: String) {
    let mut last_output_len: usize = 0;
    // Track Debug-repr of already-executed commands to avoid double-execution
    // within a session. Bounded to prevent unbounded growth.
    let mut executed: HashSet<String> = HashSet::new();

    println!(
        "[codex_harness] monitor started for agent '{}' on window '{}'",
        agent_name, window_id
    );

    // When we flush a pending message, Codex reads the inject file via its
    // shell tool — the file content (including any @@BEADS examples in the
    // original message) appears verbatim in the tmux output. If we scan that
    // output immediately, the monitor would parse @@BEADS blocks from the
    // *injected message text* rather than from Codex's *response*, then dedup
    // would block the real response when it arrives.
    //
    // Fix: after a flush, skip scanning for one full cycle to let the inject
    // content settle, then resume scanning only Codex's genuine output.
    let mut skip_scan_cycles: u8 = 0;

    loop {
        std::thread::sleep(Duration::from_secs(2));

        // ── Flush pending messages into the live session ──────────────────
        let flushed = flush_pending_messages(&window_id, &agent_name);
        if flushed {
            // Advance the baseline past whatever is currently on screen so
            // we don't scan the inject content on the next cycle.
            if let Ok(current) = tmux::tmux_capture_pane(&window_id) {
                last_output_len = current.len();
            }
            skip_scan_cycles = 2; // skip 2 cycles (~4s) — enough for Codex to read the file
            continue;
        }

        if skip_scan_cycles > 0 {
            skip_scan_cycles -= 1;
            // Update baseline so we don't re-scan old output after the skip
            if let Ok(current) = tmux::tmux_capture_pane(&window_id) {
                last_output_len = current.len();
            }
            continue;
        }

        // ── Poll tmux pane output ─────────────────────────────────────────
        let output = match tmux::tmux_capture_pane(&window_id) {
            Ok(o) => o,
            Err(_) => {
                // Window gone — agent stopped or killed
                println!(
                    "[codex_harness] monitor: window '{}' gone, exiting for agent '{}'",
                    window_id, agent_name
                );
                break;
            }
        };

        let current_len = output.len();

        // Handle terminal clear/reset: if output shrank, reset baseline
        if current_len < last_output_len {
            last_output_len = 0;
        }

        if current_len == last_output_len {
            continue;
        }

        let new_content = &output[last_output_len..];
        last_output_len = current_len;

        // ── Parse @@BEADS@@ blocks from new output ────────────────────────
        let commands = parse_beads_blocks(new_content);
        if commands.is_empty() {
            continue;
        }

        for cmd in &commands {
            let dedup_key = format!("{:?}", cmd);
            if executed.contains(&dedup_key) {
                continue;
            }
            executed.insert(dedup_key);
            execute_command(cmd, &agent_name);
        }

        // Bound the dedup set — clear after 1 000 entries
        if executed.len() > 1_000 {
            executed.clear();
        }
    }
}

/// Flush any buffered pending messages into the Codex session via tmux.
/// Clears the pending file after injection. Best-effort — never panics.
/// Returns `true` if a flush actually happened, `false` if nothing to flush.
fn flush_pending_messages(window_id: &str, agent_name: &str) -> bool {
    let path = pending_msgs_path(agent_name);
    let content = match fs::read_to_string(&path) {
        Ok(c) if !c.trim().is_empty() => c,
        _ => return false,
    };

    // Send message content as a direct natural-language prompt to Codex.
    //
    // Previous approach (shell pipeline via printf/cat) failed because Codex's
    // interactive loop is not a shell — it ran the pipeline as a tool call,
    // printed output, then waited without treating the content as a BEADS
    // message requiring a @@BEADS@@ block response.
    //
    // New approach: write the raw message content to a temp file and pass the
    // path to Codex as a natural-language instruction to read and respond.
    // This gives Codex clear intent ("you have messages, respond with @@BEADS@@")
    // rather than a shell command to execute.
    let tmp = format!("/tmp/aperture-codex-{}-inject.md", agent_name);
    if fs::write(&tmp, &content).is_err() {
        return false;
    }

    // Instruct Codex to read the file and respond using @@BEADS@@ blocks.
    // The file path is unambiguous; Codex will use its shell tool to read it.
    let prompt = format!(
        "You have new BEADS messages waiting at {}. Read that file now and respond to each message using @@BEADS send_message@@ blocks as defined in your codex-comms skill.",
        tmp
    );
    let _ = tmux::tmux_send_keys(window_id.to_string(), prompt);

    // Belt-and-suspenders: send an explicit Enter after a short delay.
    // Codex's interactive loop sometimes buffers the combined text+Enter
    // from a single send-keys call and needs a second press to submit.
    std::thread::sleep(Duration::from_millis(200));
    let _ = tmux::tmux_send_enter(window_id);

    // Clear the pending file so we don't re-inject on the next cycle
    let _ = fs::write(&path, "");
    true
}

// ─────────────────────────────────────────────────────────────────────────────
// 4. Command executor (public for testing / future extension)
// ─────────────────────────────────────────────────────────────────────────────

/// Execute a slice of parsed BEADS commands on behalf of `agent_name`.
///
/// Logs every execution outcome — success or failure. Never silently drops.
pub fn execute_commands(commands: &[BeadsCommand], agent_name: &str) {
    for cmd in commands {
        execute_command(cmd, agent_name);
    }
}

fn execute_command(cmd: &BeadsCommand, agent_name: &str) {
    match cmd {
        BeadsCommand::SendMessage { to, message } => {
            // Title format mirrors MCP server: [sender->recipient] preview...
            let preview: String = message.chars().take(60).collect();
            let preview = preview.replace('\n', " ");
            let title = format!("[{}->{}] {}", agent_name, to, preview);

            let result = std::process::Command::new(bd_path())
                .args(["create", &title, "-p", "3", "--type", "message", "-d", message])
                .env("BEADS_DIR", beads_dir())
                .env("BD_ACTOR", agent_name)
                .env("PATH", path_env())
                .output();

            log_result("send_message", &format!("to:{}", to), result);
        }

        BeadsCommand::UpdateTask { id, notes, status } => {
            let mut args = vec!["update", id.as_str(), "--notes", notes.as_str()];
            // status is optional — only append if present
            let status_str; // keep alive for the lifetime of args
            if let Some(s) = status {
                status_str = s.clone();
                args.push("--status");
                args.push(&status_str);
            }

            let result = std::process::Command::new(bd_path())
                .args(&args)
                .env("BEADS_DIR", beads_dir())
                .env("BD_ACTOR", agent_name)
                .env("PATH", path_env())
                .output();

            log_result("update_task", id, result);
        }

        BeadsCommand::StoreArtifact {
            task_id,
            artifact_type,
            value,
        } => {
            // Mirror MCP storeArtifact: append "artifact:<type>:<value>" to task notes
            let artifact_line = format!("artifact:{}:{}", artifact_type, value);

            let result = std::process::Command::new(bd_path())
                .args(["update", task_id, "--notes", &artifact_line])
                .env("BEADS_DIR", beads_dir())
                .env("BD_ACTOR", agent_name)
                .env("PATH", path_env())
                .output();

            log_result("store_artifact", task_id, result);
        }

        BeadsCommand::CloseTask { id, notes } => {
            let result = std::process::Command::new(bd_path())
                .args(["close", id, "--reason", notes])
                .env("BEADS_DIR", beads_dir())
                .env("BD_ACTOR", agent_name)
                .env("PATH", path_env())
                .output();

            log_result("close_task", id, result);
        }
    }
}

fn log_result(command: &str, target: &str, result: std::io::Result<std::process::Output>) {
    match result {
        Ok(output) if output.status.success() => {
            println!("[codex_harness] @@BEADS {} {}@@ — OK", command, target);
        }
        Ok(output) => {
            eprintln!(
                "[codex_harness] @@BEADS {} {}@@ — FAILED (exit {:?}): {}",
                command,
                target,
                output.status.code(),
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Err(e) => {
            eprintln!(
                "[codex_harness] @@BEADS {} {}@@ — ERROR (bd not found or spawn failed): {}",
                command, target, e
            );
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Internal BEADS query helpers
// ─────────────────────────────────────────────────────────────────────────────

fn query_unread(recipient: &str) -> Vec<BeadsMessage> {
    let query = format!("type=message AND status=open AND title=\"->{recipient}]\"");
    let output = std::process::Command::new(bd_path())
        .args(["query", &query, "--json", "-n", "0", "-q"])
        .env("BEADS_DIR", beads_dir())
        .env("BD_ACTOR", "codex-harness")
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

fn mark_read(message_id: &str) {
    let _ = std::process::Command::new(bd_path())
        .args(["close", message_id, "--reason", "delivered", "-q"])
        .env("BEADS_DIR", beads_dir())
        .env("BD_ACTOR", "codex-harness")
        .env("PATH", path_env())
        .output();
}

fn parse_sender(title: &str) -> String {
    // Title format: [sender->recipient] preview...
    if let Some(start) = title.find('[') {
        if let Some(arrow) = title.find("->") {
            return title[start + 1..arrow].to_string();
        }
    }
    "unknown".to_string()
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::beads_parser::parse_beads_blocks;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    // Serialize env-var-sensitive tests to prevent HOME races in parallel runs.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    // ── TmpHome: RAII temp directory ─────────────────────────────────────────

    /// A self-cleaning temporary HOME directory for tests.
    struct TmpHome {
        path: PathBuf,
    }

    impl TmpHome {
        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TmpHome {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    // ── Helpers ──────────────────────────────────────────────────────────────

    /// Create a self-contained temp HOME with a mock `bd` that logs every
    /// invocation to `$tmp/.bd-calls` and exits 0.
    fn setup_mock_home() -> TmpHome {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos();
        let pid = std::process::id();
        let dir = std::env::temp_dir().join(format!("aperture-test-home-{}-{}", pid, nanos));
        fs::create_dir_all(&dir).unwrap();

        let bin_dir = dir.join(".local/bin");
        fs::create_dir_all(&bin_dir).unwrap();

        let log_path = dir.join(".bd-calls");
        let script = format!(
            "#!/bin/bash\necho \"$@\" >> '{log}'\nexit 0\n",
            log = log_path.display()
        );
        let bd = bin_dir.join("bd");
        fs::write(&bd, &script).unwrap();
        fs::set_permissions(&bd, fs::Permissions::from_mode(0o755)).unwrap();

        // Create .beads dir so beads_dir() resolves without error
        let beads = dir.join(".aperture/.beads");
        fs::create_dir_all(&beads).unwrap();

        TmpHome { path: dir }
    }

    /// Read the mock bd call log, returning lines of space-separated args.
    fn read_bd_calls(tmp: &TmpHome) -> Vec<String> {
        let log_path = tmp.path().join(".bd-calls");
        match fs::read_to_string(&log_path) {
            Ok(s) => s.lines().map(String::from).collect(),
            Err(_) => vec![],
        }
    }

    // ── parse_sender (private helper) ────────────────────────────────────────

    #[test]
    fn parse_sender_standard_title() {
        assert_eq!(parse_sender("[wheatley->izzy] task complete"), "wheatley");
    }

    #[test]
    fn parse_sender_no_arrow_returns_unknown() {
        assert_eq!(parse_sender("no arrow here"), "unknown");
    }

    #[test]
    fn parse_sender_no_bracket_returns_unknown() {
        assert_eq!(parse_sender("glados -> izzy"), "unknown");
    }

    #[test]
    fn parse_sender_empty_string_returns_unknown() {
        assert_eq!(parse_sender(""), "unknown");
    }

    // ── buffer_pending_message ───────────────────────────────────────────────

    #[test]
    fn buffer_pending_message_creates_file_and_appends() {
        // pending_msgs_path is hardcoded under /tmp — no HOME manipulation needed.
        // Note: buffer_pending_message requires the parent directory to already
        // exist (it uses OpenOptions::create, not create_all). Pre-create it.
        let agent = format!("test-buf-{}", std::process::id());
        let path = pending_msgs_path(&agent);

        // Ensure parent dir exists; clear any stale file from prior runs.
        if let Some(parent) = std::path::Path::new(&path).parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let _ = fs::remove_file(&path);

        buffer_pending_message(&agent, "Hello from glados");
        buffer_pending_message(&agent, "Second message");

        let content = fs::read_to_string(&path)
            .unwrap_or_else(|_| panic!("pending-msgs file should exist at {path}"));

        assert!(content.contains("Hello from glados"), "first message missing");
        assert!(content.contains("Second message"), "second message missing");
        // Each buffer_pending_message call appends "---\n" as a separator
        assert_eq!(content.matches("---").count(), 2, "expected 2 separators");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn buffer_pending_message_creates_parent_dir_if_missing() {
        // buffer_pending_message now creates parent directories if absent.
        // Previously this was a known limitation — fixed after Izzy's Wave 3 audit.
        let agent = "test-no-such-dir-ever-xyzzy";
        let path = pending_msgs_path(agent);
        // Ensure parent dir does NOT exist before the call
        if let Some(parent) = std::path::Path::new(&path).parent() {
            let _ = fs::remove_dir_all(parent);
        }

        buffer_pending_message(agent, "probe"); // should create dir + file, not warn

        // File SHOULD now exist — parent dir was created automatically
        let content = fs::read_to_string(&path)
            .expect("pending-msgs file should be created even when parent dir was absent");
        assert!(content.contains("probe"), "message content missing");

        // Cleanup
        if let Some(parent) = std::path::Path::new(&path).parent() {
            let _ = fs::remove_dir_all(parent);
        }
    }

    // ── inject_pending_messages — no-message path ────────────────────────────

    #[test]
    fn inject_pending_messages_no_messages_returns_prompt_unchanged() {
        let _guard = ENV_LOCK.lock().unwrap();
        let tmp = setup_mock_home();
        let original_home = std::env::var("HOME").unwrap_or_default();

        // Override the mock bd to output "[]" for any query call (simulate no messages)
        let bin_dir = tmp.path().join(".local/bin");
        let bd_script = "#!/bin/bash\necho '[]'\nexit 0\n";
        fs::write(bin_dir.join("bd"), bd_script).unwrap();
        fs::set_permissions(
            bin_dir.join("bd"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();

        unsafe { std::env::set_var("HOME", tmp.path()) };
        let prompt = "You are a Codex test agent.".to_string();
        let result = inject_pending_messages("test-codex", prompt.clone());
        unsafe { std::env::set_var("HOME", &original_home) };

        assert_eq!(
            result, prompt,
            "prompt should be unchanged when there are no unread messages"
        );
    }

    // ── execute_commands — full round-trip ───────────────────────────────────

    /// Core E2E test: a raw Codex-style response containing a @@BEADS@@ block
    /// is parsed, then executed — verifying the full harness loop.
    ///
    /// Round-trip: raw text → parse_beads_blocks → execute_commands → bd invoked
    #[test]
    fn e2e_send_message_round_trip() {
        let _guard = ENV_LOCK.lock().unwrap();
        let tmp = setup_mock_home();
        let original_home = std::env::var("HOME").unwrap_or_default();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        // Simulate a Codex agent response that contains a BEADS command
        let codex_response = r#"I've finished the parser implementation.

@@BEADS send_message to:glados message:"Parser complete. All tests passing."@@"#;

        // Step 1: parse
        let commands = parse_beads_blocks(codex_response);
        assert_eq!(commands.len(), 1, "exactly one command should be parsed");
        assert!(
            matches!(
                &commands[0],
                BeadsCommand::SendMessage { to, message }
                    if to == "glados" && message == "Parser complete. All tests passing."
            ),
            "parsed command should be SendMessage to glados"
        );

        // Step 2: execute (calls mock bd)
        execute_commands(&commands, "test-codex");

        unsafe { std::env::set_var("HOME", &original_home) };

        // Step 3: verify bd was called with the right arguments
        let calls = read_bd_calls(&tmp);
        assert_eq!(calls.len(), 1, "bd should have been called once");

        let call = &calls[0];
        assert!(
            call.contains("create"),
            "bd should have been called with 'create': {call}"
        );
        assert!(
            call.contains("test-codex->glados"),
            "title should contain sender->recipient: {call}"
        );
        assert!(
            call.contains("--type"),
            "bd create should pass --type: {call}"
        );
        assert!(
            call.contains("message"),
            "artifact type 'message' should be in args: {call}"
        );
    }

    #[test]
    fn e2e_update_task_round_trip() {
        let _guard = ENV_LOCK.lock().unwrap();
        let tmp = setup_mock_home();
        let original_home = std::env::var("HOME").unwrap_or_default();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        let codex_response = concat!(
            "Starting work on the harness.\n",
            "@@BEADS update_task id:src-tauri-09k status:in_progress ",
            "notes:\"Implementation underway.\"@@"
        );

        let commands = parse_beads_blocks(codex_response);
        assert_eq!(commands.len(), 1);
        execute_commands(&commands, "test-codex");

        unsafe { std::env::set_var("HOME", &original_home) };

        let calls = read_bd_calls(&tmp);
        assert_eq!(calls.len(), 1);
        let call = &calls[0];
        assert!(call.contains("update"), "should call bd update: {call}");
        assert!(
            call.contains("src-tauri-09k"),
            "task id should appear: {call}"
        );
        assert!(
            call.contains("--status"),
            "status flag should appear: {call}"
        );
        assert!(
            call.contains("in_progress"),
            "status value should appear: {call}"
        );
    }

    #[test]
    fn e2e_close_task_round_trip() {
        let _guard = ENV_LOCK.lock().unwrap();
        let tmp = setup_mock_home();
        let original_home = std::env::var("HOME").unwrap_or_default();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        let codex_response =
            r#"@@BEADS close_task id:src-tauri-3ni notes:"Tests written and passing."@@"#;

        let commands = parse_beads_blocks(codex_response);
        assert_eq!(commands.len(), 1);
        execute_commands(&commands, "test-codex");

        unsafe { std::env::set_var("HOME", &original_home) };

        let calls = read_bd_calls(&tmp);
        assert_eq!(calls.len(), 1);
        let call = &calls[0];
        assert!(call.contains("close"), "should call bd close: {call}");
        assert!(
            call.contains("src-tauri-3ni"),
            "task id should appear: {call}"
        );
        assert!(
            call.contains("--reason"),
            "--reason flag should appear: {call}"
        );
    }

    #[test]
    fn e2e_multi_command_response_all_executed() {
        let _guard = ENV_LOCK.lock().unwrap();
        let tmp = setup_mock_home();
        let original_home = std::env::var("HOME").unwrap_or_default();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        // A realistic Codex multi-command response
        let codex_response = concat!(
            "Done with my work. Updating task and notifying.\n\n",
            "@@BEADS update_task id:src-tauri-09k status:done notes:\"E2E test written.\"@@\n",
            "@@BEADS store_artifact task_id:src-tauri-09k type:file value:src-tauri/src/codex_harness.rs@@\n",
            "@@BEADS send_message to:glados message:\"E2E round-trip test complete.\"@@"
        );

        let commands = parse_beads_blocks(codex_response);
        assert_eq!(commands.len(), 3, "all three blocks should parse");

        execute_commands(&commands, "test-codex");

        unsafe { std::env::set_var("HOME", &original_home) };

        // All three bd invocations should have been logged
        let calls = read_bd_calls(&tmp);
        assert_eq!(calls.len(), 3, "bd should be called once per command");
    }

    #[test]
    fn e2e_malformed_blocks_do_not_reach_bd() {
        let _guard = ENV_LOCK.lock().unwrap();
        let tmp = setup_mock_home();
        let original_home = std::env::var("HOME").unwrap_or_default();
        unsafe { std::env::set_var("HOME", tmp.path()) };

        // Mix of malformed and one valid block
        let codex_response = concat!(
            "@@BEADS bananas to:nobody@@\n",
            "@@BEADS send_message to:peppy@@\n", // missing message
            "@@BEADS send_message to:peppy message:\"only valid one\"@@"
        );

        let commands = parse_beads_blocks(codex_response);
        assert_eq!(commands.len(), 1, "only the valid block should parse");
        execute_commands(&commands, "test-codex");

        unsafe { std::env::set_var("HOME", &original_home) };

        let calls = read_bd_calls(&tmp);
        assert_eq!(calls.len(), 1, "bd should only be called for valid commands");
        assert!(calls[0].contains("peppy"), "call should reference peppy");
    }
}
