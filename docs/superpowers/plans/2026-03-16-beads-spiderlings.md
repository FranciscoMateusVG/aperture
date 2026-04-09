# BEADS Integration + Spiderling Spawning — Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add BEADS-powered artifact/task tracking and GLaDOS-controlled spiderling spawning to Aperture.

**Architecture:** Two independent features wired through the existing MCP server. BEADS tools wrap the `bd` CLI (async). Spiderling spawning uses the mailbox pattern (MCP writes request → Rust poller picks up → creates worktree + tmux window + launcher). Both features surface in the UI via new panel and agent cards.

**Tech Stack:** Rust (Tauri backend), TypeScript (MCP server + frontend), BEADS/bd CLI, git worktrees, tmux.

**Spec:** `docs/superpowers/specs/2026-03-16-beads-spiderlings-design.md`

---

## Chunk 1: BEADS MCP Tools

### Task 1: BEADS helper module

**Files:**
- Create: `mcp-server/src/beads.ts`

- [ ] **Step 1: Create `beads.ts` with `runBd` helper**

```typescript
// mcp-server/src/beads.ts
import { execFile } from "node:child_process";
import { homedir } from "node:os";
import { resolve } from "node:path";

const BEADS_DIR = resolve(homedir(), ".aperture", ".beads");
const BD_PATH = process.env.BD_PATH ?? "bd";

function getActor(): string {
  return process.env.BD_ACTOR ?? process.env.AGENT_NAME ?? "unknown";
}

export function runBd(args: string[]): Promise<string> {
  return new Promise((resolve, reject) => {
    const env = {
      ...process.env,
      BEADS_DIR,
      BD_ACTOR: getActor(),
      PATH: `/opt/homebrew/bin:/usr/local/bin:${process.env.PATH ?? ""}`,
    };
    execFile(BD_PATH, args, { env, timeout: 30000 }, (err, stdout, stderr) => {
      if (err) {
        reject(new Error(stderr || err.message));
      } else {
        resolve(stdout.trim());
      }
    });
  });
}

export async function createTask(
  title: string,
  priority: number,
  description?: string,
): Promise<string> {
  const args = ["create", title, "-p", String(priority), "--json"];
  if (description) {
    args.push("--body-file=-");
  }
  // For description, we need stdin — use a different approach
  if (description) {
    return new Promise((resolve, reject) => {
      const env = {
        ...process.env,
        BEADS_DIR,
        BD_ACTOR: getActor(),
        PATH: `/opt/homebrew/bin:/usr/local/bin:${process.env.PATH ?? ""}`,
      };
      const proc = require("node:child_process").spawn(BD_PATH, args, { env });
      let stdout = "";
      let stderr = "";
      proc.stdout.on("data", (d: Buffer) => (stdout += d.toString()));
      proc.stderr.on("data", (d: Buffer) => (stderr += d.toString()));
      proc.on("close", (code: number) => {
        if (code !== 0) reject(new Error(stderr || `bd exited ${code}`));
        else resolve(stdout.trim());
      });
      proc.stdin.write(description);
      proc.stdin.end();
    });
  }
  return runBd(args);
}

export async function updateTask(id: string, flags: Record<string, string>): Promise<string> {
  const args = ["update", id];
  for (const [key, value] of Object.entries(flags)) {
    args.push(`--${key}`, value);
  }
  args.push("--json");
  return runBd(args);
}

export async function closeTask(id: string, reason: string): Promise<string> {
  return runBd(["close", id, "--reason", reason, "--json"]);
}

export async function queryTasks(mode: string, id?: string): Promise<string> {
  if (mode === "show" && id) {
    return runBd(["show", id, "--json"]);
  }
  if (mode === "ready") {
    return runBd(["ready", "--json"]);
  }
  return runBd(["list", "--json"]);
}

export async function storeArtifact(
  taskId: string,
  type: string,
  value: string,
): Promise<string> {
  const artifactLine = `artifact:${type}:${value}`;
  return runBd(["update", taskId, "--notes", artifactLine, "--json"]);
}

export async function searchTasks(label?: string): Promise<string> {
  const args = ["list", "--json"];
  if (label) {
    args.push("--label", label);
  }
  return runBd(args);
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cd /Users/<your-username>/projetos/aperture/mcp-server && npx tsc --noEmit`
Expected: No errors

- [ ] **Step 3: Commit**

```bash
git add mcp-server/src/beads.ts
git commit -m "feat: add BEADS helper module wrapping bd CLI"
```

### Task 2: Register BEADS MCP tools

**Files:**
- Modify: `mcp-server/src/index.ts`

- [ ] **Step 1: Add BEADS tool imports and registrations**

Add after the existing `get_identity` tool registration in `mcp-server/src/index.ts`:

```typescript
import { createTask, updateTask, closeTask, queryTasks, storeArtifact, searchTasks } from "./beads.js";
```

Then register 6 tools:

