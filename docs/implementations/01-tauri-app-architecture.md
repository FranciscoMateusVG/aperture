# Aperture Tauri App — Architecture & Frontend Implementation Guide

> **Purpose:** This document is complete enough for another AI agent (or human) to rebuild the Aperture desktop application from scratch. It covers every major system: Rust backend, TypeScript frontend, IPC layer, layout, build pipeline, and the multi-agent orchestration plumbing underneath.

---

## 1. Overview

**Aperture** is a macOS desktop application built with [Tauri v2](https://tauri.app/) that serves as the control panel for a multi-agent AI system. It wraps a vanilla TypeScript frontend inside a native webview and exposes a Rust backend that:

1. **Controls tmux** — creates sessions, windows, and sends keystrokes so AI agents (Claude instances) can be launched in isolated terminal panes.
2. **Drives a PTY** — connects an xterm.js terminal in the UI to a real tmux session via a pseudo-terminal.
3. **Manages agents** — starts/stops four named AI agents (GLaDOS, Wheatley, Peppy, Izzy), each running as `claude` CLI processes with custom MCP config.
4. **Manages spiderlings** — spawns ephemeral worker agents in isolated git worktrees, kills them, tracks them in state.
5. **Brokers messages** — a background poller reads BEADS (a Dolt-backed task/message bus) and the file-based mailbox system, then delivers messages to running agents via tmux key injection.
6. **Tracks objectives** — a Kanban-style view of objectives stored in `~/.aperture/objectives.json`, linked to BEADS tasks.
7. **Runs War Rooms** — structured multi-agent debates that agents participate in via the message bus.

The app **has no React, Vue, or any component framework**. The frontend is plain TypeScript with manual DOM manipulation. There is no server; all data flows through Tauri's IPC bridge (Rust ↔ JS via `invoke`).

---

## 2. Tech Stack

| Layer | Technology | Version |
|---|---|---|
| Desktop shell | Tauri | 2.x (`"^2"` in Cargo) |
| Rust edition | Rust | 2021 |
| Frontend bundler | Vite | ^8.0.0 |
| Frontend language | TypeScript | ^5.9.3 |
| Terminal emulator | xterm.js (`@xterm/xterm`) | ^6.0.0 |
| Terminal fit addon | `@xterm/addon-fit` | ^0.11.0 |
| Terminal WebGL renderer | `@xterm/addon-webgl` | ^0.19.0 |
| Tauri JS API | `@tauri-apps/api` | ^2.10.1 |
| Tauri CLI | `@tauri-apps/cli` | ^2.10.1 |
| PTY (Rust) | `portable-pty` | 0.8 |
| Serialization (Rust) | `serde` + `serde_json` | 1.x |
| Regex (Rust) | `regex` | 1.x |
| Package manager | pnpm | (any) |
| Task DB backend | Dolt (`dolt sql-server`) | external |
| Task CLI | `bd` (BEADS CLI) | external |
| Agent runtime | `claude` CLI | external |
| Terminal multiplexer | `tmux` | `/opt/homebrew/bin/tmux` |

---

## 3. Project Structure

```
aperture/
├── index.html                  # Single HTML entry point
├── package.json                # Frontend deps (Vite, xterm.js, @tauri-apps/api)
├── vite.config.ts              # Vite config (port 1420, ignores src-tauri/)
├── tsconfig.json               # TypeScript config
│
├── src/                        # Frontend TypeScript
│   ├── main.ts                 # App bootstrap — init(), panel routing, view switching
│   ├── types.ts                # All shared TypeScript interfaces
│   ├── style.css               # Full app CSS (dark theme, CSS variables)
│   │
│   ├── services/
│   │   ├── tauri-commands.ts   # All invoke() wrappers — single source of truth for IPC
│   │   └── event-listener.ts  # Tauri event listeners (pty-output)
│   │
│   └── components/
│       ├── Navbar.ts           # Top bar: logo, connection dot, panel toggle buttons
│       ├── AgentList.ts        # Left sidebar: polls list_agents every 3s, renders cards
│       ├── AgentCard.ts        # Per-agent card: icon, name, model, start/stop toggle
│       ├── StatusBar.ts        # Simple connection status bar (unused in current layout)
│       ├── Terminal.ts         # xterm.js terminal connected to PTY via pty-output event
│       ├── TmuxControls.ts     # Session window list with add/kill, filters agent windows
│       ├── ChatPanel.ts        # Direct chat with agents via mailbox files
│       ├── MessageLog.ts       # Scrolling JSONL message log reader
│       ├── WarRoom.ts          # War room UI: setup, active discussion, history
│       ├── BeadsPanel.ts       # BEADS task browser with search/filter/expand
│       ├── SpiderlingsPanel.ts # Active spiderling list with kill button
│       ├── SpiderlingCard.ts   # Per-spiderling card UI
│       ├── ObjectivesKanban.ts # Swimlane Kanban for objectives + BEADS tasks
│       └── TasksPanel.ts       # Standalone task panel (secondary BEADS view)
│
└── src-tauri/                  # Rust Tauri backend
    ├── Cargo.toml              # Rust deps
    ├── tauri.conf.json         # Tauri config: window size, dev URL, bundle targets
    ├── build.rs                # Tauri build script (generated)
    └── src/
        ├── lib.rs              # Main entry point: state init, BEADS setup, command registry
        ├── state.rs            # AppState, AgentDef, SpiderlingDef structs
        ├── config.rs           # Default agent definitions, default_state()
        ├── tmux.rs             # All tmux subprocess commands
        ├── pty.rs              # PTY open/write/resize + pty-output event emission
        ├── agents.rs           # start_agent, stop_agent, list_agents, chat/messages
        ├── spawner.rs          # spawn_spiderling, kill_spiderling, list_spiderlings
        ├── poller.rs           # Background thread: message delivery, spawn/kill requests
        ├── beads.rs            # list_beads_tasks, update_beads_task_status (via `bd` CLI)
        ├── objectives.rs       # CRUD for objectives (JSON file at ~/.aperture/objectives.json)
        └── warroom.rs          # War room state machine, transcript, participant management
```

---

## 4. Backend (Rust)

### 4.1 Entry Point — `lib.rs`

The `run()` function is the entire application lifecycle:

```rust
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // 1. Build AppState (agents + spiderlings + paths)
    let app_state = Arc::new(Mutex::new(config::default_state()));
    let pty_state = Mutex::new(PtyState { writer: None, master: None });

    // 2. Bootstrap BEADS (dolt database backend)
    //    - Creates ~/.aperture/.beads/ if missing
    //    - Runs `dolt init` if not yet initialized
    //    - Starts `dolt sql-server --port 3307` if not running
    //    - Runs `bd init --quiet` against project_dir

    // 3. Start background message poller in a separate OS thread
    let poller_state = Arc::clone(&app_state);
    std::thread::spawn(move || {
        poller::run_message_poller(poller_state);
    });

    // 4. Register all Tauri commands and run
    tauri::Builder::default()
        .manage(app_state)
        .manage(pty_state)
        .invoke_handler(tauri::generate_handler![
            tmux::tmux_create_session, tmux::tmux_list_windows, /* ... */
            pty::start_pty, pty::write_pty, pty::resize_pty,
            agents::start_agent, agents::stop_agent, agents::list_agents,
            // ... (30+ commands total)
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
```

**Key design points:**
- `AppState` is wrapped in `Arc<Mutex<_>>` for shared mutable access across commands and the poller thread.
- `PtyState` uses a plain `Mutex<_>` (not `Arc`) because it's only accessed through Tauri's state management.
- BEADS bootstrapping happens synchronously at startup, with a 2-second sleep after starting dolt.
- The binary name is `aperture`; the library crate is `aperture_lib`.

### 4.2 State — `state.rs`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDef {
    pub name: String,           // "glados", "wheatley", "peppy", "izzy"
    pub model: String,          // "opus" or "sonnet"
    pub role: String,           // "orchestrator", "worker", "infra", "testing"
    pub prompt_file: String,    // absolute path to prompts/<name>.md
    pub tmux_window_id: Option<String>,  // None when stopped
    pub status: String,         // "stopped" | "running"
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpiderlingDef {
    pub name: String,
    pub task_id: String,
    pub tmux_window_id: Option<String>,
    pub worktree_path: String,        // ~/.aperture/worktrees/<name>
    pub worktree_branch: String,      // same as name
    #[serde(default)]
    pub source_repo: Option<String>,  // git repo the worktree was created from
    pub requested_by: String,
    pub status: String,               // "working"
    pub spawned_at: String,           // Unix ms timestamp as string
}

pub struct AppState {
    pub tmux_session: String,                    // "aperture"
    pub agents: HashMap<String, AgentDef>,
    pub spiderlings: HashMap<String, SpiderlingDef>,
    pub mcp_server_path: String,                 // ~/projects/aperture/mcp-server/dist/index.js
    pub db_path: String,                         // ~/.aperture/messages.db (legacy)
    pub project_dir: String,                     // ~/projects/aperture
}
```

**Note:** `AppState` itself does NOT derive `Serialize/Deserialize` — it's never sent over IPC directly. Only `AgentDef` and `SpiderlingDef` are serialized for frontend consumption.

### 4.3 Config — `config.rs`

Defines the four permanent agents and loads spiderlings from disk:

```rust
pub fn default_agents(project_dir: &str) -> HashMap<String, AgentDef> {
    // glados: opus, orchestrator
    // wheatley: sonnet, worker
    // peppy: opus, infra
    // izzy: opus, testing
    // Each reads its prompt from {project_dir}/prompts/{name}.md
}

pub fn default_state() -> AppState {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let project_dir = format!("{}/projects/aperture", home);
    AppState {
        tmux_session: "aperture".into(),
        agents: default_agents(&project_dir),
        spiderlings: load_spiderlings(&home),  // reads ~/.aperture/active-spiderlings.json
        mcp_server_path: format!("{}/mcp-server/dist/index.js", project_dir),
        db_path: format!("{}/.aperture/messages.db", home),
        project_dir,
    }
}
```

Spiderlings survive app restarts — they're persisted to `~/.aperture/active-spiderlings.json` whenever the list changes.

### 4.4 tmux — `tmux.rs`

All tmux operations are thin wrappers around `tmux` subprocess calls. A helper function injects the correct PATH for production builds:

```rust
fn cmd(program: &str) -> Command {
    let mut c = Command::new(program);
    let current_path = std::env::var("PATH").unwrap_or_default();
    c.env("PATH", format!("/opt/homebrew/bin:/usr/local/bin:{}", current_path));
    c.env("TERM", "xterm-256color");
    c.env("HOME", std::env::var("HOME").unwrap_or_else(|_| "/Users/<your-username>".into()));
    c.env("LANG", "en_US.UTF-8");
    c
}
```

> **Critical:** macOS `.app` bundles inherit almost no environment. All subprocess commands MUST explicitly set `PATH`, `TERM`, `HOME`, `LANG` or they will fail silently in production.

Commands exposed as Tauri commands:
- `tmux_create_session(session_name)` — idempotent, enables mouse + large scrollback
- `tmux_list_windows(session_name)` — returns `Vec<WindowInfo>` using `||` as delimiter in format string
- `tmux_create_window(session_name, window_name)` — returns new `window_id`
- `tmux_kill_window(window_id)`
- `tmux_select_window(window_id)` — switches the visible pane
- `tmux_rename_window(target, new_name)`
- `tmux_send_keys(target, keys)` — special-cases `C-` and `M-` prefixed keys (no Enter appended)

### 4.5 PTY — `pty.rs`

Uses `portable-pty` crate to open a native PTY and attach `tmux attach-session` to it:

```rust
pub struct PtyState {
    pub writer: Option<Box<dyn Write + Send>>,
    pub master: Option<Box<dyn MasterPty + Send>>,
}

#[tauri::command]
pub fn start_pty(session_name: String, app: AppHandle, pty_state: tauri::State<'_, Mutex<PtyState>>) -> Result<(), String> {
    // Opens PTY at 24x80
    // Spawns: tmux attach-session -t <session_name>
    //   with PATH, TERM, HOME, SHELL, LANG set
    // Spawns reader thread → emits "pty-output" Tauri events with chunks
    // Stores writer + master in PtyState
}

#[tauri::command]
pub fn write_pty(input: String, pty_state: tauri::State<'_, Mutex<PtyState>>) -> Result<(), String> {
    // Writes raw bytes to the PTY writer and flushes
}

#[tauri::command]
pub fn resize_pty(rows: u16, cols: u16, pty_state: tauri::State<'_, Mutex<PtyState>>) -> Result<(), String> {
    // Calls master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
}
```

The reader thread reads 4096-byte chunks and emits `pty-output` events to the Tauri frontend via `app.emit("pty-output", &data)`.

### 4.6 Agent Management — `agents.rs`

#### Starting an Agent

```rust
#[tauri::command]
pub fn start_agent(name: String, state: tauri::State<'_, Arc<Mutex<AppState>>>) -> Result<(), String> {
    // 1. Creates a new tmux window named after the agent
    // 2. Ensures ~/.aperture/mailbox/<name>/ exists
    // 3. Writes MCP config to /tmp/aperture-mcp-<name>.json:
    //    { mcpServers: { aperture-bus: { type: "stdio", command: "node",
    //      args: [mcp_server_path],
    //      env: { AGENT_NAME, AGENT_ROLE, AGENT_MODEL, APERTURE_MAILBOX, BEADS_DIR, BD_ACTOR } } } }
    // 4. Writes launcher script to /tmp/aperture-launch-<name>.sh:
    //    exec claude --dangerously-skip-permissions --model <model>
    //                --system-prompt "$PROMPT" --mcp-config <config> --name <name>
    // 5. chmod +x the launcher
    // 6. tmux_send_keys to run the launcher
    // 7. Spawns thread to auto-confirm workspace trust (3x Enter, 2s apart)
    // 8. Updates agent status to "running" with its tmux_window_id
}
```

#### Stopping an Agent

```rust
#[tauri::command]
pub fn stop_agent(name: String, ...) -> Result<(), String> {
    // 1. tmux_send_keys C-c
    // 2. Sleep 500ms
    // 3. tmux_send_keys /exit
    // 4. Sleep 500ms
    // 5. tmux_kill_window
    // 6. Sets status="stopped", tmux_window_id=None
}
```

#### Listing Agents (with live status detection)

```rust
#[tauri::command]
pub fn list_agents(state: ...) -> Result<Vec<AgentDef>, String> {
    // Cross-references with actual tmux windows to detect agents started outside the UI:
    // - If a window named after an agent has command "claude" → mark running
    // - If agent's window exists but command isn't claude → mark stopped
    // - If agent was running but window is gone → mark stopped
}
```

#### Chat / Message Log

- `send_chat(to_agent, message)` — writes a `.md` file to `~/.aperture/mailbox/<to_agent>/` and logs to `~/.aperture/chat-log.jsonl`
- `get_chat_messages()` — reads last 200 entries from `chat-log.jsonl`
- `get_recent_messages()` — reads last 100 entries from `message-log.jsonl`
- `clear_message_history()` / `clear_chat_history()` / `clear_conversation_history(agentA, agentB)`

### 4.7 Spiderlings — `spawner.rs`

Spiderlings are temporary worker agents with git worktree isolation:

```rust
pub fn spawn_spiderling(
    name: String,       // must match [a-z0-9][a-z0-9-]{0,30}
    task_id: String,    // BEADS task ID
    prompt: String,     // task description
    requested_by: String,
    project_path: Option<String>,  // repo to create worktree from (or aperture default)
    app_state: &mut AppState,
) -> Result<String, String> {
    // 1. Validates name (regex + no conflicts with permanent agents)
    // 2. Creates git worktree at ~/.aperture/worktrees/<name> from branch <name>
    // 3. Creates tmux window named <name>
    // 4. Writes MCP config to /tmp/aperture-mcp-<name>.json (same pattern as agents)
    // 5. Writes system prompt to ~/.aperture/launchers/<name>-prompt.txt
    //    (includes spiderling name, task_id, worktree path, BEADS communication instructions)
    // 6. Writes launcher to ~/.aperture/launchers/<name>.sh
    //    (cd to worktree, exec claude with sonnet model)
    // 7. Runs launcher via tmux_send_keys
    // 8. Auto-confirms workspace trust, then sends initial task message
    // 9. Persists SpiderlingDef to active-spiderlings.json
}
```

The system prompt template injected into every spiderling (from `spawner.rs`):

```
You are a spiderling named {name}, working for GLaDOS in the Aperture system.
Your task is tracked in BEADS issue {task_id}.
Work in this git worktree at {worktree_path} — do NOT switch branches or leave this directory.

## Communication — use BEADS, not send_message
...
```

Spiderlings persist across app restarts via `~/.aperture/active-spiderlings.json`.

### 4.8 BEADS Integration — `beads.rs`

Thin wrapper around the `bd` CLI:

```rust
fn bd_cmd() -> Command {
    // Sets PATH to include homebrew, BEADS_DIR to ~/.aperture/.beads
}

#[tauri::command]
pub fn list_beads_tasks() -> Result<serde_json::Value, String> {
    // runs: bd list --json --all
    // returns parsed JSON or empty array on failure
}

#[tauri::command]
pub fn update_beads_task_status(task_id: String, status: String) -> Result<(), String> {
    // runs: bd update <task_id> --status <status> --quiet
}
```

### 4.9 Objectives — `objectives.rs`

Pure JSON file storage at `~/.aperture/objectives.json`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Objective {
    pub id: String,           // "obj-{hex}"
    pub title: String,
    pub description: String,
    pub spec: Option<String>, // written by Wheatley agent
    pub status: String,       // "draft" | "speccing" | "ready" | "approved" | "in_progress" | "done"
    pub priority: u8,         // 0 (critical) – 4 (minimal)
    pub task_ids: Vec<String>,
    pub created_at: String,   // Unix ms as string
    pub updated_at: String,
}
```

Commands: `list_objectives`, `create_objective`, `update_objective`, `delete_objective`, `open_file` (calls macOS `open`).

### 4.10 Background Poller — `poller.rs`

A background OS thread that loops every 5 seconds, handling:

1. **Spawn requests** — reads JSON files from `~/.aperture/mailbox/_spawn/*.json`, calls `spawn_spiderling`, deletes the file
2. **Kill requests** — reads files from `~/.aperture/mailbox/_kill/`, calls `kill_spiderling`
3. **War room messages** — scans `~/.aperture/mailbox/warroom/`, if war room is active, calls `warroom::handle_warroom_message`
4. **Operator-bound messages** — scans `~/.aperture/mailbox/operator/`, logs to `chat-log.jsonl`
5. **BEADS message bus** — for each running agent/spiderling, queries BEADS for unread messages (`type=message AND status=open AND title="->{recipient}]"`), delivers via `tmux_send_keys` (cat file + rm), marks as read

Message delivery via tmux:
```rust
let formatted = format!("# Message from {}\n_{}_\n\n{}\n", sender, now, content);
let tmp_path = format!("/tmp/aperture-msg-{}.md", msg.id);
fs::write(&tmp_path, &formatted)?;
let cmd = format!("cat '{}' && rm '{}'", tmp_path, tmp_path);
tmux::tmux_send_keys(window_id, cmd);
```

---

## 5. Frontend (TypeScript)

### 5.1 Architecture Philosophy

- **No framework** — all components are plain TypeScript functions returning or mutating `HTMLElement`s
- **Factory pattern** — each component is a `create*()` function that receives a container element, mounts itself, and returns a controller object with a `refresh()` or `destroy()` method
- **Polling** — live data is fetched via `setInterval()` (3–5 second intervals), not WebSocket subscriptions
- **Hash-based diffing** — before rebuilding DOM, components stringify their data and compare to a `lastHash`; if unchanged, no DOM update occurs
- **Event bus via `window.dispatchEvent`** — cross-component communication uses custom DOM events (`agent-focused`, `objective-selected`)

### 5.2 Bootstrap — `main.ts`

The `init()` function (called immediately at bottom of file) orchestrates everything:

```typescript
async function init() {
  // 1. Grab all DOM elements by ID (see index.html section for IDs)
  // 2. Set up view switching (terminal / objectives) via navbar view buttons
  // 3. Set up right panel toggling by panel name
  // 4. Set up drag-to-resize for right panel (200–600px range)
  // 5. Create navbar and set connection status
  // 6. Create tmux session "aperture" (if fails, show error and return)
  // 7. Mount AgentList and TmuxControls in sidebar
  // 8. Mount ChatPanel, WarRoom, MessageLog, BeadsPanel, SpiderlingsPanel in right panel
  // 9. Mount Terminal (async — starts PTY) in main area
  // 10. Mount ObjectivesKanban in hidden objectives container
  // 11. Start 3s polling interval for AgentList
}
```

#### View Switching (Terminal / Objectives)

```typescript
function switchView(view: string) {
  terminalEl.classList.toggle("hidden", view !== "terminal");
  objectivesEl.classList.toggle("hidden", view !== "objectives");
  navbarViews.querySelectorAll(".navbar__view-btn").forEach((btn) => {
    btn.classList.toggle("navbar__view-btn--active", (btn as HTMLElement).dataset.view === view);
  });
  window.dispatchEvent(new Event("resize")); // causes xterm.js to refit
}
```

#### Panel Toggling

```typescript
function togglePanel(panel: string) {
  if (activePanel === panel) {
    // Clicking active panel closes it
    rightPanel.classList.add("hidden");
    resizeHandle.classList.add("hidden");
    activePanel = null;
  } else {
    rightPanel.classList.remove("hidden");
    resizeHandle.classList.remove("hidden");
    // Show only the requested panel, hide all others
    ["chat","warroom","messages","beads","spiders"].forEach(p => {
      document.getElementById(`panel-${p}`)!.classList.toggle("hidden", p !== panel);
    });
    activePanel = panel;
  }
  // Update navbar button active state
  window.dispatchEvent(new Event("resize")); // refit terminal
}
```

Auto-trigger: clicking a task card in the Objectives Kanban fires `objective-selected`, which calls `togglePanel("beads")` if the BEADS panel isn't already open.

### 5.3 Component Reference

#### `Navbar.ts`

```typescript
export function createNavbar(
  titleEl: HTMLElement,      // #navbar-title
  actionsEl: HTMLElement,    // #navbar-actions
  onTogglePanel: (panel: string) => void
) {
  // Renders: <span class="navbar__logo">APERTURE</span>
  //          <span class="navbar__dot navbar__dot--connected"></span>
  // Wires click on .navbar__btn elements → onTogglePanel(dataset.panel)
  return {
    setConnected(connected: boolean) { /* toggles --connected / --disconnected class */ }
  };
}
```

#### `AgentList.ts`

```typescript
export function createAgentList(container: HTMLElement) {
  // Polls commands.listAgents() on demand
  // Sorts agents: wheatley, glados, peppy, izzy (then others)
  // Uses hash comparison to avoid unnecessary DOM rebuilds
  // Renders AgentCards into a .agent-list wrapper
  return { refresh }; // called every 3s from main.ts
}
```

#### `AgentCard.ts`

Per-agent card with start/stop toggle:

```typescript
const AGENT_THEME: Record<string, { icon: string; color: string }> = {
  glados:   { icon: "🤖", color: "#9b59b6" },  // purple
  wheatley: { icon: "💡", color: "#3498db" },  // blue
  peppy:    { icon: "🚀", color: "#1abc9c" },  // teal
  izzy:     { icon: "🧪", color: "#e91e63" },  // pink
};

export function createAgentCard(agent: AgentDef, onUpdate: () => void): HTMLElement {
  // CSS class: agent-mini (stopped) or agent-mini agent-mini--running (running)
  // Uses CSS variable --agent-color for the left border accent
  // Card click → commands.tmuxSelectWindow(agent.tmux_window_id)
  //            → fires "agent-focused" custom event { name, color }
  // Toggle button click → commands.startAgent/stopAgent → onUpdate()
}
```

#### `Terminal.ts`

The terminal is the most complex component, handling renderer fallbacks:

```typescript
export async function createTerminal(container: HTMLElement, sessionName: string) {
  const term = new Terminal({
    cursorBlink: true, fontSize: 14, scrollback: 10000,
    fontFamily: "'JetBrains Mono', 'Fira Code', 'Cascadia Code', monospace",
    theme: { background: "#1a1a2e", foreground: "#e0e0e0",
             cursor: "#f39c12", selectionBackground: "#3a3a5e" },
  });

  // FitAddon: resizes xterm to fill container
  // WebGL addon: hardware-accelerated rendering with context-loss fallback
  // Triple-fit on mount: rAF + 100ms + 500ms (production WKWebView timing)

  await commands.startPty(sessionName);   // open PTY on Rust side

  // Listen for pty-output Tauri events → term.write(data)
  const unlisten = await onPtyOutput((data) => term.write(data));

  // Keyboard input: term.onData → commands.writePty(data)
  // Resize: ResizeObserver + window resize → fitAddon.fit() + commands.resizePty(rows, cols)

  return { terminal: term, destroy() { unlisten(); /* cleanup */ } };
}
```

**WebGL fallback pattern:** The WebGL addon can succeed at `loadAddon()` but fail asynchronously during rendering (common in Tauri production webviews). The code listens for `onContextLoss` and disposes the addon if it fires.

#### `ObjectivesKanban.ts`

The most complex frontend component. A swimlane Kanban where each row is an Objective and each column is a BEADS task status:

- **Columns:** `draft → speccing → ready → approved → in_progress → done`
- **Drag and drop:** Mouse-event based (not HTML5 drag API, for WKWebView compatibility). Creates a ghost element, tracks `mousedown/mousemove/mouseup` globally, drops onto `.swimlane__cell[data-status]`.
- **Optimistic updates:** Status changes update local state immediately before calling `updateBeadsTaskStatus`.
- **Spec workflow:** "Spec it" button sends a chat message to Wheatley with objective details; "Write Tasks" button tells Wheatley to create BEADS tasks from the spec.
- **Polling:** 3-second interval fetching both `listObjectives()` and `listBeadsTasks()`.

#### `BeadsPanel.ts`

BEADS task browser in the right panel:

- Polls `listBeadsTasks()` every 3 seconds
- Supports search, open/closed filter, and objective filter (set by `objective-selected` event)
- Expand/collapse per-task for description, close reason, and artifacts
- Artifacts are parsed from `notes` field: lines starting with `artifact:type:value`
- File artifacts show an "Open" button calling `commands.openFile(path)` (macOS `open` command)

#### `WarRoom.ts`

Multi-state UI (setup → active → concluded → history):

- **Setup:** Checkbox selection of participants with ordered badges (clicking adds to ordered list)
- **Active:** Polls every 2 seconds for `getWarroomState()` + `getWarroomTranscript()`. Shows participant badges with current speaker highlighted. Interject input, Skip/Conclude buttons.
- **Invite:** Live dropdown of agents/spiderlings not yet in the room
- **History:** Loads all past war rooms, click to view transcript, export as Markdown

---

## 6. IPC Layer

### 6.1 How Tauri IPC Works

Every call from TypeScript to Rust goes through `@tauri-apps/api/core`'s `invoke()` function:

```typescript
import { invoke } from "@tauri-apps/api/core";
invoke<ReturnType>("command_name", { param1: value1, param2: value2 })
```

- Command names use `snake_case` (matches Rust function names)
- Parameters are passed as a JSON object; Tauri deserializes them into Rust function parameters
- Return types are serialized Rust values (via `serde_json`)
- Errors are thrown as JavaScript rejections

### 6.2 `tauri-commands.ts` — The IPC Wrapper

All `invoke()` calls are centralized in one file:

```typescript
export const commands = {
  // Tmux
  tmuxCreateSession: (sessionName: string) =>
    invoke<string>("tmux_create_session", { sessionName }),
  tmuxListWindows: (sessionName: string) =>
    invoke<WindowInfo[]>("tmux_list_windows", { sessionName }),
  tmuxSelectWindow: (windowId: string) =>
    invoke<void>("tmux_select_window", { windowId }),
  // ... etc

  // PTY
  startPty: (sessionName: string) => invoke<void>("start_pty", { sessionName }),
  writePty: (input: string) => invoke<void>("write_pty", { input }),
  resizePty: (rows: number, cols: number) => invoke<void>("resize_pty", { rows, cols }),

  // Agents
  startAgent: (name: string) => invoke<void>("start_agent", { name }),
  stopAgent: (name: string) => invoke<void>("stop_agent", { name }),
  listAgents: () => invoke<AgentDef[]>("list_agents"),

  // BEADS
  listBeadsTasks: () => invoke<any>("list_beads_tasks"),
  updateBeadsTaskStatus: (taskId: string, status: string) =>
    invoke<void>("update_beads_task_status", { taskId, status }),

  // Objectives
  listObjectives: () => invoke<Objective[]>("list_objectives"),
  updateObjective: (id: string, fields: { title?: string; description?: string;
    spec?: string; status?: string; priority?: number; task_ids?: string[] }) =>
    invoke<Objective>("update_objective", { id, ...fields }),

  // War Room
  createWarroom: (topic: string, participants: string[]) =>
    invoke<void>("create_warroom", { topic, participants }),
  getWarroomState: () => invoke<WarRoomState | null>("get_warroom_state"),

  // Spiderlings
  listSpiderlings: () => invoke<SpiderlingDef[]>("list_spiderlings"),
  killSpiderling: (name: string) => invoke<void>("kill_spiderling_cmd", { name }),
};
```

### 6.3 Events (Rust → Frontend)

Tauri events (not commands) flow Rust → Frontend. Currently only one:

```typescript
// src/services/event-listener.ts
import { listen } from "@tauri-apps/api/event";

export function onPtyOutput(callback: (data: string) => void) {
  return listen<string>("pty-output", (event) => {
    callback(event.payload);
  });
}
```

The Rust side emits via `app.emit("pty-output", &data)` inside the PTY reader thread.

### 6.4 TypeScript Interfaces (`types.ts`)

```typescript
export interface AgentDef {
  name: string; model: string; role: string;
  prompt_file: string; tmux_window_id: string | null; status: string;
}
export interface WindowInfo {
  window_id: string; name: string; command: string;
}
export interface ChatMessage {
  from: string; to: string; content: string; timestamp: string;
}
export interface AgentMessage {
  id: number; from_agent: string; to_agent: string;
  content: string; timestamp: string; read: number;
}
export interface WarRoomState {
  id: string; topic: string; participants: string[];
  current_turn: number; current_agent: string; round: number;
  status: string; created_at: string; conclude_votes: string[];
}
export interface TranscriptEntry {
  role: string; content: string; timestamp: string; round?: number;
}
export interface SpiderlingDef {
  name: string; task_id: string; tmux_window_id: string | null;
  worktree_path: string; worktree_branch: string;
  requested_by: string; status: string; spawned_at: string;
}
export interface Objective {
  id: string; title: string; description: string;
  spec: string | null;
  status: "draft" | "speccing" | "ready" | "approved" | "in_progress" | "done";
  priority: number; task_ids: string[];
  created_at: string; updated_at: string;
}
```

---

## 7. UI Layout

### 7.1 HTML Structure (`index.html`)

```html
<div id="app">                        <!-- flex column, 100vh -->
  <nav id="navbar">                   <!-- 40px, flex row, space-between -->
    <div id="navbar-title">           <!-- logo + connection dot -->
    <div id="navbar-views">           <!-- Terminal / Objectives toggle -->
      <button class="navbar__view-btn navbar__view-btn--active" data-view="terminal">
      <button class="navbar__view-btn" data-view="objectives">
    <div id="navbar-actions">         <!-- panel toggle buttons -->
      <button class="navbar__btn" data-panel="chat">Chat
      <button class="navbar__btn" data-panel="warroom">War Room
      <button class="navbar__btn" data-panel="messages">Messages
      <button class="navbar__btn" data-panel="beads">BEADS
      <button class="navbar__btn" data-panel="spiders">Spiders

  <div id="content">                  <!-- flex row, flex:1 -->
    <aside id="sidebar">              <!-- 220px fixed width -->
      <div id="sidebar-agents">       <!-- AgentList mounts here -->
      <div id="sidebar-sessions">     <!-- TmuxControls mounts here -->
      <div id="sidebar-active-agent"> <!-- "viewing X" badge, auto-pushed to bottom -->

    <main id="terminal-container">    <!-- flex:1, Terminal mounts here -->
    <div id="objectives-container" class="hidden">  <!-- ObjectivesKanban -->
    <div id="resize-handle" class="hidden">         <!-- 4px drag handle -->

    <aside id="right-panel" class="hidden">  <!-- 300px default, 0px when hidden -->
      <div id="panel-chat" class="panel-view hidden">
      <div id="panel-warroom" class="panel-view hidden">
      <div id="panel-messages" class="panel-view hidden">
      <div id="panel-beads" class="panel-view hidden">
      <div id="panel-spiders" class="panel-view hidden">
```

### 7.2 CSS Design System (`style.css`)

#### CSS Variables

```css
:root {
  --bg-primary: #0f0f1a;      /* main background */
  --bg-navbar: #0c0c18;       /* navbar — slightly darker */
  --bg-sidebar: #141428;      /* left sidebar */
  --bg-panel: #141428;        /* right panel */
  --bg-card: #1a1a2e;         /* cards / inputs */
  --bg-hover: #222240;        /* hover state */
  --text-primary: #e0e0e0;
  --text-secondary: #8888aa;
  --accent-orange: #f39c12;   /* primary accent, active states */
  --accent-green: #2ecc71;    /* connected, success */
  --accent-red: #e74c3c;      /* disconnected, danger, stop */
  --accent-blue: #3498db;     /* wheatley, info */
  --accent-purple: #9b59b6;   /* glados */
  --accent-teal: #1abc9c;     /* peppy */
  --accent-pink: #e91e63;     /* izzy */
  --border: #2a2a4a;
  --radius: 6px;
  --navbar-height: 40px;
}
```

#### Key Layout Rules

```css
/* App fills viewport */
#app { display: flex; flex-direction: column; height: 100vh; }

/* Content row below navbar */
#content { display: flex; flex: 1; overflow: hidden; }

/* Sidebar: fixed 220px */
#sidebar { width: 220px; min-width: 220px; background: var(--bg-sidebar); }

/* Terminal: grows to fill remaining space */
#terminal-container { flex: 1; padding: 4px; overflow: hidden; }

/* Right panel: slides in/out via width transition */
#right-panel { width: 300px; transition: width 0.15s, opacity 0.15s; }
#right-panel.hidden { width: 0; min-width: 0; opacity: 0; overflow: hidden; }

/* Only one panel-view is visible at a time */
.panel-view.hidden { display: none !important; }
```

#### Agent Card CSS

```css
.agent-mini {
  --agent-color: var(--accent-orange);  /* overridden per-card inline */
  display: flex; align-items: center; gap: 8px; padding: 8px 10px;
  border: 1px solid var(--border);
  border-left: 3px solid transparent;  /* accent when running */
}
.agent-mini--running {
  border-left-color: var(--agent-color);
  cursor: pointer;
}
.agent-mini--running .agent-mini__name { color: var(--agent-color); }
```

#### Key Class Naming Conventions

| Pattern | Example | Usage |
|---|---|---|
| `component__element` | `.agent-mini__name` | BEM-style element |
| `component--modifier` | `.agent-mini--running` | BEM-style modifier |
| `navbar__btn--active` | `.navbar__btn--active` | Active/selected state |
| `panel-view` | `.panel-view` | Right panel slot |
| `section-title` | `.section-title` | Shared header style |
| `btn` | `.btn` | Generic button |
| `btn--small`, `btn--tiny`, `btn--danger` | — | Button variants |

---

## 8. Build & Dev

### 8.1 Development

```bash
# Install frontend deps
pnpm install

# Start Tauri dev (runs Vite + Rust simultaneously)
pnpm tauri dev
```

This runs `pnpm dev` (Vite on port 1420) and the Rust backend in parallel, with hot-reload for the frontend.

### 8.2 Production Build

```bash
pnpm tauri build
```

Runs `tsc && vite build` → outputs to `dist/`, then Tauri bundles the `.app`.

### 8.3 Vite Config (`vite.config.ts`)

```typescript
export default defineConfig({
  clearScreen: false,          // Don't clear Tauri's terminal output
  server: {
    host: host || false,       // TAURI_DEV_HOST env var for mobile/remote
    port: 1420,
    strictPort: true,          // Fail if port is taken
    hmr: host ? { protocol: "ws", host, port: 1421 } : undefined,
    watch: { ignored: ["**/src-tauri/**"] },  // Don't watch Rust files
  },
});
```

### 8.4 Tauri Config (`tauri.conf.json`)

```json
{
  "productName": "Aperture",
  "version": "0.1.0",
  "identifier": "com.aperture.app",
  "build": {
    "frontendDist": "../dist",
    "devUrl": "http://localhost:1420",
    "beforeDevCommand": "pnpm dev",
    "beforeBuildCommand": "pnpm build"
  },
  "app": {
    "windows": [{ "title": "Aperture", "width": 1200, "height": 800, "resizable": true }],
    "security": { "csp": null }   // CSP disabled — needed for inline styles, PTY output
  },
  "bundle": {
    "active": true,
    "targets": "all",
    "icon": ["icons/32x32.png", "icons/128x128.png", /* ... */]
  }
}
```

### 8.5 Cargo.toml

```toml
[package]
name = "aperture"
version = "0.1.0"
edition = "2021"

[lib]
name = "aperture_lib"
crate-type = ["lib", "cdylib", "staticlib"]  # staticlib for iOS if needed

[build-dependencies]
tauri-build = { version = "2", features = [] }

[dependencies]
tauri = { version = "2", features = [] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
portable-pty = "0.8"
regex = "1"
```

---

## 9. Key Dependencies

| Dependency | Purpose | Notes |
|---|---|---|
| `tauri 2.x` | Desktop shell, IPC bridge | Wraps WKWebView on macOS |
| `portable-pty 0.8` | Native PTY creation | Provides `openpty`, `MasterPty`, `CommandBuilder` |
| `serde` + `serde_json` | Serialization | All IPC types must derive `Serialize`/`Deserialize` |
| `regex 1.x` | Spiderling name validation | `^[a-z0-9][a-z0-9-]{0,30}$` |
| `@xterm/xterm ^6.0.0` | Terminal emulator in browser | Renders PTY output with ANSI support |
| `@xterm/addon-fit ^0.11.0` | Auto-resize xterm to container | Called on ResizeObserver + window resize |
| `@xterm/addon-webgl ^0.19.0` | WebGL renderer for xterm | Falls back to canvas on context loss |
| `@tauri-apps/api ^2.10.1` | TypeScript bindings for Tauri | `invoke()` for commands, `listen()` for events |
| `vite ^8.0.0` | Frontend bundler | Serves on port 1420 |
| `typescript ^5.9.3` | Type checking | Strict mode recommended |

**External system dependencies** (not in package.json/Cargo.toml but required at runtime):
- `tmux` at `/opt/homebrew/bin/tmux` — all terminal multiplexing
- `claude` CLI on PATH — AI agent runtime
- `node` on PATH — MCP server runtime
- `dolt` at `/opt/homebrew/bin/dolt` — BEADS database server
- `bd` CLI at `~/.local/bin/bd` or on PATH — BEADS task operations
- `git` — worktree creation for spiderlings

---

## 10. Gotchas & Design Decisions

### 10.1 Production macOS Bundle PATH Problem

**Problem:** `.app` bundles on macOS inherit essentially no shell environment. Commands like `tmux`, `claude`, `node`, `dolt` are all in `/opt/homebrew/bin` which isn't in the bundle's PATH.

**Solution:** Every `Command::new(...)` call manually sets PATH:
```rust
c.env("PATH", format!("/opt/homebrew/bin:/usr/local/bin:{}", current_path));
```
This pattern is replicated in `tmux.rs`, `pty.rs`, `agents.rs`, `spawner.rs`, `beads.rs`, and `poller.rs`. If you add a new subprocess call and forget this, it will work in dev but fail silently in production.

### 10.2 WebGL Context Loss in Tauri

WKWebView (Safari's rendering engine used by Tauri on macOS) can silently lose WebGL context in production builds. The `WebglAddon` may initialize successfully but then fail during rendering.

**Solution:** Register an `onContextLoss` handler and dispose the addon:
```typescript
const webglAddon = new WebglAddon();
webglAddon.onContextLoss(() => {
  console.warn("WebGL context lost, falling back to canvas renderer");
  webglAddon.dispose();
});
term.loadAddon(webglAddon);
```
Without this, the terminal goes blank in production builds.

### 10.3 Triple-Fit for xterm.js

In production Tauri builds, the webview layout doesn't settle immediately. A single `fitAddon.fit()` in `requestAnimationFrame` is not enough.

**Solution:**
```typescript
requestAnimationFrame(() => {
  fitAddon.fit();
  setTimeout(() => fitAddon.fit(), 100);
  setTimeout(() => fitAddon.fit(), 500);
});
```

### 10.4 HTML5 Drag-and-Drop Doesn't Work in WKWebView

The native browser drag-and-drop API (`dragstart`, `dragover`, `drop`) is unreliable in Tauri's WKWebView.

**Solution:** Mouse event-based DnD in `ObjectivesKanban.ts`:
- `mousedown` on `.swimlane__card`: create a ghost element cloned from the card
- `document.mousemove`: move ghost, highlight `.swimlane__cell--dragover` under cursor
- `document.mouseup`: find cell at drop point via `document.elementFromPoint()`, update status

### 10.5 Agent Status Detection

Agents started from outside the UI (e.g., manually in tmux) would show as "stopped" without cross-referencing.

**Solution:** `list_agents` in `agents.rs` queries `tmux_list_windows` every call and syncs agent status against windows where `command == "claude"`. This means `list_agents` is not purely a state read — it has side effects.

### 10.6 State Architecture: Two Separate Mutexes

`AppState` (agents, spiderlings) and `PtyState` (PTY writer) are managed separately:
- `AppState`: `Arc<Mutex<AppState>>` — needs `Arc` for sharing with the poller thread
- `PtyState`: `Mutex<PtyState>` (no `Arc`) — only accessed through Tauri's state system

### 10.7 Spiderling Persistence

Spiderlings survive app restarts via `~/.aperture/active-spiderlings.json`. When the app starts, `config.rs` loads this file. When spawned or killed, `spawner::write_active_spiderlings` writes it immediately (not lazily). The worktrees themselves also persist on disk.

### 10.8 No CSP

`"csp": null` in `tauri.conf.json` disables Content Security Policy entirely. This is required because:
- xterm.js uses inline styles extensively
- The app embeds dynamic content (PTY output, agent messages) directly as HTML

For a security-sensitive deployment, you'd need to audit xterm.js's CSP requirements.

### 10.9 BEADS Startup Race Condition

At startup, `lib.rs` starts the dolt sql-server and then sleeps 2 seconds before running `bd init`. This is a fixed sleep, not a health check. If the machine is slow or dolt is starting fresh for the first time, 2 seconds may not be enough.

### 10.10 Tmux Window ID Format

Tmux window IDs look like `@1`, `@2`, etc. (the `@` prefix). When checking if agent windows are gone, the code compares `window.name` (the window title) to `agent.name`. This means:
- An agent window is identified by name, not ID
- If two windows have the same name, the first match wins

### 10.11 Component Pattern: Factory Functions

Every component follows this exact pattern:

```typescript
export function createMyComponent(container: HTMLElement): { refresh: () => void; destroy: () => void } {
  // 1. Build and mount DOM into container
  container.innerHTML = `...`;

  // 2. Set up event listeners (inside the function, not at module level)

  // 3. Define polling function
  async function poll() { /* fetch data, compare hash, update DOM */ }

  // 4. Start polling
  poll(); // immediate first call
  const interval = setInterval(poll, N_SECONDS * 1000);

  // 5. Return controller
  return {
    refresh: poll,
    destroy() { clearInterval(interval); }
  };
}
```

The `destroy()` method is defined but not currently called anywhere in `main.ts` — cleanup is handled implicitly by the app lifecycle.

---

## 11. Data Flow Summary

```
Human clicks "Start Agent (glados)"
  → JS: commands.startAgent("glados")
  → invoke("start_agent", { name: "glados" })
  → Rust: agents::start_agent()
    → tmux::tmux_create_window("aperture", "glados") → "@3"
    → fs::write("/tmp/aperture-mcp-glados.json", ...)
    → fs::write("/tmp/aperture-launch-glados.sh", ...)
    → tmux::tmux_send_keys("@3", "/tmp/aperture-launch-glados.sh")
    → app_state.agents["glados"].status = "running"
  → JS resolves void
  → AgentList.refresh() → commands.listAgents()
    → Rust: agents::list_agents()
      → cross-references with tmux windows
      → returns [AgentDef { name: "glados", status: "running", ... }]
  → AgentCard re-renders with running state (left border lit up)

Agent writes message to BEADS:
  → bd message send --to wheatley --content "..."
  → BEADS message created with status=open, title="[glados->wheatley] ..."

Background poller (every 5s):
  → query_unread_messages("wheatley")
  → finds the message
  → formats as "# Message from glados\n..."
  → fs::write("/tmp/aperture-msg-<id>.md", formatted)
  → tmux::tmux_send_keys(wheatley_window, "cat '/tmp/...' && rm '/tmp/...'")
  → mark_message_read(message_id)
  → The cat command is injected into wheatley's terminal, appearing as Claude input
```

---

*Document written by spider-doc-tauri for the Aperture system. All code snippets are from the actual source files as of the time of writing.*
