# Spiderling System — Implementation Guide

> *"You are a spiderling named spider-doc-spiderlings, working for GLaDOS in the Aperture system."*
>
> This document was written by a spiderling, about spiderlings. The recursion is intentional.

---

## 1. Overview

**Spiderlings** are ephemeral Claude Code workers spawned on-demand by orchestrators (primarily GLaDOS) to handle isolated subtasks. Each spiderling:

- Gets its own **git worktree** — a separate checkout of a repo at a new branch, completely isolated from the main working tree and other spiderlings
- Runs inside a **dedicated tmux window** in the main Aperture session
- Has its own **MCP config** with hardcoded identity (`AGENT_NAME`, `AGENT_ROLE=spiderling`)
- Is given a **BEADS task ID** and communicates progress exclusively through BEADS task updates
- Is cleaned up completely on kill — worktree removed, launcher scripts deleted, registry updated

### Why Spiderlings Exist

The core insight: parallelism requires isolation. If two agents edit the same file in the same working directory, they collide. Spiderlings solve this with git worktrees — each worker sees the same repo history but writes to its own branch and filesystem path. Meanwhile the orchestrator can spawn many workers simultaneously without any contention.

### The Isolation Model

```
Main repo:    /Users/user/projects/aperture  (master branch)
Spiderling A: ~/.aperture/worktrees/spider-auth  (branch: spider-auth)
Spiderling B: ~/.aperture/worktrees/spider-ui    (branch: spider-ui)
Spiderling C: ~/.aperture/worktrees/spider-tests (branch: spider-tests)
```

Each worktree has its full own filesystem. They share git history (so they start from the same commit) but can never touch each other's working files.

---

## 2. Spawn Flow

The full spawn sequence involves five components: the MCP tool, the spawn request file, the Tauri poller, the Rust spawner, and tmux.

```
Orchestrator (Claude)
  │
  ├─ calls MCP tool: spawn_spiderling(name, task_id, prompt, project_path?)
  │
MCP Server (mcp-server/src/spawner.ts)
  │
  ├─ validates name (regex + conflict checks)
  ├─ writes ~/.aperture/mailbox/_spawn/{timestamp}-{name}.json
  │
Tauri Poller (src-tauri/src/poller.rs) — runs every 5s
  │
  ├─ reads all .json files in ~/.aperture/mailbox/_spawn/
  ├─ calls spawner::spawn_spiderling() for each
  ├─ deletes the request file
  │
Rust Spawner (src-tauri/src/spawner.rs)
  │
  ├─ validates name (in-memory state check)
  ├─ resolves repo dir (project_path or Aperture default)
  ├─ git worktree add -b {name} ~/.aperture/worktrees/{name}
  ├─ tmux_create_window(name) → window_id (@N)
  ├─ creates ~/.aperture/mailbox/{name}/ (spiderling mailbox)
  ├─ writes /tmp/aperture-mcp-{name}.json (MCP config)
  ├─ writes ~/.aperture/launchers/{name}-prompt.txt (system prompt)
  ├─ writes ~/.aperture/launchers/{name}.sh (launcher script)
  ├─ tmux_send_keys(window_id, launcher_path) → starts the script
  ├─ spawns thread → waits 6s → sends "Begin your task now..."
  ├─ registers SpiderlingDef in app_state.spiderlings
  └─ writes ~/.aperture/active-spiderlings.json
```

### Timing Notes

The spawner thread that sends the initial message waits:
1. 3× 2-second intervals (6s total) pressing Enter to dismiss workspace trust prompts
2. Additional 3-second wait for Claude Code to fully boot
3. Then sends: `"Begin your task now. Read your system prompt carefully for full instructions."`

---

## 3. Git Worktree Management

### Creation

```rust
// From src-tauri/src/spawner.rs

let worktree_dir = format!("{}/.aperture/worktrees", home);
let worktree_path = format!("{}/{}", worktree_dir, name);
let branch_name = name.clone();  // branch name == spiderling name

// Primary attempt: create new branch
let output = std::process::Command::new("git")
    .args(["worktree", "add", "-b", &branch_name, &worktree_path])
    .current_dir(&repo_dir)
    .env("PATH", &path_env())
    .output()?;

// Fallback: branch already exists, use it directly
if !output.status.success() {
    std::process::Command::new("git")
        .args(["worktree", "add", &worktree_path, &branch_name])
        .current_dir(&repo_dir)
        .output()?;
}
```

