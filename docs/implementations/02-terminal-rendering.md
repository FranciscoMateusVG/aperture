# 02 — Terminal Rendering System

> **Purpose**: This document is a complete implementation guide for rebuilding Aperture's terminal rendering system from scratch. It covers every layer of the stack with real source code.

---

## 1. Overview

Aperture's terminal is not a simple embedded shell. It is a **multi-layer pipeline** that bridges:

```
Claude Code CLI  →  tmux window  →  PTY (Rust)  →  Tauri IPC  →  xterm.js  →  screen
```

The key architectural insight is that there is **one PTY** attached to a **tmux session**. That tmux session holds **multiple windows** — one per agent. Switching which agent you're watching means telling tmux to select a different window, which then flows through the same PTY/xterm.js rendering path.

Key components:

| Layer | Technology | File |
|---|---|---|
| Terminal emulator | xterm.js v6 | `src/components/Terminal.ts` |
| Event bus (PTY output) | Tauri events | `src/services/event-listener.ts` |
| PTY management | `portable-pty` 0.8 (Rust) | `src-tauri/src/pty.rs` |
| Session/window management | tmux CLI | `src-tauri/src/tmux.rs` |
| Agent lifecycle | Tauri commands | `src-tauri/src/agents.rs` |
| Message injection | Background poller | `src-tauri/src/poller.rs` |
| Command bridge | Tauri invoke | `src/services/tauri-commands.ts` |

---

## 2. The Pipeline

### Full Data Flow Diagram

```
┌─────────────────────────────────────────────────────────────┐
│                        FRONTEND (WebView)                    │
│                                                              │
│  xterm.js Terminal                                           │
│  ┌──────────────────────────────────────┐                   │
│  │  term.write(data)  ◄── pty-output    │                   │
│  │  term.onData(data) ──► write_pty     │                   │
│  └──────────────────────────────────────┘                   │
│           ▲ Tauri event                ▼ Tauri invoke       │
└───────────┼────────────────────────────┼────────────────────┘
            │                            │
┌───────────┼────────────────────────────┼────────────────────┐
│           │      RUST BACKEND          │                     │
│  ┌────────┴──────────┐    ┌────────────▼──────────┐        │
│  │  PtyState.master  │    │  PtyState.writer      │        │
│  │  (MasterPty)      │    │  (Box<dyn Write>)     │        │
│  │  reader thread    │    │  write_pty command    │        │
│  │  → emit event     │    │  → writer.write_all() │        │
│  └────────┬──────────┘    └────────────┬──────────┘        │
│           │   portable-pty PTY pair     │                    │
└───────────┼─────────────────────────────┼───────────────────┘
            │                             │
┌───────────┼─────────────────────────────┼───────────────────┐
│           │         SYSTEM              │                     │
│  ┌────────┴──────────────────────────────────────────────┐  │
│  │           tmux attach-session -t aperture              │  │
│  │   (PTY child process — reads/writes the tmux client)  │  │
│  └────────────────────────┬──────────────────────────────┘  │
│                            │                                  │
│  ┌─────────────────────────▼──────────────────────────────┐ │
│  │  tmux session "aperture"                                │ │
│  │  ├── window @1  "glados"    ← Claude Code CLI          │ │
│  │  ├── window @2  "wheatley"  ← Claude Code CLI          │ │
│  │  ├── window @3  "peppy"     ← Claude Code CLI          │ │
│  │  └── window @4  "izzy"      ← Claude Code CLI          │ │
│  └─────────────────────────────────────────────────────────┘ │
└──────────────────────────────────────────────────────────────┘
```

### Data Flow Narratives

