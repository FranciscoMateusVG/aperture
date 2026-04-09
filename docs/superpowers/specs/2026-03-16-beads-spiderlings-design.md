# BEADS Integration + Spiderling Spawning — Design Spec

**Date:** 2026-03-16
**Status:** Approved
**Pillars:** Artifact Storage (BEADS), Dynamic Agent Spawning (Spiderlings)

## Context

Aperture already has inter-agent messaging (mailbox + MCP) and moderated group discussion (War Room). Two missing orchestration pillars:

1. **Queryable artifact storage** — agents need to store and query what each other produced
2. **Dynamic agent spawning** — GLaDOS needs to spin up ephemeral workers to execute plans

## Decisions

- **BEADS** (`bd` CLI, Dolt-backed) is the artifact/task storage layer. One global database at `~/.aperture/.beads/` — shared across all projects, not per-project.
- **Spiderlings** are ephemeral Claude Code CLI instances in tmux windows, spawned by GLaDOS via MCP tool.
- Both features are exposed as **MCP tools** in the existing `aperture-bus` server — agents use one unified interface.
- Spiderlings run **sonnet** model (cheap workers).
- Spiderlings **persist after completion** — operator tells GLaDOS when to kill them.
- Each spiderling works in an **isolated git worktree** to avoid codebase conflicts.
- No cap on spiderling count — operator monitors and intervenes as needed.
- No automated crash recovery — GLaDOS nudges spiderlings to check if they're alive.

## Orchestration Workflow

```
Operator + Wheatley plan together
        │
        ▼
Wheatley ──send_message──▶ GLaDOS (here's the plan)
        │
        ◄── GLaDOS feedback
        │
Operator + Wheatley adjust
        │
        ▼
Wheatley ──send_message──▶ GLaDOS (execute this)
        │
        ▼
GLaDOS breaks down the plan:
  ├─ create_task("Implement auth", p1)     → bd-a1b2
  ├─ create_task("Write API routes", p1)   → bd-c3d4
  └─ create_task("Add tests", p2)          → bd-e5f6
        │
        ▼
GLaDOS spawns:
  ├─ spawn_spiderling("spider-1", "bd-a1b2", "Implement auth...")
  ├─ spawn_spiderling("spider-2", "bd-c3d4", "Write API routes...")
  └─ spawn_spiderling("spider-3", "bd-e5f6", "Add tests...")
        │
        ▼
Each spiderling (in its own git worktree):
  1. Claims task: update_task(id, --claim)
  2. Does the work in isolated worktree
  3. Stores artifacts: store_artifact(id, type, value)
  4. Closes task: close_task(id, "Done - implemented X")
  5. Messages GLaDOS: send_message(to: "glados", "Task complete")
        │
        ▼
GLaDOS collects results, reports to Wheatley/Operator
Operator merges worktree branches when ready
```

---

## Feature 1: BEADS Integration

### Global Database

BEADS runs as a single global database at `~/.aperture/.beads/`. On app startup, Rust backend runs `bd init` with `BEADS_DIR=~/.aperture/.beads` if not already initialized. All agents share this database via the `BEADS_DIR` environment variable in their MCP config.

Each agent gets `BD_ACTOR=<agent_name>` so BEADS tracks who did what.

### MCP Tools

All tools use `child_process.execFile` (async) to call `bd` CLI with `--json` output. The `BEADS_DIR` env var is set on every call.

| Tool | bd command | Used by |
|------|-----------|---------|
| `create_task` | `bd create "title" -p <priority> --json` + optional description via `--stdin` | GLaDOS (creates tasks for spiderlings), any agent |
| `update_task` | `bd update <id> --claim`, `--status`, `--description`, `--notes` | Any agent working a task |
| `close_task` | `bd close <id> --reason "..."` | Agent that finishes work |
| `query_tasks` | `bd ready --json`, `bd list --json`, `bd show <id> --json` | Any agent querying what's available/done |
| `store_artifact` | Writes structured artifact line to task notes (see Artifacts section) | Any agent storing deliverables |
| `search_tasks` | `bd list --json --label <label>` or filtered queries | Agents looking for what others produced |

### Artifacts