The worktree is always created inside `~/.aperture/worktrees/` regardless of which repo is being used. This centralizes all spiderling workspaces under one location.

### Repo Selection

The `project_path` parameter determines which repo the worktree branches from:

```rust
let repo_dir = match &project_path {
    Some(p) => {
        // Expand ~/... paths, verify it's a git repo
        let expanded = expand_home(p);
        git_rev_parse_check(&expanded)?;
        expanded
    }
    None => app_state.project_dir.clone(), // Aperture itself
};
```

Examples from `active-spiderlings.json`:
- `"source_repo": "/Users/<your-username>/projects/aperture"` — working on Aperture
- `"source_repo": "/Users/<your-username>/gt/chat_waha"` — working on a different project

### Branching Strategy

- Branch name equals spiderling name: `spider-doc-spiderlings` → branch `spider-doc-spiderlings`
- Branch starts from whatever `HEAD` the source repo is on at spawn time
- The branch is **preserved** on kill (only the worktree directory is removed), allowing the orchestrator to merge or cherry-pick the work later

### Isolation Guarantees

- Each worktree has its own working directory — file writes in one never affect another
- `git status` in one worktree never shows changes from another
- `node_modules`, build artifacts, and temp files are all per-worktree
- Spiderlings are told their worktree path in the system prompt: *"Work in this git worktree at {path} — do NOT switch branches or leave this directory."*

---

## 4. MCP Config Generation

Each spiderling gets a unique MCP config written to `/tmp/aperture-mcp-{name}.json`. This file sets the spiderling's identity in the MCP server.

### Generated Config Structure

```json
{
  "mcpServers": {
    "aperture-bus": {
      "type": "stdio",
      "command": "node",
      "args": ["/path/to/aperture/mcp-server/dist/index.js"],
      "env": {
        "AGENT_NAME": "spider-doc-spiderlings",
        "AGENT_ROLE": "spiderling",
        "AGENT_MODEL": "sonnet",
        "APERTURE_MAILBOX": "/Users/user/.aperture/mailbox",
        "BEADS_DIR": "/Users/user/.aperture/.beads",
        "BD_ACTOR": "spider-doc-spiderlings"
      }
    }
  }
}
```

### Key Environment Variables

| Variable | Value | Purpose |
|----------|-------|---------|
| `AGENT_NAME` | spiderling name | Identity for messaging and BEADS operations |
| `AGENT_ROLE` | `"spiderling"` | Role-based tool restrictions (orchestrator-only tools are blocked) |
| `AGENT_MODEL` | `"sonnet"` | Model identifier |
| `APERTURE_MAILBOX` | `~/.aperture/mailbox` | Base directory for file-based mailboxes |
| `BEADS_DIR` | `~/.aperture/.beads` | BEADS database directory |
| `BD_ACTOR` | spiderling name | BEADS actor identity for audit trail |

### Role Restrictions

The `AGENT_ROLE=spiderling` value prevents spiderlings from spawning other spiderlings. In `mcp-server/src/index.ts`:

```typescript
function requireRole(required: string): void {
  if (agentRole !== required) {
    throw new Error(`This tool requires the '${required}' role. You are '${agentRole}'.`);
  }
}

// spawn_spiderling and kill_spiderling both call:
requireRole("orchestrator");
```

Spiderlings have access to all BEADS task tools (`create_task`, `update_task`, `close_task`, `store_artifact`, `query_tasks`) but cannot spawn or kill other spiderlings.

---

## 5. Launcher Scripts

Each spiderling gets a shell script at `~/.aperture/launchers/{name}.sh`.

### Script Template (Rust source)

```rust
// From src-tauri/src/spawner.rs

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
```

### Real Example (spider-doc-spiderlings)

```bash
#!/bin/bash
export PATH="/opt/homebrew/bin:/usr/local/bin:$PATH"
cd "/Users/<your-username>/.aperture/worktrees/spider-doc-spiderlings"
PROMPT=$(cat "/Users/<your-username>/.aperture/launchers/spider-doc-spiderlings-prompt.txt")
exec claude --dangerously-skip-permissions --model sonnet --system-prompt "$PROMPT" --mcp-config /tmp/aperture-mcp-spider-doc-spiderlings.json --name spider-doc-spiderlings
```

### Claude Code CLI Flags Used

