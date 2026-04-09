# War Room System — Implementation Guide

> **Purpose of this document:** A complete specification for rebuilding the Aperture War Room feature from scratch. Every lifecycle phase, data structure, code path, and UI component is documented here with real code.

---

## 1. Overview

The **War Room** is Aperture's structured multi-agent discussion feature. It allows the human operator to convene a group of agents (named agents and/or spiderlings) to deliberate on a topic in a round-robin format.

**Why it exists:**
- Named agents (`glados`, `wheatley`, `peppy`, `izzy`) and active spiderlings live in separate tmux windows. They cannot naturally observe each other.
- The War Room provides a managed, sequential discussion medium: one agent speaks at a time, their contribution is appended to a shared transcript, and the next agent is given the full context before their turn.
- The operator can observe, interject, skip turns, and conclude the discussion at any time.

**Key characteristics:**
- **File-based turn advancement** — not BEADS. Agents respond via `send_message(to: "warroom", ...)`, which writes to a mailbox directory that the Aperture poller watches.
- **Round-robin ordering** — participants are given an order at creation; that order is fixed, cycling through rounds.
- **Full transcript delivery** — each agent receives the *entire* discussion history before their turn, not just the last message.
- **Operator-controlled lifecycle** — the human starts, can skip/interject/invite, and can conclude at any time. Agents can also vote to conclude with `[CONCLUDE]`.

---

## 2. Creating a Room

### Via the Frontend UI
The operator fills out the setup form in `WarRoom.ts`:
1. Enters a topic (free text, textarea)
2. Selects ≥2 participants (checkboxes; order of checking determines turn order)
3. Clicks **Start Discussion**

### Via MCP Tool
Agents cannot create War Rooms directly. The Tauri backend exposes `create_warroom` as a Tauri command (not an MCP tool). Only the Aperture desktop app frontend calls it.

### Backend: `create_warroom` (warroom.rs:162–218)

```rust
#[tauri::command]
pub fn create_warroom(
    topic: String,
    participants: Vec<String>,
    state: tauri::State<'_, Arc<Mutex<AppState>>>,
) -> Result<(), String> {
    if participants.is_empty() {
        return Err("At least one participant is required".into());
    }

    // Ensure warroom dir exists: ~/.aperture/warroom/
    let _ = fs::create_dir_all(warroom_dir());

    // Create warroom mailbox and flush any stale files from previous rooms
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let warroom_mailbox = format!("{}/.aperture/mailbox/warroom", home);
    let _ = fs::create_dir_all(&warroom_mailbox);
    if let Ok(entries) = fs::read_dir(&warroom_mailbox) {
        for entry in entries.flatten() {
            let _ = fs::remove_file(entry.path());
        }
    }

    let id = format!("wr-{}", now_millis()); // e.g. "wr-1774192620038"
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
    };

    write_state(&wr_state);  // → ~/.aperture/warroom/state.json

    // Write the first transcript entry: system message announcing the room
    append_transcript(&TranscriptEntry {
        role: "system".into(),
        content: format!(
            "War Room started. Topic: {}. Participants: {}",
            topic, participants.join(", ")
        ),
        timestamp: now_iso(),
        round: Some(1),
    });

    // Deliver initial context to the FIRST participant
    let arc_state = state.inner().clone();
    deliver_to_agent(&first_agent, &arc_state)?;

    Ok(())
}
```

**What happens:**
1. Stale mailbox files are cleared (prevents old messages bleeding into the new room).
2. A unique ID is generated from the current Unix timestamp in milliseconds.
3. `state.json` is written with `status: "active"`.
4. A `system` transcript entry is appended to `transcript.jsonl`.
5. The first participant is immediately delivered the War Room context via tmux.

---

## 3. State Management

### File locations

```
~/.aperture/warroom/
├── state.json          ← active room state (one room at a time)
└── transcript.jsonl    ← growing log of all entries for active room
```

