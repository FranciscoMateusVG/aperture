# 04 — Poller System

> The background delivery daemon. The heartbeat of Aperture.

---

## 1. Overview

The Poller is a Rust background thread that runs an infinite loop with a **5-second sleep between each tick**. It is the only component in Aperture responsible for:

1. **Spawning spiderlings** — consuming JSON spawn requests from a mailbox directory
2. **Killing spiderlings** — consuming kill requests from a mailbox directory
3. **War Room turn advancement** — routing agent messages through the War Room turn system
4. **Operator message logging** — routing agent→human messages to `chat-log.jsonl`
5. **BEADS message delivery** — querying unread BEADS messages and injecting them into agent tmux windows
6. **Legacy file-based delivery** — scanning per-agent mailbox directories for `.md` files and injecting them

Nothing else in the system does message routing. If the poller is not running, agents cannot receive messages and spiderlings cannot be spawned or killed.

**Entry point:** `src-tauri/src/poller.rs`, function `run_message_poller`.

---

## 2. Initialization — Spawning the Poller in `lib.rs`

The Tauri application initializes BEADS (the task/message database) and then spawns the poller as a detached OS thread. This happens once, at startup, before the Tauri event loop begins.

```rust
// src-tauri/src/lib.rs (lines 85–89)

// Start background message delivery poller
let poller_state = Arc::clone(&app_state);
std::thread::spawn(move || {
    poller::run_message_poller(poller_state);
});
```

The poller receives an `Arc<Mutex<AppState>>` — a reference-counted, mutex-guarded handle to the application state. This allows it to safely read agent/spiderling lists and mutate them (e.g., when spawning or killing a spiderling) without data races.

### Pre-Poller Setup

Before spawning the poller, `lib.rs` ensures BEADS is ready:

1. **Dolt initialization** — checks if `~/.aperture/.beads/config.json` exists; if not, runs `dolt init` in that directory
2. **Dolt SQL server** — runs `bd dolt test` to check connectivity; if not running, spawns `dolt sql-server --port 3307 --host 127.0.0.1` as a background process (with a 2-second wait for startup)
3. **BEADS init** — runs `bd init --quiet` to create the schema if not yet done

Only after these steps completes does `std::thread::spawn` kick off the poller.

---

## 3. The Main Loop — Exact Sequence Per Tick

The poller's `run_message_poller` function is a single `loop {}` block. Every iteration:

```rust
pub fn run_message_poller(state: Arc<Mutex<AppState>>) {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let mailbox_base = format!("{}/.aperture/mailbox", home);
    let message_log = format!("{}/.aperture/message-log.jsonl", home);
    let chat_log = format!("{}/.aperture/chat-log.jsonl", home);

    // Ensure operator mailbox exists
    let _ = fs::create_dir_all(format!("{}/operator", mailbox_base));

    let mut notified: HashSet<String> = HashSet::new();
    let mut warroom_notified: HashSet<String> = HashSet::new();

    loop {
        std::thread::sleep(Duration::from_secs(5));

        // 1. Handle spawn requests  (_spawn/ mailbox)
        // 2. Handle kill requests   (_kill/ mailbox)
        // 3. Handle war room messages (warroom/ mailbox)
        // 4. Handle operator-bound messages (operator/ mailbox)
        // 5. Handle BEADS message delivery (per running agent/spiderling)
        // 6. Handle legacy file-based messages (per-agent mailbox dirs)
    }
}
```

**Key state initialized before the loop:**
- `mailbox_base` — `~/.aperture/mailbox` — root of all mailbox directories
- `message_log` — `~/.aperture/message-log.jsonl` — all agent↔agent messages logged here
- `chat_log` — `~/.aperture/chat-log.jsonl` — agent→operator messages logged here
- `notified: HashSet<String>` — tracks already-delivered BEADS message IDs and operator file paths to avoid duplicate delivery
- `warroom_notified: HashSet<String>` — tracks war room files already processed this cycle

The **first thing** in each iteration is `std::thread::sleep(Duration::from_secs(5))`. The poller always waits before acting.

---

## 4. Spawn Request Handling

### Mailbox Directory
```
~/.aperture/mailbox/_spawn/
```

The poller creates this directory if it doesn't exist on every tick (idempotent `create_dir_all`).