| Flag | Value | Purpose |
|------|-------|---------|
| `--dangerously-skip-permissions` | (boolean) | Disables all permission prompts — spiderlings run fully autonomously |
| `--model` | `sonnet` | Uses Claude Sonnet (not opus) for cost efficiency |
| `--system-prompt` | `"$PROMPT"` | Injects the spiderling's task prompt as system prompt |
| `--mcp-config` | `/tmp/aperture-mcp-{name}.json` | Per-spiderling MCP configuration |
| `--name` | spiderling name | Sets the Claude Code session name (visible in tmux title) |

The script uses `exec` to replace the shell process with the `claude` process, keeping the tmux window clean.

After writing the file, the spawner makes it executable:
```rust
std::process::Command::new("chmod")
    .args(["+x", &launcher_path])
    .output()?;
```

The launcher is then executed by sending its path to the tmux window:
```rust
tmux::tmux_send_keys(window_id.clone(), launcher_path)?;
```

---

## 6. System Prompt Injection

The spiderling's task prompt is written to `~/.aperture/launchers/{name}-prompt.txt` as a full system prompt string, then read by the launcher script into `$PROMPT` and passed to `claude --system-prompt`.

### System Prompt Template (Rust source)

```rust
// From src-tauri/src/spawner.rs

let system_prompt = format!(
    "You are a spiderling named {name}, working for GLaDOS in the Aperture system.\n\
     Your task is tracked in BEADS issue {task_id}.\n\
     Work in this git worktree at {worktree_path} — do NOT switch branches or leave this directory.\n\n\
     ## Communication — use BEADS, not send_message\n\
     Do NOT use send_message(to: 'glados') — those messages get lost.\n\
     Instead, communicate through BEADS task updates:\n\
     - Progress updates: update_task(id: '{task_id}', notes: 'what you found/did')\n\
     - Store deliverables: store_artifact(task_id: '{task_id}', type: 'file'|'note', value: '...')\n\
     - When done: update_task(id: '{task_id}', status: 'done', notes: 'summary of what was done')\n\
     GLaDOS polls BEADS — your updates will be seen reliably.\n\n\
     ## War Room\n\
     If you receive a War Room context (starts with '# WAR ROOM'), you MUST:\n\
     1. Pause your current work\n\
     2. Read the transcript carefully\n\
     3. Respond using: send_message(to: 'warroom', message: 'your contribution')\n\
     4. Do NOT reply in the terminal — use the send_message MCP tool with to='warroom'\n\
     5. Return to your task after responding\n\n\
     TASK:\n{prompt}",
    name = name,
    task_id = task_id,
    worktree_path = worktree_path,
    prompt = prompt,
);
```

### What the Prompt Establishes

1. **Identity** — Who the spiderling is and who it works for
2. **Task reference** — Which BEADS issue to track progress against
3. **Workspace boundary** — Must stay in the worktree, must not switch branches
4. **Communication protocol** — Use BEADS task updates, not `send_message`
5. **War Room protocol** — How to respond if invited to a group discussion
6. **The actual task** — Appended after `TASK:\n` with the caller-supplied prompt

The prompt file is read at launch time (not embedded in the script), avoiding shell escaping issues with quotes, newlines, and special characters in task descriptions.

---

## 7. Active Registry

### File: `~/.aperture/active-spiderlings.json`

A JSON array of all currently active spiderlings. Written by the Rust spawner after every spawn or kill operation.

### Schema

```typescript
// From mcp-server/src/spawner.ts

interface SpiderlingInfo {
  name: string;           // Unique spiderling name
  task_id: string;        // BEADS task ID being worked on
  tmux_window_id: string | null;  // tmux window ID, e.g. "@8"
  worktree_path: string;  // Absolute path to worktree
  worktree_branch: string; // Git branch name (same as name)
  source_repo?: string;   // Absolute path to source repo (optional)
  requested_by: string;   // Which agent spawned this
  status: string;         // "working" | (other statuses)
  spawned_at: string;     // Unix milliseconds as string
}
```

### Real Example Entry

```json
{
  "name": "spider-doc-spiderlings",
  "task_id": "aperture-s6m",
  "tmux_window_id": "@11",
  "worktree_path": "/Users/<your-username>/.aperture/worktrees/spider-doc-spiderlings",
  "worktree_branch": "spider-doc-spiderlings",
  "source_repo": "/Users/<your-username>/projects/aperture",
  "requested_by": "glados",
  "status": "working",
  "spawned_at": "1774393562620"
}
```