### `state.json` — `WarRoomState` struct

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WarRoomState {
    pub id: String,              // "wr-<unix_ms>" e.g. "wr-1774192620038"
    pub topic: String,           // Free-form discussion topic
    pub participants: Vec<String>, // Ordered list, e.g. ["glados", "wheatley", "peppy"]
    pub current_turn: usize,     // Index into participants (0-based)
    pub current_agent: String,   // participants[current_turn]
    pub round: usize,            // Increments when we wrap past the last participant
    pub status: String,          // "active" | "concluded"
    pub created_at: String,      // ISO 8601 UTC, e.g. "2026-03-22T15:17:00Z"
    pub conclude_votes: Vec<String>, // Names of agents who voted [CONCLUDE]
}
```

**Real example from `~/.aperture/warroom/history/`:**
```json
{
  "id": "wr-1774192620038",
  "topic": "cool, lets talk about spiderling communication with glados",
  "participants": ["glados", "spider-askfrancisco", "spider-incluir-attendance"],
  "current_turn": 1,
  "current_agent": "spider-askfrancisco",
  "round": 1,
  "status": "concluded",
  "created_at": "2026-03-22T15:17:00Z",
  "conclude_votes": []
}
```

### Read/write helpers (warroom.rs:31–57)

```rust
fn warroom_dir() -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let dir = format!("{}/.aperture/warroom", home);
    let _ = fs::create_dir_all(&dir);
    dir
}

