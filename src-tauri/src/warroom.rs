use crate::state::AppState;
use crate::tmux;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, Write};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WarRoomState {
    pub id: String,
    pub topic: String,
    pub participants: Vec<String>,
    pub current_turn: usize,
    pub current_agent: String,
    pub round: usize,
    pub status: String,
    pub created_at: String,
    #[serde(default)]
    pub conclude_votes: Vec<String>,
    /// Tracks the transcript index each agent has already received.
    /// On delivery, agents only get transcript[cursor..], then cursor advances to transcript.len().
    #[serde(default)]
    pub read_cursors: HashMap<String, usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptEntry {
    pub role: String,
    pub content: String,
    pub timestamp: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub round: Option<usize>,
}

fn warroom_dir() -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let dir = format!("{}/.aperture/warroom", home);
    let _ = fs::create_dir_all(&dir);
    dir
}

fn state_path() -> String {
    format!("{}/state.json", warroom_dir())
}

fn transcript_path() -> String {
    format!("{}/transcript.jsonl", warroom_dir())
}

fn read_state() -> Option<WarRoomState> {
    let path = state_path();
    let data = fs::read_to_string(&path).ok()?;
    serde_json::from_str(&data).ok()
}

fn write_state(state: &WarRoomState) {
    let path = state_path();
    if let Ok(data) = serde_json::to_string_pretty(state) {
        let _ = fs::write(&path, data);
    }
}

fn append_transcript(entry: &TranscriptEntry) {
    let path = transcript_path();
    if let Ok(data) = serde_json::to_string(entry) {
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) {
            let _ = writeln!(file, "{}", data);
        }
    }
}

fn read_transcript() -> Vec<TranscriptEntry> {
    let path = transcript_path();
    let file = match fs::File::open(&path) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };
    let reader = std::io::BufReader::new(file);
    reader
        .lines()
        .flatten()
        .filter_map(|line| serde_json::from_str(&line).ok())
        .collect()
}

fn now_iso() -> String {
    let output = std::process::Command::new("date")
        .args(["-u", "+%Y-%m-%dT%H:%M:%SZ"])
        .output();
    match output {
        Ok(o) => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        Err(_) => "unknown".into(),
    }
}

fn now_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn deliver_to_agent(agent_name: &str, app_state: &Arc<Mutex<AppState>>) -> Result<(), String> {
    let mut state = read_state().ok_or("No active war room")?;
    let transcript = read_transcript();

    // Determine what this agent has already seen
    let cursor = state.read_cursors.get(agent_name).copied().unwrap_or(0);
    let is_first_turn = cursor == 0;
    let new_entries = &transcript[cursor.min(transcript.len())..];

    // Format only the unseen entries
    let mut formatted = String::new();
    for entry in new_entries {
        formatted.push_str(&format!("[{}]: {}\n", entry.role.to_uppercase(), entry.content));
    }

    let context = if is_first_turn {
        // First turn: full brief + instructions + full transcript
        format!(
            "# WAR ROOM — {topic}\n## Room: {id} | Round {round}\n\n\
            ⚠️ THIS IS A BRAND NEW DISCUSSION (Room ID: {id}). Forget any previous War Room conversations. Start fresh.\n\n\
            You are participating in a War Room discussion. Read the transcript below and share your perspective.\n\
            When done, respond using: send_message(to: \"warroom\", message: \"your contribution\")\n\
            DO NOT reply in the terminal — use the send_message MCP tool.\n\
            DO NOT reference previous War Room discussions — focus only on the topic and transcript shown here.\n\n\
            ---\n{transcript}\n---\n\n\
            It is now YOUR turn ({agent}). Share your perspective on the topic above.\n",
            topic = state.topic,
            id = state.id,
            round = state.round,
            transcript = formatted,
            agent = agent_name,
        )
    } else {
        // Subsequent turns: compact header + only new messages since last turn
        format!(
            "# WAR ROOM — {topic}\n## Room: {id} | Round {round}\n\n\
            ---\n{transcript}\n---\n\n\
            It is now YOUR turn ({agent}).\n",
            topic = state.topic,
            id = state.id,
            round = state.round,
            transcript = if formatted.is_empty() { "(no new messages since your last turn)\n".into() } else { formatted },
            agent = agent_name,
        )
    };

    // Advance the agent's read cursor to the current end of transcript
    state.read_cursors.insert(agent_name.to_string(), transcript.len());
    write_state(&state);

    let (is_spiderling, window_id) = {
        let locked = app_state.lock().map_err(|e| e.to_string())?;
        let is_spider = locked.spiderlings.contains_key(agent_name);
        let wid = locked
            .agents
            .get(agent_name)
            .and_then(|a| a.tmux_window_id.clone())
            .or_else(|| {
                locked
                    .spiderlings
                    .get(agent_name)
                    .and_then(|s| s.tmux_window_id.clone())
            })
            .ok_or_else(|| format!("Agent {} has no tmux window", agent_name))?;
        (is_spider, wid)
    };

    let spiderling_note = if is_spiderling {
        "⚠️ NOTE: You are a spiderling. Pause your current task to participate in this discussion. Return to your task after you have sent your war room contribution.\n\n"
    } else {
        ""
    };

    let full_context = format!("{}{}", spiderling_note, context);

    let context_path = "/tmp/aperture-warroom-context.md";
    fs::write(context_path, &full_context).map_err(|e| e.to_string())?;

    tmux::tmux_send_keys(window_id, format!("cat {}", context_path))?;

    Ok(())
}