**Output (agent → user's screen)**:
1. Claude Code CLI writes bytes to its stdout/stderr inside its tmux window
2. tmux multiplexes that output within the session
3. The `tmux attach-session` process (running inside the PTY) receives the bytes on its stdout
4. The PTY reader thread in Rust reads those bytes (`reader.read(&mut buf)`)
5. The thread emits a `pty-output` Tauri event with the data as a UTF-8 string
6. The frontend listens via `listen("pty-output", ...)` and calls `term.write(data)`
7. xterm.js renders the escape sequences and text to the canvas

**Input (user → agent)**:
1. User presses a key in the xterm.js terminal
2. `term.onData((data) => commands.writePty(data))` fires
3. Tauri invokes the `write_pty` Rust command
4. Rust writes the bytes to `PtyState.writer` (the PTY master's write side)
5. The `tmux attach-session` client receives the input and forwards it to the active tmux window
6. The Claude Code CLI running in that window receives the keypress

---

## 3. xterm.js Setup

### Initialization (`src/components/Terminal.ts`)

The `createTerminal` function is the single entry point for creating a terminal instance. It is called once at app startup with the `#terminal-container` DOM element and the tmux session name (`"aperture"`).

```typescript
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";
import { commands } from "../services/tauri-commands";
import { onPtyOutput } from "../services/event-listener";

export async function createTerminal(container: HTMLElement, sessionName: string) {
  const term = new Terminal({
    cursorBlink: true,
    fontSize: 14,
    scrollback: 10000,
    fontFamily: "'JetBrains Mono', 'Fira Code', 'Cascadia Code', monospace",
    theme: {
      background: "#1a1a2e",
      foreground: "#e0e0e0",
      cursor: "#f39c12",
      selectionBackground: "#3a3a5e",
    },
  });
  // ...
}
```

**Notable configuration choices**:
- `scrollback: 10000` — large buffer for long-running agent sessions
- `fontFamily` — prefers JetBrains Mono with multiple fallbacks for monospace rendering
- `theme.background: "#1a1a2e"` — deep navy matches Aperture's dark UI palette
- `cursorBlink: true` — visual feedback that the terminal is active

### Addons

Two addons are loaded:

**FitAddon** — Resizes the xterm.js canvas to fill the container element. Must be called after layout is established.

**WebglAddon** — Enables WebGL-accelerated rendering for performance. Falls back to the default canvas renderer if WebGL is unavailable or the context is lost.

```typescript
const fitAddon = new FitAddon();
term.loadAddon(fitAddon);

term.open(container);  // Attach to DOM first, then load WebGL

// Try WebGL renderer with robust fallback.
// The WebGL addon can succeed at loadAddon() but fail asynchronously during
// rendering (common in Tauri production webviews). Listen for context loss
// and dispose the addon if it fires.
try {
  const webglAddon = new WebglAddon();
  webglAddon.onContextLoss(() => {
    console.warn("WebGL context lost, falling back to canvas renderer");
    webglAddon.dispose();
  });
  term.loadAddon(webglAddon);
} catch {
  console.warn("WebGL addon failed to load, using canvas renderer");
}
```

**Important ordering**: `term.open(container)` must be called **before** loading WebglAddon. The WebGL addon requires a real DOM element to create its canvas context.

**WebGL context loss handling**: In Tauri production `.app` bundles (macOS app sandbox), the WebGL context can be lost asynchronously after `loadAddon()` succeeds. The `onContextLoss` handler disposes the addon, causing xterm.js to silently fall back to the canvas renderer. Without this handler, rendering would freeze.

### Initial Fit Strategy

After opening the terminal, layout dimensions may not be settled yet (especially in production builds). A "triple-fit" strategy handles this:

```typescript
// Delay initial fit to ensure container has layout dimensions.
// Triple-fit strategy: immediate rAF, short delay, and longer delay
// to handle production build timing where layout may settle late.
requestAnimationFrame(() => {
  fitAddon.fit();
  setTimeout(() => fitAddon.fit(), 100);
  setTimeout(() => fitAddon.fit(), 500);
});
```

Why three fits?
- `requestAnimationFrame` — after the first paint, layout is usually ready
- `100ms` — handles cases where CSS flexbox hasn't fully resolved
- `500ms` — catches late-settling layouts in production Tauri builds

### PTY Connection and Event Wiring

After visual setup, the terminal connects to the PTY and wires up events:

```typescript
// Start PTY and connect
await commands.startPty(sessionName);

// Listen for PTY output
const unlisten = await onPtyOutput((data) => {
  term.write(data);
});

// Send keyboard input to PTY
term.onData((data) => {
  commands.writePty(data);
});
```

`commands.startPty(sessionName)` is async — it blocks until the Rust side has attached to tmux. `onPtyOutput` returns an `unlisten` function used during cleanup.

### Cleanup

The function returns a `destroy()` method that properly tears down all listeners:

```typescript
return {
  terminal: term,
  destroy() {
    unlisten();                                      // Stop Tauri event listener
    resizeObserver.disconnect();                     // Stop watching container size
    window.removeEventListener("resize", onWindowResize);
    term.dispose();                                  // Free xterm.js resources
  }
};
```

---

## 4. PTY Integration

### The `PtyState` Struct (`src-tauri/src/pty.rs`)

The PTY state is stored as a Tauri-managed `Mutex<PtyState>`. The struct holds the write half (for sending input) and the master PTY (for resizing):

```rust
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use std::io::{Read, Write};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter};

pub struct PtyState {
    pub writer: Option<Box<dyn Write + Send>>,
    pub master: Option<Box<dyn MasterPty + Send>>,
}
```

The `writer` and `master` are separate because `portable-pty`'s API splits them: `take_writer()` consumes ownership of the write channel, while the master PTY handle is kept for resize operations.

### `start_pty` Command

This is the core PTY setup. It creates a PTY pair, spawns `tmux attach-session` as the child process inside it, then starts a background thread to relay PTY output as Tauri events:

```rust
#[tauri::command]
pub fn start_pty(
    session_name: String,
    app: AppHandle,
    pty_state: tauri::State<'_, Mutex<PtyState>>,
) -> Result<(), String> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize { rows: 24, cols: 80, pixel_width: 0, pixel_height: 0 })
        .map_err(|e| e.to_string())?;

    let mut cmd = CommandBuilder::new("/opt/homebrew/bin/tmux");
    cmd.arg("attach-session");
    cmd.arg("-t");
    cmd.arg(&session_name);

    // Production Tauri .app bundles inherit almost no environment.
    // We must explicitly set the essentials for tmux and the shell to work.
    let current_path = std::env::var("PATH").unwrap_or_default();
    cmd.env("PATH", format!("/opt/homebrew/bin:/usr/local/bin:{}", current_path));
    cmd.env("TERM", "xterm-256color");
    cmd.env("HOME", std::env::var("HOME").unwrap_or_else(|_| "/Users/<your-username>".into()));
    cmd.env("SHELL", std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into()));
    cmd.env("LANG", "en_US.UTF-8");

    let mut child = pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;
    drop(pair.slave);  // Must drop slave after spawning child

    let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
    let writer = pair.master.take_writer().map_err(|e| e.to_string())?;

    {
        let mut state = pty_state.lock().map_err(|e| e.to_string())?;
        state.writer = Some(writer);
        state.master = Some(pair.master);
    }

    // Output relay thread: reads PTY master output, emits Tauri events
    let app_clone = app.clone();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let data = String::from_utf8_lossy(&buf[..n]).to_string();
                    let _ = app_clone.emit("pty-output", &data);
                }
                Err(_) => break,
            }
        }
    });

    // Wait thread: prevents zombie process
    std::thread::spawn(move || {
        let _ = child.wait();
    });

    Ok(())
}
```

**Key details**:
- **Buffer size**: 4096 bytes per read — balanced between latency and throughput for terminal output
- **`drop(pair.slave)`**: The slave must be dropped after spawning. If the slave fd stays open in the parent process, the child's EOF will never fire
- **Environment injection**: Tauri `.app` bundles on macOS have a stripped environment. `PATH`, `TERM`, `HOME`, `SHELL`, and `LANG` must all be set explicitly or tmux/shells won't find their binaries
- **`TERM=xterm-256color`**: Required for correct color rendering in Claude Code's TUI
- **`app_clone.emit("pty-output", &data)`**: The event name `"pty-output"` is the contract between Rust and the frontend. All frontend terminals subscribe to this single event.

### `write_pty` Command

Writes raw bytes from xterm.js keyboard input into the PTY master:

```rust
#[tauri::command]
pub fn write_pty(input: String, pty_state: tauri::State<'_, Mutex<PtyState>>) -> Result<(), String> {
    let mut state = pty_state.lock().map_err(|e| e.to_string())?;
    if let Some(ref mut writer) = state.writer {
        writer.write_all(input.as_bytes()).map_err(|e| e.to_string())?;
        writer.flush().map_err(|e| e.to_string())?;
        Ok(())
    } else {
        Err("PTY not started".into())
    }
}
```

Note the explicit `flush()` — some buffered writers won't send data until flushed, causing keystrokes to appear delayed.

### `resize_pty` Command

Propagates xterm.js dimensions to the underlying PTY:

```rust
#[tauri::command]
pub fn resize_pty(
    rows: u16,
    cols: u16,
    pty_state: tauri::State<'_, Mutex<PtyState>>,
) -> Result<(), String> {
    let state = pty_state.lock().map_err(|e| e.to_string())?;
    if let Some(ref master) = state.master {
        master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
            .map_err(|e| e.to_string())?;
        Ok(())
    } else {
        Err("PTY not started".into())
    }
}
```

This sends a `SIGWINCH` signal to the child process (tmux), which then propagates it to the active window's running program (Claude Code CLI).

### Tauri State Registration (`src-tauri/src/lib.rs`)

Both `AppState` and `PtyState` are registered as Tauri managed state:

```rust
let app_state = Arc::new(Mutex::new(config::default_state()));
let pty_state = Mutex::new(PtyState {
    writer: None,
    master: None,
});

tauri::Builder::default()
    .manage(app_state)
    .manage(pty_state)
    .invoke_handler(tauri::generate_handler![
        pty::start_pty,
        pty::write_pty,
        pty::resize_pty,
        // ... other commands
    ])
```

Note that `PtyState` uses a plain `Mutex` (not `Arc<Mutex>`) because Tauri wraps it in its own `Arc` internally. `AppState` uses `Arc<Mutex>` because it is also shared with the background poller thread.

### Frontend Event Listener (`src/services/event-listener.ts`)

The thin wrapper around Tauri's `listen` API:

```typescript
import { listen } from "@tauri-apps/api/event";

export function onPtyOutput(callback: (data: string) => void) {
  return listen<string>("pty-output", (event) => {
    callback(event.payload);
  });
}
```

The return value of `listen(...)` is a Promise that resolves to an `UnlistenFn`. The `Terminal.ts` `destroy()` method calls this to unsubscribe.

---

## 5. tmux Integration

### Session Initialization (`src-tauri/src/tmux.rs`)

All tmux commands use a `cmd()` helper that injects the required environment:

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

### `tmux_create_session`

Creates the `"aperture"` session at app startup (idempotent — returns `"already exists"` if it's running):

```rust
#[tauri::command]
pub fn tmux_create_session(session_name: String) -> Result<String, String> {
    let check = cmd("tmux")
        .args(["has-session", "-t", &session_name])
        .output()
        .map_err(|e| e.to_string())?;

    if check.status.success() {
        return Ok("already exists".into());
    }

    let output = cmd("tmux")
        .args(["new-session", "-d", "-s", &session_name])
        .output()
        .map_err(|e| e.to_string())?;

    if output.status.success() {
        // Enable mouse scrolling and increase scrollback history
        let _ = cmd("tmux")
            .args(["set-option", "-t", &session_name, "-g", "mouse", "on"])
            .output();
        let _ = cmd("tmux")
            .args(["set-option", "-t", &session_name, "-g", "history-limit", "50000"])
            .output();
        Ok("created".into())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).to_string())
    }
}
```

The session is created with `-d` (detached) — it runs in the background, not attached to any terminal. The `history-limit 50000` allows scrollback within tmux itself.

### `WindowInfo` Struct

Windows are described by this serializable struct:

```rust
#[derive(Debug, Serialize, Clone)]
pub struct WindowInfo {
    pub window_id: String,   // e.g. "@1", "@2"
    pub name: String,        // e.g. "glados", "wheatley"
    pub command: String,     // e.g. "claude", "bash"
}
```

### `tmux_list_windows`

Lists all windows with their IDs, names, and running commands:

```rust
#[tauri::command]
pub fn tmux_list_windows(session_name: String) -> Result<Vec<WindowInfo>, String> {
    let output = cmd("tmux")
        .args([
            "list-windows", "-t", &session_name,
            "-F", "#{window_id}||#{window_name}||#{pane_current_command}",
        ])
        .output()
        .map_err(|e| e.to_string())?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let windows = stdout
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let parts: Vec<&str> = line.splitn(3, "||").collect();
            WindowInfo {
                window_id: parts.first().unwrap_or(&"").to_string(),
                name:      parts.get(1).unwrap_or(&"").to_string(),
                command:   parts.get(2).unwrap_or(&"").to_string(),
            }
        })
        .collect();

    Ok(windows)
}
```

The `||` separator is used because it's unlikely to appear in window names or commands. The `#{pane_current_command}` field is used by `list_agents` to detect whether `claude` is still running in a window.

### `tmux_create_window`

Creates a new named window and returns its window ID:

```rust
#[tauri::command]
pub fn tmux_create_window(session_name: String, window_name: String) -> Result<String, String> {
    let output = cmd("tmux")
        .args([
            "new-window", "-t", &session_name,
            "-n", &window_name,
            "-P", "-F", "#{window_id}",   // Print the new window's ID
        ])
        .output()
        .map_err(|e| e.to_string())?;

    let window_id = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(window_id)
}
```

The `-P -F "#{window_id}"` flags print the new window's ID to stdout, which is returned to the caller. This ID (e.g. `@3`) is stored in `AgentDef.tmux_window_id`.

### `tmux_select_window`

Switches the active window in the tmux session (used when clicking an agent card):

```rust
#[tauri::command]
pub fn tmux_select_window(window_id: String) -> Result<(), String> {
    let output = cmd("tmux")
        .args(["select-window", "-t", &window_id])
        .output()
        .map_err(|e| e.to_string())?;
    // ...
}
```

When this runs, the PTY (which is running `tmux attach-session`) sees the active window switch, and immediately starts streaming the output of the newly selected window through the existing PTY → Tauri event → xterm.js pipeline.

### `tmux_send_keys`

Sends text or key sequences to a specific tmux window:

```rust
#[tauri::command]
pub fn tmux_send_keys(target: String, keys: String) -> Result<(), String> {
    // Special keys like C-c should not be quoted or followed by Enter
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
    // ...
}
```

The `--` separator prevents keys starting with `-` from being parsed as tmux flags. Regular text is followed by `"Enter"` to submit the command. Special keys (`C-c`, `M-x`, etc.) are sent without Enter — they are control sequences, not commands.

This function is the backbone of:
- Starting agents (sending the launcher script path)
- Stopping agents (sending `C-c` then `/exit`)
- Injecting messages (sending `cat /tmp/file.md && rm /tmp/file.md`)

---

## 6. Resize Handling

Resize events must propagate from the DOM layout through to the PTY process. If they don't, programs like Claude Code CLI will think the terminal is 80×24 and wrap lines incorrectly.

The resize stack has two triggers:

```typescript
// Trigger 1: Container element resizes (e.g. sidebar open/close)
const resizeObserver = new ResizeObserver(() => {
  fitAddon.fit();
  commands.resizePty(term.rows, term.cols);
});
resizeObserver.observe(container);

// Trigger 2: Window resize events (triggered by panel toggle/drag)
const onWindowResize = () => {
  fitAddon.fit();
  commands.resizePty(term.rows, term.cols);
};
window.addEventListener("resize", onWindowResize);
```

**Step-by-step resize flow**:

1. User drags the resize handle or toggles a panel
2. `main.ts` dispatches `window.dispatchEvent(new Event("resize"))` after any layout change
3. The `onWindowResize` handler fires and calls `fitAddon.fit()`
4. `FitAddon.fit()` measures the container's pixel dimensions, calculates the new rows/cols, and resizes xterm.js's internal buffer and canvas
5. `commands.resizePty(term.rows, term.cols)` invokes the `resize_pty` Tauri command
6. Rust calls `master.resize(PtySize { rows, cols, ... })` which sends `SIGWINCH` to the child (`tmux attach-session`)
7. tmux receives `SIGWINCH`, resizes the active window's pseudo-terminal
8. Claude Code CLI receives `SIGWINCH`, redraws its TUI at the new dimensions

### Layout-Triggered Resize (`src/main.ts`)

Every layout change in `main.ts` explicitly fires a resize event:

```typescript
// Panel toggle dispatches resize
function togglePanel(panel: string) {
  // ... toggle panel visibility ...
  window.dispatchEvent(new Event("resize"));  // Always fire after layout change
}

// View toggle (terminal ↔ objectives) also dispatches resize
function switchView(view: string) {
  // ... toggle view ...
  window.dispatchEvent(new Event("resize"));
}

// Drag-to-resize right panel continuously fires
document.addEventListener("mousemove", (e) => {
  if (!isResizing) return;
  // ... calculate new width ...
  window.dispatchEvent(new Event("resize"));
});
```

---

## 7. Agent Terminal Switching

Each agent runs in its own tmux window. Switching which agent you're viewing is done by telling tmux to select a different window — the PTY's output stream immediately switches.

### The Agent Card Click Handler (`src/components/AgentCard.ts`)

```typescript
card.addEventListener("click", async () => {
  if (isRunning && agent.tmux_window_id) {
    await commands.tmuxSelectWindow(agent.tmux_window_id);
    window.dispatchEvent(new CustomEvent("agent-focused", {
      detail: { name: agent.name, color: theme.color }
    }));
  }
});
```

**What happens**:
1. `commands.tmuxSelectWindow(agent.tmux_window_id)` calls `tmux select-window -t @N`
2. The tmux session's active window changes
3. The PTY (running `tmux attach-session -t aperture`) now receives output from the new window
4. xterm.js displays the new agent's terminal
5. The `"agent-focused"` custom event updates the "viewing: [agent]" badge in the sidebar

### Agent Status Tracking (`src-tauri/src/agents.rs`)

The `AgentDef` struct in `src-tauri/src/state.rs` stores the window ID:

```rust
pub struct AgentDef {
    pub name: String,
    pub model: String,
    pub role: String,
    pub prompt_file: String,
    pub tmux_window_id: Option<String>,  // e.g. Some("@3")
    pub status: String,                  // "running" | "stopped"
}
```

`list_agents` cross-references tmux's actual window list with stored state to detect externally-started agents:

```rust
if let Ok(windows) = tmux::tmux_list_windows(app_state.tmux_session.clone()) {
    for window in &windows {
        if let Some(agent) = app_state.agents.get_mut(&window.name) {
            if window.command == "claude" || window.command.contains("claude") {
                if agent.status != "running" {
                    agent.status = "running".into();
                    agent.tmux_window_id = Some(window.window_id.clone());
                }
            }
        }
    }
}
```

This enables Aperture to detect agents started directly in the terminal (outside the UI).

### Agent List Polling (`src/main.ts`)

The agent list refreshes every 3 seconds:

```typescript
setInterval(() => agentList.refresh(), 3000);
```

`refresh()` calls `commands.listAgents()` and rebuilds the sidebar cards if any agent's state changed (detected via a hash of `name:status` pairs).

---

## 8. Agent Startup Pipeline (`src-tauri/src/agents.rs`)

When an agent is started, a dedicated tmux window is created and the Claude Code CLI is launched inside it:

```rust
pub fn start_agent(name: String, state: tauri::State<'_, Arc<Mutex<AppState>>>) -> Result<(), String> {
    // 1. Create a dedicated tmux window for this agent
    let window_id = tmux::tmux_create_window(
        app_state.tmux_session.clone(),
        name.clone(),
    )?;

    // 2. Build MCP config JSON for the aperture-bus server
    let mcp_config = serde_json::json!({
        "mcpServers": {
            "aperture-bus": {
                "type": "stdio",
                "command": "node",
                "args": [&app_state.mcp_server_path],
                "env": {
                    "AGENT_NAME": &name,
                    "AGENT_ROLE": &agent.role,
                    "AGENT_MODEL": &agent.model,
                    "APERTURE_MAILBOX": &mailbox_dir,
                    "BEADS_DIR": "~/.aperture/.beads",
                    "BD_ACTOR": &name
                }
            }
        }
    });

    // Write MCP config to /tmp/aperture-mcp-{name}.json
    fs::write(&config_path, serde_json::to_string_pretty(&mcp_config).unwrap())?;

    // 3. Write a launcher shell script
    let launcher_script = format!(
        r#"#!/bin/bash
export PATH="/opt/homebrew/bin:/usr/local/bin:$PATH"
PROMPT=$(cat "{}")
exec claude --dangerously-skip-permissions --model {} --system-prompt "$PROMPT" --mcp-config {} --name {}
"#,
        agent.prompt_file, agent.model, config_path, name
    );
    fs::write(&launcher_path, &launcher_script)?;

    // 4. Send the launcher script path to the tmux window via send-keys
    tmux::tmux_send_keys(window_id.clone(), launcher_path)?;

    // 5. Auto-confirm Claude's workspace trust prompt (3 attempts × 2s delay)
    std::thread::spawn(move || {
        for _ in 0..3 {
            std::thread::sleep(std::time::Duration::from_secs(2));
            let _ = tmux::tmux_send_keys(window_id_clone.clone(), "".into());
        }
    });

    // 6. Store window ID in agent state
    agent_mut.tmux_window_id = Some(window_id);
    agent_mut.status = "running".into();
}
```

**Key design decisions**:
- The launcher **reads the prompt from a file** (`cat "{agent.prompt_file}"`) rather than inlining it. This avoids shell escaping issues with multi-line prompts containing special characters.
- `exec claude ...` replaces the bash process with claude, so the tmux window's `pane_current_command` becomes `claude` (used by `list_agents` for status detection).
- The workspace trust auto-confirm sends empty Enter presses because Claude Code prompts `>` with no pre-filled text.

### Agent Stop

```rust
if let Some(ref window_id) = agent.tmux_window_id {
    let _ = tmux::tmux_send_keys(window_id.clone(), "C-c".into());  // Interrupt
    std::thread::sleep(std::time::Duration::from_millis(500));
    let _ = tmux::tmux_send_keys(window_id.clone(), "/exit".into()); // Claude exit command
    std::thread::sleep(std::time::Duration::from_millis(500));
    let _ = tmux::tmux_kill_window(window_id.clone());               // Force kill window
}
```

---

## 9. Terminal Injection (Message Delivery)

### Overview

The `poller.rs` background thread delivers messages to agent terminals using tmux. This is how agents receive messages from other agents (via BEADS) and from legacy file-based mailboxes.

The core technique is the **cat file pattern**:

```
tmux send-keys -t @N -- "cat '/tmp/aperture-msg-{id}.md' && rm '/tmp/aperture-msg-{id}.md'" Enter
```

This causes the agent's shell (which is idle while Claude Code is running, since `exec` replaced it — actually, this runs in the tmux pane's shell context) to print the file contents to stdout, which Claude Code CLI sees as input to its context.

Wait — let me be precise. The `tmux send-keys` approach sends keystrokes to the **currently active process in the pane**. When Claude Code is running (in interactive mode), it reads stdin. The `cat file` command is typed as if the user typed it, which Claude Code interprets as a user message being typed into its prompt.

### BEADS Message Injection Flow (`src-tauri/src/poller.rs`)

```rust
// For each running agent, query BEADS for unread messages
for (agent_name, window_id) in &agents {
    let messages = query_unread_messages(agent_name);

    for msg in &messages {
        let sender = parse_sender_from_title(&msg.title);
        let content = msg.description.as_deref().unwrap_or("(no content)");

        // Format as markdown
        let formatted = format!(
            "# Message from {}\n_{}_\n\n{}\n",
            sender, now, content
        );

        // Write to temp file and inject via tmux (safer than inline content)
        let tmp_path = format!("/tmp/aperture-msg-{}.md", msg.id);
        if fs::write(&tmp_path, &formatted).is_ok() {
            let cmd = format!("cat '{}' && rm '{}'", tmp_path, tmp_path);
            let _ = tmux::tmux_send_keys(window_id.clone(), cmd);
        }

        // Mark as read immediately after delivery
        mark_message_read(&msg.id);
    }
}
```

**Why write to a temp file instead of sending inline**?

The message content may contain:
- Newlines (would prematurely submit the command)
- Single quotes (would break the shell quoting)
- Special characters that tmux or the shell would interpret

Writing to a file and using `cat` sidesteps all escaping issues.

**BEADS query**:

```rust
fn query_unread_messages(recipient: &str) -> Vec<BeadsMessage> {
    let query = format!("type=message AND status=open AND title=\"->{recipient}]\"");
    let output = std::process::Command::new(bd_path())
        .args(["query", &query, "--json", "-n", "0", "-q"])
        .env("BEADS_DIR", beads_dir())
        .env("BD_ACTOR", "poller")
        .env("PATH", path_env())
        .output();
    // parse JSON result...
}
```

Messages in BEADS have titles formatted as `[sender->recipient] preview...`. The query matches on `->recipient]` to find messages for each agent. After delivery, the message is closed with reason `"delivered"`.

### Legacy File-Based Injection

For backward compatibility with older mailbox files:

```rust
let mailbox_path = format!("{}/{}", mailbox_base, agent_name);
let files = scan_mailbox(&mailbox_path);
if !files.is_empty() {
    let cmd = format!(
        "for f in '{}'/*.md; do [ -f \"$f\" ] && cat \"$f\" && rm \"$f\"; done",
        mailbox_path
    );
    let _ = tmux::tmux_send_keys(window_id.clone(), cmd);
}
```

The `for f in *.md` loop processes all pending messages in one tmux send-keys call, reading and deleting each file.

### Poller Loop Timing

```rust
pub fn run_message_poller(state: Arc<Mutex<AppState>>) {
    loop {
        std::thread::sleep(Duration::from_secs(5));
        // ... process spawn requests, kill requests, war room messages,
        //     operator messages, BEADS messages, legacy mailbox files ...
    }
}
```

The poller sleeps 5 seconds between cycles. This means message delivery latency is 0–5 seconds.

---

## 10. CSS Layout (`src/style.css`)

The terminal container uses flexbox to fill available space:

```css
/* ── Terminal ── */
#terminal-container {
  flex: 1;
  min-height: 200px;
  background: var(--bg-primary);
  padding: 4px;
  overflow: hidden;
  position: relative;
}
```

`flex: 1` means the terminal takes all remaining height after the navbar. `overflow: hidden` prevents scrollbars from appearing on the container itself (xterm.js manages its own scrollback). The 4px padding gives a small visual margin around the xterm.js canvas.

The xterm.js CSS is imported globally in `src/main.ts`:

```typescript
import "@xterm/xterm/css/xterm.css";
```

This import is required — without it, the xterm.js canvas will not be styled correctly and the terminal may not be visible.

---

## 11. App Startup Sequence (`src/main.ts`)

The terminal is initialized late in the startup sequence, after the tmux session is confirmed:

```typescript
const SESSION_NAME = "aperture";

async function init() {
  // 1. Create/attach tmux session
  try {
    await commands.tmuxCreateSession(SESSION_NAME);
    navbar.setConnected(true);
  } catch (e) {
    // Show error in terminal container and bail out
    terminalEl.innerHTML = `<div>Terminal connection failed: ${e.message}</div>`;
    return;
  }

  // 2. Mount all UI components...

  // 3. Create the terminal (starts PTY, attaches to tmux session)
  await createTerminal(terminalEl, SESSION_NAME);

  // 4. Poll agent list every 3 seconds
  setInterval(() => agentList.refresh(), 3000);
}
```

If `tmuxCreateSession` fails (tmux not installed, wrong path), the app shows an error in the terminal area and skips `createTerminal`. The `await createTerminal(...)` is the point where the PTY is started and xterm.js is rendered.

---

## 12. Command Bridge (`src/services/tauri-commands.ts`)

All PTY and tmux Tauri commands are wrapped in a typed `commands` object:

```typescript
import { invoke } from "@tauri-apps/api/core";

export const commands = {
  // tmux session management
  tmuxCreateSession: (sessionName: string) => invoke<string>("tmux_create_session", { sessionName }),
  tmuxListWindows:   (sessionName: string) => invoke<WindowInfo[]>("tmux_list_windows", { sessionName }),
  tmuxCreateWindow:  (sessionName: string, windowName: string) => invoke<string>("tmux_create_window", { sessionName, windowName }),
  tmuxKillWindow:    (windowId: string) => invoke<void>("tmux_kill_window", { windowId }),
  tmuxSelectWindow:  (windowId: string) => invoke<void>("tmux_select_window", { windowId }),
  tmuxRenameWindow:  (target: string, newName: string) => invoke<void>("tmux_rename_window", { target, newName }),

  // PTY management
  startPty:  (sessionName: string) => invoke<void>("start_pty", { sessionName }),
  writePty:  (input: string)       => invoke<void>("write_pty", { input }),
  resizePty: (rows: number, cols: number) => invoke<void>("resize_pty", { rows, cols }),

  // Agent management
  startAgent:  (name: string) => invoke<void>("start_agent", { name }),
  stopAgent:   (name: string) => invoke<void>("stop_agent", { name }),
  listAgents:  ()             => invoke<AgentDef[]>("list_agents"),
};
```

The Tauri IPC bridge converts camelCase JS keys to snake_case Rust command names automatically. E.g. `{ sessionName: "aperture" }` maps to the Rust parameter `session_name: String`.

---

## 13. Dependencies & Versions

### Frontend (`package.json`)

```json
{
  "dependencies": {
    "@tauri-apps/api":   "^2.10.1",
    "@xterm/addon-fit":  "^0.11.0",
    "@xterm/addon-webgl": "^0.19.0",
    "@xterm/xterm":      "^6.0.0"
  },
  "devDependencies": {
    "@tauri-apps/cli": "^2.10.1",
    "typescript":      "^5.9.3",
    "vite":            "^8.0.0"
  }
}
```

**xterm package notes**:
- The `@xterm/` namespace (not `xterm`) is the current official package. The old `xterm` npm package is deprecated.
- `@xterm/addon-fit` replaces the old `xterm-addon-fit` package.
- `@xterm/addon-webgl` replaces the old `xterm-addon-webgl` package.
- All three packages must use compatible versions (xterm 6.x requires addon-fit 0.10+/0.11+, addon-webgl 0.18+/0.19+).

### Backend (`src-tauri/Cargo.toml`)

```toml
[dependencies]
portable-pty = "0.8"
tauri = { version = "2", features = ["macos-private-api"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

**`portable-pty` notes**:
- Provides cross-platform PTY creation (`native_pty_system()` uses the OS native PTY API)
- On macOS, this uses the BSD PTY API via `/dev/ptmx`
- The `MasterPty` trait provides `try_clone_reader()` and `take_writer()` for split ownership
- `PtySize` represents the terminal dimensions in rows/cols

---

## 14. Key Invariants to Preserve

When rebuilding this system, these invariants must be maintained:

1. **One PTY, one tmux session**: The single PTY instance attaches to the tmux session. All output routing happens inside tmux via window selection.

2. **`term.open()` before `WebglAddon`**: xterm.js requires the DOM element before it can create a WebGL context.

3. **`drop(pair.slave)` after `spawn_command`**: Failing to drop the slave end in the parent process causes the PTY to never send EOF.

4. **Environment injection in production**: Without explicit `PATH`, `TERM`, `HOME`, `SHELL`, and `LANG` env vars, tmux and Claude Code will fail to start in a packaged `.app` bundle.

5. **`writer.flush()` after writes**: The PTY writer is buffered; keystrokes without flush appear delayed.

6. **Resize must propagate both ways**: `fitAddon.fit()` alone resizes xterm.js locally. `commands.resizePty()` must also be called to resize the PTY, or terminal programs will reflow incorrectly.

7. **Message injection via temp files**: Injecting message content inline via `tmux send-keys` is unreliable for multi-line or special-character content. Always write to a temp file and use `cat`.

8. **`TERM=xterm-256color`**: Must be set in both the `start_pty` command builder and tmux's environment. Required for Claude Code's 256-color TUI rendering.