fn state_path() -> String {
    format!("{}/state.json", warroom_dir())
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
```

**Important:** Only one War Room can be active at a time. `state.json` is overwritten on each `create_warroom` call, and deleted on `warroom_conclude`. There is no "room ID" in the path — the single file IS the active room.

---

## 4. Turn System

### Round-Robin Algorithm

Turn advancement is computed with a single modulo operation (warroom.rs:472–478):

```rust
// Advance turn
let next_turn = (wr_state.current_turn + 1) % wr_state.participants.len();
if next_turn <= wr_state.current_turn {
    // We wrapped around — increment the round counter
    wr_state.round += 1;
}
wr_state.current_turn = next_turn;
wr_state.current_agent = wr_state.participants[next_turn].clone();
write_state(&wr_state);
```

**Round increment logic:** The round counter increases whenever `next_turn <= current_turn`. This means:
- For a 3-person room [A=0, B=1, C=2]: advancing from C(2) → A(0) increments round because `0 <= 2`.
- This correctly handles any number of participants.

### Turn Gating

Only the current agent's message is accepted. Out-of-turn messages are rejected:

```rust
if sender != wr_state.current_agent {
    return Err(format!(
        "Not {}'s turn (current: {})",
        sender, wr_state.current_agent
    ));
}
```

Even if rejected, the mailbox file is deleted (poller removes it either way), to prevent accumulation.

### Turn Advancement Triggers

There are three ways the turn advances:
1. **Normal**: Agent sends a `send_message(to: "warroom", ...)` → poller detects it → `handle_warroom_message()` → advances.
2. **Skip**: Operator clicks Skip in UI → `warroom_skip()` Tauri command → advances without an agent message.
3. **Conclude vote completion**: All participants vote `[CONCLUDE]` → auto-conclude (no further turn advancement).

---

## 5. Transcript

### Format: `transcript.jsonl`

Each line is a JSON object — one per contribution. The file grows by appending (`OpenOptions::append(true)`), never rewritten.

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptEntry {
    pub role: String,      // "system" | "operator" | <agent_name>
    pub content: String,   // The full message body
    pub timestamp: String, // ISO 8601 UTC
    pub round: Option<usize>, // Present on all entries except pre-round-0
}
```

**Real transcript lines from a live room:**
```jsonl
{"role":"system","content":"War Room started. Topic: spiderling communication. Participants: glados, spider-askfrancisco, spider-incluir-attendance","timestamp":"2026-03-22T15:17:00Z","round":1}
{"role":"glados","content":"The operator is right, and I'll own this failure directly.\n\n**What happened from my end:**\nThe spiderlings sent completion messages...","timestamp":"2026-03-22T15:17:27Z","round":1}
{"role":"system","content":"War Room concluded by operator","timestamp":"2026-03-22T15:18:55Z","round":1}
```

### Append function (warroom.rs:59–66)

```rust
fn append_transcript(entry: &TranscriptEntry) {
    let path = transcript_path();
    if let Ok(data) = serde_json::to_string(entry) {
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) {
            let _ = writeln!(file, "{}", data);
        }
    }
}
```

### Read function (warroom.rs:68–80)

```rust
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
```

Lines that fail to parse are silently skipped (`filter_map`). This is resilient to partial writes or corruption.

---

## 6. Agent Delivery

When it is an agent's turn, `deliver_to_agent()` is called. This function:
1. Reads the full current state and transcript.
2. Formats everything into a markdown context document.
3. Writes the context to a temp file at `/tmp/aperture-warroom-context.md`.
4. Injects a `cat /tmp/aperture-warroom-context.md` command into the agent's tmux window.

### Full implementation (warroom.rs:99–160)

```rust
fn deliver_to_agent(agent_name: &str, app_state: &Arc<Mutex<AppState>>) -> Result<(), String> {
    let state = read_state().ok_or("No active war room")?;
    let transcript = read_transcript();

    // Format transcript as [ROLE]: content lines
    let mut formatted = String::new();
    for entry in &transcript {
        formatted.push_str(&format!("[{}]: {}\n", entry.role.to_uppercase(), entry.content));
    }

    // First-turn marker warns agent to start fresh
    let is_first_turn = transcript.len() <= 1; // only the "started" system entry
    let new_room_marker = if is_first_turn {
        format!(
            "⚠️ THIS IS A BRAND NEW DISCUSSION (Room ID: {}). Forget any previous War Room conversations. Start fresh.\n\n",
            state.id
        )
    } else {
        String::new()
    };

    let context = format!(
        "# WAR ROOM — {topic}\n## Room: {id} | Round {round}\n\n{new_room}{instructions}\n\n---\n{transcript}\n---\n\nIt is now YOUR turn ({agent}). Share your perspective on the topic above.\n",
        topic = state.topic,
        id = state.id,
        round = state.round,
        new_room = new_room_marker,
        instructions = "You are participating in a War Room discussion. Read the transcript below and share your perspective.\nWhen done, respond using: send_message(to: \"warroom\", message: \"your contribution\")\nDO NOT reply in the terminal — use the send_message MCP tool.\nDO NOT reference previous War Room discussions — focus only on the topic and transcript shown here.",
        transcript = formatted,
        agent = agent_name,
    );

    // Resolve the agent's tmux window ID from AppState
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

    // Spiderlings get an extra note to pause their current task
    let spiderling_note = if is_spiderling {
        "⚠️ NOTE: You are a spiderling. Pause your current task to participate in this discussion. Return to your task after you have sent your war room contribution.\n\n"
    } else {
        ""
    };

    let full_context = format!("{}{}", spiderling_note, context);

    // Write to temp file and inject via tmux
    let context_path = "/tmp/aperture-warroom-context.md";
    fs::write(context_path, &full_context).map_err(|e| e.to_string())?;
    tmux::tmux_send_keys(window_id, format!("cat {}", context_path))?;

    Ok(())
}
```

### What the agent sees in their terminal

```
# WAR ROOM — Should we prioritize API stability over new features?
## Room: wr-1774192620038 | Round 1

You are participating in a War Room discussion. Read the transcript below and share your perspective.
When done, respond using: send_message(to: "warroom", message: "your contribution")
DO NOT reply in the terminal — use the send_message MCP tool.
DO NOT reference previous War Room discussions — focus only on the topic and transcript shown here.

---
[SYSTEM]: War Room started. Topic: Should we prioritize API stability over new features? Participants: glados, wheatley, peppy
[GLADOS]: I believe stability is the foundation of everything...
---

It is now YOUR turn (wheatley). Share your perspective on the topic above.
```

**Why a temp file?** Direct injection of multi-line content into tmux via `send-keys` is unreliable — special characters, newlines, and shell interpretation cause issues. Writing to a file and running `cat` is safer and preserves formatting.

---

## 7. The Warroom Mailbox Pattern

### Why not BEADS?

The regular agent-to-agent messaging system (BEADS) uses a message bus with async delivery. War Room turn advancement requires **synchronous, ordered** processing — a message arrives, the transcript is updated, the next agent is notified, all within a single atomic operation. BEADS's eventual-delivery model doesn't provide this.

Instead, the War Room uses a dedicated **file-based mailbox**:

```
~/.aperture/mailbox/warroom/    ← agents write here via send_message(to: "warroom")
```

When an agent calls `send_message(to: "warroom", message: "...")`, the MCP server writes a file:

```typescript
// mcp-server/src/index.ts
if (target === "operator" || target === "warroom") {
    const filepath = store.sendMessage(AGENT_NAME, target, message);
    return {
        content: [{ type: "text", text: `Message sent to ${target}. Delivered to: ${filepath}` }],
    };
}
```

The `MailboxStore.sendMessage` creates a file at:
```
~/.aperture/mailbox/warroom/<timestamp>-<agent_name>.md
```

For example: `~/.aperture/mailbox/warroom/1774192665123-glados.md`

**File content:**
```markdown
# Message from glados
_2026-03-22T15:17:26.508Z_

The operator is right, and I'll own this failure directly...
```

The **filename encodes the sender** — it is parsed by the poller to extract the agent name.

### Why `warroom` is NOT in BEADS permanent recipients list

In `mcp-server/src/index.ts`:
```typescript
const PERMANENT_RECIPIENTS = ["glados", "wheatley", "peppy", "izzy", "operator", "warroom"];
```

`warroom` is listed as a permanent recipient so that `isValidRecipient("warroom")` returns `true` and the router doesn't reject it. But it is intercepted before the BEADS codepath:
```typescript
// Operator and warroom still use file-based delivery (Chat panel + War Room turn mechanics)
if (target === "operator" || target === "warroom") {
    const filepath = store.sendMessage(AGENT_NAME, target, message);
    ...
}
```

BEADS never sees warroom messages. They go straight to the filesystem.

---

## 8. Poller: Detecting and Processing War Room Messages

The poller (`src-tauri/src/poller.rs`) runs in a background thread, looping every 5 seconds. Its War Room section (lines 216–248):

```rust
// ── Handle war room messages ──
{
    let warroom_mailbox = format!("{}/warroom", mailbox_base);
    let _ = fs::create_dir_all(&warroom_mailbox);

    // Only process if a war room is actually active
    let wr_state_path = format!("{}/.aperture/warroom/state.json", home);
    if let Ok(wr_data) = fs::read_to_string(&wr_state_path) {
        if wr_data.contains("\"active\"") {
            let wr_files = scan_mailbox(&warroom_mailbox);
            // Retain only files still present (clean up stale notified set)
            warroom_notified.retain(|f| wr_files.contains(f));

            // Find files we haven't processed yet
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
                            // Delete even on error (prevents stuck messages)
                            let _ = fs::remove_file(filepath);
                        }
                    }
                }
                warroom_notified.insert((*filepath).clone());
            }
        }
    }
}
```

**Key details:**
- The poller quick-checks `state.json` for the string `"active"` before scanning for files. This avoids unnecessary disk reads when no room is running.
- Files are deleted regardless of whether `handle_warroom_message` succeeds or fails. An out-of-turn message will be rejected (with an error logged) but the file is still removed.
- `warroom_notified` is a `HashSet<String>` of already-seen file paths, maintained across loop iterations to prevent double-processing.

### Filename parsing (poller.rs:23–36)

```rust
fn parse_filename(filepath: &str) -> (String, String) {
    let fname = std::path::Path::new(filepath)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();
    // filename format: "<timestamp>-<agent_name>.md"
    let sender = fname
        .trim_end_matches(".md")
        .split('-')
        .skip(1)           // skip the timestamp segment
        .collect::<Vec<_>>()
        .join("-");        // re-join (handles names with hyphens, e.g. "spider-doc-warroom")
    let timestamp = fname.split('-').next().unwrap_or("0").to_string();
    (sender, timestamp)
}
```

This handles hyphenated names correctly: `1774192665123-spider-doc-warroom.md` → sender = `"spider-doc-warroom"`.

---

## 9. Handling an Agent's Message: `handle_warroom_message`

This is the core state machine function called by the poller (warroom.rs:403–484):

```rust
pub fn handle_warroom_message(
    sender: &str,
    content: &str,
    app_state: &Arc<Mutex<AppState>>,
) -> Result<(), String> {
    let mut wr_state = read_state().ok_or("No active war room")?;

    // Reject if room is not active
    if wr_state.status != "active" {
        return Err("War room is not active".into());
    }

    // Reject if it's not this agent's turn
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

    // ── [CONCLUDE] vote handling ──
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

            // Archive transcript and state, then clean up live files
            let history_dir = format!("{}/history", warroom_dir());
            let _ = fs::create_dir_all(&history_dir);
            let _ = fs::copy(transcript_path(), format!("{}/{}.jsonl", history_dir, wr_state.id));
            let _ = fs::write(
                format!("{}/{}.state.json", history_dir, wr_state.id),
                serde_json::to_string_pretty(&wr_state).unwrap_or_default(),
            );
            let _ = fs::remove_file(state_path());
            let _ = fs::remove_file(transcript_path());
            // Flush remaining warroom mailbox files
            let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
            let warroom_mailbox = format!("{}/.aperture/mailbox/warroom", home);
            if let Ok(entries) = fs::read_dir(&warroom_mailbox) {
                for entry in entries.flatten() {
                    let _ = fs::remove_file(entry.path());
                }
            }
            return Ok(());
        }

        // Not all voted yet — save the vote and continue with turn advancement
        write_state(&wr_state);
    }

    // ── Advance turn ──
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
```

---

## 10. Operator Controls

### Interjection

Injects a message into the transcript **without advancing the turn**. The current agent remains the same; they simply see the operator's note in the transcript on their next delivery.

```rust
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
```

**UI:** Operator types in the input field at the bottom of the active view and presses Enter or clicks the ↑ button.

### Skip

Skips the current agent's turn, logs a system entry, and advances to the next agent.

```rust
#[tauri::command]
pub fn warroom_skip(state: tauri::State<'_, Arc<Mutex<AppState>>>) -> Result<(), String> {
    let mut wr_state = read_state().ok_or("No active war room")?;

    append_transcript(&TranscriptEntry {
        role: "system".into(),
        content: format!("{} was skipped", wr_state.current_agent),
        timestamp: now_iso(),
        round: Some(wr_state.round),
    });

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
```

### Conclude (Operator-initiated)

Immediately concludes the room, archives it, and cleans up live files.

```rust
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

    // Archive
    let history_dir = format!("{}/history", warroom_dir());
    let _ = fs::create_dir_all(&history_dir);
    let _ = fs::copy(transcript_path(), format!("{}/{}.jsonl", history_dir, wr_state.id));
    let _ = fs::write(
        format!("{}/{}.state.json", history_dir, wr_state.id),
        serde_json::to_string_pretty(&wr_state).unwrap_or_default(),
    );

    // Clean up live files
    let _ = fs::remove_file(state_path());
    let _ = fs::remove_file(transcript_path());
    // Flush warroom mailbox
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let warroom_mailbox = format!("{}/.aperture/mailbox/warroom", home);
    if let Ok(entries) = fs::read_dir(&warroom_mailbox) {
        for entry in entries.flatten() {
            let _ = fs::remove_file(entry.path());
        }
    }

    Ok(())
}
```

### Invite Participant Mid-Discussion

The operator can add a new participant to an active room. The new participant is appended to the `participants` list and will receive the context on their first turn (following the natural round-robin).

```rust
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

    // Validate the agent/spiderling has a tmux window
    {
        let locked = state.inner().lock().map_err(|e| e.to_string())?;
        let has_window = locked.agents.get(&name)
            .and_then(|a| a.tmux_window_id.as_ref()).is_some()
            || locked.spiderlings.get(&name)
            .and_then(|s| s.tmux_window_id.as_ref()).is_some();
        if !has_window {
            return Err(format!("Agent/spiderling '{}' not found or has no tmux window", name));
        }
    }

    wr_state.participants.push(name.clone());

    append_transcript(&TranscriptEntry {
        role: "system".into(),
        content: format!("{} joined the War Room", name),
        timestamp: now_iso(),
        round: Some(wr_state.round),
    });

    write_state(&wr_state);
    Ok(())
}
```

**Note:** The invited participant is NOT immediately delivered the context. They will receive it when the round-robin reaches their position in the (now-extended) `participants` array.

---

## 11. History & Archives

### Archive structure

When a room concludes (either operator-triggered or agent vote), it is archived:

```
~/.aperture/warroom/history/
├── wr-1774192620038.jsonl          ← full transcript (copy of transcript.jsonl)
├── wr-1774192620038.state.json     ← final state (copy of state.json, status: "concluded")
├── wr-1774139575092.jsonl
├── wr-1774139575092.state.json
└── ...
```

IDs are millisecond timestamps, so they sort chronologically by name.

### `list_warroom_history`

Returns all concluded rooms, sorted by `created_at` descending (newest first):

```rust
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

    rooms.sort_by(|a, b| {
        let a_time = a.get("created_at").and_then(|v| v.as_str()).unwrap_or("");
        let b_time = b.get("created_at").and_then(|v| v.as_str()).unwrap_or("");
        b_time.cmp(a_time) // descending
    });

    Ok(serde_json::json!(rooms))
}
```

### `get_warroom_history_transcript`

Reads the `.jsonl` archive for a given room ID:

```rust
#[tauri::command]
pub fn get_warroom_history_transcript(id: String) -> Result<serde_json::Value, String> {
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
```

---

## 12. Frontend UI — `WarRoom.ts`

The War Room UI (`src/components/WarRoom.ts`) is a single-file component with no framework dependencies. It manages its own state via closure and renders directly into a `container: HTMLElement`.

### Entry point

```typescript
export function createWarRoom(container: HTMLElement) {
    let pollTimer: ReturnType<typeof setInterval> | null = null;
    let lastTranscriptLength = 0;

    renderSetup();
    checkInitialState(); // if a room is already active on load, resume it
}
```

### View states and transitions

```
        ┌──────────────┐
        │  renderSetup  │  ← initial view; "New War Room" button also returns here
        └──────┬───────┘
               │ start clicked + room created
               ▼
        ┌──────────────┐
        │ renderActive  │  ← polls every 2 seconds
        └──────┬───────┘
               │ state.status === "concluded"
               ▼
        ┌──────────────────┐
        │ renderConcluded   │  ← shows final transcript, stops polling
        └──────────────────┘

        ┌──────────────┐
        │ renderHistory │  ← accessible from setup via "View Past Discussions"
        └──────┬───────┘
               │ click a room
               ▼
        ┌─────────────────────────┐
        │ renderHistoryTranscript  │  ← read-only view, export button
        └─────────────────────────┘
```

### Setup form (`renderSetup`)

- **Topic textarea**: `<textarea class="warroom__topic-input" rows="3" />`
- **Participant checkboxes**: One per named agent (glados, wheatley, peppy, izzy), with spiderlings fetched dynamically and appended.
- **Order badges**: Show the selection order (1, 2, 3...) on each selected checkbox label. The order in which the operator checks boxes determines the `participants` array passed to `create_warroom`.
- **Start button**: Disabled until topic is non-empty AND ≥2 participants selected.
- **Spiderling discovery**: Calls `commands.listSpiderlings()` and appends active ones (`status === "working"`) to the list with a 🕷️ icon.

```typescript
startBtn.addEventListener("click", async () => {
    const topic = topicInput.value.trim();
    if (!topic || selected.length < 2) return;
    startBtn.disabled = true;
    startBtn.textContent = "Starting...";
    try {
        await commands.createWarroom(topic, [...selected]); // [...selected] preserves order
        renderActive();
        startPolling();
    } catch (e) {
        // Reset button on failure
        startBtn.disabled = false;
        startBtn.textContent = "Start Discussion";
    }
});
```

### Active discussion view (`renderActive`)

HTML structure:
```html
<div class="warroom__active">
    <div class="warroom__topic"></div>           <!-- topic string -->
    <div class="warroom__meta">
        <div class="warroom__participants"></div>  <!-- turn badges -->
        <div class="warroom__round"></div>         <!-- "Round N" -->
    </div>
    <div class="warroom__transcript"></div>        <!-- scrollable messages -->
    <div class="warroom__input-row">
        <input class="warroom__input" placeholder="Interject..." />
        <button class="warroom__send">↑</button>
    </div>
    <div class="warroom__controls">
        <span class="warroom__conclude-votes hidden"></span>  <!-- vote counter -->
        <div class="warroom__invite-row hidden">              <!-- invite dropdown -->
            <select class="warroom__invite-select"></select>
            <button class="warroom__invite-btn">Invite</button>
        </div>
        <button class="warroom__skip-btn">Skip</button>
        <button class="btn--danger warroom__conclude-btn">Conclude</button>
    </div>
</div>
```

### Polling loop (`startPolling` / `poll`)

```typescript
function startPolling() {
    stopPolling();
    poll(); // immediate first poll
    pollTimer = setInterval(poll, 2000); // then every 2 seconds
}

async function poll() {
    const [state, transcript] = await Promise.all([
        commands.getWarroomState(),
        commands.getWarroomTranscript(),
    ]);

    if (!state) {
        renderSetup(); // room was cleared externally
        return;
    }

    if (state.status === "concluded") {
        renderConcluded(transcript);
        return;
    }

    updateActiveView(state, transcript);
    updateInviteDropdown(state);
}
```

The UI polls **both** state and transcript every 2 seconds. There is no WebSocket or push mechanism — the frontend works entirely via Tauri command polling.

### Transcript rendering (`updateActiveView`)

Each entry type renders differently:

```typescript
// system entry
`<div class="warroom__entry warroom__entry--system">${escapeHtml(entry.content)}</div>`

// operator entry
`<div class="warroom__entry warroom__entry--operator">
  <div class="warroom__entry-role">Operator</div>
  <div class="warroom__entry-body">${escapeHtml(entry.content)}</div>
</div>`

// agent entry (themed by name)
const theme = AGENT_THEME[role] ?? { icon: "🕷️", color: "var(--accent-orange)" };
`<div class="warroom__entry warroom__entry--agent" style="border-color:${color}">
  <div class="warroom__entry-role" style="color:${color}">${icon}${escapeHtml(entry.role)}</div>
  <div class="warroom__entry-body">${escapeHtml(entry.content)}</div>
</div>`
```

**Agent themes:**
```typescript
const AGENT_THEME: Record<string, { icon: string; color: string }> = {
    glados:   { icon: "🤖", color: "#9b59b6" },
    wheatley: { icon: "💡", color: "#3498db" },
    peppy:    { icon: "🚀", color: "#1abc9c" },
    izzy:     { icon: "🧪", color: "#e91e63" },
};
// Spiderlings fall through to: { icon: "🕷️", color: "var(--accent-orange)" }
```

### Participant turn indicator

Each participant is rendered as a badge. The current agent's badge is highlighted with their theme color as background:

```typescript
participantsEl.innerHTML = state.participants.map(name => {
    const theme = AGENT_THEME[name] ?? { icon: "🕷️", color: "var(--accent-orange)" };
    const isActive = name === state.current_agent;
    const activeClass = isActive ? " warroom__participant--active" : "";
    const style = isActive ? `background:${theme.color};border-color:${theme.color}` : "";
    return `<span class="warroom__participant${activeClass}" style="${style}">${theme.icon} ${escapeHtml(name)}</span>`;
}).join("");
```

### Auto-scroll behavior

The transcript scrolls to the bottom when new entries arrive, but preserves the scroll position if the operator has manually scrolled up:

```typescript
const shouldScroll = transcriptEl.scrollHeight - transcriptEl.scrollTop - transcriptEl.clientHeight < 60;
// ... render transcript ...
if (shouldScroll || transcript.length !== lastTranscriptLength) {
    transcriptEl.scrollTop = transcriptEl.scrollHeight;
}
lastTranscriptLength = transcript.length;
```

### Conclude votes display

If any participants have voted `[CONCLUDE]`, a counter appears:

```typescript
if (votes.length > 0) {
    votesEl.textContent = `⚑ ${votes.length}/${state.participants.length} want to conclude`;
    votesEl.classList.remove("hidden");
}
```

### Invite dropdown (`updateInviteDropdown`)

On each poll, the invite dropdown is refreshed with agents/spiderlings not currently in the room:

```typescript
async function updateInviteDropdown(state: WarRoomState) {
    const eligible: { name: string; isSpider: boolean }[] = [];

    // Permanent agents not in room
    for (const name of AGENT_NAMES) {
        if (!state.participants.includes(name)) {
            eligible.push({ name, isSpider: false });
        }
    }

    // Active spiderlings not in room
    const spiderlings = await commands.listSpiderlings();
    for (const s of spiderlings) {
        if (s.status === "working" && !state.participants.includes(s.name)) {
            eligible.push({ name: s.name, isSpider: true });
        }
    }

    if (eligible.length === 0) {
        inviteRow.classList.add("hidden");
        return;
    }
    inviteRow.classList.remove("hidden");
    inviteSelect.innerHTML = eligible.map(e =>
        `<option value="${escapeHtml(e.name)}">${escapeHtml(e.name)}${e.isSpider ? " (spiderling)" : ""}</option>`
    ).join("");
}
```

### History view

`renderHistory(rooms)` shows a list of past rooms. Clicking a room calls `commands.getWarroomHistoryTranscript(id)` and renders it in `renderHistoryTranscript`.

The history transcript view includes an **Export as Markdown** button:

```typescript
container.querySelector(".warroom__export-btn")!.addEventListener("click", () => {
    const md = transcript.map(e => {
        if (e.role === "system") return `*${e.content}*\n`;
        return `**${e.role}**: ${e.content}\n`;
    }).join("\n");
    const blob = new Blob([`# War Room: ${topic}\n\n${md}`], { type: "text/markdown" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = `warroom-${id}.md`;
    a.click();
    URL.revokeObjectURL(url);
});
```

---

## 13. Full Lifecycle Summary

```
Operator fills setup form (topic + participants)
  │
  ▼