### Protocol
Any process (e.g., an agent using the MCP `spawn_spiderling` tool) writes a `.json` file into `_spawn/`. The poller reads it, spawns the spiderling, and deletes the file.

### JSON Format
```json
{
  "name": "spider-my-task",
  "task_id": "aperture-abc123",
  "prompt": "Your detailed task prompt here...",
  "requested_by": "glados",
  "project_path": "~/path/to/repo"  // optional
}
```

### Poller Code
```rust
// src-tauri/src/poller.rs (lines 128–188)
let spawn_dir = format!("{}/_spawn", mailbox_base);
let _ = fs::create_dir_all(&spawn_dir);

if let Ok(entries) = fs::read_dir(&spawn_dir) {
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        if let Ok(content) = fs::read_to_string(&path) {
            if let Ok(req) = serde_json::from_str::<serde_json::Value>(&content) {
                let name = req
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let task_id = req
                    .get("task_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let prompt = req
                    .get("prompt")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let requested_by = req
                    .get("requested_by")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let project_path = req
                    .get("project_path")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());

                if !name.is_empty() {
                    let mut app_state = match state.lock() {
                        Ok(s) => s,
                        Err(_) => continue,
                    };
                    match crate::spawner::spawn_spiderling(
                        name.clone(),
                        task_id,
                        prompt,
                        requested_by,
                        project_path,
                        &mut app_state,
                    ) {
                        Ok(_) => println!("Spawned spiderling: {}", name),
                        Err(e) => eprintln!("Failed to spawn {}: {}", name, e),
                    }
                }
            }
        }
        let _ = fs::remove_file(&path);
    }
}
```

**Key details:**
- Only `.json` files are processed; other extensions are skipped
- The file is **always deleted** regardless of whether parsing/spawning succeeded
- The mutex lock is acquired per-spawn (not held across all entries)
- If the name is empty, no spawn is attempted but the file is still deleted

### `spawn_spiderling` function (in `spawner.rs`)
Signature:
```rust
pub fn spawn_spiderling(
    name: String,
    task_id: String,
    prompt: String,
    requested_by: String,
    project_path: Option<String>,
    app_state: &mut AppState,
) -> Result<String, String>
```

What it does (in order):
1. Validates name (regex `^[a-z0-9][a-z0-9-]{0,30}$`, no conflicts with permanent agents)
2. Creates a git worktree at `~/.aperture/worktrees/<name>` on a branch named `<name>`
3. Creates a tmux window in the main session
4. Creates the spiderling mailbox directory: `~/.aperture/mailbox/<name>/`
5. Writes an MCP config JSON to `/tmp/aperture-mcp-<name>.json` (sets `AGENT_NAME`, `AGENT_ROLE`, `BEADS_DIR`, etc.)
6. Writes a system prompt to `~/.aperture/launchers/<name>-prompt.txt`
7. Writes a shell launcher script to `~/.aperture/launchers/<name>.sh` that runs:
   ```bash
   exec claude --dangerously-skip-permissions --model sonnet \
     --system-prompt "$PROMPT" \
     --mcp-config /tmp/aperture-mcp-<name>.json \
     --name <name>
   ```
8. Sends the launcher path to the tmux window via `tmux_send_keys`
9. Spawns a background thread that presses Enter 3 times (to confirm workspace trust) and then sends "Begin your task now. Read your system prompt carefully for full instructions."
10. Adds the `SpiderlingDef` to `app_state.spiderlings` and writes `active-spiderlings.json`

---

## 5. Kill Request Handling

### Mailbox Directory
```
~/.aperture/mailbox/_kill/
```

### Protocol
Any file dropped into `_kill/` whose text content is a spiderling name triggers termination. The file extension doesn't matter.

### Poller Code
```rust
// src-tauri/src/poller.rs (lines 190–214)
let kill_dir = format!("{}/_kill", mailbox_base);
let _ = fs::create_dir_all(&kill_dir);

if let Ok(entries) = fs::read_dir(&kill_dir) {
    for entry in entries.flatten() {
        let path = entry.path();
        if let Ok(content) = fs::read_to_string(&path) {
            let name = content.trim().to_string();
            if !name.is_empty() {
                let mut app_state = match state.lock() {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                match crate::spawner::kill_spiderling(name.clone(), &mut app_state) {
                    Ok(_) => println!("Killed spiderling: {}", name),
                    Err(e) => eprintln!("Failed to kill {}: {}", name, e),
                }
            }
        }
        let _ = fs::remove_file(&path);
    }
}
```