Artifacts are typed references stored as structured notes on BEADS tasks:

```
artifact:<type>:<value>
```

| Type | Value | Example |
|------|-------|---------|
| `file` | Path in the repo | `artifact:file:src/auth/middleware.ts` |
| `pr` | GitHub PR URL | `artifact:pr:github.com/user/repo/pull/42` |
| `session` | Path to saved session log | `artifact:session:~/.aperture/artifacts/bd-a1b2/session.md` |
| `url` | Any external link | `artifact:url:https://grafana.internal/d/api-latency` |
| `note` | Inline text summary | `artifact:note:Implemented JWT auth with RS256 signing` |

The `store_artifact` MCP tool takes `task_id`, `type` (one of the above), and `value`. It appends the structured line to the task's notes via `bd update <id> --notes`.

Session artifacts (type `session`) are written to `~/.aperture/artifacts/<task-id>/` before recording the reference.

### New Module: `mcp-server/src/beads.ts`

Wraps `bd` CLI calls:
- Executes `bd` commands via `child_process.execFile` (async, non-blocking)
- Sets `BEADS_DIR` and `BD_ACTOR` env vars on every call
- Parses JSON output
- Returns structured results to MCP tool handlers
- Handles errors (bd not found, db not initialized, etc.)

### UI: Tasks Panel

New "Tasks" button in navbar (alongside Chat, War Room, Messages). Shows:
- BEADS tasks with status, assignee, priority
- Artifact references (clickable for files/URLs)
- Polls `bd list --json` periodically via a Tauri command that runs with `BEADS_DIR` set

---

## Feature 2: Spiderling Spawning

### MCP Tools

Spawn and kill tools enforce **caller identity** — only the `orchestrator` role (GLaDOS) can call them. The MCP server checks `AGENT_ROLE` env var (already available) and rejects unauthorized callers with an error message.

| Tool | Restriction | Description |
|------|-------------|-------------|
| `spawn_spiderling` | orchestrator only | Creates ephemeral Claude Code instance. Takes: `name`, `task_id` (BEADS issue), `prompt`. Returns spiderling name. |
| `list_spiderlings` | any agent | Lists all active spiderlings with status. |
| `kill_spiderling` | orchestrator only | Kills a spiderling's tmux window and worktree. |

### Name Validation

Spiderling names are sanitized before use:
- Must match `/^[a-z0-9][a-z0-9-]{0,30}$/` (lowercase alphanumeric + hyphens, max 31 chars)
- Rejected with error if invalid
- Rejected with error if name collides with a permanent agent name or existing spiderling

### Spawn Flow

1. GLaDOS calls `spawn_spiderling(name: "spider-auth", task_id: "bd-a1b2", prompt: "Implement the auth module...")`
2. MCP server validates name + caller role, then writes a spawn request file to `~/.aperture/mailbox/_spawn/`:

**Spawn request format** (`~/.aperture/mailbox/_spawn/<timestamp>-<name>.json`):
```json
{
  "name": "spider-auth",
  "task_id": "bd-a1b2",
  "prompt": "Implement the auth module...",
  "requested_by": "glados",
  "timestamp": "1710000000000"
}
```

3. Rust poller picks up the spawn request and:
   - Creates a git worktree at `~/.aperture/worktrees/spider-auth` on branch `spider-auth`
   - Creates a tmux window named `spider-auth` in the aperture session
   - Writes a launcher script at `~/.aperture/launchers/spider-auth.sh`:
     ```bash
     #!/bin/bash
     export PATH="/opt/homebrew/bin:/usr/local/bin:$PATH"
     cd ~/.aperture/worktrees/spider-auth
     PROMPT="You are a spiderling named spider-auth, working for GLaDOS.
     Your task is tracked in BEADS issue bd-a1b2.
     Work in this git worktree — do NOT switch branches.
     When done: close_task('bd-a1b2', 'reason'), then send_message(to: 'glados', message: 'done').
     Store deliverables with store_artifact.

     TASK: Implement the auth module..."
     exec claude --dangerously-skip-permissions --model sonnet --system-prompt "$PROMPT" --mcp-config /tmp/aperture-mcp-spider-auth.json --name spider-auth
     ```
   - Writes MCP config at `/tmp/aperture-mcp-spider-auth.json`:
     ```json
     {
       "mcpServers": {
         "aperture-bus": {
           "type": "stdio",
           "command": "node",
           "args": ["<mcp_server_path>"],
           "env": {
             "AGENT_NAME": "spider-auth",
             "AGENT_ROLE": "spiderling",
             "AGENT_MODEL": "sonnet",
             "APERTURE_MAILBOX": "~/.aperture/mailbox",
             "BEADS_DIR": "~/.aperture/.beads",
             "BD_ACTOR": "spider-auth"
           }
         }
       }
     }
     ```
   - Sends the launcher path to tmux via `send-keys`
   - Auto-confirms workspace trust prompt (same Enter-key approach as named agents)