```typescript
server.tool(
  "create_task",
  "Create a new BEADS task. Returns the task ID.",
  {
    title: z.string().describe("Task title"),
    priority: z.number().min(0).max(4).describe("Priority 0-4 (0 = highest)"),
    description: z.string().optional().describe("Task description"),
  },
  async ({ title, priority, description }) => {
    try {
      const result = await createTask(title, priority, description);
      return { content: [{ type: "text", text: result }] };
    } catch (e: any) {
      return { content: [{ type: "text", text: `ERROR: ${e.message}` }], isError: true };
    }
  }
);

server.tool(
  "update_task",
  "Update a BEADS task. Use --claim to assign to yourself.",
  {
    id: z.string().describe("Task ID (e.g. bd-a1b2)"),
    claim: z.boolean().optional().describe("Claim this task for yourself"),
    status: z.string().optional().describe("New status"),
    description: z.string().optional().describe("New description"),
    notes: z.string().optional().describe("Append notes"),
  },
  async ({ id, claim, status, description, notes }) => {
    try {
      const flags: Record<string, string> = {};
      if (claim) flags["claim"] = "";
      if (status) flags["status"] = status;
      if (description) flags["description"] = description;
      if (notes) flags["notes"] = notes;
      const result = await updateTask(id, flags);
      return { content: [{ type: "text", text: result }] };
    } catch (e: any) {
      return { content: [{ type: "text", text: `ERROR: ${e.message}` }], isError: true };
    }
  }
);

server.tool(
  "close_task",
  "Close a BEADS task with a reason.",
  {
    id: z.string().describe("Task ID"),
    reason: z.string().describe("Reason for closing"),
  },
  async ({ id, reason }) => {
    try {
      const result = await closeTask(id, reason);
      return { content: [{ type: "text", text: result }] };
    } catch (e: any) {
      return { content: [{ type: "text", text: `ERROR: ${e.message}` }], isError: true };
    }
  }
);

server.tool(
  "query_tasks",
  "Query BEADS tasks. Modes: 'list' (all), 'ready' (unblocked), 'show' (single task by ID).",
  {
    mode: z.enum(["list", "ready", "show"]).describe("Query mode"),
    id: z.string().optional().describe("Task ID (required for 'show' mode)"),
  },
  async ({ mode, id }) => {
    try {
      const result = await queryTasks(mode, id);
      return { content: [{ type: "text", text: result }] };
    } catch (e: any) {
      return { content: [{ type: "text", text: `ERROR: ${e.message}` }], isError: true };
    }
  }
);

server.tool(
  "store_artifact",
  "Store an artifact reference on a BEADS task. Types: file, pr, session, url, note.",
  {
    task_id: z.string().describe("Task ID to attach artifact to"),
    type: z.enum(["file", "pr", "session", "url", "note"]).describe("Artifact type"),
    value: z.string().describe("Artifact value (path, URL, or text)"),
  },
  async ({ task_id, type, value }) => {
    try {
      const result = await storeArtifact(task_id, type, value);
      return { content: [{ type: "text", text: `Artifact stored: ${type}:${value}\n${result}` }] };
    } catch (e: any) {
      return { content: [{ type: "text", text: `ERROR: ${e.message}` }], isError: true };
    }
  }
);

server.tool(
  "search_tasks",
  "Search BEADS tasks, optionally filtered by label.",
  {
    label: z.string().optional().describe("Filter by label"),
  },
  async ({ label }) => {
    try {
      const result = await searchTasks(label);
      return { content: [{ type: "text", text: result }] };
    } catch (e: any) {
      return { content: [{ type: "text", text: `ERROR: ${e.message}` }], isError: true };
    }
  }
);
```

- [ ] **Step 2: Build MCP server**

Run: `cd /Users/<your-username>/projetos/aperture/mcp-server && npm run build`
Expected: Compiles with no errors

- [ ] **Step 3: Commit**

```bash
git add mcp-server/src/index.ts
git commit -m "feat: register BEADS MCP tools (create, update, close, query, artifact, search)"
```

### Task 3: BEADS init on app startup

**Files:**
- Modify: `src-tauri/src/lib.rs`

- [ ] **Step 1: Add BEADS init before tauri::Builder**

In `lib.rs`, after `let pty_state = ...` and before the poller thread spawn, add:

```rust
// Initialize BEADS database if not present
let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
let beads_dir = format!("{}/.aperture/.beads", home);
if !std::path::Path::new(&beads_dir).exists() {
    let _ = std::fs::create_dir_all(&beads_dir);
    let mut cmd = std::process::Command::new("bd");
    cmd.arg("init").arg("--quiet");
    cmd.env("BEADS_DIR", &beads_dir);
    let current_path = std::env::var("PATH").unwrap_or_default();
    cmd.env("PATH", format!("/opt/homebrew/bin:/usr/local/bin:{}", current_path));
    match cmd.output() {
        Ok(output) if output.status.success() => {
            println!("BEADS initialized at {}", beads_dir);
        }
        Ok(output) => {
            eprintln!("BEADS init warning: {}", String::from_utf8_lossy(&output.stderr));
        }
        Err(e) => {
            eprintln!("BEADS init failed (bd not found?): {}", e);
        }
    }
}
```

- [ ] **Step 2: Add `BEADS_DIR` and `BD_ACTOR` to MCP config in agents.rs**

In `src-tauri/src/agents.rs`, in the `start_agent` function, find the `mcp_config` JSON and add to the `env` object:

```rust
"BEADS_DIR": format!("{}/.aperture/.beads", std::env::var("HOME").unwrap_or_else(|_| "/tmp".into())),
"BD_ACTOR": &name,
```

- [ ] **Step 3: Build Rust backend**