**Key details:**
- File content is trimmed to get the name (handles trailing newlines)
- File is deleted regardless of success/failure

### `kill_spiderling` function (in `spawner.rs`)
Signature:
```rust
pub fn kill_spiderling(name: String, app_state: &mut AppState) -> Result<(), String>
```

What it does:
1. Looks up the spiderling by name in `app_state.spiderlings`
2. Sends `C-c` to the tmux window (interrupts current process)
3. Waits 500ms
4. Sends `/exit` (exits Claude)
5. Waits 500ms
6. Kills the tmux window via `tmux kill-window`
7. Removes the git worktree with `git worktree remove --force`
8. Deletes: `~/.aperture/launchers/<name>.sh`, `~/.aperture/launchers/<name>-prompt.txt`, `/tmp/aperture-mcp-<name>.json`
9. Removes entry from `app_state.spiderlings` and writes `active-spiderlings.json`

---

## 6. War Room Message Handling

### Mailbox Directory
```
~/.aperture/mailbox/warroom/
```

### Protocol
When an agent uses `send_message(to: 'warroom', message: '...')` via the MCP server, the MCP server writes a `.md` file into the `warroom/` mailbox. The poller picks it up, calls `warroom::handle_warroom_message`, advances the turn, and delivers the transcript to the next agent.

### Poller Code
```rust
// src-tauri/src/poller.rs (lines 216–248)
let warroom_mailbox = format!("{}/warroom", mailbox_base);
let _ = fs::create_dir_all(&warroom_mailbox);

let wr_state_path = format!("{}/.aperture/warroom/state.json", home);
if let Ok(wr_data) = fs::read_to_string(&wr_state_path) {
    if wr_data.contains("\"active\"") {
        let wr_files = scan_mailbox(&warroom_mailbox);
        warroom_notified.retain(|f| wr_files.contains(f));

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
                        let _ = fs::remove_file(filepath);
                    }
                }
            }
            warroom_notified.insert((*filepath).clone());
        }
    }
}
```

