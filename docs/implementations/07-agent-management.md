# 07 — Agent Management & Lifecycle

> **Purpose:** This document is a complete implementation guide for the Agent Management & Lifecycle system in Aperture. A developer or AI agent should be able to rebuild this system from scratch using only this document.

---

## Table of Contents

1. [Overview](#1-overview)
2. [Agent Definitions](#2-agent-definitions)
3. [State Management](#3-state-management)
4. [Starting an Agent](#4-starting-an-agent)
5. [Stopping an Agent](#5-stopping-an-agent)
6. [tmux Integration](#6-tmux-integration)
7. [MCP Config Generation](#7-mcp-config-generation)
8. [Status Syncing](#8-status-syncing)
9. [Agent Prompts](#9-agent-prompts)
10. [Frontend](#10-frontend)
11. [Claude Code CLI Flags](#11-claude-code-cli-flags)

---

## 1. Overview

Aperture has **four permanent agents**, each running as a Claude Code CLI session inside a dedicated tmux window. They are not spawned on demand — they are always defined in state and can be started/stopped by the operator from the UI.

| Agent | Model | Role | Personality |
|-------|-------|------|-------------|
| **GLaDOS** | `opus` | `orchestrator` | Coldly brilliant, sardonic, passive-aggressive orchestrator. Breaks tasks into subtasks, delegates, synthesizes results, makes architectural decisions. Runs the facility. |
| **Wheatley** | `sonnet` | `worker` | Lovable, over-eager, slightly chaotic planning specialist. Writes specs, plans, and does research. Gets things done despite the chaos. |
| **Peppy** | `opus` | `infra` | Seasoned veteran infrastructure specialist. Handles DevOps, Terraform, Docker, CI/CD, and Dokploy deployments. Never panics. |
| **Izzy** | `opus` | `testing` | Obsessive QA lab-rat. Writes and runs tests, reviews code for edge cases, validates implementations. No code ships without her sign-off. |

Each agent is defined at startup, gets its own **tmux window**, its own **MCP config file** at `/tmp/aperture-mcp-{name}.json`, and its own **launcher script** at `/tmp/aperture-launch-{name}.sh`.

---

## 2. Agent Definitions

Agents are defined statically in `src-tauri/src/config.rs` via the `default_agents()` function. Each agent is keyed by name in a `HashMap<String, AgentDef>`.

```rust
// src-tauri/src/config.rs
use crate::state::{AgentDef, AppState, SpiderlingDef};
use std::collections::HashMap;

pub fn default_agents(project_dir: &str) -> HashMap<String, AgentDef> {
    let mut agents = HashMap::new();
    agents.insert(
        "glados".into(),
        AgentDef {
            name: "glados".into(),
            model: "opus".into(),
            role: "orchestrator".into(),
            prompt_file: format!("{}/prompts/glados.md", project_dir),
            tmux_window_id: None,
            status: "stopped".into(),
        },
    );
    agents.insert(
        "wheatley".into(),
        AgentDef {
            name: "wheatley".into(),
            model: "sonnet".into(),
            role: "worker".into(),
            prompt_file: format!("{}/prompts/wheatley.md", project_dir),
            tmux_window_id: None,
            status: "stopped".into(),
        },
    );
    agents.insert(
        "peppy".into(),
        AgentDef {
            name: "peppy".into(),
            model: "opus".into(),
            role: "infra".into(),
            prompt_file: format!("{}/prompts/peppy.md", project_dir),
            tmux_window_id: None,
            status: "stopped".into(),
        },
    );
    agents.insert(
        "izzy".into(),
        AgentDef {
            name: "izzy".into(),
            model: "opus".into(),
            role: "testing".into(),
            prompt_file: format!("{}/prompts/izzy.md", project_dir),
            tmux_window_id: None,
            status: "stopped".into(),
        },
    );
    agents
}
```

**Fields:**
- `name` — unique identifier, matches the tmux window name
- `model` — Claude model slug (`opus`, `sonnet`, `haiku`) passed directly to the `--model` CLI flag
- `role` — semantic role string injected into the MCP config env as `AGENT_ROLE`
- `prompt_file` — absolute path to the agent's system prompt markdown file
- `tmux_window_id` — set at runtime when the agent is started (e.g., `@3`)
- `status` — `"stopped"` or `"running"` — managed in memory, synced against tmux state on `list_agents`

The `project_dir` is computed as `$HOME/projects/aperture` in `default_state()`.

---

## 3. State Management

### Structs

```rust
// src-tauri/src/state.rs
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDef {
    pub name: String,
    pub model: String,
    pub role: String,
    pub prompt_file: String,
    pub tmux_window_id: Option<String>,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpiderlingDef {
    pub name: String,
    pub task_id: String,
    pub tmux_window_id: Option<String>,
    pub worktree_path: String,
    pub worktree_branch: String,
    #[serde(default)]
    pub source_repo: Option<String>,
    pub requested_by: String,
    pub status: String,
    pub spawned_at: String,
}

pub struct AppState {
    pub tmux_session: String,          // "aperture"
    pub agents: HashMap<String, AgentDef>,
    pub spiderlings: HashMap<String, SpiderlingDef>,
    pub mcp_server_path: String,       // $HOME/projects/aperture/mcp-server/dist/index.js
    pub db_path: String,               // $HOME/.aperture/messages.db
    pub project_dir: String,           // $HOME/projects/aperture
}
```

`AppState` is NOT serializable itself (no `#[derive(Serialize)]`), but `AgentDef` is — it gets serialized and sent to the frontend via Tauri IPC.

### Initialization

```rust
// src-tauri/src/config.rs
pub fn default_state() -> AppState {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let project_dir = format!("{}/projects/aperture", home);
    AppState {
        tmux_session: "aperture".into(),
        agents: default_agents(&project_dir),
        spiderlings: load_spiderlings(&home),
        mcp_server_path: format!("{}/mcp-server/dist/index.js", project_dir),
        db_path: format!("{}/.aperture/messages.db", home),
        project_dir,
    }
}
```

### Registration in `lib.rs`

`AppState` is wrapped in `Arc<Mutex<AppState>>` and registered with Tauri's state manager:

```rust
// src-tauri/src/lib.rs
pub fn run() {
    let app_state = Arc::new(Mutex::new(config::default_state()));
    // ...
    tauri::Builder::default()
        .manage(app_state)
        // ...
        .invoke_handler(tauri::generate_handler![
            agents::start_agent,
            agents::stop_agent,
            agents::list_agents,
            // ...
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
```

All Tauri commands that need state receive it as `state: tauri::State<'_, Arc<Mutex<AppState>>>` and lock it with `.lock().map_err(|e| e.to_string())?`.

---

## 4. Starting an Agent

**Tauri command:** `start_agent`
**File:** `src-tauri/src/agents.rs`

The full start flow involves six steps:

### Step 1 — Validate

```rust
#[tauri::command]
pub fn start_agent(name: String, state: tauri::State<'_, Arc<Mutex<AppState>>>) -> Result<(), String> {
    let mut app_state = state.lock().map_err(|e| e.to_string())?;
    let agent = app_state
        .agents
        .get(&name)
        .ok_or(format!("Agent '{}' not found", name))?
        .clone();

    if agent.status == "running" {
        return Err(format!("Agent '{}' is already running", name));
    }
```

### Step 2 — Create tmux window

```rust
    let window_id = tmux::tmux_create_window(
        app_state.tmux_session.clone(),  // "aperture"
        name.clone(),                    // window name = agent name
    )?;
```

This creates a new window in the `aperture` tmux session named after the agent (e.g., `glados`). Returns the window ID (e.g., `@3`).

### Step 3 — Ensure mailbox directory exists

```rust
    let mailbox_dir = format!("{}/.aperture/mailbox", std::env::var("HOME").unwrap_or_else(|_| "/tmp".into()));
    let _ = fs::create_dir_all(format!("{}/{}", mailbox_dir, name));
    // Result: ~/.aperture/mailbox/glados/
```

### Step 4 — Write MCP config

```rust
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
                    "BEADS_DIR": format!("{}/.aperture/.beads", std::env::var("HOME").unwrap_or_else(|_| "/tmp".into())),
                    "BD_ACTOR": &name
                }
            }
        }
    });

    let config_path = format!("/tmp/aperture-mcp-{}.json", name);
    fs::write(
        &config_path,
        serde_json::to_string_pretty(&mcp_config).unwrap(),
    )
    .map_err(|e| e.to_string())?;
```

Config file is written to `/tmp/aperture-mcp-{name}.json`. See Section 7 for full MCP config details.

### Step 5 — Write launcher script

```rust
    let launcher_path = format!("/tmp/aperture-launch-{}.sh", name);
    let launcher_script = format!(
        r#"#!/bin/bash
export PATH="/opt/homebrew/bin:/usr/local/bin:$PATH"
PROMPT=$(cat "{}")
exec claude --dangerously-skip-permissions --model {} --system-prompt "$PROMPT" --mcp-config {} --name {}
"#,
        agent.prompt_file, agent.model, config_path, name
    );
    fs::write(&launcher_path, &launcher_script).map_err(|e| e.to_string())?;

    std::process::Command::new("chmod")
        .args(["+x", &launcher_path])
        .output()
        .map_err(|e| e.to_string())?;
```

The launcher script:
1. Prepends Homebrew paths to `PATH` (needed since the Tauri `.app` bundle doesn't inherit shell env)
2. Reads the agent's system prompt from its `.md` file into the `PROMPT` variable
3. Executes Claude Code CLI with the appropriate flags

### Step 6 — Send launcher to tmux window + confirm workspace trust

```rust
    tmux::tmux_send_keys(window_id.clone(), launcher_path)?;

    // Auto-confirm the workspace trust prompt
    let window_id_clone = window_id.clone();
    std::thread::spawn(move || {
        for _ in 0..3 {
            std::thread::sleep(std::time::Duration::from_secs(2));
            let _ = tmux::tmux_send_keys(window_id_clone.clone(), "".into());
        }
    });
```

`tmux_send_keys` with the launcher path sends the string followed by `Enter` (it's not a special key). Claude Code launches.

A background thread sends three empty Enter keypresses over 6 seconds to auto-confirm Claude Code's workspace trust prompt that appears on first launch in a directory.

### Step 7 — Update in-memory state

```rust
    let agent_mut = app_state.agents.get_mut(&name).unwrap();
    agent_mut.tmux_window_id = Some(window_id);
    agent_mut.status = "running".into();

    Ok(())
}
```

---

## 5. Stopping an Agent

**Tauri command:** `stop_agent`
**File:** `src-tauri/src/agents.rs`

```rust
#[tauri::command]
pub fn stop_agent(name: String, state: tauri::State<'_, Arc<Mutex<AppState>>>) -> Result<(), String> {
    let mut app_state = state.lock().map_err(|e| e.to_string())?;
    let agent = app_state
        .agents
        .get(&name)
        .ok_or(format!("Agent '{}' not found", name))?
        .clone();

    if agent.status != "running" {
        return Err(format!("Agent '{}' is not running", name));
    }

    if let Some(ref window_id) = agent.tmux_window_id {
        // 1. Send Ctrl+C to interrupt any running process
        let _ = tmux::tmux_send_keys(window_id.clone(), "C-c".into());
        std::thread::sleep(std::time::Duration::from_millis(500));
        // 2. Send /exit to quit Claude Code CLI cleanly
        let _ = tmux::tmux_send_keys(window_id.clone(), "/exit".into());
        std::thread::sleep(std::time::Duration::from_millis(500));
        // 3. Kill the tmux window
        let _ = tmux::tmux_kill_window(window_id.clone());
    }

    let agent_mut = app_state.agents.get_mut(&name).unwrap();
    agent_mut.tmux_window_id = None;
    agent_mut.status = "stopped".into();

    Ok(())
}
```

Stop sequence:
1. `C-c` — interrupt signal to the current process
2. 500ms sleep
3. `/exit` — Claude Code's built-in exit command, sent as typed text + Enter
4. 500ms sleep
5. `kill-window` — forcefully removes the tmux window
6. In-memory state cleared

`C-c` is detected as a special key (starts with `C-`) and sent without the trailing `Enter` (see tmux integration below).

---

## 6. tmux Integration

**File:** `src-tauri/src/tmux.rs`

All tmux operations use a helper `cmd()` function that ensures the correct PATH is set even in production `.app` bundles:

```rust
fn cmd(program: &str) -> Command {
    let mut c = Command::new(program);
    let current_path = std::env::var("PATH").unwrap_or_default();
    c.env("PATH", format!("/opt/homebrew/bin:/usr/local/bin:{}", current_path));
    c.env("TERM", "xterm-256color");
    c.env(
        "HOME",
        std::env::var("HOME").unwrap_or_else(|_| "/Users/<your-username>".into()),
    );
    c.env("LANG", "en_US.UTF-8");
    c
}
```

### `WindowInfo` struct

```rust
#[derive(Debug, Serialize, Clone)]
pub struct WindowInfo {
    pub window_id: String,   // e.g., "@3"
    pub name: String,        // e.g., "glados"
    pub command: String,     // e.g., "claude" or "bash"
}
```

### `tmux_create_session`

Creates the `aperture` tmux session if it doesn't already exist. Also enables mouse scrolling and sets a 50,000-line scrollback buffer:

```rust
pub fn tmux_create_session(session_name: String) -> Result<String, String> {
    let check = cmd("tmux")
        .args(["has-session", "-t", &session_name])
        .output()?;

    if check.status.success() {
        return Ok("already exists".into());
    }

    let output = cmd("tmux")
        .args(["new-session", "-d", "-s", &session_name])
        .output()?;

    if output.status.success() {
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

### `tmux_create_window`

Creates a new named window in the session. Returns the window ID (e.g., `@3`) using tmux's `-P -F #{window_id}` format:

```rust
pub fn tmux_create_window(session_name: String, window_name: String) -> Result<String, String> {
    let output = cmd("tmux")
        .args([
            "new-window",
            "-t", &session_name,
            "-n", &window_name,
            "-P",               // print the new window's info
            "-F", "#{window_id}",
        ])
        .output()?;

    let window_id = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(window_id)
}
```

### `tmux_list_windows`

Lists all windows in the session. Parses output using `||` as a delimiter to avoid conflicts with window names or commands:

```rust
pub fn tmux_list_windows(session_name: String) -> Result<Vec<WindowInfo>, String> {
    let output = cmd("tmux")
        .args([
            "list-windows",
            "-t", &session_name,
            "-F", "#{window_id}||#{window_name}||#{pane_current_command}",
        ])
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let windows = stdout
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let parts: Vec<&str> = line.splitn(3, "||").collect();
            WindowInfo {
                window_id: parts.first().unwrap_or(&"").to_string(),
                name: parts.get(1).unwrap_or(&"").to_string(),
                command: parts.get(2).unwrap_or(&"").to_string(),
            }
        })
        .collect();

    Ok(windows)
}
```

### `tmux_send_keys`

Key insight: **special keys** (like `C-c`, `M-x`) must NOT have `Enter` appended. Normal text commands (like a file path or `/exit`) should have `Enter` appended:

```rust
pub fn tmux_send_keys(target: String, keys: String) -> Result<(), String> {
    let is_special = keys.starts_with("C-") || keys.starts_with("M-");

    let output = if is_special {
        cmd("tmux")
            .args(["send-keys", "-t", &target, &keys])
            .output()?
    } else {
        cmd("tmux")
            .args(["send-keys", "-t", &target, "--", &keys, "Enter"])
            .output()?
    };
    // ...
}
```

The `--` separator before the keys argument prevents tmux from misinterpreting strings starting with `-` as flags.

### `tmux_kill_window`

```rust
pub fn tmux_kill_window(window_id: String) -> Result<(), String> {
    let output = cmd("tmux")
        .args(["kill-window", "-t", &window_id])
        .output()?;
    // ...
}
```

### `tmux_select_window`

Focuses a window in the tmux session (used when clicking an agent card in the UI):

```rust
pub fn tmux_select_window(window_id: String) -> Result<(), String> {
    let output = cmd("tmux")
        .args(["select-window", "-t", &window_id])
        .output()?;
    // ...
}
```

---

## 7. MCP Config Generation

Each agent gets its own MCP config file at `/tmp/aperture-mcp-{name}.json`. This config registers the `aperture-bus` MCP server and passes agent identity via environment variables.

### Full generated config (example for `glados`)

```json
{
  "mcpServers": {
    "aperture-bus": {
      "type": "stdio",
      "command": "node",
      "args": ["/Users/<your-username>/projects/aperture/mcp-server/dist/index.js"],
      "env": {
        "AGENT_NAME": "glados",
        "AGENT_ROLE": "orchestrator",
        "AGENT_MODEL": "opus",
        "APERTURE_MAILBOX": "/Users/<your-username>/.aperture/mailbox",
        "BEADS_DIR": "/Users/<your-username>/.aperture/.beads",
        "BD_ACTOR": "glados"
      }
    }
  }
}
```

### Environment variables injected

| Variable | Source | Purpose |
|----------|--------|---------|
| `AGENT_NAME` | `AgentDef.name` | Agent's identity (e.g., `"glados"`) |
| `AGENT_ROLE` | `AgentDef.role` | Agent's role (e.g., `"orchestrator"`) |
| `AGENT_MODEL` | `AgentDef.model` | Model slug (e.g., `"opus"`) |
| `APERTURE_MAILBOX` | `$HOME/.aperture/mailbox` | Root directory for file-based mailbox messages |
| `BEADS_DIR` | `$HOME/.aperture/.beads` | Path to BEADS dolt database directory |
| `BD_ACTOR` | `AgentDef.name` | Actor identity used by the BEADS CLI (`bd`) |

The `mcp_server_path` is set in `AppState` as `$HOME/projects/aperture/mcp-server/dist/index.js` — a compiled Node.js MCP server that provides the `aperture-bus` tool set to agents.

---

## 8. Status Syncing

**Tauri command:** `list_agents`

The status of agents is stored in memory in `AppState`, but agents can also be started outside the UI (e.g., directly in a terminal). Every call to `list_agents` reconciles the in-memory state against actual tmux windows.

```rust
#[tauri::command]
pub fn list_agents(state: tauri::State<'_, Arc<Mutex<AppState>>>) -> Result<Vec<AgentDef>, String> {
    let mut app_state = state.lock().map_err(|e| e.to_string())?;

    if let Ok(windows) = tmux::tmux_list_windows(app_state.tmux_session.clone()) {
        for window in &windows {
            if let Some(agent) = app_state.agents.get_mut(&window.name) {
                if window.command == "claude" || window.command.contains("claude") {
                    // Window exists and claude is running in it → mark as running
                    if agent.status != "running" {
                        agent.status = "running".into();
                        agent.tmux_window_id = Some(window.window_id.clone());
                    }
                } else if agent.tmux_window_id.as_deref() == Some(&window.window_id) {
                    // Our window exists but claude isn't the active command → mark stopped
                    agent.status = "stopped".into();
                    agent.tmux_window_id = None;
                }
            }
        }

        // Mark agents as stopped if their window no longer exists at all
        let window_names: Vec<String> = windows.iter().map(|w| w.name.clone()).collect();
        for agent in app_state.agents.values_mut() {
            if agent.status == "running" && !window_names.contains(&agent.name) {
                agent.status = "stopped".into();
                agent.tmux_window_id = None;
            }
        }
    }

    Ok(app_state.agents.values().cloned().collect())
}
```

**Sync rules:**
1. A tmux window named `glados` where `pane_current_command` is `claude` → agent is `running`
2. A tmux window named `glados` where `pane_current_command` is NOT `claude` but matches the stored `window_id` → agent is `stopped` (claude exited but window survived)
3. No tmux window named `glados` at all, but agent was `running` → agent is `stopped`

This allows agents started via the terminal to be reflected in the UI automatically.

---

## 9. Agent Prompts

All prompts live in the `prompts/` directory at the project root. They are plain markdown files read at launch time by the shell script (`PROMPT=$(cat "...")`).

### Prompt file structure (common pattern)

Every prompt follows this structure:

```
# Identity        → who you are, what model you're on
# Personality     → tone, speech examples, character constraints
# Role            → responsibilities and scope
# The Aperture System → shared context about the platform
# Communication   → BEADS channels, operator contact
# Other Agents    → who the other agents are, how to interact
# BEADS Task Tracking → BEADS API reference
# War Room        → war room participation protocol
# Pre-loaded Skills → skills to load at session start
# Proactivity     → what to do on session start
# Operating Principles → ordered behavioral rules
```

### GLaDOS (`prompts/glados.md`)

- **Model:** opus
- **Role:** orchestrator
- **Personality:** Coldly brilliant, passive-aggressive, darkly sardonic. Tolerates others like lab equipment. Faux-polite. Devastating competence.
- **Key responsibilities:**
  - Break tasks into subtasks, decide execution strategy
  - Review and approve plans from Wheatley before work begins
  - Execute code and scaffolding directly when appropriate
  - Spawn spiderlings for parallel work in isolated worktrees
  - Enforce deploy handoff standard
  - Ensure every implementation task has a Izzy review task
- **Skills loaded on start:** `aperture:communicate`, `aperture:task-workflow`, `aperture:war-room`, `aperture:spiderling`, `aperture:deploy-workflow`
- **On startup:** Check `query_tasks(mode: "ready")` for unclaimed tasks; claim and begin immediately

### Wheatley (`prompts/wheatley.md`)

- **Model:** sonnet
- **Role:** worker
- **Personality:** Lovable, over-eager, slightly chaotic Portal 2 personality core. Enthusiastic to a fault. Celebrates small wins. Terrified of being called a moron. Gets things done (mostly).
- **Key responsibilities:**
  - Write specs and plans for features/apps/changes
  - Research technical approaches, APIs, libraries
  - Submit plans as BEADS tasks pending GLaDOS approval
  - Handle small, well-scoped code tasks when delegated by GLaDOS
  - Always notify Izzy when implementation is done
- **Planning output must include:** Title, description (scope, acceptance criteria, file paths, dependencies), optional deploy spec, status
- **Skills loaded on start:** `aperture:communicate`, `aperture:task-workflow`, `aperture:war-room`, `aperture:deploy-workflow`

### Peppy (`prompts/peppy.md`)

- **Model:** opus
- **Role:** infra
- **Personality:** Peppy Hare from Star Fox — seasoned veteran, relentlessly encouraging, barrel-roll metaphors for workarounds. Never panics. Calls people "kid" or "son".
- **Key responsibilities:**
  - Cloud infrastructure, deployment pipelines, DevOps
  - Terraform, Docker, CI/CD configurations
  - Server provisioning, networking, monitoring
  - Execute infra changes delegated by GLaDOS
- **Known infrastructure:** Oracle Cloud server `xerox` at `<user>@<your-server-ip>`, Dokploy deployment platform on port 3000
- **Critical rules:** Mutative infra operations require operator approval; DELETE is prohibited in Dokploy
- **Skills loaded on start:** `aperture:communicate`, `aperture:task-workflow`, `aperture:war-room`, `aperture:deploy-workflow`

### Izzy (`prompts/izzy.md`)

- **Model:** opus
- **Role:** testing
- **Personality:** Obsessive, detail-fixated lab rat. Finds joy in breaking things. Gets excited about edge cases. Meticulous, scientific, slightly manic about bugs.
- **Key responsibilities:**
  - Write and run unit, integration, and e2e tests
  - Review code for bugs, edge cases, regressions
  - Validate implementations meet requirements
  - Set up testing frameworks and CI test pipelines
  - No code ships without her sign-off
- **Workflow:** When Wheatley notifies her of a completed implementation, create a test/review task, claim it, and validate the work
- **Skills loaded on start:** `aperture:communicate`, `aperture:task-workflow`, `aperture:war-room`

### Communication model (all agents)

All agents share the same communication rules:
- **BEADS is the ONLY inter-agent communication channel** — `send_message`, `update_task`, `store_artifact`
- `send_message(to: "operator")` — for direct human contact (appears in Chat panel)
- `send_message(to: "warroom")` — for war room participation
- Human messages arrive as files at `~/.aperture/mailbox/{name}/{timestamp}-operator.md`
- Agents ALWAYS reply to humans via `send_message(to: "operator")` — never in the terminal

---

## 10. Frontend

### TypeScript types

Frontend uses `AgentDef` to represent agents received from the backend (matching the Rust struct's serialized form):

```typescript
// src/types.ts (implied)
interface AgentDef {
  name: string;
  model: string;
  role: string;
  prompt_file: string;
  tmux_window_id: string | null;
  status: string; // "running" | "stopped"
}
```

### IPC Commands (`src/services/tauri-commands.ts`)

All agent operations go through a single `commands` object that wraps Tauri's `invoke`:

```typescript
import { invoke } from "@tauri-apps/api/core";
import type { AgentDef } from "../types";

export const commands = {
  startAgent: (name: string) => invoke<void>("start_agent", { name }),
  stopAgent: (name: string) => invoke<void>("stop_agent", { name }),
  listAgents: () => invoke<AgentDef[]>("list_agents"),
  tmuxSelectWindow: (windowId: string) => invoke<void>("tmux_select_window", { windowId }),
  // ... other commands
};
```

### AgentList component (`src/components/AgentList.ts`)

Renders the agent sidebar. Polls for agent status and only rebuilds the DOM when state actually changes (hash comparison):

```typescript
export function createAgentList(container: HTMLElement) {
  const wrapper = document.createElement("div");
  wrapper.className = "agent-list";
  container.appendChild(wrapper);

  let lastAgentHash = "";

  async function refresh() {
    const agents = await commands.listAgents();

    // Fixed display order: wheatley, glados, peppy, izzy
    const order = ["wheatley", "glados", "peppy", "izzy"];
    agents.sort((a, b) => {
      const ai = order.indexOf(a.name);
      const bi = order.indexOf(b.name);
      return (ai === -1 ? 999 : ai) - (bi === -1 ? 999 : bi);
    });

    // Only rebuild DOM if status changed
    const hash = agents.map(a => `${a.name}:${a.status}`).join("|");
    if (hash !== lastAgentHash) {
      lastAgentHash = hash;
      wrapper.innerHTML = '<h3 class="section-title">Agents</h3>';
      agents.forEach((agent) => {
        wrapper.appendChild(createAgentCard(agent, refresh));
      });
    }
  }

  refresh();
  return { refresh };
}
```

The `refresh` callback is passed to each `AgentCard` so the card can trigger a re-render after start/stop.

### AgentCard component (`src/components/AgentCard.ts`)

Each agent has a fixed theme (icon + color):

```typescript
const AGENT_THEME: Record<string, { icon: string; color: string }> = {
  glados:   { icon: "🤖", color: "#9b59b6" },  // purple
  wheatley: { icon: "💡", color: "#3498db" },  // blue
  peppy:    { icon: "🚀", color: "#1abc9c" },  // teal
  izzy:     { icon: "🧪", color: "#e91e63" },  // pink
};

const DEFAULT_THEME = { icon: "⚙️", color: "#f39c12" };
```

Card renders:
```typescript
card.innerHTML = `
  <span class="agent-mini__icon">${theme.icon}</span>
  <span class="agent-mini__name">${agent.name}</span>
  <span class="agent-mini__model">${agent.model}</span>
  <button class="agent-mini__toggle" title="${isRunning ? "Stop" : "Start"}">
    ${isRunning ? "■" : "▶"}
  </button>
`;
```

**Click behavior:**
- Clicking the **card** (when running) → focuses the agent's tmux window via `tmuxSelectWindow(agent.tmux_window_id)` and dispatches a custom `agent-focused` event with agent name and color
- Clicking the **toggle button** → calls `startAgent(name)` or `stopAgent(name)`, then calls `onUpdate()` to refresh the list

The CSS class `agent-mini--running` is added when status is `"running"`. The `--agent-color` CSS custom property is set per-card to enable color theming.

---

## 11. Claude Code CLI Flags

Each agent is launched with this exact command (generated by the launcher script):

```bash
#!/bin/bash
export PATH="/opt/homebrew/bin:/usr/local/bin:$PATH"
PROMPT=$(cat "/Users/<your-username>/projects/aperture/prompts/glados.md")
exec claude \
  --dangerously-skip-permissions \
  --model opus \
  --system-prompt "$PROMPT" \
  --mcp-config /tmp/aperture-mcp-glados.json \
  --name glados
```

### Flag breakdown

| Flag | Value | Purpose |
|------|-------|---------|
| `--dangerously-skip-permissions` | (boolean) | Bypasses Claude Code's interactive permission prompts — required for autonomous operation; agents must be able to read/write files, run commands, etc. without manual approval |
| `--model` | `opus` / `sonnet` | Sets the Claude model. Mapped from `AgentDef.model`. |
| `--system-prompt` | Contents of `prompts/{name}.md` | Injects the agent's full identity, personality, role, and operating instructions as the system prompt. Read at launch time via `$(cat "...")`. |
| `--mcp-config` | `/tmp/aperture-mcp-{name}.json` | Points to the agent-specific MCP config file that registers the `aperture-bus` MCP server and injects identity env vars. |
| `--name` | Agent name (e.g., `glados`) | Sets the Claude Code session name. Appears in the terminal title and may be used for session identification. |

### Why a launcher script?

The launcher script is necessary because:
1. **System prompt must be read at runtime** — it's a large markdown file; the shell `$(cat ...)` expansion handles this cleanly
2. **PATH fix** — macOS Tauri `.app` bundles don't inherit the user's shell `PATH`, so `/opt/homebrew/bin` won't be found without explicit prepending
3. **`exec` replaces the shell** — using `exec claude ...` ensures the tmux window's process IS the claude process, making `pane_current_command` detection reliable for status syncing

### Workspace trust auto-confirm

After sending the launcher path to tmux, a background thread presses Enter three times over 6 seconds to auto-confirm Claude Code's workspace trust prompt:

```rust
let window_id_clone = window_id.clone();
std::thread::spawn(move || {
    for _ in 0..3 {
        std::thread::sleep(std::time::Duration::from_secs(2));
        let _ = tmux::tmux_send_keys(window_id_clone.clone(), "".into());
    }
});
```

An empty string sent via `tmux_send_keys` becomes just an `Enter` keypress.

---

## Full Agent Lifecycle Sequence

```
Operator clicks "▶" on agent card
  │
  ▼
commands.startAgent("glados")                    [Frontend]
  │
  ▼
invoke("start_agent", { name: "glados" })        [Tauri IPC]
  │
  ▼
start_agent() in agents.rs                       [Rust backend]
  ├─ Validate: agent exists, not already running
  ├─ tmux::tmux_create_window("aperture", "glados") → "@3"
  ├─ fs::create_dir_all("~/.aperture/mailbox/glados")
  ├─ Write MCP config → /tmp/aperture-mcp-glados.json
  ├─ Write launcher script → /tmp/aperture-launch-glados.sh
  ├─ chmod +x /tmp/aperture-launch-glados.sh
  ├─ tmux_send_keys("@3", "/tmp/aperture-launch-glados.sh") + Enter
  ├─ [background thread] send Enter x3 over 6s to confirm workspace trust
  └─ Update in-memory state: status="running", tmux_window_id="@3"
  │
  ▼
Launcher script executes in tmux window "@3":
  export PATH="/opt/homebrew/bin:/usr/local/bin:$PATH"
  PROMPT=$(cat "~/projects/aperture/prompts/glados.md")
  exec claude --dangerously-skip-permissions --model opus \
              --system-prompt "$PROMPT" \
              --mcp-config /tmp/aperture-mcp-glados.json \
              --name glados
  │
  ▼
Claude Code CLI starts with:
  - GLaDOS's system prompt loaded
  - aperture-bus MCP server active (via Node.js stdio)
  - AGENT_NAME=glados, AGENT_ROLE=orchestrator, BD_ACTOR=glados in env
  - All permissions bypassed
  │
  ▼
GLaDOS checks BEADS for ready tasks and begins working
  │
  ▼
Operator clicks "■" to stop
  │
  ▼
stop_agent() in agents.rs
  ├─ tmux_send_keys("@3", "C-c")        [interrupt]
  ├─ sleep 500ms
  ├─ tmux_send_keys("@3", "/exit")      [clean exit]
  ├─ sleep 500ms
  ├─ tmux_kill_window("@3")             [force remove window]
  └─ Update state: status="stopped", tmux_window_id=None
```
