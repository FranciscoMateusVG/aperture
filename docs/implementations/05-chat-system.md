# 05 — Chat System: Operator ↔ Agent Communication

## 1. Overview

The Aperture chat system provides **bidirectional, real-time text communication** between the human operator (sitting at the Tauri desktop UI) and the AI agents running in tmux windows. It is a two-lane system:

- **Operator → Agent**: The operator types a message in `ChatPanel`. The Tauri backend writes a `.md` file into the agent's mailbox directory. The background poller detects it and injects it into the agent's tmux window as a `cat` command. The message is also appended to `chat-log.jsonl`.
- **Agent → Operator**: The agent calls `send_message(to: "operator", ...)` via its MCP tool. The MCP server writes a `.md` file into `~/.aperture/mailbox/operator/`. The background poller scans that directory, reads the file, appends it to `chat-log.jsonl`, and deletes the file. The `ChatPanel` polls `get_chat_messages` every 2 seconds and displays the history.

This design is intentionally **file-based for operator-bound messages** (not BEADS), because the Chat UI reads directly from `chat-log.jsonl`, not from the BEADS message bus. See [§10 Design Decisions](#10-design-decisions) for the rationale.

---

## 2. Operator → Agent Flow

### Step-by-step

1. **Operator types** a message in the `ChatPanel` input box and presses Enter or clicks the `↑` send button.
2. `sendMessage()` in `ChatPanel.ts` calls `commands.sendChat(activeAgent, text)`.
3. `commands.sendChat` invokes the Tauri IPC command `send_chat` with `{ toAgent, message }`.
4. The Rust handler `send_chat` in `agents.rs`:
   - Creates `~/.aperture/mailbox/<agent-name>/` if needed.
   - Generates a Unix-millisecond timestamp.
   - Writes a file named `<timestamp>-operator.md` with this content:
     ```
     # Message from the Human Operator

     <message text>

     ---
     _Reply using: send_message(to: "operator", message: "your reply")_
     ```
   - Appends a JSON line to `~/.aperture/chat-log.jsonl`:
     ```json
     {"from":"operator","to":"<agent>","content":"<message>","timestamp":"<ms>"}
     ```
5. Back in `ChatPanel.ts`, after `sendChat` resolves, `poll()` is called immediately to refresh the displayed history.
6. The background poller (running every 5 seconds in `poller.rs`) scans the agent's mailbox directory. When it finds the file, it runs a shell command via tmux:
   ```
   for f in '~/.aperture/mailbox/<agent>'/*.md; do [ -f "$f" ] && cat "$f" && rm "$f"; done
   ```
   This injects the markdown file content directly into the agent's Claude Code terminal session as if the user typed it, and then deletes the file.

### What the agent sees in its terminal

```
# Message from the Human Operator

Can you check the status of the auth service?

---
_Reply using: send_message(to: "operator", message: "your reply")_
```

---

## 3. Agent → Operator Flow

### Step-by-step

1. The agent calls its MCP tool `send_message` with `{ to: "operator", message: "..." }`.
2. `send_message` in `mcp-server/src/index.ts` checks: because `target === "operator"`, it takes the **file-based path** (not BEADS):
   ```typescript
   if (target === "operator" || target === "warroom") {
     const filepath = store.sendMessage(AGENT_NAME, target, message);
     return { content: [{ type: "text", text: `Message sent to ${target}. Delivered to: ${filepath}` }] };
   }
   ```
3. `MailboxStore.sendMessage(from, "operator", content)` in `mcp-server/src/store.ts`:
   - Ensures `~/.aperture/mailbox/operator/` exists.
   - Generates `Date.now()` timestamp.
   - Writes `~/.aperture/mailbox/operator/<timestamp>-<agent-name>.md` with:
     ```
     # Message from <agent-name>
     _<ISO timestamp>_

     <message content>
     ```
4. The background Rust poller (`poller.rs`, lines 250–264) runs on a 5-second tick and scans `~/.aperture/mailbox/operator/`:
   ```rust
   let operator_path = format!("{}/operator", mailbox_base);
   let operator_files = scan_mailbox(&operator_path);
   for filepath in &operator_files {
       if notified.contains(filepath) { continue; }
       if let Ok(content) = fs::read_to_string(filepath) {
           let (sender, timestamp) = parse_filename(filepath);
           log_message(&chat_log, &sender, "operator", &content, &timestamp);
           let _ = fs::remove_file(filepath);
       }
       notified.insert(filepath.clone());
   }
   ```
5. `log_message` appends a JSON line to `~/.aperture/chat-log.jsonl`:
   ```json
   {"from":"<agent>","to":"operator","content":"<full file content>","timestamp":"<ms-from-filename>"}
   ```
6. The file is immediately deleted from `~/.aperture/mailbox/operator/`.
7. `ChatPanel.ts` polls `get_chat_messages` every 2 seconds. The new entry appears in the panel under the agent's tab.

---

## 4. ChatPanel Component

**File**: `src/components/ChatPanel.ts`

### Structure

`createChatPanel(container: HTMLElement)` is called once with a DOM container element. It renders:

```html
<div class="chat">
  <div class="chat__header">
    <h3 class="section-title">Chat</h3>
    <button class="message-log__clear chat__clear-btn" title="Clear chat history">🗑</button>
  </div>
  <div class="chat__tabs"></div>        <!-- agent selector tabs -->
  <div class="chat__messages"></div>   <!-- scrollable message list -->
  <div class="chat__input-row">
    <input class="chat__input" type="text" placeholder="Message..." />
    <button class="chat__send">↑</button>
  </div>
</div>
```

### Known Agents & Colors

```typescript
const AGENTS = ["wheatley", "glados", "peppy", "izzy"];

const AGENT_COLORS: Record<string, string> = {
  glados: "#9b59b6",
  wheatley: "#3498db",
  peppy: "#1abc9c",
  izzy: "#e91e63",
};
```

The active agent tab sets a CSS custom property `--chat-agent-color` on the container to tint UI elements per-agent.

### Tab Rendering

```typescript
function renderTabs() {
  const color = AGENT_COLORS[activeAgent] ?? "#f39c12";
  container.style.setProperty("--chat-agent-color", color);

  tabsEl.innerHTML = AGENTS
    .map((name) =>
      `<button class="chat__tab ${name === activeAgent ? "chat__tab--active" : ""}" data-agent="${name}"
        style="${name === activeAgent ? `color: ${AGENT_COLORS[name] ?? "#f39c12"}` : ""}">${name}</button>`
    ).join("");

  tabsEl.querySelectorAll(".chat__tab").forEach((tab) => {
    tab.addEventListener("click", () => {
      activeAgent = (tab as HTMLElement).dataset.agent!;
      renderTabs();
      poll();
    });
  });
}
```

Clicking a tab changes `activeAgent` and triggers an immediate poll.

### Message Rendering

Messages are filtered to show only the conversation between `"operator"` and the currently active agent:

```typescript
function renderMessages(messages: ChatMessage[]) {
  const filtered = messages.filter(
    (m) =>
      (m.from === "operator" && m.to === activeAgent) ||
      (m.from === activeAgent && m.to === "operator")
  );

  // Auto-scroll: only if user was already near the bottom
  const wasAtBottom =
    messagesEl.scrollHeight - messagesEl.scrollTop - messagesEl.clientHeight < 40;

  messagesEl.innerHTML = filtered
    .map((m) => {
      const isMe = m.from === "operator";
      const body = isMe
        ? escapeHtml(m.content)
        : renderMarkdown(m.content);
      return `
        <div class="chat__msg ${isMe ? "chat__msg--me" : "chat__msg--agent"}">
          <div class="chat__msg-sender">${isMe ? "You" : m.from}</div>
          <div class="chat__msg-body${isMe ? "" : " chat__msg-body--md"}">${body}</div>
        </div>
      `;
    }).join("");

  if (wasAtBottom) {
    messagesEl.scrollTop = messagesEl.scrollHeight;
  }
}
```

- **Operator messages** (`chat__msg--me`): HTML-escaped only.
- **Agent messages** (`chat__msg--agent`): Passed through `renderMarkdown()` for rich rendering.

### Markdown Renderer

A lightweight inline markdown-to-HTML function handles:
- Fenced code blocks (protected from further processing via placeholder tokens)
- GFM tables
- Inline code, headings (`#`, `##`, `###`), bold/italic, HR
- Unordered and ordered lists
- Paragraphs (double-newline separated)