#[tauri::command]
pub fn create_warroom(
    topic: String,
    participants: Vec<String>,
    state: tauri::State<'_, Arc<Mutex<AppState>>>,
) -> Result<(), String> {
    if participants.is_empty() {
        return Err("At least one participant is required".into());
    }

    // Ensure warroom dir exists
    let _ = fs::create_dir_all(warroom_dir());

    // Create warroom mailbox and flush any stale files
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let warroom_mailbox = format!("{}/.aperture/mailbox/warroom", home);
    let _ = fs::create_dir_all(&warroom_mailbox);
    if let Ok(entries) = fs::read_dir(&warroom_mailbox) {
        for entry in entries.flatten() {
            let _ = fs::remove_file(entry.path());
        }
    }

    let id = format!("wr-{}", now_millis());
    let first_agent = participants[0].clone();

    let wr_state = WarRoomState {
        id,
        topic: topic.clone(),
        participants: participants.clone(),
        current_turn: 0,
        current_agent: first_agent.clone(),
        round: 1,
        status: "active".into(),
        created_at: now_iso(),
        conclude_votes: vec![],
        read_cursors: HashMap::new(),
    };

    write_state(&wr_state);

    let participants_str = participants.join(", ");
    append_transcript(&TranscriptEntry {
        role: "system".into(),
        content: format!(
            "War Room started. Topic: {}. Participants: {}",
            topic, participants_str
        ),
        timestamp: now_iso(),
        round: Some(1),
    });

    // Deliver initial context to the first agent
    let arc_state = state.inner().clone();
    deliver_to_agent(&first_agent, &arc_state)?;

    Ok(())
}

#[tauri::command]
pub fn get_warroom_state() -> Result<serde_json::Value, String> {
    match read_state() {
        Some(s) => serde_json::to_value(&s).map_err(|e| e.to_string()),
        None => Ok(serde_json::Value::Null),
    }
}