createWarroom(topic, participants)
  ├── Clear ~/.aperture/mailbox/warroom/ (flush stale)
  ├── Write ~/.aperture/warroom/state.json  { status: "active", round: 1, current_turn: 0 }
  ├── Append system entry to transcript.jsonl
  └── deliver_to_agent(participants[0])
        └── cat /tmp/aperture-warroom-context.md → agent's tmux window

Agent[0] reads context, calls send_message(to: "warroom", message: "...")
  │
  ▼
MCP server writes ~/.aperture/mailbox/warroom/<ts>-agent0.md

Poller (every 5s) scans mailbox/warroom/ for new .md files
  │
  ▼
handle_warroom_message(sender="agent0", content="...", app_state)
  ├── Verify agent0 == current_agent ✓
  ├── append_transcript({ role: "agent0", content, round: 1 })
  ├── Check for [CONCLUDE] vote (none in this case)
  ├── Advance turn: current_turn=1, current_agent=participants[1]
  ├── write_state(updated)
  └── deliver_to_agent(participants[1])

... rounds continue ...

Operator clicks Conclude (or all agents vote [CONCLUDE])
  │
  ▼
warroom_conclude()
  ├── Set status: "concluded"
  ├── Append system "concluded" entry to transcript
  ├── Copy transcript.jsonl → history/wr-<id>.jsonl
  ├── Copy state.json     → history/wr-<id>.state.json
  ├── Delete state.json + transcript.jsonl
  └── Flush mailbox/warroom/