Run: `cd /Users/<your-username>/projetos/aperture && cargo build --manifest-path src-tauri/Cargo.toml`
Expected: Compiles

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/lib.rs src-tauri/src/agents.rs
git commit -m "feat: init BEADS on startup, add BEADS env to agent MCP config"
```

---

## Chunk 2: Spiderling Spawner (Rust Backend)

### Task 4: Spiderling state types

**Files:**
- Modify: `src-tauri/src/state.rs`

- [ ] **Step 1: Add SpiderlingDef and update AppState**

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpiderlingDef {
    pub name: String,
    pub task_id: String,
    pub tmux_window_id: Option<String>,
    pub worktree_path: String,
    pub worktree_branch: String,
    pub requested_by: String,
    pub status: String, // "spawning" | "working" | "done" | "killed"
    pub spawned_at: String,
}

// In AppState, add:
pub struct AppState {
    pub tmux_session: String,
    pub agents: HashMap<String, AgentDef>,
    pub spiderlings: HashMap<String, SpiderlingDef>,
    pub mcp_server_path: String,
    pub db_path: String,
    pub project_dir: String,
}
```

- [ ] **Step 2: Update config.rs to initialize spiderlings HashMap**

In `default_state()`, add `spiderlings: HashMap::new()` to the AppState construction.

- [ ] **Step 3: Build**

Run: `cargo build --manifest-path src-tauri/Cargo.toml`
Expected: May have errors in other files referencing AppState — fix them (just add the field everywhere AppState is constructed).

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/state.rs src-tauri/src/config.rs
git commit -m "feat: add SpiderlingDef type and spiderlings to AppState"
```

### Task 5: Spawner module

**Files:**
- Create: `src-tauri/src/spawner.rs`

- [ ] **Step 1: Write spawner.rs**

```rust
use crate::state::{AppState, SpiderlingDef};
use crate::tmux;
use std::fs;
use std::sync::{Arc, Mutex};

const NAME_REGEX: &str = r"^[a-z0-9][a-z0-9-]{0,30}$";
const PERMANENT_AGENTS: &[&str] = &["glados", "wheatley", "peppy", "izzy", "operator", "warroom"];

fn home() -> String {
    std::env::var("HOME").unwrap_or_else(|_| "/tmp".into())
}

fn active_spiderlings_path() -> String {
    format!("{}/.aperture/active-spiderlings.json", home())
}

pub fn write_active_spiderlings(state: &AppState) {
    let spiderlings: Vec<&SpiderlingDef> = state.spiderlings.values().collect();
    if let Ok(json) = serde_json::to_string_pretty(&spiderlings) {
        let _ = fs::write(active_spiderlings_path(), json);
    }
}

fn validate_name(name: &str, state: &AppState) -> Result<(), String> {
    let re = regex::Regex::new(NAME_REGEX).unwrap();
    if !re.is_match(name) {
        return Err(format!(
            "Invalid spiderling name '{}'. Must match [a-z0-9][a-z0-9-]{{0,30}}",
            name
        ));
    }
    if PERMANENT_AGENTS.contains(&name) || state.agents.contains_key(name) {
        return Err(format!("Name '{}' conflicts with a permanent agent", name));
    }
    if state.spiderlings.contains_key(name) {
        return Err(format!("Spiderling '{}' already exists", name));
    }
    Ok(())
}