#[tauri::command]
pub fn get_warroom_transcript() -> Result<serde_json::Value, String> {
    let transcript = read_transcript();
    serde_json::to_value(&transcript).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn warroom_interject(message: String) -> Result<(), String> {
    append_transcript(&TranscriptEntry {
        role: "operator".into(),
        content: message,
        timestamp: now_iso(),
        round: read_state().map(|s| s.round),
    });
    Ok(())
}

#[tauri::command]
pub fn warroom_skip(state: tauri::State<'_, Arc<Mutex<AppState>>>) -> Result<(), String> {
    let mut wr_state = read_state().ok_or("No active war room")?;

    append_transcript(&TranscriptEntry {
        role: "system".into(),
        content: format!("{} was skipped", wr_state.current_agent),
        timestamp: now_iso(),
        round: Some(wr_state.round),
    });

    // Advance turn
    let next_turn = (wr_state.current_turn + 1) % wr_state.participants.len();
    if next_turn <= wr_state.current_turn {
        wr_state.round += 1;
    }
    wr_state.current_turn = next_turn;
    wr_state.current_agent = wr_state.participants[next_turn].clone();
    write_state(&wr_state);

    let arc_state = state.inner().clone();
    deliver_to_agent(&wr_state.current_agent, &arc_state)?;

    Ok(())
}

#[tauri::command]
pub fn warroom_conclude() -> Result<(), String> {
    let mut wr_state = read_state().ok_or("No active war room")?;
    wr_state.status = "concluded".into();

    append_transcript(&TranscriptEntry {
        role: "system".into(),
        content: "War Room concluded by operator".into(),
        timestamp: now_iso(),
        round: Some(wr_state.round),
    });

    write_state(&wr_state);

    // Archive transcript and state
    let history_dir = format!("{}/history", warroom_dir());
    let _ = fs::create_dir_all(&history_dir);
    let archive_transcript = format!("{}/{}.jsonl", history_dir, wr_state.id);
    let archive_state = format!("{}/{}.state.json", history_dir, wr_state.id);
    let _ = fs::copy(transcript_path(), &archive_transcript);
    let _ = fs::write(&archive_state, serde_json::to_string_pretty(&wr_state).unwrap_or_default());

    // Clean up state, transcript, and any pending warroom mailbox files
    let _ = fs::remove_file(state_path());
    let _ = fs::remove_file(transcript_path());
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let warroom_mailbox = format!("{}/.aperture/mailbox/warroom", home);
    if let Ok(entries) = fs::read_dir(&warroom_mailbox) {
        for entry in entries.flatten() {
            let _ = fs::remove_file(entry.path());
        }
    }

    Ok(())
}

#[tauri::command]
pub fn list_warroom_history() -> Result<serde_json::Value, String> {
    let history_dir = format!("{}/history", warroom_dir());
    let _ = fs::create_dir_all(&history_dir);

    let mut rooms: Vec<serde_json::Value> = Vec::new();

    if let Ok(entries) = fs::read_dir(&history_dir) {
        for entry in entries.flatten() {
            let fname = entry.file_name().to_string_lossy().to_string();
            if fname.ends_with(".state.json") {
                if let Ok(content) = fs::read_to_string(entry.path()) {
                    if let Ok(state) = serde_json::from_str::<serde_json::Value>(&content) {
                        rooms.push(state);
                    }
                }
            }
        }
    }

    // Sort by created_at descending
    rooms.sort_by(|a, b| {
        let a_time = a.get("created_at").and_then(|v| v.as_str()).unwrap_or("");
        let b_time = b.get("created_at").and_then(|v| v.as_str()).unwrap_or("");
        b_time.cmp(a_time)
    });

    Ok(serde_json::json!(rooms))
}

#[tauri::command]
pub fn get_warroom_history_transcript(id: String) -> Result<serde_json::Value, String> {
    // Validate id against expected pattern to prevent path traversal
    let valid_id = regex::Regex::new(r"^wr-\d+$").map_err(|e| e.to_string())?;
    if !valid_id.is_match(&id) {
        return Err(format!("Invalid war room id: {}", id));
    }

    let history_dir = format!("{}/history", warroom_dir());
    let path = format!("{}/{}.jsonl", history_dir, id);

    let content = fs::read_to_string(&path)
        .map_err(|e| format!("Failed to read transcript: {}", e))?;

    let entries: Vec<serde_json::Value> = content
        .lines()
        .filter(|l| !l.is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();

    Ok(serde_json::json!(entries))
}

#[tauri::command]
pub fn warroom_cancel() -> Result<(), String> {
    // Hard cancel — delete everything, no archive
    let _ = fs::remove_file(state_path());
    let _ = fs::remove_file(transcript_path());

    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let warroom_mailbox = format!("{}/.aperture/mailbox/warroom", home);
    if let Ok(entries) = fs::read_dir(&warroom_mailbox) {
        for entry in entries.flatten() {
            let _ = fs::remove_file(entry.path());
        }
    }

    Ok(())
}

#[tauri::command]
pub fn warroom_invite_participant(
    name: String,
    state: tauri::State<'_, Arc<Mutex<AppState>>>,
) -> Result<(), String> {
    let mut wr_state = read_state().ok_or("No active war room")?;

    if wr_state.status != "active" {
        return Err("War room is not active".into());
    }

    if wr_state.participants.contains(&name) {
        return Err(format!("{} is already in the war room", name));
    }

    // Validate the agent/spiderling exists and has a tmux window
    {
        let locked = state.inner().lock().map_err(|e| e.to_string())?;
        let has_window = locked
            .agents
            .get(&name)
            .and_then(|a| a.tmux_window_id.as_ref())
            .is_some()
            || locked
                .spiderlings
                .get(&name)
                .and_then(|s| s.tmux_window_id.as_ref())
                .is_some();
        if !has_window {
            return Err(format!("Agent/spiderling '{}' not found or has no tmux window", name));
        }
    }

    // Add to participants
    wr_state.participants.push(name.clone());

    // Log system entry
    append_transcript(&TranscriptEntry {
        role: "system".into(),
        content: format!("{} joined the War Room", name),
        timestamp: now_iso(),
        round: Some(wr_state.round),
    });

    write_state(&wr_state);

    Ok(())
}

// Called from poller to handle warroom turn advancement
pub fn handle_warroom_message(
    sender: &str,
    content: &str,
    app_state: &Arc<Mutex<AppState>>,
) -> Result<(), String> {
    let mut wr_state = read_state().ok_or("No active war room")?;

    if wr_state.status != "active" {
        return Err("War room is not active".into());
    }

    if sender != wr_state.current_agent {
        return Err(format!(
            "Not {}'s turn (current: {})",
            sender, wr_state.current_agent
        ));
    }

    // Append agent's contribution to transcript
    append_transcript(&TranscriptEntry {
        role: sender.into(),
        content: content.into(),
        timestamp: now_iso(),
        round: Some(wr_state.round),
    });

    // Advance the sender's cursor past their own message so they don't receive it back next turn
    let current_len = read_transcript().len();
    wr_state.read_cursors.insert(sender.to_string(), current_len);

    // Check for [CONCLUDE] vote
    if content.contains("[CONCLUDE]") {
        if !wr_state.conclude_votes.contains(&sender.to_string()) {
            wr_state.conclude_votes.push(sender.to_string());
        }

        if wr_state.conclude_votes.len() >= wr_state.participants.len() {
            // All participants voted — auto-conclude
            wr_state.status = "concluded".into();
            append_transcript(&TranscriptEntry {
                role: "system".into(),
                content: "War Room auto-concluded — all participants voted [CONCLUDE]".into(),
                timestamp: now_iso(),
                round: Some(wr_state.round),
            });
            write_state(&wr_state);

            // Archive
            let history_dir = format!("{}/history", warroom_dir());
            let _ = fs::create_dir_all(&history_dir);
            let _ = fs::copy(transcript_path(), format!("{}/{}.jsonl", history_dir, wr_state.id));
            let _ = fs::write(
                format!("{}/{}.state.json", history_dir, wr_state.id),
                serde_json::to_string_pretty(&wr_state).unwrap_or_default(),
            );
            let _ = fs::remove_file(state_path());
            let _ = fs::remove_file(transcript_path());
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
            let warroom_mailbox = format!("{}/.aperture/mailbox/warroom", home);
            if let Ok(entries) = fs::read_dir(&warroom_mailbox) {
                for entry in entries.flatten() {
                    let _ = fs::remove_file(entry.path());
                }
            }
            return Ok(());
        }

        // Not all voted yet — save vote and continue turn advancement
        write_state(&wr_state);
    }

    // Advance turn
    let next_turn = (wr_state.current_turn + 1) % wr_state.participants.len();
    if next_turn <= wr_state.current_turn {
        wr_state.round += 1;
    }
    wr_state.current_turn = next_turn;
    wr_state.current_agent = wr_state.participants[next_turn].clone();
    write_state(&wr_state);

    // Deliver to next agent
    deliver_to_agent(&wr_state.current_agent, app_state)?;

    Ok(())
}
