//! Parser for `@@BEADS ... @@` command blocks emitted by Codex agents.
//!
//! This module is **pure** — it takes raw text and returns structured command
//! objects. No BEADS calls, no side effects, no I/O (except `eprintln!` warnings).
//!
//! # Usage
//!
//! ```rust,no_run
//! // use aperture_lib::beads_parser::{parse_beads_blocks, BeadsCommand};
//!
//! // let text = r#"@@BEADS send_message to:glados message:"Wave 0 complete."@@"#;
//! // let commands = parse_beads_blocks(text);
//! // commands = [BeadsCommand::SendMessage { to: "glados", message: "Wave 0 complete." }]
//! ```
//!
//! See `docs/codex-beads-syntax.md` for the full canonical spec (task src-tauri-fdx).

use std::collections::HashMap;

// ───────────────────────────────────────────────────────────────────────────
// Public types
// ───────────────────────────────────────────────────────────────────────────

/// A parsed BEADS command, ready for execution by the harness.
///
/// All variants own their data — callers can store and pass these freely.
#[derive(Debug, Clone, PartialEq)]
pub enum BeadsCommand {
    /// `@@BEADS send_message to:<agent> message:"<text>"@@`
    SendMessage {
        to: String,
        message: String,
    },
    /// `@@BEADS update_task id:<task-id> notes:"<text>" [status:<status>]@@`
    UpdateTask {
        id: String,
        notes: String,
        /// Optional — if absent, the task status is left unchanged.
        status: Option<String>,
    },
    /// `@@BEADS store_artifact task_id:<id> type:<type> value:<value>@@`
    StoreArtifact {
        task_id: String,
        /// Artifact type string — e.g. "file", "url", "note", "pr", "session".
        artifact_type: String,
        value: String,
    },
    /// `@@BEADS close_task id:<task-id> notes:"<text>"@@`
    CloseTask {
        id: String,
        notes: String,
    },
}

// ───────────────────────────────────────────────────────────────────────────
// Public API
// ───────────────────────────────────────────────────────────────────────────

/// Parse all `@@BEADS ... @@` blocks from a raw text string.
///
/// # Guarantees
/// - Commands are returned in **document order** (top-to-bottom, left-to-right per line).
/// - Malformed blocks are logged via `eprintln!` and skipped — this function **never panics**.
/// - Multiple blocks on the same line are supported.
/// - Blocks may appear anywhere in a line (inline with prose).
pub fn parse_beads_blocks(text: &str) -> Vec<BeadsCommand> {
    let mut commands = Vec::new();

    for line in text.lines() {
        // `search` is a moving window into the current line.
        // We advance it past each processed `@@BEADS ... @@` block.
        let mut search = line;

        while let Some(start) = search.find("@@BEADS") {
            let after_marker = &search[start + 7..]; // "@@BEADS" is 7 chars

            match after_marker.find("@@") {
                None => {
                    eprintln!(
                        "[beads_parser] warn: unclosed @@BEADS block: '{}'",
                        truncate(search, 80)
                    );
                    break; // nothing more to parse on this line
                }
                Some(end_idx) => {
                    let content = after_marker[..end_idx].trim();
                    if let Some(cmd) = parse_block_content(content) {
                        commands.push(cmd);
                    }
                    // Advance past the closing @@
                    search = &after_marker[end_idx + 2..];
                }
            }
        }
    }

    commands
}

// ───────────────────────────────────────────────────────────────────────────
// Internal: block content → BeadsCommand
// ───────────────────────────────────────────────────────────────────────────