pub fn spawn_spiderling(
    name: String,
    task_id: String,
    prompt: String,
    requested_by: String,
    app_state: &mut AppState,
) -> Result<String, String> {
    validate_name(&name, app_state)?;

    let home = home();
    let project_dir = app_state.project_dir.clone();
    let mcp_server_path = app_state.mcp_server_path.clone();
    let tmux_session = app_state.tmux_session.clone();

    // Create git worktree
    let worktree_dir = format!("{}/.aperture/worktrees", home);
    let _ = fs::create_dir_all(&worktree_dir);
    let worktree_path = format!("{}/{}", worktree_dir, name);
    let branch_name = name.clone();

    let current_path = std::env::var("PATH").unwrap_or_default();
    let path_env = format!("/opt/homebrew/bin:/usr/local/bin:{}", current_path);

    // Create worktree branch and worktree
    let output = std::process::Command::new("git")
        .args(["worktree", "add", "-b", &branch_name, &worktree_path])
        .current_dir(&project_dir)
        .env("PATH", &path_env)
        .output()
        .map_err(|e| format!("Failed to create worktree: {}", e))?;

    if !output.status.success() {
        // Branch might already exist, try without -b
        let output2 = std::process::Command::new("git")
            .args(["worktree", "add", &worktree_path, &branch_name])
            .current_dir(&project_dir)
            .env("PATH", &path_env)
            .output()
            .map_err(|e| format!("Failed to create worktree: {}", e))?;

        if !output2.status.success() {
            return Err(format!(
                "Failed to create git worktree: {}",
                String::from_utf8_lossy(&output2.stderr)
            ));
        }
    }

    // Create tmux window
    let window_id = tmux::tmux_create_window(tmux_session, name.clone())?;

    // Ensure spiderling mailbox
    let mailbox_dir = format!("{}/.aperture/mailbox/{}", home, name);
    let _ = fs::create_dir_all(&mailbox_dir);

    // Write MCP config
    let mcp_config = serde_json::json!({
        "mcpServers": {
            "aperture-bus": {
                "type": "stdio",
                "command": "node",
                "args": [&mcp_server_path],
                "env": {
                    "AGENT_NAME": &name,
                    "AGENT_ROLE": "spiderling",
                    "AGENT_MODEL": "sonnet",
                    "APERTURE_MAILBOX": format!("{}/.aperture/mailbox", home),
                    "BEADS_DIR": format!("{}/.aperture/.beads", home),
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

    // Write launcher script
    let launcher_dir = format!("{}/.aperture/launchers", home);
    let _ = fs::create_dir_all(&launcher_dir);
    let launcher_path = format!("{}/{}.sh", launcher_dir, name);

    let system_prompt = format!(
        "You are a spiderling named {name}, working for GLaDOS in the Aperture system.\n\
         Your task is tracked in BEADS issue {task_id}.\n\
         Work in this git worktree at {worktree_path} — do NOT switch branches or leave this directory.\n\
         When done: close_task('{task_id}', 'reason'), store_artifact for deliverables, then send_message(to: 'glados', message: 'done').\n\n\
         TASK:\n{prompt}",
        name = name,
        task_id = task_id,
        worktree_path = worktree_path,
        prompt = prompt,
    );

    // Write prompt to file to avoid shell escaping issues
    let prompt_path = format!("{}/{}-prompt.txt", launcher_dir, name);
    fs::write(&prompt_path, &system_prompt).map_err(|e| e.to_string())?;

    let launcher_script = format!(
        r#"#!/bin/bash
export PATH="/opt/homebrew/bin:/usr/local/bin:$PATH"
cd "{worktree_path}"
PROMPT=$(cat "{prompt_path}")
exec claude --dangerously-skip-permissions --model sonnet --system-prompt "$PROMPT" --mcp-config {config_path} --name {name}
"#,
        worktree_path = worktree_path,
        prompt_path = prompt_path,
        config_path = config_path,
        name = name,
    );

    fs::write(&launcher_path, &launcher_script).map_err(|e| e.to_string())?;
    std::process::Command::new("chmod")
        .args(["+x", &launcher_path])
        .output()
        .map_err(|e| e.to_string())?;

    // Launch in tmux
    tmux::tmux_send_keys(window_id.clone(), launcher_path)?;

    // Auto-confirm workspace trust
    let window_id_clone = window_id.clone();
    std::thread::spawn(move || {
        for _ in 0..3 {
            std::thread::sleep(std::time::Duration::from_secs(2));
            let _ = tmux::tmux_send_keys(window_id_clone.clone(), "".into());
        }
    });

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .to_string();

    // Track spiderling
    let spiderling = SpiderlingDef {
        name: name.clone(),
        task_id,
        tmux_window_id: Some(window_id),
        worktree_path,
        worktree_branch: branch_name,
        requested_by,
        status: "working".into(),
        spawned_at: timestamp,
    };

    app_state.spiderlings.insert(name.clone(), spiderling);
    write_active_spiderlings(app_state);

    Ok(name)
}

pub fn kill_spiderling(
    name: String,
    app_state: &mut AppState,
) -> Result<(), String> {
    let spiderling = app_state
        .spiderlings
        .get(&name)
        .ok_or(format!("Spiderling '{}' not found", name))?
        .clone();

    // Kill tmux window
    if let Some(ref window_id) = spiderling.tmux_window_id {
        let _ = tmux::tmux_send_keys(window_id.clone(), "C-c".into());
        std::thread::sleep(std::time::Duration::from_millis(500));
        let _ = tmux::tmux_send_keys(window_id.clone(), "/exit".into());
        std::thread::sleep(std::time::Duration::from_millis(500));
        let _ = tmux::tmux_kill_window(window_id.clone());
    }

    // Remove worktree (preserve branch)
    let project_dir = app_state.project_dir.clone();
    let _ = std::process::Command::new("git")
        .args(["worktree", "remove", "--force", &spiderling.worktree_path])
        .current_dir(&project_dir)
        .env(
            "PATH",
            format!(
                "/opt/homebrew/bin:/usr/local/bin:{}",
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .output();

    // Clean up launcher and config files
    let home = home();
    let _ = fs::remove_file(format!("{}/.aperture/launchers/{}.sh", home, name));
    let _ = fs::remove_file(format!("{}/.aperture/launchers/{}-prompt.txt", home, name));
    let _ = fs::remove_file(format!("/tmp/aperture-mcp-{}.json", name));

    app_state.spiderlings.remove(&name);
    write_active_spiderlings(app_state);

    Ok(())
}

#[tauri::command]
pub fn list_spiderlings(
    state: tauri::State<'_, Arc<Mutex<AppState>>>,
) -> Result<Vec<SpiderlingDef>, String> {
    let app_state = state.lock().map_err(|e| e.to_string())?;
    Ok(app_state.spiderlings.values().cloned().collect())
}

#[tauri::command]
pub fn kill_spiderling_cmd(
    name: String,
    state: tauri::State<'_, Arc<Mutex<AppState>>>,
) -> Result<(), String> {
    let mut app_state = state.lock().map_err(|e| e.to_string())?;
    kill_spiderling(name, &mut app_state)
}
```

- [ ] **Step 2: Add `regex` dependency to Cargo.toml**

Add to `[dependencies]` in `src-tauri/Cargo.toml`:
```toml
regex = "1"
```

- [ ] **Step 3: Register module and commands in lib.rs**

Add `mod spawner;` to the top of `lib.rs`.

Add to the `invoke_handler` list:
```rust
spawner::list_spiderlings,
spawner::kill_spiderling_cmd,
```

- [ ] **Step 4: Build**

Run: `cargo build --manifest-path src-tauri/Cargo.toml`
Expected: Compiles

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/spawner.rs src-tauri/src/lib.rs src-tauri/Cargo.toml
git commit -m "feat: add spiderling spawner module (worktree, tmux, launcher, lifecycle)"
```

### Task 6: Spawn/kill mailbox polling

**Files:**
- Modify: `src-tauri/src/poller.rs`

- [ ] **Step 1: Add spawn mailbox handling to the poller loop**

In `poller.rs`, add at the start of the `loop` block (before warroom handling):

```rust
// ── Handle spawn requests ──
{
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
                    let name = req.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let task_id = req.get("task_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let prompt = req.get("prompt").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    let requested_by = req.get("requested_by").and_then(|v| v.as_str()).unwrap_or("").to_string();

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
}

// ── Handle kill requests ──
{
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
}
```

- [ ] **Step 2: Add spiderling message routing to agent-bound section**

In the agent-bound messages section, after collecting `agents` from `app_state.agents`, also collect spiderlings:

```rust
// Also include spiderlings as message recipients
let spiderling_agents: Vec<(String, String)> = {
    let Ok(app_state) = state.lock() else { continue };
    app_state
        .spiderlings
        .values()
        .filter(|s| s.status == "working")
        .filter_map(|s| {
            s.tmux_window_id
                .as_ref()
                .map(|wid| (s.name.clone(), wid.clone()))
        })
        .collect()
};

// Combine and iterate over both
let all_agents: Vec<(String, String)> = agents.into_iter().chain(spiderling_agents).collect();
```

Then change `for (agent_name, window_id) in &agents` to `for (agent_name, window_id) in &all_agents`.

- [ ] **Step 3: Build**

Run: `cargo build --manifest-path src-tauri/Cargo.toml`
Expected: Compiles

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/poller.rs
git commit -m "feat: add spawn/kill mailbox polling and spiderling message routing"
```

---

## Chunk 3: Spiderling MCP Tools

### Task 7: Spawner MCP module

**Files:**
- Create: `mcp-server/src/spawner.ts`

- [ ] **Step 1: Create spawner.ts**

```typescript
// mcp-server/src/spawner.ts
import { writeFileSync, readFileSync, mkdirSync } from "node:fs";
import { join, resolve } from "node:path";
import { homedir } from "node:os";

const MAILBOX_BASE = resolve(
  process.env.APERTURE_MAILBOX ?? join(homedir(), ".aperture", "mailbox"),
);

const NAME_RE = /^[a-z0-9][a-z0-9-]{0,30}$/;
const PERMANENT_NAMES = ["glados", "wheatley", "peppy", "izzy", "operator", "warroom"];

export interface SpiderlingInfo {
  name: string;
  task_id: string;
  tmux_window_id: string | null;
  worktree_path: string;
  worktree_branch: string;
  requested_by: string;
  status: string;
  spawned_at: string;
}

function activeSpiderlingsPath(): string {
  return resolve(homedir(), ".aperture", "active-spiderlings.json");
}

export function readActiveSpiderlings(): SpiderlingInfo[] {
  try {
    const data = readFileSync(activeSpiderlingsPath(), "utf-8");
    return JSON.parse(data);
  } catch {
    return [];
  }
}

export function isValidRecipient(name: string): boolean {
  if (PERMANENT_NAMES.includes(name)) return true;
  const spiderlings = readActiveSpiderlings();
  return spiderlings.some((s) => s.name === name);
}

export function requestSpawn(
  name: string,
  taskId: string,
  prompt: string,
  requestedBy: string,
): string {
  if (!NAME_RE.test(name)) {
    throw new Error(
      `Invalid spiderling name '${name}'. Must match [a-z0-9][a-z0-9-]{0,30}`,
    );
  }
  if (PERMANENT_NAMES.includes(name)) {
    throw new Error(`Name '${name}' conflicts with a permanent agent`);
  }
  const existing = readActiveSpiderlings();
  if (existing.some((s) => s.name === name)) {
    throw new Error(`Spiderling '${name}' already exists`);
  }

  const spawnDir = join(MAILBOX_BASE, "_spawn");
  mkdirSync(spawnDir, { recursive: true });

  const timestamp = Date.now();
  const filename = `${timestamp}-${name}.json`;
  const request = { name, task_id: taskId, prompt, requested_by: requestedBy, timestamp: String(timestamp) };
  writeFileSync(join(spawnDir, filename), JSON.stringify(request, null, 2));
  return name;
}

export function requestKill(name: string): void {
  const killDir = join(MAILBOX_BASE, "_kill");
  mkdirSync(killDir, { recursive: true });
  const timestamp = Date.now();
  writeFileSync(join(killDir, `${timestamp}-${name}.txt`), name);
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cd /Users/<your-username>/projetos/aperture/mcp-server && npx tsc --noEmit`
Expected: No errors

- [ ] **Step 3: Commit**

```bash
git add mcp-server/src/spawner.ts
git commit -m "feat: add spawner MCP module for spiderling requests"
```

### Task 8: Register spawning MCP tools + dynamic recipients

**Files:**
- Modify: `mcp-server/src/index.ts`

- [ ] **Step 1: Add imports**

```typescript
import { requestSpawn, requestKill, readActiveSpiderlings, isValidRecipient } from "./spawner.js";
```

- [ ] **Step 2: Replace hardcoded VALID_RECIPIENTS check in send_message**

Replace:
```typescript
if (!VALID_RECIPIENTS.includes(target)) {
```
With:
```typescript
if (!isValidRecipient(target)) {
```

Update the error message to include dynamic recipients:
```typescript
const spiderlingNames = readActiveSpiderlings().map(s => s.name);
const allRecipients = [...VALID_RECIPIENTS, ...spiderlingNames];
return {
  content: [{
    type: "text",
    text: `ERROR: Unknown recipient "${to}". Valid recipients are: ${allRecipients.join(", ")}. Use "operator" to message the human.`,
  }],
  isError: true,
};
```

- [ ] **Step 3: Add role check helper**

```typescript
const agentRole = process.env.AGENT_ROLE ?? "agent";

function requireRole(required: string): void {
  if (agentRole !== required) {
    throw new Error(`This tool requires the '${required}' role. You are '${agentRole}'.`);
  }
}
```

- [ ] **Step 4: Register spawn/kill/list tools**

```typescript
server.tool(
  "spawn_spiderling",
  "Spawn an ephemeral Claude Code worker in a git worktree. Orchestrator only.",
  {
    name: z.string().describe("Spiderling name (lowercase alphanumeric + hyphens, e.g. 'spider-auth')"),
    task_id: z.string().describe("BEADS task ID this spiderling will work on"),
    prompt: z.string().describe("Task description and instructions for the spiderling"),
  },
  async ({ name, task_id, prompt }) => {
    try {
      requireRole("orchestrator");
      const result = requestSpawn(name, task_id, prompt, AGENT_NAME!);
      return { content: [{ type: "text", text: `Spawn request submitted for '${result}'. It will appear shortly.` }] };
    } catch (e: any) {
      return { content: [{ type: "text", text: `ERROR: ${e.message}` }], isError: true };
    }
  }
);

server.tool(
  "list_spiderlings",
  "List all active spiderlings and their status.",
  {},
  async () => {
    const spiderlings = readActiveSpiderlings();
    if (spiderlings.length === 0) {
      return { content: [{ type: "text", text: "No active spiderlings." }] };
    }
    const summary = spiderlings
      .map((s) => `${s.name} | task: ${s.task_id} | status: ${s.status} | by: ${s.requested_by}`)
      .join("\n");
    return { content: [{ type: "text", text: summary }] };
  }
);

server.tool(
  "kill_spiderling",
  "Kill a spiderling and clean up its worktree. Orchestrator only.",
  {
    name: z.string().describe("Spiderling name to kill"),
  },
  async ({ name }) => {
    try {
      requireRole("orchestrator");
      requestKill(name);
      return { content: [{ type: "text", text: `Kill request submitted for '${name}'.` }] };
    } catch (e: any) {
      return { content: [{ type: "text", text: `ERROR: ${e.message}` }], isError: true };
    }
  }
);
```

- [ ] **Step 5: Build**

Run: `cd /Users/<your-username>/projetos/aperture/mcp-server && npm run build`
Expected: Compiles

- [ ] **Step 6: Commit**

```bash
git add mcp-server/src/index.ts
git commit -m "feat: add spawn/kill/list spiderling MCP tools with role enforcement"
```

---

## Chunk 4: Frontend — Tasks Panel + Spiderling UI

### Task 9: Tauri commands for BEADS + spiderlings

**Files:**
- Modify: `src-tauri/src/lib.rs` (already has spawner commands)
- Create: `src-tauri/src/beads.rs`

- [ ] **Step 1: Create beads.rs with list_beads_tasks command**

```rust
// src-tauri/src/beads.rs
use std::process::Command;

fn bd_cmd() -> Command {
    let mut c = Command::new("bd");
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let current_path = std::env::var("PATH").unwrap_or_default();
    c.env("PATH", format!("/opt/homebrew/bin:/usr/local/bin:{}", current_path));
    c.env("BEADS_DIR", format!("{}/.aperture/.beads", home));
    c
}

#[tauri::command]
pub fn list_beads_tasks() -> Result<serde_json::Value, String> {
    let output = bd_cmd()
        .args(["list", "--json"])
        .output()
        .map_err(|e| format!("Failed to run bd: {}", e))?;

    if !output.status.success() {
        return Ok(serde_json::json!([]));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(&stdout).unwrap_or(Ok(serde_json::json!([])))
        .map_err(|e| e.to_string())
}
```

- [ ] **Step 2: Register in lib.rs**

Add `mod beads;` and add `beads::list_beads_tasks` to the invoke handler.

- [ ] **Step 3: Build**

Run: `cargo build --manifest-path src-tauri/Cargo.toml`
Expected: Compiles

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/beads.rs src-tauri/src/lib.rs
git commit -m "feat: add Tauri command for listing BEADS tasks"
```

### Task 10: Frontend types + tauri-commands

**Files:**
- Modify: `src/types.ts`
- Modify: `src/services/tauri-commands.ts`

- [ ] **Step 1: Add types**

```typescript
// Add to src/types.ts
export interface SpiderlingDef {
  name: string;
  task_id: string;
  tmux_window_id: string | null;
  worktree_path: string;
  worktree_branch: string;
  requested_by: string;
  status: string;
  spawned_at: string;
}

export interface BeadsTask {
  id: string;
  title: string;
  status: string;
  priority: number;
  assignee?: string;
  description?: string;
  notes?: string;
  [key: string]: any;
}
```

- [ ] **Step 2: Add Tauri commands**

```typescript
// Add to src/services/tauri-commands.ts
listSpiderlings: () => invoke<SpiderlingDef[]>("list_spiderlings"),
killSpiderling: (name: string) => invoke<void>("kill_spiderling_cmd", { name }),
listBeadsTasks: () => invoke<any>("list_beads_tasks"),
```

Add `SpiderlingDef` to the import from `"../types"`.

- [ ] **Step 3: Commit**

```bash
git add src/types.ts src/services/tauri-commands.ts
git commit -m "feat: add spiderling and BEADS types + Tauri commands"
```

### Task 11: Tasks panel component

**Files:**
- Create: `src/components/TasksPanel.ts`

- [ ] **Step 1: Create TasksPanel.ts**

```typescript
// src/components/TasksPanel.ts
import { commands } from "../services/tauri-commands";

export function createTasksPanel(container: HTMLElement) {
  container.innerHTML = `
    <div class="tasks-panel">
      <div class="section-title">BEADS Tasks</div>
      <div class="tasks-panel__list"></div>
    </div>
  `;

  const listEl = container.querySelector(".tasks-panel__list") as HTMLElement;

  async function refresh() {
    try {
      const tasks = await commands.listBeadsTasks();
      if (!Array.isArray(tasks) || tasks.length === 0) {
        listEl.innerHTML = '<div style="color: var(--text-secondary); font-size: 12px; padding: 8px;">No tasks yet.</div>';
        return;
      }

      listEl.innerHTML = tasks
        .map((t: any) => {
          const statusColor =
            t.status === "closed" ? "var(--accent-green)" :
            t.status === "in_progress" ? "var(--accent-orange)" :
            "var(--text-secondary)";
          const priorityLabel = t.priority !== undefined ? `P${t.priority}` : "";
          const assignee = t.assignee ? ` · ${t.assignee}` : "";
          const notes = t.notes ?? "";
          const artifacts = notes
            .split("\n")
            .filter((l: string) => l.startsWith("artifact:"))
            .map((l: string) => {
              const [, type, ...rest] = l.split(":");
              const value = rest.join(":");
              return `<div class="tasks-panel__artifact">${type}: ${value}</div>`;
            })
            .join("");

          return `
            <div class="tasks-panel__task">
              <div class="tasks-panel__task-header">
                <span class="tasks-panel__task-id">${t.id}</span>
                <span class="tasks-panel__task-priority">${priorityLabel}</span>
                <span class="tasks-panel__task-status" style="color: ${statusColor}">${t.status ?? "open"}</span>
              </div>
              <div class="tasks-panel__task-title">${t.title ?? ""}</div>
              <div class="tasks-panel__task-meta">${t.assignee ?? "unassigned"}${assignee ? "" : ""}</div>
              ${artifacts ? `<div class="tasks-panel__artifacts">${artifacts}</div>` : ""}
            </div>
          `;
        })
        .join("");
    } catch {
      listEl.innerHTML = '<div style="color: var(--text-secondary); font-size: 12px; padding: 8px;">BEADS not available.</div>';
    }
  }

  refresh();
  setInterval(refresh, 5000);

  return { refresh };
}
```

- [ ] **Step 2: Commit**

```bash
git add src/components/TasksPanel.ts
git commit -m "feat: add Tasks panel component for BEADS tasks"
```

### Task 12: Spiderling agent cards + UI wiring

**Files:**
- Modify: `src/components/AgentList.ts`
- Modify: `src/components/AgentCard.ts`
- Modify: `src/main.ts`
- Modify: `index.html`
- Modify: `src/style.css`

- [ ] **Step 1: Update AgentCard.ts to handle spiderling role**

In the `AGENT_THEME` object (or equivalent), add a default spiderling theme:

```typescript
const SPIDERLING_THEME = { icon: "🕷️", color: "#95a5a6" };
```

Update the card rendering to use this theme when `agent.role === "spiderling"`, and add a kill button instead of stop button for spiderlings.

- [ ] **Step 2: Update AgentList.ts to also show spiderlings**

After listing named agents, call `commands.listSpiderlings()` and render them as additional agent cards below a "SPIDERLINGS" section title.

- [ ] **Step 3: Add Tasks button to index.html**

In `#navbar-actions`, add:
```html
<button class="navbar__btn" data-panel="tasks">Tasks</button>
```

Add inside `#right-panel`:
```html
<div id="panel-tasks" class="panel-view hidden"></div>
```

- [ ] **Step 4: Wire up in main.ts**

Add imports for `createTasksPanel`. Get `panelTasks` element. Update `togglePanel` to handle `"tasks"`. Call `createTasksPanel(panelTasks)`.

- [ ] **Step 5: Add CSS for tasks panel and spiderling badge**

```css
/* ── Tasks Panel ── */
.tasks-panel {
  padding: 12px;
  overflow-y: auto;
}

.tasks-panel__list {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.tasks-panel__task {
  padding: 10px;
  background: var(--bg-card);
  border: 1px solid var(--border);
  border-radius: var(--radius);
}

.tasks-panel__task-header {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 4px;
}

.tasks-panel__task-id {
  font-family: monospace;
  font-size: 11px;
  color: var(--accent-orange);
}

.tasks-panel__task-priority {
  font-size: 10px;
  font-weight: 700;
  color: var(--accent-red);
}

.tasks-panel__task-status {
  font-size: 11px;
  margin-left: auto;
}

.tasks-panel__task-title {
  font-size: 13px;
  font-weight: 600;
  color: var(--text-primary);
  margin-bottom: 2px;
}

.tasks-panel__task-meta {
  font-size: 11px;
  color: var(--text-secondary);
}

.tasks-panel__artifacts {
  margin-top: 6px;
  padding-top: 6px;
  border-top: 1px solid var(--border);
}

.tasks-panel__artifact {
  font-size: 11px;
  color: var(--accent-teal);
  font-family: monospace;
  padding: 2px 0;
}

/* Spiderling role badge */
.agent-mini[data-role="spiderling"] {
  --agent-color: #95a5a6;
  border-style: dashed;
}
```

- [ ] **Step 6: Build frontend**

Run: `cd /Users/<your-username>/projetos/aperture && pnpm build`
Expected: Compiles

- [ ] **Step 7: Commit**

```bash
git add src/components/AgentCard.ts src/components/AgentList.ts src/main.ts index.html src/style.css
git commit -m "feat: add Tasks panel, spiderling cards, and UI wiring"
```

---

## Chunk 5: Agent Prompts + Final Integration

### Task 13: Update GLaDOS prompt with spawning capabilities

**Files:**
- Modify: `prompts/glados.md`

- [ ] **Step 1: Add spawning section to GLaDOS prompt**

After the "Operating Principles" section, add:

```markdown
# BEADS Task Tracking

You have access to BEADS, a task/artifact tracking system. Use it to:
- Create tasks for work items: `create_task(title, priority, description)`
- Track progress: `update_task(id, claim/status/notes)`
- Close completed work: `close_task(id, reason)`
- Query what exists: `query_tasks(mode: "list"|"ready"|"show", id?)`
- Store deliverables: `store_artifact(task_id, type: "file"|"pr"|"session"|"url"|"note", value)`
- Search: `search_tasks(label?)`

Always create BEADS tasks for work you delegate. This creates a paper trail the operator can inspect.

# Spiderling Spawning

You can spawn **spiderlings** — ephemeral Claude Code workers that run in isolated git worktrees.

- `spawn_spiderling(name, task_id, prompt)` — Spin up a worker. Give it a clear name (e.g., "spider-auth") and detailed instructions.
- `list_spiderlings()` — Check on your workers.
- `kill_spiderling(name)` — Clean up a finished worker (only when the operator says to).

**Workflow:**
1. Receive a plan from Wheatley/operator
2. Break it into BEADS tasks with `create_task`
3. Spawn spiderlings for each task with `spawn_spiderling`
4. Monitor progress — spiderlings will message you when done
5. Collect results, verify quality, report to operator

**Rules:**
- Spiderlings work in git worktrees — no branch conflicts with the main codebase
- Each spiderling gets one BEADS task — keep scope focused
- Spiderlings communicate with you via `send_message(to: "glados", ...)`
- You communicate with them via `send_message(to: "spider-name", ...)`
- Do NOT kill spiderlings yourself — the operator will tell you when to clean up
- If a spiderling seems stuck, message it to check on progress
```

- [ ] **Step 2: Commit**

```bash
git add prompts/glados.md
git commit -m "feat: update GLaDOS prompt with BEADS and spiderling capabilities"
```

### Task 14: Update all agent prompts with BEADS tools

**Files:**
- Modify: `prompts/wheatley.md`
- Modify: `prompts/peppy.md`
- Modify: `prompts/izzy.md`

- [ ] **Step 1: Add BEADS section to each agent prompt**

Add to each agent's prompt (after communication section):

```markdown
# BEADS Task Tracking

You have access to BEADS for tracking tasks and artifacts:
- `query_tasks(mode: "list"|"ready"|"show", id?)` — See what tasks exist
- `update_task(id, claim/status/notes)` — Claim or update a task you're working on
- `close_task(id, reason)` — Mark a task as done
- `store_artifact(task_id, type: "file"|"pr"|"session"|"url"|"note", value)` — Attach deliverables
- `search_tasks(label?)` — Find tasks by label
- `create_task(title, priority, description)` — Create new tasks if needed

When assigned a task, claim it first with `update_task(id, claim: true)`. When done, store artifacts and close it.
```

- [ ] **Step 2: Commit**

```bash
git add prompts/wheatley.md prompts/peppy.md prompts/izzy.md
git commit -m "feat: add BEADS tool instructions to all agent prompts"
```

### Task 15: End-to-end verification

- [ ] **Step 1: Build everything**

```bash
cd /Users/<your-username>/projetos/aperture
cd mcp-server && npm run build && cd ..
cargo build --manifest-path src-tauri/Cargo.toml
pnpm build
```
Expected: All three compile without errors.

- [ ] **Step 2: Verify BEADS init works**

```bash
BEADS_DIR=~/.aperture/.beads bd init --quiet
BEADS_DIR=~/.aperture/.beads bd create "Test task" -p 2 --json
BEADS_DIR=~/.aperture/.beads bd list --json
```
Expected: Task created and listed.

- [ ] **Step 3: Clean up test data**

```bash
BEADS_DIR=~/.aperture/.beads bd list --json
# Close the test task
```

- [ ] **Step 4: Run dev mode**

```bash
pnpm tauri dev
```

Verify:
- App launches, navbar shows "Tasks" button
- Tasks panel opens and shows BEADS tasks (or "No tasks yet")
- Agent cards still work, start/stop agents
- No console errors

- [ ] **Step 5: Final commit**

```bash
git add -A
git commit -m "feat: BEADS integration + spiderling spawning complete"
```