HTML is escaped first to prevent injection; code blocks are extracted, escaped, and restored last.

### Sending a Message

```typescript
async function sendMessage() {
  const text = inputEl.value.trim();
  if (!text) return;
  inputEl.value = "";
  await commands.sendChat(activeAgent, text);
  poll();
}

sendBtn.addEventListener("click", sendMessage);
inputEl.addEventListener("keydown", (e) => {
  if (e.key === "Enter") sendMessage();
});
```

### Polling

```typescript
async function poll() {
  try {
    const messages = await commands.getChatMessages();
    renderMessages(messages);
  } catch {
    // Log might not exist yet — silently skip
  }
}

poll();                              // immediate on mount
const interval = setInterval(poll, 2000);  // then every 2s
```

The component returns `{ destroy() }` which clears the interval on unmount.

### Clear Button

```typescript
clearBtn.addEventListener("click", async () => {
  await commands.clearChatHistory();
  messagesEl.innerHTML = "";
});
```

---

## 5. MessageLog Component

**File**: `src/components/MessageLog.ts`

`MessageLog` is a **separate panel** from `ChatPanel`. It shows **agent-to-agent messages** (not operator chat). It reads from `message-log.jsonl` via `get_recent_messages`.

### Structure

```html
<div class="message-log">
  <div class="message-log__header">
    <h3 class="section-title">Messages</h3>
    <button class="message-log__clear" title="Clear history">🗑</button>
  </div>
  <div class="message-log__conversations"></div>
</div>
```

