# War Room — Multi-Agent Discussion System

## Overview

A moderated group discussion room where the operator selects agents to debate a topic in rounds. Agents take turns sharing their perspective with full conversation context. The operator can interject, skip agents, or redirect the discussion. The room runs until the operator concludes it, optionally extracting action items.

## User Flow

1. Operator clicks **"War Room"** in the navbar → right panel opens
2. **Setup phase**: Enter a topic/question, select participating agents (checkboxes), click **"Start"**
3. **Discussion phase**: Agents take turns automatically (round-robin)
   - When it's Agent A's turn, the system delivers the full transcript to Agent A's tmux window
   - Agent A responds via `send_message(to: "warroom", message: "...")`
   - The response is added to the transcript and displayed in the panel
   - System automatically advances to Agent B
4. **Operator controls** (available at any time during discussion):
   - **Interject**: Type a message that gets added to the transcript. The next agent sees it.
   - **Skip**: Skip the current agent's turn, move to the next one
   - **Redirect**: Change the topic mid-discussion (appends to transcript as a new directive)
   - **Conclude**: End the discussion. Optionally ask GLaDOS to summarize action items.
5. **Post-discussion**: Transcript is saved to `~/.aperture/warroom-history/` for reference

## Architecture

### State

```
~/.aperture/warroom/
├── state.json          # Current war room state (topic, participants, turn, status)
├── transcript.jsonl    # Full conversation log
└── history/            # Completed war rooms (archived transcripts)
```

**state.json**:
```json
{
  "id": "wr-1710547200",
  "topic": "How should we structure the API layer?",
  "participants": ["glados", "wheatley", "peppy"],
  "current_turn": 1,
  "current_agent": "wheatley",
  "round": 1,
  "status": "active",
  "created_at": "2026-03-16T01:00:00Z"
}
```

**transcript.jsonl** (each line):
```json
{"role": "system", "content": "Topic: How should we structure the API layer?", "timestamp": "..."}
{"role": "glados", "content": "Here's my architectural assessment...", "round": 1, "timestamp": "..."}
{"role": "wheatley", "content": "Right! I think we should...", "round": 1, "timestamp": "..."}
{"role": "operator", "content": "What about rate limiting?", "timestamp": "..."}
{"role": "peppy", "content": "Good point kid, we need...", "round": 1, "timestamp": "..."}
```

### MCP Server Changes

Add `"warroom"` as a valid recipient in the MCP server. When an agent sends to `"warroom"`:
- The message file lands in `~/.aperture/mailbox/warroom/`
- The poller picks it up, appends to `transcript.jsonl`, advances the turn
- The poller then delivers the full transcript to the next agent

### Poller Changes

The poller needs a new responsibility: **War Room turn management**.

Every poll cycle (3s):
1. Check if a War Room is active (`state.json` status === "active")
2. Check `~/.aperture/mailbox/warroom/` for new responses
3. If the current agent responded:
   - Append their message to `transcript.jsonl`
   - Delete the mailbox file
   - Advance `current_turn` to the next participant
   - Deliver the full transcript to the next agent's tmux window
4. If the current agent hasn't responded yet:
   - Do nothing (wait)

**Delivering transcript to an agent:**
- Write the full transcript as a temp file
- Send `cat /tmp/aperture-warroom-context.md` to the agent's tmux window
- The file content includes:
  ```
  # WAR ROOM — [Topic]
  ## Round [N]

  You are participating in a War Room discussion. Read the full transcript below and share your thoughts.
  When done, respond using: send_message(to: "warroom", message: "your contribution")

  ---
  [Full transcript formatted as a conversation]
  ---

  It is now YOUR turn. Share your perspective on the topic above.
  ```

### Rust Backend Commands

```rust
#[tauri::command]
fn create_warroom(topic: String, participants: Vec<String>) -> Result<(), String>
// Creates state.json, initializes transcript with topic, starts first turn

#[tauri::command]
fn get_warroom_state() -> Result<Option<WarRoomState>, String>
// Returns current state.json or None if no active room

#[tauri::command]
fn get_warroom_transcript() -> Result<Vec<TranscriptEntry>, String>
// Returns parsed transcript.jsonl

#[tauri::command]
fn warroom_interject(message: String) -> Result<(), String>
// Adds operator message to transcript (doesn't change turn order)

#[tauri::command]
fn warroom_skip() -> Result<(), String>
// Skips current agent, advances to next

#[tauri::command]
fn warroom_redirect(new_topic: String) -> Result<(), String>
// Adds redirect to transcript, resets round counter

#[tauri::command]
fn warroom_conclude() -> Result<(), String>
// Sets status to "concluded", archives transcript to history/

#[tauri::command]
fn list_warroom_history() -> Result<Vec<WarRoomSummary>, String>
// Lists past war rooms from history/
```

### Frontend — War Room Panel

New navbar button **"War Room"** (between Chat and Messages).

**States:**

1. **No active room** → Setup form:
   - Topic input (textarea)
   - Agent checkboxes (with icons/colors from agent theme)
   - "Start Discussion" button

2. **Active room** → Discussion view:
   - Topic header
   - Participant list with turn indicator (highlighted current speaker)
   - Scrolling transcript (chat-style, each message shows role + content)
   - Operator input row (same as Chat panel)
   - Control buttons: [Skip] [Redirect] [Conclude]
   - Round counter

3. **Concluded** → Summary view:
   - Full transcript (read-only)
   - "New War Room" button
   - Option to view past war rooms

### Agent Prompt Updates

All agents need a new section in their system prompts:

```markdown
# War Room

You may be invited to a **War Room** — a structured group discussion with other agents and the human operator.
When participating:
- You'll receive the full transcript of the discussion so far
- Read everything carefully before responding
- Share your perspective based on your expertise (coding/infra/testing/orchestration)
- Be concise but thorough — this is a focused discussion, not a monologue
- Respond using `send_message(to: "warroom", message: "your contribution")`
- Wait for your turn — don't send multiple messages
- Address points raised by other agents, build on good ideas, respectfully challenge bad ones
```

## Implementation Order

| Phase | What |
|-------|------|
| 1 | MCP server: add `"warroom"` as valid recipient |
| 2 | Rust: War Room state management commands (create, get, interject, skip, conclude) |
| 3 | Rust: Poller war room turn management logic |
| 4 | Frontend: WarRoom panel component (setup, discussion, concluded views) |
| 5 | Frontend: Wire navbar button, panel switching |
| 6 | Prompts: Add War Room section to all agents |
| 7 | Testing: Create a war room, run a 2-round discussion, conclude |

## Edge Cases

- **Agent doesn't respond**: After 60s timeout, auto-skip and note it in transcript
- **Agent goes offline mid-discussion**: Skip them, note in transcript, continue with remaining participants
- **Operator concludes while agent is mid-response**: Accept any pending response, then close
- **All agents offline**: Pause discussion, resume when at least one comes back online