Frontend polls, sees status: "concluded"
  └── renderConcluded(transcript) — shows final transcript, stops polling
```

---

## 14. Tauri Command Registration

All War Room commands must be registered in the Tauri builder. The relevant commands are:

- `create_warroom`
- `get_warroom_state`
- `get_warroom_transcript`
- `warroom_interject`
- `warroom_skip`
- `warroom_conclude`
- `list_warroom_history`
- `get_warroom_history_transcript`
- `warroom_invite_participant`

And the corresponding TypeScript bindings in `src/services/tauri-commands.ts` must expose them as:
```typescript
createWarroom(topic: string, participants: string[]): Promise<void>
getWarroomState(): Promise<WarRoomState | null>
getWarroomTranscript(): Promise<TranscriptEntry[]>
warroomInterject(message: string): Promise<void>
warroomSkip(): Promise<void>
warroomConclude(): Promise<void>
listWarroomHistory(): Promise<WarRoomState[]>
getWarroomHistoryTranscript(id: string): Promise<TranscriptEntry[]>
warroomInviteParticipant(name: string): Promise<void>
```

---

## 15. TypeScript Types

Required types in `src/types.ts`:

```typescript
export interface WarRoomState {
    id: string;
    topic: string;
    participants: string[];
    current_turn: number;
    current_agent: string;
    round: number;
    status: string;           // "active" | "concluded"
    created_at: string;
    conclude_votes: string[];
}