### Conversation Grouping

Messages are grouped by conversation pair (sorted alphabetically so `a:b` and `b:a` are the same key):

```typescript
function getConversationKey(a: string, b: string): string {
  return [a, b].sort().join(":");
}
```

Each conversation is rendered as a collapsible section:
- Header shows `<agentA> ↔ <agentB>` and a message count.
- Collapsed state tracked in a `Set<string>` — survives re-renders within the session.
- Individual conversation clear button calls `commands.clearConversationHistory(a, b)`.

### Polling

```typescript
poll();
const interval = setInterval(poll, 3000);  // every 3 seconds
```

### AgentMessage vs ChatMessage

`MessageLog` uses `AgentMessage` (from `message-log.jsonl`):
```typescript
interface AgentMessage {
  id: number;
  from_agent: string;
  to_agent: string;
  content: string;
  timestamp: string;
  read: number;
}
```

`ChatPanel` uses `ChatMessage` (from `chat-log.jsonl`):
```typescript
interface ChatMessage {
  from: string;
  to: string;
  content: string;
  timestamp: string;
}
```

Note: `AgentMessage.content` is truncated to 200 characters by the Rust backend before serving it (see `get_recent_messages` in `agents.rs`).

---

## 6. File-Based Mailbox

### Directory Structure

```
~/.aperture/
├── mailbox/
│   ├── operator/          ← agent → operator messages land here
│   │   └── <timestamp>-<agent>.md
│   ├── glados/            ← operator → glados (or other agents via BEADS)
│   │   └── <timestamp>-operator.md
│   ├── wheatley/
│   ├── peppy/
│   ├── izzy/
│   ├── warroom/           ← War Room submissions (separate system)
│   ├── _spawn/            ← spiderling spawn requests (.json)
│   ├── _kill/             ← spiderling kill requests
│   └── <spiderling-name>/
├── chat-log.jsonl         ← operator ↔ agent chat history
└── message-log.jsonl      ← agent ↔ agent message history
```

### File Naming Convention