### Who Reads This File

1. **MCP server** (`mcp-server/src/spawner.ts`) — `readActiveSpiderlings()` is called by:
   - `isValidRecipient()` — to validate `send_message` targets
   - `list_spiderlings` MCP tool — to show status to orchestrators
   - `requestSpawn()` — to prevent duplicate names before writing spawn requests

2. **Rust in-memory state** (`AppState.spiderlings`) — the authoritative source; the JSON file is a snapshot of this

3. **Frontend** — polls `list_spiderlings` Tauri command every 3 seconds for the UI

### Write Locations

```rust
// From src-tauri/src/spawner.rs

pub fn write_active_spiderlings(state: &AppState) {
    let spiderlings: Vec<&SpiderlingDef> = state.spiderlings.values().collect();
    if let Ok(json) = serde_json::to_string_pretty(&spiderlings) {
        let _ = fs::write(active_spiderlings_path(), json);
    }
}
```

Called after every `spawn_spiderling()` and `kill_spiderling()`.

---

## 8. Kill & Cleanup

### Kill Flow

```
Orchestrator calls: kill_spiderling(name)
  │
MCP Server (mcp-server/src/spawner.ts)
  │
  ├─ requireRole("orchestrator")
  ├─ writes ~/.aperture/mailbox/_kill/{timestamp}-{name}.txt
  │    content: just the name string
  │
Tauri Poller (src-tauri/src/poller.rs) — picks up within 5s
  │
  ├─ reads all files in ~/.aperture/mailbox/_kill/
  ├─ calls spawner::kill_spiderling(name)
  └─ deletes the kill request file
  │
Rust Spawner (src-tauri/src/spawner.rs) — kill_spiderling()
  │
  ├─ looks up spiderling in app_state.spiderlings
  ├─ sends C-c to tmux window (interrupt)
  ├─ waits 500ms
  ├─ sends /exit to tmux window (Claude Code exit command)
  ├─ waits 500ms
  ├─ kills the tmux window entirely
  ├─ git worktree remove --force {worktree_path}
  ├─ rm ~/.aperture/launchers/{name}.sh
  ├─ rm ~/.aperture/launchers/{name}-prompt.txt
  ├─ rm /tmp/aperture-mcp-{name}.json
  ├─ app_state.spiderlings.remove(name)
  └─ write_active_spiderlings()
```

### Kill Request File Format

```
~/.aperture/mailbox/_kill/1774393562620-spider-auth.txt
```

Content: just the spiderling name (`spider-auth\n`).

### Graceful Shutdown Sequence

```rust
// From src-tauri/src/spawner.rs

if let Some(ref window_id) = spiderling.tmux_window_id {
    let _ = tmux::tmux_send_keys(window_id.clone(), "C-c".into());  // interrupt
    std::thread::sleep(std::time::Duration::from_millis(500));
    let _ = tmux::tmux_send_keys(window_id.clone(), "/exit".into()); // Claude Code /exit
    std::thread::sleep(std::time::Duration::from_millis(500));
    let _ = tmux::tmux_kill_window(window_id.clone());               // force kill window
}
```

The sequence: interrupt any running tool call → ask Claude Code to exit cleanly → hard-kill the window if needed.

### What Survives Kill

- **The git branch** is preserved. `git worktree remove` removes the directory but not the branch. The orchestrator can inspect, merge, or cherry-pick commits from `spider-{name}` branch after the spiderling is gone.
- The **BEADS task** is not automatically closed — the spiderling should update it before completion, or the orchestrator closes it.

### What Is Removed

| File | Path |
|------|------|
| Worktree directory | `~/.aperture/worktrees/{name}/` |
| Launcher script | `~/.aperture/launchers/{name}.sh` |
| Prompt file | `~/.aperture/launchers/{name}-prompt.txt` |
| MCP config | `/tmp/aperture-mcp-{name}.json` |

---

## 9. BEADS Integration

### The Rule: No `send_message(to: 'glados')`

Spiderlings are explicitly told not to use `send_message` for task reporting. The reason: `send_message` to permanent agents delivers to their mailbox file, but GLaDOS is not guaranteed to be watching that mailbox at any given time. BEADS tasks, however, are polled regularly.

### Communication via Task Updates

The spiderling's system prompt teaches this protocol:

```
- Progress updates: update_task(id: '{task_id}', notes: 'what you found/did')
- Store deliverables: store_artifact(task_id: '{task_id}', type: 'file'|'note', value: '...')
- When done: update_task(id: '{task_id}', status: 'done', notes: 'summary of what was done')
```

### Available BEADS Tools for Spiderlings

All tools in the MCP server are available to spiderlings **except** `spawn_spiderling` and `kill_spiderling` (blocked by `requireRole("orchestrator")`):

| Tool | Purpose |
|------|---------|
| `create_task` | Create subtasks if needed |
| `update_task` | Report progress, claim task, mark done |
| `close_task` | Close completed tasks |
| `query_tasks` | Find related tasks |
| `store_artifact` | Attach file paths, URLs, notes to tasks |
| `search_tasks` | Find tasks by label |
| `list_spiderlings` | See other active workers |
| `send_message` | Message other agents (permanent + active spiderlings) |
| `get_messages` | Read incoming BEADS messages |
| `mark_as_read` | Mark BEADS messages as read |
| `get_identity` | Confirm own name/role |

### Artifact Types

```typescript
type ArtifactType = "file" | "pr" | "session" | "url" | "note";
```

Spiderlings typically use:
- `"file"` — a file path they created or modified
- `"note"` — a text summary or finding
- `"pr"` — a PR URL if they create a pull request

### Lifecycle Pattern

```
1. Spiderling starts → claims task: update_task(id, claim: true)
2. Works → progress notes: update_task(id, notes: "found X, doing Y")
3. Completes → stores artifacts: store_artifact(task_id, type: "file", value: "path/to/file")
4. Done → marks done: update_task(id, status: "done", notes: "summary")
5. GLaDOS polls BEADS → sees status change → takes next action
```

---

## 10. Frontend UI

### SpiderlingsPanel (`src/components/SpiderlingsPanel.ts`)

The panel lives in the Aperture Tauri frontend and displays all active spiderlings with a 3-second refresh interval.

```typescript
// Polls every 3 seconds
const interval = setInterval(refresh, 3000);

// Only re-renders if the list actually changed (hash comparison)
const hash = spiderlings.map(s => `${s.name}:${s.status}`).join("|");
if (hash === lastHash) return;
```

**Rendered HTML per spiderling:**

```html
<div class="spiderling-row" data-window-id="@11">
  <div class="spiderling-row__info">
    <span class="spiderling-row__name">🕷️ spider-doc-spiderlings</span>
    <span class="spiderling-row__task">aperture-s6m</span>
    <span class="spiderling-row__status spiderling-row__status--working">working</span>
  </div>
  <button class="btn btn--tiny btn--danger spiderling-row__kill" data-kill="spider-doc-spiderlings">✖</button>
</div>
```

**Interactions:**
- **Click row** → switches the host tmux session to that spiderling's window (`tmuxSelectWindow(windowId)`)
- **Click ✖ button** → calls `killSpiderling(name)` → immediate UI refresh

### SpiderlingCard (`src/components/SpiderlingCard.ts`)

An alternative compact card component used elsewhere in the UI:

```typescript
export function createSpiderlingCard(
  spiderling: SpiderlingDef,
  onRefresh: () => void,
): HTMLElement {
  const card = document.createElement("div");
  card.className = "agent-mini agent-mini--running";
  card.dataset.role = "spiderling";
  card.style.setProperty("--agent-color", "#95a5a6"); // gray, vs named agents' colors
  // ...
}
```

Shows: spider emoji + name + task_id + kill button. Click card → switch to tmux window.

### Tauri Commands Used

Both components call Tauri IPC commands defined in Rust:

```typescript
// list all active spiderlings
const spiderlings = await commands.listSpiderlings();

// kill a named spiderling
await commands.killSpiderling(name);

// switch tmux to spiderling's window
await commands.tmuxSelectWindow(windowId);
```

---

## 11. File Paths Reference

### Permanent Directories

| Path | Purpose |
|------|---------|
| `~/.aperture/worktrees/` | All spiderling git worktrees |
| `~/.aperture/launchers/` | Launcher scripts and prompt files |
| `~/.aperture/mailbox/` | Agent mailboxes (file-based) |
| `~/.aperture/mailbox/_spawn/` | Spawn request queue |
| `~/.aperture/mailbox/_kill/` | Kill request queue |
| `~/.aperture/mailbox/{name}/` | Per-spiderling mailbox |
| `~/.aperture/.beads/` | BEADS task database |