/// Parse the text between `@@BEADS` and the closing `@@` into a `BeadsCommand`.
///
/// Returns `None` (with a logged warning) if the content is malformed or
/// the command name is unknown.
fn parse_block_content(content: &str) -> Option<BeadsCommand> {
    if content.is_empty() {
        eprintln!("[beads_parser] warn: empty @@BEADS block — skipping");
        return None;
    }

    // Split off the command name (first whitespace-delimited token).
    let (cmd_name, fields_str) = match content.find(|c: char| c.is_whitespace()) {
        Some(ws_idx) => (&content[..ws_idx], &content[ws_idx..]),
        None => (content, ""), // command with no fields
    };

    let fields = parse_fields(fields_str);

    match cmd_name {
        "send_message" => {
            let to = require_field(&fields, "to", "send_message")?;
            let message = require_field(&fields, "message", "send_message")?;
            Some(BeadsCommand::SendMessage { to, message })
        }
        "update_task" => {
            let id = require_field(&fields, "id", "update_task")?;
            let notes = require_field(&fields, "notes", "update_task")?;
            let status = fields.get("status").cloned();
            Some(BeadsCommand::UpdateTask { id, notes, status })
        }
        "store_artifact" => {
            let task_id = require_field(&fields, "task_id", "store_artifact")?;
            let artifact_type = require_field(&fields, "type", "store_artifact")?;
            let value = require_field(&fields, "value", "store_artifact")?;
            Some(BeadsCommand::StoreArtifact {
                task_id,
                artifact_type,
                value,
            })
        }
        "close_task" => {
            let id = require_field(&fields, "id", "close_task")?;
            let notes = require_field(&fields, "notes", "close_task")?;
            Some(BeadsCommand::CloseTask { id, notes })
        }
        other => {
            eprintln!(
                "[beads_parser] warn: unknown command '{}' — skipping block",
                other
            );
            None
        }
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Internal: field string parser
// ───────────────────────────────────────────────────────────────────────────

/// Parse a field string like `to:glados message:"hello world" status:done`
/// into a `HashMap<String, String>`.
///
/// Rules:
/// - Keys are terminated by `:`.
/// - Values are either bare (no spaces) or double-quoted.
/// - Quoted values support `\"` (literal `"`) and `\\` (literal `\`) escapes.
/// - Unknown field keys are warned about but still inserted (executor decides).
/// - Unclosed quoted values are warned about; the partial value is accepted.
fn parse_fields(s: &str) -> HashMap<String, String> {
    let mut fields = HashMap::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        // ── Skip leading whitespace ──────────────────────────────────────
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        if i >= chars.len() {
            break;
        }

        // ── Read key (up to ':') ─────────────────────────────────────────
        let key_start = i;
        while i < chars.len() && chars[i] != ':' && !chars[i].is_whitespace() {
            i += 1;
        }

        if i >= chars.len() || chars[i] != ':' {
            // Orphan token with no colon — warn if non-empty.
            if i > key_start {
                let orphan: String = chars[key_start..i].iter().collect();
                eprintln!(
                    "[beads_parser] warn: token without ':' separator: '{}' — skipping",
                    orphan
                );
            }
            break;
        }

        let key: String = chars[key_start..i].iter().collect();
        i += 1; // consume ':'

        // ── Read value ───────────────────────────────────────────────────
        if i >= chars.len() {
            eprintln!(
                "[beads_parser] warn: key '{}' has no value — storing empty string",
                key
            );
            fields.insert(key, String::new());
            break;
        }

        let value = if chars[i] == '"' {
            // Quoted value — parse until unescaped closing '"'
            i += 1; // consume opening '"'
            let mut val = String::new();
            let mut closed = false;

            while i < chars.len() {
                match chars[i] {
                    '\\' if i + 1 < chars.len() => {
                        i += 1; // consume '\'
                        match chars[i] {
                            '"' => val.push('"'),
                            '\\' => val.push('\\'),
                            c => {
                                // Unknown escape — preserve literally per spec
                                val.push('\\');
                                val.push(c);
                            }
                        }
                        i += 1;
                    }
                    '"' => {
                        i += 1; // consume closing '"'
                        closed = true;
                        break;
                    }
                    c => {
                        val.push(c);
                        i += 1;
                    }
                }
            }

            if !closed {
                eprintln!(
                    "[beads_parser] warn: unclosed quoted value for key '{}' — accepted as-is",
                    key
                );
            }
            val
        } else {
            // Bare value — everything up to next whitespace
            let val_start = i;
            while i < chars.len() && !chars[i].is_whitespace() {
                i += 1;
            }
            chars[val_start..i].iter().collect()
        };

        // Warn on unknown field keys (still insert — executor owns the decision)
        if !is_known_field(&key) {
            eprintln!(
                "[beads_parser] warn: unknown field key '{}' — stored but may be ignored by executor",
                key
            );
        }

        fields.insert(key, value);
    }

    fields
}

// ───────────────────────────────────────────────────────────────────────────
// Internal helpers
// ───────────────────────────────────────────────────────────────────────────

/// Require a field by name, logging a warning and returning `None` if absent.
fn require_field(fields: &HashMap<String, String>, key: &str, cmd: &str) -> Option<String> {
    match fields.get(key) {
        Some(v) => Some(v.clone()),
        None => {
            eprintln!(
                "[beads_parser] warn: missing required field '{}' for command '{}' — skipping block",
                key, cmd
            );
            None
        }
    }
}

/// Known field keys across all commands. Used to warn on unexpected keys.
fn is_known_field(key: &str) -> bool {
    matches!(
        key,
        "to" | "message" | "id" | "notes" | "status" | "task_id" | "type" | "value"
    )
}

/// Truncate a string to `max` chars for display in warnings.
fn truncate(s: &str, max: usize) -> String {
    let mut out: String = s.chars().take(max).collect();
    if s.chars().count() > max {
        out.push_str("…");
    }
    out
}

// ───────────────────────────────────────────────────────────────────────────
// Unit tests
// ───────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Happy path: individual commands ────────────────────────────────────

    #[test]
    fn send_message_bare_to() {
        let cmds = parse_beads_blocks(
            r#"@@BEADS send_message to:glados message:"Wave 0 complete."@@"#,
        );
        assert_eq!(cmds.len(), 1);
        assert_eq!(
            cmds[0],
            BeadsCommand::SendMessage {
                to: "glados".into(),
                message: "Wave 0 complete.".into(),
            }
        );
    }

    #[test]
    fn send_message_quoted_to() {
        let cmds = parse_beads_blocks(
            r#"@@BEADS send_message to:"planner" message:"hello"@@"#,
        );
        assert_eq!(cmds.len(), 1);
        match &cmds[0] {
            BeadsCommand::SendMessage { to, message } => {
                assert_eq!(to, "planner");
                assert_eq!(message, "hello");
            }
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn update_task_without_status() {
        let cmds = parse_beads_blocks(
            r#"@@BEADS update_task id:src-tauri-a2z notes:"Parser implemented."@@"#,
        );
        assert_eq!(cmds.len(), 1);
        assert_eq!(
            cmds[0],
            BeadsCommand::UpdateTask {
                id: "src-tauri-a2z".into(),
                notes: "Parser implemented.".into(),
                status: None,
            }
        );
    }

    #[test]
    fn update_task_with_status() {
        let cmds = parse_beads_blocks(
            r#"@@BEADS update_task id:src-tauri-a2z status:in_progress notes:"Starting work."@@"#,
        );
        assert_eq!(cmds.len(), 1);
        assert_eq!(
            cmds[0],
            BeadsCommand::UpdateTask {
                id: "src-tauri-a2z".into(),
                notes: "Starting work.".into(),
                status: Some("in_progress".into()),
            }
        );
    }

    #[test]
    fn store_artifact_bare_value() {
        let cmds = parse_beads_blocks(
            r#"@@BEADS store_artifact task_id:src-tauri-a2z type:file value:src-tauri/src/beads_parser.rs@@"#,
        );
        assert_eq!(cmds.len(), 1);
        assert_eq!(
            cmds[0],
            BeadsCommand::StoreArtifact {
                task_id: "src-tauri-a2z".into(),
                artifact_type: "file".into(),
                value: "src-tauri/src/beads_parser.rs".into(),
            }
        );
    }

    #[test]
    fn store_artifact_quoted_value() {
        let cmds = parse_beads_blocks(
            r#"@@BEADS store_artifact task_id:src-tauri-ae5 type:note value:"Prompt updated in codex agent template"@@"#,
        );
        assert_eq!(cmds.len(), 1);
        match &cmds[0] {
            BeadsCommand::StoreArtifact {
                task_id,
                artifact_type,
                value,
            } => {
                assert_eq!(task_id, "src-tauri-ae5");
                assert_eq!(artifact_type, "note");
                assert_eq!(value, "Prompt updated in codex agent template");
            }
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn store_artifact_url() {
        let cmds = parse_beads_blocks(
            r#"@@BEADS store_artifact task_id:src-tauri-ae5 type:url value:https://github.com/aperture/pull/42@@"#,
        );
        assert_eq!(cmds.len(), 1);
        match &cmds[0] {
            BeadsCommand::StoreArtifact { value, .. } => {
                assert_eq!(value, "https://github.com/aperture/pull/42");
            }
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn close_task() {
        let cmds = parse_beads_blocks(
            r#"@@BEADS close_task id:src-tauri-fdx notes:"Spec locked and stored as artifact."@@"#,
        );
        assert_eq!(cmds.len(), 1);
        assert_eq!(
            cmds[0],
            BeadsCommand::CloseTask {
                id: "src-tauri-fdx".into(),
                notes: "Spec locked and stored as artifact.".into(),
            }
        );
    }

    // ── Multi-block: ordering and mixing ───────────────────────────────────

    #[test]
    fn multiple_blocks_document_order() {
        let text = concat!(
            "@@BEADS update_task id:task-1 status:in_progress notes:\"starting\"@@\n",
            "@@BEADS update_task id:task-1 notes:\"done\"@@\n",
            "@@BEADS close_task id:task-1 notes:\"complete\"@@",
        );
        let cmds = parse_beads_blocks(text);
        assert_eq!(cmds.len(), 3);

        match &cmds[0] {
            BeadsCommand::UpdateTask {
                status: Some(s), ..
            } => assert_eq!(s, "in_progress"),
            _ => panic!("expected UpdateTask with status"),
        }
        match &cmds[1] {
            BeadsCommand::UpdateTask {
                status: None,
                notes,
                ..
            } => assert_eq!(notes, "done"),
            _ => panic!("expected UpdateTask without status"),
        }
        match &cmds[2] {
            BeadsCommand::CloseTask { .. } => {}
            _ => panic!("expected CloseTask"),
        }
    }

    #[test]
    fn blocks_inline_with_prose() {
        let text =
            r#"Here is my update: @@BEADS update_task id:t-1 notes:"inline update"@@ — done."#;
        let cmds = parse_beads_blocks(text);
        assert_eq!(cmds.len(), 1);
        match &cmds[0] {
            BeadsCommand::UpdateTask { notes, .. } => assert_eq!(notes, "inline update"),
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn two_blocks_same_line() {
        let text = r#"@@BEADS send_message to:a message:"hi"@@ @@BEADS send_message to:b message:"bye"@@"#;
        let cmds = parse_beads_blocks(text);
        assert_eq!(cmds.len(), 2);
        match (&cmds[0], &cmds[1]) {
            (
                BeadsCommand::SendMessage { to: t1, .. },
                BeadsCommand::SendMessage { to: t2, .. },
            ) => {
                assert_eq!(t1, "a");
                assert_eq!(t2, "b");
            }
            _ => panic!("unexpected variants"),
        }
    }

    // ── Escape handling ─────────────────────────────────────────────────────

    #[test]
    fn escaped_quote_in_value() {
        let cmds = parse_beads_blocks(
            r#"@@BEADS send_message to:glados message:"She said \"hello\"."@@"#,
        );
        assert_eq!(cmds.len(), 1);
        match &cmds[0] {
            BeadsCommand::SendMessage { message, .. } => {
                assert_eq!(message, r#"She said "hello"."#);
            }
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn escaped_backslash_in_value() {
        let cmds = parse_beads_blocks(
            r#"@@BEADS send_message to:glados message:"path\\to\\file"@@"#,
        );
        assert_eq!(cmds.len(), 1);
        match &cmds[0] {
            BeadsCommand::SendMessage { message, .. } => {
                assert_eq!(message, r"path\to\file");
            }
            _ => panic!("unexpected variant"),
        }
    }

    // ── Error handling: malformed blocks ───────────────────────────────────

    #[test]
    fn unknown_command_skipped() {
        let cmds = parse_beads_blocks(r#"@@BEADS bananas to:glados@@"#);
        assert!(cmds.is_empty());
    }

    #[test]
    fn missing_required_field_skipped() {
        // Missing 'message' — send_message requires both 'to' and 'message'
        let cmds = parse_beads_blocks(r#"@@BEADS send_message to:glados@@"#);
        assert!(cmds.is_empty());
    }

    #[test]
    fn unclosed_block_skipped() {
        let cmds = parse_beads_blocks(r#"@@BEADS send_message to:glados message:"no close"#);
        assert!(cmds.is_empty());
    }

    #[test]
    fn valid_block_after_malformed_still_parsed() {
        let text = concat!(
            "@@BEADS unknown_cmd foo:bar@@\n",
            "@@BEADS send_message to:glados message:\"valid\"@@",
        );
        let cmds = parse_beads_blocks(text);
        assert_eq!(cmds.len(), 1);
        match &cmds[0] {
            BeadsCommand::SendMessage { to, .. } => assert_eq!(to, "glados"),
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn empty_text_returns_empty() {
        assert!(parse_beads_blocks("").is_empty());
    }

    #[test]
    fn no_beads_blocks_returns_empty() {
        assert!(parse_beads_blocks("Just a regular response. No commands here.").is_empty());
    }

    #[test]
    fn empty_block_content_skipped() {
        // "@@BEADS@@" — no content between markers
        let cmds = parse_beads_blocks("@@BEADS@@");
        assert!(cmds.is_empty());
    }

    #[test]
    fn unknown_field_key_does_not_break_parsing() {
        // 'extra' is unknown — should warn but still parse known fields
        let cmds = parse_beads_blocks(
            r#"@@BEADS update_task id:t-1 extra:foo notes:"real note"@@"#,
        );
        assert_eq!(cmds.len(), 1);
        match &cmds[0] {
            BeadsCommand::UpdateTask { notes, .. } => assert_eq!(notes, "real note"),
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn fields_in_any_order() {
        // notes before id — order should not matter
        let cmds = parse_beads_blocks(
            r#"@@BEADS update_task notes:"first" id:t-2 status:done@@"#,
        );
        assert_eq!(cmds.len(), 1);
        assert_eq!(
            cmds[0],
            BeadsCommand::UpdateTask {
                id: "t-2".into(),
                notes: "first".into(),
                status: Some("done".into()),
            }
        );
    }

    // ── Missing required fields — all commands ─────────────────────────────

    #[test]
    fn send_message_missing_to_skipped() {
        // 'to' is required — block should be skipped
        let cmds = parse_beads_blocks(r#"@@BEADS send_message message:"hello"@@"#);
        assert!(cmds.is_empty());
    }

    #[test]
    fn update_task_missing_id_skipped() {
        let cmds = parse_beads_blocks(r#"@@BEADS update_task notes:"doing stuff"@@"#);
        assert!(cmds.is_empty());
    }

    #[test]
    fn update_task_missing_notes_skipped() {
        let cmds = parse_beads_blocks(r#"@@BEADS update_task id:task-123@@"#);
        assert!(cmds.is_empty());
    }

    #[test]
    fn close_task_missing_id_skipped() {
        let cmds = parse_beads_blocks(r#"@@BEADS close_task notes:"all done"@@"#);
        assert!(cmds.is_empty());
    }

    #[test]
    fn close_task_missing_notes_skipped() {
        let cmds = parse_beads_blocks(r#"@@BEADS close_task id:task-abc@@"#);
        assert!(cmds.is_empty());
    }

    #[test]
    fn store_artifact_missing_task_id_skipped() {
        let cmds =
            parse_beads_blocks(r#"@@BEADS store_artifact type:file value:src/main.rs@@"#);
        assert!(cmds.is_empty());
    }

    #[test]
    fn store_artifact_missing_type_skipped() {
        let cmds =
            parse_beads_blocks(r#"@@BEADS store_artifact task_id:t-1 value:src/main.rs@@"#);
        assert!(cmds.is_empty());
    }

    #[test]
    fn store_artifact_missing_value_skipped() {
        let cmds =
            parse_beads_blocks(r#"@@BEADS store_artifact task_id:t-1 type:file@@"#);
        assert!(cmds.is_empty());
    }

    // ── Command-only block (no fields at all) ───────────────────────────────

    #[test]
    fn command_only_no_fields_skipped() {
        // A command with no fields is missing all required fields — should skip
        let cmds = parse_beads_blocks(r#"@@BEADS send_message@@"#);
        assert!(cmds.is_empty());
    }

    // ── Empty and whitespace edge cases ────────────────────────────────────

    #[test]
    fn empty_quoted_value_accepted() {
        // message:"" is valid — empty string value
        let cmds = parse_beads_blocks(
            r#"@@BEADS send_message to:glados message:""@@"#,
        );
        assert_eq!(cmds.len(), 1);
        match &cmds[0] {
            BeadsCommand::SendMessage { message, .. } => {
                assert_eq!(message, "");
            }
            _ => panic!("unexpected variant"),
        }
    }

    #[test]
    fn whitespace_only_block_content_skipped() {
        // @@BEADS   @@ — only whitespace between markers → empty content → skip
        let cmds = parse_beads_blocks("@@BEADS   @@");
        assert!(cmds.is_empty());
    }

    // ── Escape sequences ───────────────────────────────────────────────────

    #[test]
    fn unknown_escape_sequence_preserved_literally() {
        // The spec only defines \" and \\ as escape sequences.
        // Unknown escapes (e.g. \n, \t) must be preserved literally: \<char>
        let cmds = parse_beads_blocks(
            r#"@@BEADS send_message to:glados message:"line1\nline2"@@"#,
        );
        assert_eq!(cmds.len(), 1);
        match &cmds[0] {
            BeadsCommand::SendMessage { message, .. } => {
                // \n is unknown → preserved as backslash + 'n' (two chars)
                assert_eq!(message, r"line1\nline2");
            }
            _ => panic!("unexpected variant"),
        }
    }

    // ── Multi-block: 3 blocks on same line ─────────────────────────────────

    #[test]
    fn three_blocks_same_line() {
        let text = concat!(
            r#"@@BEADS send_message to:a message:"one"@@ "#,
            r#"@@BEADS send_message to:b message:"two"@@ "#,
            r#"@@BEADS send_message to:c message:"three"@@"#,
        );
        let cmds = parse_beads_blocks(text);
        assert_eq!(cmds.len(), 3);
        let recipients: Vec<&str> = cmds
            .iter()
            .map(|c| match c {
                BeadsCommand::SendMessage { to, .. } => to.as_str(),
                _ => panic!("unexpected variant"),
            })
            .collect();
        assert_eq!(recipients, ["a", "b", "c"]);
    }

    // ── Parser limitation: @@ inside a quoted value ─────────────────────────

    #[test]
    fn double_at_inside_quoted_value_truncates_block() {
        // If a message value contains "@@", the parser's closing-marker search
        // terminates early — the block becomes malformed or parsed with a
        // truncated message. This is a known parser limitation documented here.
        //
        // Input:  @@BEADS send_message to:a message:"use @@BEADS@@ syntax"@@
        // The parser finds the first @@ after "@@BEADS" at "use " → content is
        // "send_message to:a message:\"use " which is a malformed block.
        // The result: 0 or 1 commands depending on where the @@ lands.
        // The invariant we test: the parser must NOT panic.
        let text = r#"@@BEADS send_message to:a message:"use @@BEADS@@ syntax"@@"#;
        let cmds = parse_beads_blocks(text); // must not panic
        // We don't assert a specific count — the exact result depends on where
        // the premature @@ falls. What matters is no crash and no partial state.
        let _ = cmds;
    }

    // ── Unicode in values ──────────────────────────────────────────────────

    #[test]
    fn unicode_and_emoji_in_message() {
        let cmds = parse_beads_blocks(
            "@@BEADS send_message to:glados message:\"Test complete \u{2705} \u{1F9EA}\"@@",
        );
        assert_eq!(cmds.len(), 1);
        match &cmds[0] {
            BeadsCommand::SendMessage { message, .. } => {
                assert!(message.contains('\u{2705}')); // ✅
                assert!(message.contains('\u{1F9EA}')); // 🧪
            }
            _ => panic!("unexpected variant"),
        }
    }

    // ── Recovery: multiple malformed then multiple valid ───────────────────

    #[test]
    fn multiple_malformed_then_multiple_valid() {
        let text = concat!(
            "@@BEADS unknown_cmd x:y@@\n",
            "@@BEADS send_message to:glados@@\n", // missing message
            "@@BEADS send_message to:peppy message:\"hello\"@@\n",
            "@@BEADS close_task id:t-1 notes:\"done\"@@",
        );
        let cmds = parse_beads_blocks(text);
        assert_eq!(cmds.len(), 2);
        match &cmds[0] {
            BeadsCommand::SendMessage { to, .. } => assert_eq!(to, "peppy"),
            _ => panic!("expected SendMessage"),
        }
        match &cmds[1] {
            BeadsCommand::CloseTask { id, .. } => assert_eq!(id, "t-1"),
            _ => panic!("expected CloseTask"),
        }
    }
}