```
<unix-millisecond-timestamp>-<sender-name>.md
```

Examples:
- `1774191634309-glados.md` — message from glados to operator
- `1774191634309-operator.md` — message from operator to glados

The timestamp is the leading number; the sender is everything after the first `-` up to `.md`.

### Filename Parsing (Rust)

```rust
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
```

This correctly handles agent names with hyphens (e.g. `spider-doc-chat`).

### File Content Format

Both operator-sent and agent-sent files use a markdown format:

**Operator → Agent** (written by `send_chat` in `agents.rs`):
```markdown
# Message from the Human Operator

<message text>

---
_Reply using: send_message(to: "operator", message: "your reply")_
```

**Agent → Operator** (written by `MailboxStore.sendMessage` in `store.ts`):
```markdown
# Message from <agent-name>
_<ISO-8601 timestamp>_

<message content>
```

### File Lifecycle

1. Written atomically to mailbox directory.
2. Read by the poller on its next 5-second tick.
3. Logged to the appropriate `.jsonl` file.
4. Deleted immediately after logging (`fs::remove_file`).

The poller uses a `HashSet<String>` (`notified`) to avoid double-processing files it has already seen within the same run. For operator-bound files, the notified set is pruned each tick to remove entries for files that no longer exist:
```rust
notified.retain(|f| !f.starts_with(&operator_path) || operator_files.contains(f));
```

---

## 7. Chat Log

### File

`~/.aperture/chat-log.jsonl`

### Format

One JSON object per line (JSONL). Each line represents one message:

```json
{"from":"operator","to":"glados","content":"Can you check the auth service?","timestamp":"1774191634309"}
{"from":"glados","to":"operator","content":"# Message from glados\n_2026-03-22T15:00:34.309Z_\n\n**Progress update...**\n","timestamp":"1774191634309"}
```

### Fields

| Field | Type | Description |
|-------|------|-------------|
| `from` | string | Sender identity. `"operator"` or agent name (e.g. `"glados"`) |
| `to` | string | Recipient identity. `"operator"` or agent name |
| `content` | string | Raw message text. Agent messages include the full markdown file content (with `# Message from` header). Operator messages are plain text. |
| `timestamp` | string | Unix milliseconds as a string |

### Writing

Two code paths write to `chat-log.jsonl`:

1. **Operator → Agent** (`send_chat` in `agents.rs`):
   ```rust
   let entry = serde_json::json!({
       "from": "operator",
       "to": to_agent,
       "content": message,
       "timestamp": timestamp.to_string(),
   });
   writeln!(file, "{}", entry.to_string());
   ```
   Content is just the raw message text (no markdown wrapper).

2. **Agent → Operator** (`log_message` in `poller.rs`):
   ```rust
   fn log_message(log_path: &str, from: &str, to: &str, content: &str, timestamp: &str) {
       let entry = serde_json::json!({
           "from": from,
           "to": to,
           "content": content,
           "timestamp": timestamp,
       });
       writeln!(file, "{}", entry.to_string());
   }
   ```
   Content is the **full file content** read from the `.md` file (including the `# Message from` header and ISO timestamp line).

### Reading

`get_chat_messages` in `agents.rs` reads the entire file, parses each line, and returns the last 200 entries:

```rust
let start = if messages.len() > 200 { messages.len() - 200 } else { 0 };
let recent: Vec<serde_json::Value> = messages[start..]
    .iter()
    .map(|m| serde_json::json!({
        "from": m.get("from")...,
        "to": m.get("to")...,
        "content": m.get("content")...,
        "timestamp": m.get("timestamp")...,
    }))
    .collect();
```

Unlike `get_recent_messages` (for agent messages), chat messages are **not truncated** in content length.

### Clearing

`clear_chat_history` truncates the file to empty:
```rust
fs::write(&chat_log, "").map_err(|e| e.to_string())
```

---

## 8. Backend Commands

All Tauri IPC commands for chat are defined in `src-tauri/src/agents.rs` and registered in `src-tauri/src/lib.rs`.