4. Updates `~/.aperture/active-spiderlings.json`:

**Active spiderlings schema** (`~/.aperture/active-spiderlings.json`):
```json
[
  {
    "name": "spider-auth",
    "task_id": "bd-a1b2",
    "tmux_window_id": "@5",
    "worktree_path": "~/.aperture/worktrees/spider-auth",
    "worktree_branch": "spider-auth",
    "requested_by": "glados",
    "status": "working",
    "spawned_at": "1710000000000"
  }
]
```

5. Spiderling appears in UI as agent card with "spiderling" role badge and kill button.

### Dynamic Recipients

Current `VALID_RECIPIENTS` is hardcoded. Change to:
- **Permanent:** `glados`, `wheatley`, `peppy`, `izzy`, `operator`, `warroom`
- **Dynamic:** MCP server reads `~/.aperture/active-spiderlings.json` on each `send_message` call to get current spiderling names
- Spiderlings can message GLaDOS and other spiderlings
- GLaDOS can message any spiderling
- Spiderlings get their own mailbox at `~/.aperture/mailbox/<spiderling-name>/`

### Spiderling Lifecycle

| State | Description |
|-------|-------------|
| **Spawning** | Tmux window + worktree created, Claude Code starting |
| **Working** | Active, communicating with GLaDOS, updating BEADS |
| **Done** | Task closed, results delivered, window still alive for inspection |
| **Killed** | Window destroyed, worktree cleaned up (branch preserved for merging) |

### Kill Mechanism

- `kill_spiderling` MCP tool (orchestrator only) — writes kill request to `~/.aperture/mailbox/_kill/`
- Kill button on spiderling agent cards in UI — calls Tauri command directly
- Rust backend: sends `C-c` → `/exit` → `tmux kill-window` (same as `stop_agent`)
- Removes from `active-spiderlings.json`
- Removes worktree directory (but preserves the git branch for later merging)

---

## File Changes

| File | Changes |
|------|---------|
| `mcp-server/src/index.ts` | Add BEADS tools + spawning tools, role-based access control |
| `mcp-server/src/beads.ts` | **New** — wraps `bd` CLI calls (async), parses JSON output |
| `mcp-server/src/spawner.ts` | **New** — writes spawn/kill requests, validates names |
| `src-tauri/src/spawner.rs` | **New** — spiderling lifecycle (worktree, tmux window, launcher script, tracking, cleanup) |
| `src-tauri/src/poller.rs` | Add `_spawn` + `_kill` mailbox polling, spiderling message routing |
| `src-tauri/src/state.rs` | Add `spiderlings: HashMap<String, SpiderlingDef>` to `AppState` |
| `src-tauri/src/lib.rs` | Register new commands, init BEADS on startup |
| `src/components/AgentCard.ts` | Support spiderling role badge + kill button |
| `src/components/TasksPanel.ts` | **New** — shows BEADS tasks in right panel |
| `src/main.ts` | Wire Tasks panel toggle |
| `src/style.css` | Spiderling badge color, tasks panel styles |
| `prompts/glados.md` | Update with spawning + BEADS workflow instructions |
| All agent prompts | Add BEADS tool usage instructions |

## What Doesn't Change

- Existing messaging system (mailbox, poller, chat)
- War Room
- Terminal, Navbar, existing agent lifecycle
- Named agents (wheatley, glados, peppy, izzy) — still hardcoded, spiderlings are separate