**Key details:**
- The poller only enters this block if `state.json` exists AND contains the string `"active"` (fast substring check, not full JSON parse)
- `warroom_notified` prevents double-processing files within the same cycle (though files are deleted on success anyway)
- Files are deleted even on `Err` — errors from `handle_warroom_message` (e.g., wrong agent's turn) are swallowed

### `handle_warroom_message` function (in `warroom.rs`)
Signature:
```rust
pub fn handle_warroom_message(
    sender: &str,
    content: &str,
    app_state: &Arc<Mutex<AppState>>,
) -> Result<(), String>
```

What it does:
1. Reads the War Room state from `~/.aperture/warroom/state.json`
2. Validates the war room is "active" and that `sender == wr_state.current_agent` (wrong-turn errors are returned as `Err`)
3. Appends the agent's contribution to `~/.aperture/warroom/transcript.jsonl`
4. Checks for `[CONCLUDE]` in the content:
   - If all participants have voted, marks status "concluded", archives to history, cleans up state/transcript files
   - If not all voted, saves the partial votes and continues turn advancement
5. Advances `current_turn` with modular arithmetic: `next_turn = (current_turn + 1) % participants.len()`
6. If `next_turn <= current_turn`, increments `round` (wrap-around detection)
7. Updates `current_agent` to the next participant
8. Calls `deliver_to_agent(next_agent, app_state)` to inject the transcript context into the next agent's tmux window

### `deliver_to_agent` (in `warroom.rs`)
This function builds the full War Room context string:
```
# WAR ROOM — {topic}
## Room: {id} | Round {round}

[instructions to agent]

---
[AGENT_A]: message...
[AGENT_B]: message...
---

It is now YOUR turn ({agent}). Share your perspective on the topic above.
```

It writes this to `/tmp/aperture-warroom-context.md` and delivers it via:
```rust
tmux::tmux_send_keys(window_id, format!("cat {}", context_path))?;
```

Note: this uses `cat` without `rm`, so the file persists (it will be overwritten on the next delivery).

---

## 7. Operator Message Handling

### Mailbox Directory
```
~/.aperture/mailbox/operator/
```

### Protocol
Agents write `.md` files here when they want to send a message to the human operator. The poller logs them to `chat-log.jsonl` and deletes the files.

### Poller Code
```rust
// src-tauri/src/poller.rs (lines 250–264)
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

**Key details:**
- Uses the shared `notified` HashSet to avoid re-reading a file that was already processed
- Files are cleaned from `notified` when they no longer appear in the directory scan (the `retain` at the end)
- Logged to `chat-log.jsonl` (not `message-log.jsonl`) — operator messages use a different log

### `log_message` helper
```rust
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
```

Each log entry is a single-line JSON object. Files are created if they don't exist. Write failures are silently ignored.

---

## 8. BEADS Message Delivery

This is the primary inter-agent message delivery mechanism post-BEADS migration. Every 5 seconds, the poller queries BEADS for unread messages destined for each running agent and spiderling.

### Step 1 — Build the agent list

```rust
// src-tauri/src/poller.rs (lines 266–295)
let agents: Vec<(String, String)> = {
    let Ok(app_state) = state.lock() else {
        continue;
    };

    let named: Vec<(String, String)> = app_state
        .agents
        .values()
        .filter(|a| a.status == "running")
        .filter_map(|a| {
            a.tmux_window_id
                .as_ref()
                .map(|wid| (a.name.clone(), wid.clone()))
        })
        .collect();

    let spiderlings: Vec<(String, String)> = app_state
        .spiderlings
        .values()
        .filter(|s| s.status == "working")
        .filter_map(|s| {
            s.tmux_window_id
                .as_ref()
                .map(|wid| (s.name.clone(), wid.clone()))
        })
        .collect();

    named.into_iter().chain(spiderlings).collect()
};
```

Only agents with `status == "running"` and spiderlings with `status == "working"` are considered. Both must have a `tmux_window_id` to be eligible for delivery.

The mutex is held only to collect the list, then released before doing any I/O.

### Step 2 — Query BEADS for unread messages

```rust
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
```

**BEADS message format:** Messages have titles like `[sender->recipient] preview text...`. The query uses `title="->glados]"` to match all messages sent to "glados", regardless of sender.

**`bd` path:** `~/.local/bin/bd`

**`BEADS_DIR`:** `~/.aperture/.beads`

**Environment for all `bd` calls:**
```rust
fn path_env() -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let current = std::env::var("PATH").unwrap_or_default();
    format!("{}/.local/bin:/opt/homebrew/bin:/usr/local/bin:{}", home, current)
}
```

The parsed message struct:
```rust
#[derive(Debug, Deserialize)]
struct BeadsMessage {
    id: String,
    title: String,
    description: Option<String>,
}
```

### Step 3 — Format and deliver each message

```rust
// src-tauri/src/poller.rs (lines 297–335)
for (agent_name, window_id) in &agents {
    let messages = query_unread_messages(agent_name);

    for msg in &messages {
        if notified.contains(&msg.id) {
            continue;
        }

        let sender = parse_sender_from_title(&msg.title);
        let content = msg.description.as_deref().unwrap_or("(no content)");
        let timestamp = &msg.id;

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

        log_message(&message_log, &sender, agent_name, &formatted, timestamp);

        let tmp_path = format!("/tmp/aperture-msg-{}.md", msg.id);
        if fs::write(&tmp_path, &formatted).is_ok() {
            let cmd = format!("cat '{}' && rm '{}'", tmp_path, tmp_path);
            let _ = tmux::tmux_send_keys(window_id.clone(), cmd);
        }

        mark_message_read(&msg.id);
        notified.insert(msg.id.clone());
    }
    // ... (legacy file handling below)
}
```

### Parsing the sender from the title

```rust
fn parse_sender_from_title(title: &str) -> String {
    if let Some(start) = title.find('[') {
        if let Some(arrow) = title.find("->") {
            return title[start + 1..arrow].to_string();
        }
    }
    "unknown".to_string()
}
```

Given title `[glados->peppy] Fix the deploy`, this returns `"glados"`.

### Formatted message delivered to agent

```markdown
# Message from glados
_1742831234567_