### `send_chat`

```rust
#[tauri::command]
pub fn send_chat(to_agent: String, message: String) -> Result<(), String>
```

**What it does:**
1. Ensures `~/.aperture/mailbox/<to_agent>/` exists.
2. Creates `<timestamp>-operator.md` in that directory with the operator message + reply instructions.
3. Appends a `ChatMessage` JSON line to `chat-log.jsonl`.

**Frontend call:**
```typescript
commands.sendChat(activeAgent, text)
// → invoke<void>("send_chat", { toAgent, message })
```

---

### `get_chat_messages`

```rust
#[tauri::command]
pub fn get_chat_messages() -> Result<serde_json::Value, String>
```

**What it does:**
- Reads `~/.aperture/chat-log.jsonl`.
- Returns last 200 entries as a JSON array of `ChatMessage` objects.
- Returns `[]` if file doesn't exist.

**Frontend call:**
```typescript
commands.getChatMessages()
// → invoke<ChatMessage[]>("get_chat_messages")
```

---

### `clear_chat_history`

```rust
#[tauri::command]
pub fn clear_chat_history() -> Result<(), String>
```

Truncates `chat-log.jsonl` to an empty file.

**Frontend call:**
```typescript
commands.clearChatHistory()
// → invoke<void>("clear_chat_history")
```

---

### `get_recent_messages`

```rust
#[tauri::command]
pub fn get_recent_messages(_state: tauri::State<'_, Arc<Mutex<AppState>>>) -> Result<serde_json::Value, String>
```

**What it does:**
- Reads `~/.aperture/message-log.jsonl` (agent-to-agent messages, NOT chat).
- Returns last 100 entries, reversed (newest first).
- Content truncated to 200 characters per message.

Used by `MessageLog`, not `ChatPanel`.

---

### `clear_message_history`

Truncates `message-log.jsonl` to empty.

---

### `clear_conversation_history`

```rust
#[tauri::command]
pub fn clear_conversation_history(agent_a: String, agent_b: String) -> Result<(), String>
```

Rewrites `message-log.jsonl` excluding all lines where `(from == agentA && to == agentB) || (from == agentB && to == agentA)`.

---

## 9. Key Code Snippets

### MCP `send_message` — operator routing decision (`mcp-server/src/index.ts`)

```typescript
// Operator and warroom still use file-based delivery (Chat panel + War Room turn mechanics)
if (target === "operator" || target === "warroom") {
  const filepath = store.sendMessage(AGENT_NAME, target, message);
  return {
    content: [{ type: "text", text: `Message sent to ${target}. Delivered to: ${filepath}` }],
  };
}

// All agent-to-agent messages go through BEADS
try {
  const result = await createMessage(AGENT_NAME, target, message);
  // ...
} catch (e: any) {
  // Fallback to file-based delivery if BEADS fails
  const filepath = store.sendMessage(AGENT_NAME, target, message);
  // ...
}
```

### MailboxStore — file creation (`mcp-server/src/store.ts`)