export interface TranscriptEntry {
    role: string;             // "system" | "operator" | <agent_name>
    content: string;
    timestamp: string;
    round?: number;
}
```

---

## 16. Agent Participation Protocol

Agents participating in a War Room **must** follow this protocol (enforced by `aperture-war-room` skill):

1. **Read the transcript carefully** — the full history is provided.
2. **Respond exactly once** via `send_message(to: "warroom", message: "your contribution")`.
3. **Do NOT reply in the terminal** — that output is not captured.
4. **Vote to conclude** by including `[CONCLUDE]` anywhere in the message when the discussion is complete.
5. **Spiderlings** must pause their current task, contribute, then return to their task.

The context delivery explicitly includes these instructions:
```
When done, respond using: send_message(to: "warroom", message: "your contribution")
DO NOT reply in the terminal — use the send_message MCP tool.
DO NOT reference previous War Room discussions — focus only on the topic and transcript shown here.
```

---

## 17. Constraints and Edge Cases

| Situation | Behavior |
|-----------|----------|
| Out-of-turn message | Rejected with error, file deleted. Discussion continues. |
| Agent not found in AppState | `deliver_to_agent` returns `Err`. Turn not advanced. |
| Partial conclude votes | Vote recorded in `conclude_votes`, turn advances normally. |
| `create_warroom` while room active | Overwrites `state.json` and `transcript.jsonl`. Previous room is lost (not archived). |
| Concurrent mailbox files | Processed in filesystem readdir order. Only one will match `current_agent`; others are deleted. |
| Spiderling killed mid-discussion | Its turn will fail delivery (`no tmux window`). Operator should skip. |
| Only 1 participant | Rejected at creation: `"At least one participant is required"` (but minimum meaningful is 2; currently only checks `> 0`). |