Fix the deploy — it keeps crashing on startup.
```

### Marking as read

```rust
fn mark_message_read(message_id: &str) {
    let _ = std::process::Command::new(bd_path())
        .args(["close", message_id, "--reason", "delivered", "-q"])
        .env("BEADS_DIR", beads_dir())
        .env("BD_ACTOR", "poller")
        .env("PATH", path_env())
        .output();
}
```

Closes the BEADS task with reason "delivered". This sets `status=closed`, preventing the message from appearing in future queries.

---

## 9. Legacy File-Based Messages

For backward compatibility, the poller also scans each agent's named mailbox directory for `.md` files and delivers them via a shell loop.

```rust
// src-tauri/src/poller.rs (lines 338–353)
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
```

**Key details:**
- All `.md` files in the directory are logged before any delivery attempt
- A single shell `for` loop command is sent to tmux that cats and removes each file atomically
- There is no `notified` deduplication for these files — the shell `rm` is the idempotency guarantee

### `scan_mailbox` helper

```rust
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
```

Returns an empty Vec if the directory doesn't exist (no panic).

### `parse_filename` helper

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

**Filename convention:** `<timestamp>-<sender-name>.md`

Example: `1742831234567-glados.md` → sender `"glados"`, timestamp `"1742831234567"`

Senders with hyphens in their name (e.g., `spider-my-task`) are preserved because only the first segment is stripped and the rest are re-joined with `-`.

---

## 10. tmux Injection Pattern

### How `tmux_send_keys` works

```rust
// src-tauri/src/tmux.rs (lines 156–178)
#[tauri::command]
pub fn tmux_send_keys(target: String, keys: String) -> Result<(), String> {
    let is_special = keys.starts_with("C-") || keys.starts_with("M-");

    let output = if is_special {
        cmd("tmux")
            .args(["send-keys", "-t", &target, &keys])
            .output()
            .map_err(|e| e.to_string())?
    } else {
        cmd("tmux")
            .args(["send-keys", "-t", &target, "--", &keys, "Enter"])
            .output()
            .map_err(|e| e.to_string())?
    };

    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).to_string())
    }
}
```

For regular strings, `"Enter"` is appended automatically — the caller does NOT need to add `\n`. For control sequences like `C-c`, no Enter is appended.

### The `cat && rm` Pattern for BEADS Messages

```rust
let tmp_path = format!("/tmp/aperture-msg-{}.md", msg.id);
if fs::write(&tmp_path, &formatted).is_ok() {
    let cmd = format!("cat '{}' && rm '{}'", tmp_path, tmp_path);
    let _ = tmux::tmux_send_keys(window_id.clone(), cmd);
}
```

**Why not inline the content?**

`tmux send-keys` has shell escaping issues with multi-line content, special characters, and markdown. Instead:

1. The Rust process writes the formatted message to a temp file at `/tmp/aperture-msg-<id>.md`
2. A shell command `cat '/tmp/aperture-msg-<id>.md' && rm '/tmp/aperture-msg-<id>.md'` is sent to the agent's tmux window
3. The agent's shell executes this command, printing the message and deleting the temp file
4. Claude reads the printed output as part of its terminal context

The `&&` ensures the file is only deleted if `cat` succeeds. Single quotes are used to handle any spaces or special characters in the path.

### The shell loop for legacy files

```bash
for f in '/path/to/mailbox'/*.md; do [ -f "$f" ] && cat "$f" && rm "$f"; done
```

The `[ -f "$f" ]` guard prevents errors if the glob expands to a literal `*.md` string (when no files exist).

---

## 11. Error Handling

The poller is designed to **never crash**. All error handling uses the following patterns:

| Pattern | Usage |
|---------|-------|
| `let _ = ...` | Ignore the result entirely (fire-and-forget side effects like `create_dir_all`, `remove_file`, tmux sends) |
| `match ... { Ok(...) => ..., Err(e) => eprintln!(...) }` | Log spawn/kill failures to stderr but continue the loop |
| `if let Ok(content) = fs::read_to_string(...)` | Skip entries that can't be read |
| `unwrap_or_default()` | JSON parse failures return empty Vec (no messages delivered) |
| `let Ok(app_state) = state.lock() else { continue; }` | Mutex poison → skip this tick, try again in 5 seconds |
| `Err(_) => continue` within spawn/kill loops | Mutex failures in inner loops skip individual entries |

**Critical:** The `loop {}` has no top-level error handler. If the mutex is poisoned (extremely rare), the `continue` guards in the agent list building block cause the loop to restart. No `panic!` anywhere in the poller.

---

## 12. File Paths & Directory Structure

### Mailbox Directories (all under `~/.aperture/mailbox/`)

| Path | Purpose |
|------|---------|
| `~/.aperture/mailbox/_spawn/` | Spawn request JSON files (poller reads & deletes) |
| `~/.aperture/mailbox/_kill/` | Kill request text files (poller reads & deletes) |
| `~/.aperture/mailbox/warroom/` | War room message `.md` files (poller reads & deletes) |
| `~/.aperture/mailbox/operator/` | Agent→human message `.md` files (poller logs & deletes) |
| `~/.aperture/mailbox/<agent-name>/` | Legacy per-agent `.md` files (poller reads via shell loop) |

### Log Files

| Path | Content |
|------|---------|
| `~/.aperture/message-log.jsonl` | All agent↔agent messages (BEADS + legacy) |
| `~/.aperture/chat-log.jsonl` | Agent→operator messages |

### War Room Files

| Path | Content |
|------|---------|
| `~/.aperture/warroom/state.json` | Current war room state (participants, turn, round, status) |
| `~/.aperture/warroom/transcript.jsonl` | Running JSONL transcript (one entry per line) |
| `~/.aperture/warroom/history/<id>.jsonl` | Archived transcript after conclusion |
| `~/.aperture/warroom/history/<id>.state.json` | Archived state after conclusion |

### Spiderling Files

| Path | Content |
|------|---------|
| `~/.aperture/worktrees/<name>/` | Git worktree for spiderling |
| `~/.aperture/launchers/<name>.sh` | Shell script that launches Claude |
| `~/.aperture/launchers/<name>-prompt.txt` | System prompt text |
| `/tmp/aperture-mcp-<name>.json` | MCP server config for Claude invocation |
| `~/.aperture/active-spiderlings.json` | JSON array of all current spiderlings |

### BEADS Database

| Path | Content |
|------|---------|
| `~/.aperture/.beads/` | Dolt database directory |
| `~/.aperture/.beads/config.json` | BEADS config (existence signals initialization) |
| `~/.local/bin/bd` | The `bd` CLI binary used for all BEADS operations |

### Temp Files (delivery)

| Path | Content |
|------|---------|
| `/tmp/aperture-msg-<beads-id>.md` | Formatted message for tmux injection (created by poller, deleted by agent's shell) |
| `/tmp/aperture-warroom-context.md` | War Room transcript context (overwritten each delivery) |

---

## Summary: Complete Polling Tick

```
sleep(5s)
│
├─ _spawn/ mailbox
│   └─ for each .json file:
│       parse name/task_id/prompt/requested_by/project_path
│       → spawner::spawn_spiderling(...)
│       delete file
│
├─ _kill/ mailbox
│   └─ for each file:
│       read name from file content
│       → spawner::kill_spiderling(...)
│       delete file
│
├─ warroom/ mailbox (only if state.json exists and contains "active")
│   └─ for each new .md file:
│       parse sender from filename
│       → warroom::handle_warroom_message(sender, content, state)
│         → appends to transcript
│         → advances turn
│         → delivers transcript to next agent via tmux
│       delete file
│
├─ operator/ mailbox
│   └─ for each .md file (not in notified):
│       parse sender/timestamp from filename
│       → log_message(chat_log, sender, "operator", content, ts)
│       delete file
│       add to notified
│
└─ for each running agent/spiderling with tmux window:
    │
    ├─ BEADS delivery
    │   └─ bd query "type=message AND status=open AND title='->agent]'"
    │       for each unread message (not in notified):
    │           parse sender from title
    │           format markdown: "# Message from {sender}\n_{timestamp}_\n\n{content}\n"
    │           log_message(message_log, sender, agent, formatted, id)
    │           write to /tmp/aperture-msg-<id>.md
    │           tmux_send_keys: "cat '/tmp/...' && rm '/tmp/...'"
    │           bd close <id> --reason delivered
    │           add id to notified
    │
    └─ Legacy file delivery
        └─ scan ~/.aperture/mailbox/<agent>/*.md
            if files exist:
                log all to message_log
                tmux_send_keys: shell for-loop to cat && rm all files
```