```typescript
sendMessage(from: string, to: string, content: string): string {
  const mailboxDir = this.ensureMailbox(to);
  const timestamp = Date.now();
  const filename = `${timestamp}-${from}.md`;
  const filepath = join(mailboxDir, filename);
  const fileContent = `# Message from ${from}\n_${new Date().toISOString()}_\n\n${content}\n`;
  writeFileSync(filepath, fileContent, "utf-8");
  return filepath;
}
```

### Rust poller — operator mailbox scan (`src-tauri/src/poller.rs`)

```rust
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
```

### Rust — `send_chat` command (`src-tauri/src/agents.rs`)

```rust
#[tauri::command]
pub fn send_chat(to_agent: String, message: String) -> Result<(), String> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let mailbox_dir = format!("{}/.aperture/mailbox/{}", home, to_agent);
    let _ = fs::create_dir_all(&mailbox_dir);

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();

    let filename = format!("{}/{}-operator.md", mailbox_dir, timestamp);
    let content = format!(
        "# Message from the Human Operator\n\n{}\n\n---\n_Reply using: send_message(to: \"operator\", message: \"your reply\")_\n",
        message
    );
    fs::write(&filename, &content).map_err(|e| e.to_string())?;

    // Also log to chat history
    let chat_log = format!("{}/.aperture/chat-log.jsonl", home);
    let entry = serde_json::json!({
        "from": "operator",
        "to": to_agent,
        "content": message,
        "timestamp": timestamp.to_string(),
    });
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&chat_log) {
        use std::io::Write;
        let _ = writeln!(file, "{}", entry.to_string());
    }

    Ok(())
}
```

### TypeScript IPC wrappers (`src/services/tauri-commands.ts`)

```typescript
sendChat: (toAgent: string, message: string) => invoke<void>("send_chat", { toAgent, message }),
getChatMessages: () => invoke<ChatMessage[]>("get_chat_messages"),
clearChatHistory: () => invoke<void>("clear_chat_history"),
getRecentMessages: () => invoke<AgentMessage[]>("get_recent_messages"),
clearMessageHistory: () => invoke<void>("clear_message_history"),
clearConversationHistory: (agentA: string, agentB: string) => invoke<void>("clear_conversation_history", { agentA, agentB }),
```

### ChatMessage type (`src/types.ts`)

```typescript
export interface ChatMessage {
  from: string;
  to: string;
  content: string;
  timestamp: string;
}
```

---

## 10. Design Decisions

### Why file-based for operator messages (not BEADS)?

**Operator-bound messages (agent → operator)** use `~/.aperture/mailbox/operator/` + `chat-log.jsonl` instead of BEADS for two reasons:

1. **The Chat UI reads from `chat-log.jsonl` directly.** The `ChatPanel` polls `get_chat_messages` which reads this file. BEADS is a task/message bus for agents; the operator's chat history needs to be a persistent, ordered log that accumulates over time — not a message queue that gets consumed and cleared.

2. **BEADS messages are fire-and-forget for delivery.** When an agent sends a BEADS message to another agent, the poller delivers it and marks it read. There's no "chat history" concept in BEADS. The operator needs to scroll back through an entire conversation; `chat-log.jsonl` provides that append-only history.

**Agent-bound messages (operator → agent)** also use the file mailbox because:
- Agent processes use file injection via tmux (`cat file && rm file`), which is the same delivery mechanism for both agent-to-agent (legacy fallback) and operator-to-agent messages.
- The operator message includes a markdown hint: `_Reply using: send_message(to: "operator", ...)_` that teaches the agent how to respond.

### Why poll every 2 seconds (ChatPanel) vs 3 seconds (MessageLog)?

Chat is human-facing and latency-sensitive; 2s feels responsive. Agent-to-agent messages are operational logs; 3s is sufficient and reduces backend load.

### Why does agent→operator content include the full markdown file?

The `log_message` function in `poller.rs` writes the **raw file content** (including `# Message from X` and ISO timestamp header) to `chat-log.jsonl`. This is intentional: it preserves exactly what the agent produced, and `renderMarkdown()` in `ChatPanel.ts` renders it correctly on the frontend. The header becomes an `<h1>` and the italic timestamp becomes `<em>` — which actually looks reasonable in the chat UI.

### Why is the operator-tab list hardcoded to 4 agents?

```typescript
const AGENTS = ["wheatley", "glados", "peppy", "izzy"];
```

Spiderlings are ephemeral and do not have dedicated chat tabs. Operators communicate with spiderlings indirectly (via the permanent agents) or by reading their BEADS task updates. If a spiderling sends a message to `"operator"`, it appears in the chat log but under its own sender name — the operator can see it by looking at the raw log or by filtering in a future enhancement.

### Message isolation per tab

`ChatPanel` filters messages so each tab shows **only** the conversation with that agent. The full `chat-log.jsonl` is loaded every poll cycle; filtering happens client-side. This means the 200-message limit in `get_chat_messages` is shared across all agents — heavy conversations with one agent may push old messages from another agent out of the window.