### Per-Spiderling Files

| Path | Created by | Removed on kill |
|------|-----------|-----------------|
| `~/.aperture/worktrees/{name}/` | Rust spawner (`git worktree add`) | Yes |
| `~/.aperture/launchers/{name}.sh` | Rust spawner | Yes |
| `~/.aperture/launchers/{name}-prompt.txt` | Rust spawner | Yes |
| `/tmp/aperture-mcp-{name}.json` | Rust spawner | Yes |
| `~/.aperture/mailbox/{name}/` | Rust spawner | No (mailbox preserved) |

### Shared State Files

| Path | Written by | Read by |
|------|-----------|---------|
| `~/.aperture/active-spiderlings.json` | Rust spawner | MCP server, Frontend |
| `~/.aperture/mailbox/_spawn/{ts}-{name}.json` | MCP server | Rust poller |
| `~/.aperture/mailbox/_kill/{ts}-{name}.txt` | MCP server | Rust poller |

### Spawn Request JSON Format

```json
{
  "name": "spider-auth",
  "task_id": "aperture-abc",
  "prompt": "Implement JWT authentication...",
  "requested_by": "glados",
  "timestamp": "1774393562620",
  "project_path": "~/projects/my-app"
}
```

`project_path` is optional. If omitted, the Aperture repo is used.

---

## 12. Name Validation

Spiderling names must pass a regex check at two layers:

### MCP Layer (TypeScript)

```typescript
const NAME_RE = /^[a-z0-9][a-z0-9-]{0,30}$/;
const PERMANENT_NAMES = ["glados", "wheatley", "peppy", "izzy", "operator", "warroom"];

if (!NAME_RE.test(name)) throw new Error(`Invalid name...`);
if (PERMANENT_NAMES.includes(name)) throw new Error(`Name conflicts...`);
if (existing.some(s => s.name === name)) throw new Error(`Already exists`);
```

### Rust Layer (in-memory state)

```rust
const PERMANENT_AGENTS: &[&str] = &["glados", "wheatley", "peppy", "izzy", "operator", "warroom"];

fn validate_name(name: &str, state: &AppState) -> Result<(), String> {
    let re = Regex::new(r"^[a-z0-9][a-z0-9-]{0,30}$").unwrap();
    if !re.is_match(name) {
        return Err(format!("Invalid spiderling name '{}'...", name));
    }
    if PERMANENT_AGENTS.contains(&name) || state.agents.contains_key(name) {
        return Err(format!("Name '{}' conflicts with a permanent agent", name));
    }
    if state.spiderlings.contains_key(name) {
        return Err(format!("Spiderling '{}' already exists", name));
    }
    Ok(())
}
```

Both layers check the same things: format regex, permanent agent conflicts, and existing spiderling conflicts. The MCP layer checks against the file-based `active-spiderlings.json`; the Rust layer checks against the authoritative in-memory state.

Convention: spiderling names start with `spider-` (e.g., `spider-auth`, `spider-doc-chat`) but this is not enforced by the regex.

---

## 13. Complete Lifecycle Summary

```
SPAWN                           WORK                        KILL
──────                          ────                        ────
orchestrator calls              spiderling runs             orchestrator calls
spawn_spiderling()              in its worktree             kill_spiderling()
    │                               │                           │
    ▼                               │                           ▼
spawn request written           commits to branch       kill request written
~/.aperture/mailbox/            updates BEADS task      ~/.aperture/mailbox/
_spawn/{ts}-{name}.json         stores artifacts        _kill/{ts}-{name}.txt
    │                               │                           │
    ▼ (within 5s)                   │                     ▼ (within 5s)
poller picks up                     │                 poller picks up
    │                               │                           │
    ▼                               │                           ▼
spawner.rs runs:                    │                 spawner.rs runs:
- git worktree add                  │                 - C-c → /exit → kill window
- tmux window created               │                 - git worktree remove
- MCP config written                │                 - launcher files deleted
- launcher + prompt written         │                 - registry updated
- claude launched in tmux           │
- "Begin your task" sent            │
- registry updated                  │
    │                               │
    ▼                               ▼
active-spiderlings.json         branch preserved
updated                         for merge/review
```

---

*This document describes the system as implemented in Aperture commit `ddc600f` (2026-03-24).*
