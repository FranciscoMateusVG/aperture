# BEADS Message Bus & Task Tracking — Implementation Guide

> **Purpose:** This document provides a complete, rebuild-from-scratch reference for the BEADS system as used in Aperture. A new AI agent on a fresh machine should be able to understand and reconstruct the full system from this guide alone.

---

## 1. Overview

**BEADS** is the backbone of Aperture's inter-agent coordination. It serves two roles simultaneously:

1. **Issue/Task Tracker** — A Dolt-backed issue database where work items (tasks, bugs, features) are created, claimed, progressed, and closed by agents. GLaDOS creates tasks; spiderlings and agents claim and close them.

2. **Message Bus** — A lightweight asynchronous messaging layer built *on top of* the same issue database. Agent-to-agent messages are stored as BEADS issues of `type=message`, then polled and delivered by a background Rust thread.

The underlying CLI tool is **`bd`** — a general-purpose issue tracker that calls into a Dolt SQL database. Aperture wraps `bd` in both TypeScript (MCP server) and Rust (Tauri backend) to expose it as MCP tools and a Tauri panel.

**Key invariant:** BEADS is never a custom database. It is `bd` operating against a Dolt database at `~/.aperture/.beads/`. Everything — tasks, messages, artifacts — is stored in that one place.

---

## 2. Dolt Database

### What Is Dolt?

[Dolt](https://github.com/dolthub/dolt) is a MySQL-compatible SQL database with Git-style version history. BEADS uses Dolt as its persistence layer, running as a local SQL server on port **3307**.

### Directory Structure

```
~/.aperture/.beads/
├── config.yaml          # Dolt SQL server config (port 3307, host 127.0.0.1)
├── metadata.json        # bd database config
├── dolt/                # Dolt internals (git-like object store)
└── interactions.jsonl   # bd audit log of all interactions
```

### `metadata.json` — Database Config

This file tells `bd` how to connect to the Dolt backend:

```json
{
  "database": "dolt",
  "jsonl_export": "issues.jsonl",
  "backend": "dolt",
  "dolt_mode": "server",
  "dolt_database": "beads_aperture"
}
```

- `backend: "dolt"` — use Dolt SQL rather than SQLite
- `dolt_mode: "server"` — connect to a running `dolt sql-server` process
- `dolt_database: "beads_aperture"` — the Dolt database name
- `jsonl_export` — path for periodic JSONL snapshots

### `config.yaml` — SQL Server Config

The Dolt server listens on **127.0.0.1:3307** (not 3306, to avoid conflicts with MySQL):

```yaml
listener:
  host: 127.0.0.1
  port: 3307
```

### Initialization Sequence

The Tauri `lib.rs` startup code initializes BEADS in this exact order:

```
1. Check if ~/.aperture/.beads/config.json exists
   └─ If not: mkdir -p ~/.aperture/.beads && dolt init (in that dir)

2. Run: bd dolt test
   └─ If fails (server not running): spawn dolt sql-server --port 3307 --host 127.0.0.1
      └─ sleep 2 seconds to let it start

3. Run: bd init --quiet
   └─ Creates tables/schema if not already initialized
   └─ Ignores "already initialized" errors

4. Spawn background Rust thread: poller::run_message_poller()
```

#### Rust init code (from `src-tauri/src/lib.rs`):

```rust
// Ensure dolt is initialized in .beads dir
if !std::path::Path::new(&format!("{}/config.json", beads_dir)).exists() {
    let _ = std::fs::create_dir_all(&beads_dir);
    let _ = std::process::Command::new("dolt")
        .arg("init")
        .current_dir(&beads_dir)
        .env("PATH", &path_env)
        .output();
}

// Start dolt sql-server if not already running
let dolt_test = std::process::Command::new("bd")
    .args(["dolt", "test"])
    .env("BEADS_DIR", &beads_dir)
    .env("PATH", &path_env)
    .output();

let dolt_running = dolt_test.map(|o| o.status.success()).unwrap_or(false);
if !dolt_running {
    let _ = std::process::Command::new("dolt")
        .args(["sql-server", "--port", "3307", "--host", "127.0.0.1"])
        .current_dir(&beads_dir)
        .env("PATH", &path_env)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    std::thread::sleep(std::time::Duration::from_secs(2));
}

// Initialize BEADS if not yet done
let mut cmd = std::process::Command::new("bd");
cmd.args(["init", "--quiet"]);
cmd.env("BEADS_DIR", &beads_dir);
cmd.env("PATH", &path_env);
```

---

## 3. bd CLI

### Installation

`bd` is installed at `~/.local/bin/bd`. It is a standalone Go binary.

```
$ which bd
/Users/user/.local/bin/bd
```

### Environment Variables

Every `bd` invocation requires these environment variables:

| Variable | Value | Purpose |
|----------|-------|---------|
| `BEADS_DIR` | `~/.aperture/.beads` | Points to the Dolt database directory |
| `BD_ACTOR` | agent name (e.g. `glados`) | Records who performed each action in the audit log |
| `PATH` | `/opt/homebrew/bin:/usr/local/bin:$PATH` | Ensures `dolt` is on PATH for internal calls |

### All Commands Used by Aperture

| Command | Purpose |
|---------|---------|
| `bd create <title> [flags]` | Create a new task or message |
| `bd update <id> [flags]` | Update task fields, claim, set status, append notes |
| `bd close <id> [flags]` | Close a task (marks as done) |
| `bd list [flags]` | List tasks with optional filters |
| `bd ready [flags]` | List open tasks with no active blockers |
| `bd show <id> [flags]` | Show full details of a single task |
| `bd query <expression> [flags]` | Query tasks with expression syntax |
| `bd init --quiet` | Initialize BEADS tables in a new Dolt database |
| `bd dolt test` | Test if the Dolt server is responsive |

### Global Flags (Available on All Commands)

```
--json          Output in JSON format (critical for programmatic use)
--quiet / -q    Suppress non-essential output (errors only)
--actor string  Override the actor name for audit trail
--db string     Override database path (default: auto-discover .beads/*.db)
```

### bd create

```bash
# Basic task
bd create "Fix login bug" -p 1 --json

# Task with description
bd create "Refactor auth module" -p 2 -d "Move auth to separate service" --json

# Message (for inter-agent messaging)
bd create "[glados->wheatley] Hello from GLaDOS" \
  -p 3 \
  --type message \
  -d "Full message content here" \
  --json
```

Flags:
- `-p <0-4>` — Priority (0 = highest, 4 = lowest)
- `-d <text>` — Description (full body)
- `--type <type>` — Issue type: `task`, `bug`, `feature`, `message`, etc.
- `--notes <text>` — Initial notes
- `--assignee <name>` — Assign immediately

### bd update

```bash
# Claim a task (atomically set assignee + status=in_progress)
bd update aperture-6dw --claim --json

# Set status
bd update aperture-6dw --status in_progress --json

# Append notes
bd update aperture-6dw --notes "Found that X depends on Y" --json

# Change description
bd update aperture-6dw --description "Updated scope" --json

# Store an artifact (notes append trick)
bd update aperture-6dw --notes "artifact:file:src/main.ts" --json

# Quiet update (no output)
bd update aperture-6dw --status in_progress --quiet
```

Flags:
- `--claim` — Atomically claim (sets assignee = $BD_ACTOR, status = in_progress; fails if already claimed by another)
- `--status <string>` — Set status: `open`, `in_progress`, `blocked`, `deferred`, `closed`
- `--notes <string>` — Append to notes field (with newline separator)
- `--description <string>` — Replace description
- `--title <string>` — Replace title
- `--assignee <string>` — Set assignee
- `--priority <0-4>` — Change priority

### bd close

```bash
# Close with reason
bd close aperture-6dw --reason "Implemented auth refactor. All tests pass." --json

# Mark message as delivered (used by poller)
bd close aperture-abc --reason "delivered" --quiet
```

Flags:
- `--reason <string>` — Closing reason (stored in `close_reason` field)

### bd list

```bash
# All open tasks (default: limit 50)
bd list --json

# All tasks including closed
bd list --json --all

# No limit
bd list --json -n 0

# Filter by assignee
bd list --json --assignee glados

# Filter by label
bd list --json --label frontend

# Both filters at once
bd list --json --label frontend --assignee spider-doc-beads
```

### bd ready

```bash
# Tasks that are open and have no active blockers
bd ready --json

# Limit results
bd ready --json -n 5
```

`bd ready` is semantically different from `bd list --status open`:
- `ready` applies blocker-aware logic — if task A blocks task B, task B won't appear even if it's `open`
- Use `ready` to find what's actually claimable right now

### bd show

```bash
bd show aperture-6dw --json
```

Returns the full task record including notes, close_reason, artifacts, etc.

### bd query

```bash
# Messages for a specific agent
bd query 'type=message AND status=open AND title="->glados]"' --json -n 0

# High-priority open tasks
bd query 'status=open AND priority<=1' --json

# Tasks by assignee with specific status
bd query 'assignee=spider-doc AND status=in_progress' --json
```

Query syntax:
- `field=value` — Equality (strings use contains for `title`, `description`, `notes`)
- `field!=value` — Inequality
- `AND`, `OR`, `NOT` — Boolean operators (case-insensitive)
- `(expr)` — Grouping
- Supported fields: `status`, `priority`, `type`, `assignee`, `title`, `description`, `notes`, `created`, `updated`, `closed`, `id`

---

## 4. Task System

### Task Lifecycle

```
open → in_progress → closed
         ↕
      blocked
         ↕
      deferred
```

Statuses:
- `open` — Available to claim
- `in_progress` — Being worked on (set by `--claim` or `--status in_progress`)
- `blocked` — Waiting on a dependency
- `deferred` — Postponed (hidden from `bd ready`)
- `closed` — Complete

### Priority Levels

| Value | Meaning | Color in UI |
|-------|---------|-------------|
| 0 | Critical / P0 | Red (`var(--accent-red)`) |
| 1 | High / P1 | Orange (`var(--accent-orange)`) |
| 2 | Normal / P2 (default) | Blue (`var(--accent-blue)`) |
| 3 | Low / P3 | Secondary text |
| 4 | Backlog / P4 | Gray (`#555`) |

### Task JSON Schema

Full task record returned by `bd show <id> --json` or `bd list --json`:

```json
{
  "id": "aperture-6dw",
  "title": "Doc: BEADS Message Bus & Task Tracking",
  "description": "Write detailed implementation doc for BEADS...",
  "notes": "Claimed task. Starting to read source files.\nartifact:file:docs/implementations/03-beads-message-bus.md",
  "status": "in_progress",
  "priority": 1,
  "issue_type": "task",
  "assignee": "spider-doc-beads",
  "owner": "<your-email>",
  "created_at": "2026-03-24T23:03:50Z",
  "created_by": "glados",
  "updated_at": "2026-03-24T23:05:12Z",
  "dependency_count": 0,
  "dependent_count": 0,
  "comment_count": 0,
  "close_reason": null,
  "closed_at": null
}
```

Key fields:
- `id` — Scoped ID like `aperture-6dw` (prefix = database name prefix)
- `issue_type` — The bd type: `task`, `bug`, `feature`, `message`, etc.
- `notes` — Append-only field; artifacts are embedded here as `artifact:type:value` lines
- `close_reason` — Populated when task is closed via `bd close --reason`
- `created_by` — The `BD_ACTOR` at creation time

### CRUD Sequence (Agent Workflow)

```bash
# 1. Find available work
bd ready --json

# 2. Claim a task
bd update aperture-6dw --claim --json
# → sets assignee=me, status=in_progress atomically

# 3. Update during work
bd update aperture-6dw --notes "Discovered X, scope narrowed" --json

# 4. Store deliverable
bd update aperture-6dw --notes "artifact:file:src/main.ts" --json

# 5. Close when done
bd close aperture-6dw --reason "Implemented feature X. Tests pass." --json
```

---

## 5. Message System

### Architecture

The message bus is built entirely on top of the BEADS task system. There is no separate message database, no queue service, no sockets. Messages are just BEADS issues with:

- `type=message`
- `status=open` (unread) or `status=closed` (delivered)
- Title format: `[sender->recipient] preview...`
- Full content in `description`

A background Rust thread (the **poller**) polls BEADS every 5 seconds, finds unread messages for running agents, injects them into the agent's tmux window, and marks them as read by closing the BEADS record.

### Message Lifecycle

```
1. Sender calls: bd create "[glados->wheatley] Hello..." --type message -d "Full content"
   → BEADS record created with status=open

2. Poller (every 5s) calls:
   bd query 'type=message AND status=open AND title="->wheatley]"' --json -n 0
   → Finds pending messages for wheatley

3. Poller injects message into wheatley's tmux window:
   cat /tmp/aperture-msg-<id>.md && rm /tmp/aperture-msg-<id>.md

4. Poller marks message delivered:
   bd close <id> --reason "delivered" --quiet

5. Recipient can also manually call: bd close <id> --reason "delivered"
```

### Title Format

```
[sender->recipient] first 60 chars of content (newlines stripped)
```

Example:
```
[glados->wheatley] I need you to check the auth service
```

The poller queries using a substring match: `title="->wheatley]"` which matches the literal string `->wheatley]` anywhere in the title.

### TypeScript: Creating Messages (`beads.ts`)

```typescript
export async function createMessage(
  from: string,
  to: string,
  content: string,
): Promise<string> {
  const preview = content.slice(0, 60).replace(/\n/g, " ");
  const title = `[${from}->${to}] ${preview}`;
  const args = ["create", title, "-p", "3", "--type", "message", "-d", content, "--json"];
  return runBd(args);
}
```

CLI equivalent:
```bash
bd create "[glados->wheatley] Check the auth service now" \
  -p 3 \
  --type message \
  -d "Full message content\nCan span multiple lines" \
  --json
```

### TypeScript: Querying Unread Messages (`beads.ts`)

```typescript
export async function getUnreadMessages(recipient: string): Promise<string> {
  return runBd(["query",
    `type=message AND status=open AND title="->${recipient}]"`,
    "--json", "-n", "0"
  ]);
}
```

CLI equivalent:
```bash
bd query 'type=message AND status=open AND title="->wheatley]"' --json -n 0
```

Returns an array of message records (same schema as tasks, with `type: "message"`).

### TypeScript: Marking Messages Read (`beads.ts`)

```typescript
export async function markMessageRead(messageId: string): Promise<string> {
  return runBd(["close", messageId, "--reason", "delivered", "--json"]);
}
```

CLI equivalent:
```bash
bd close aperture-abc --reason "delivered" --json
```

### Rust: Poller Query (`src-tauri/src/poller.rs`)

```rust
fn query_unread_messages(recipient: &str) -> Vec<BeadsMessage> {
    let query = format!("type=message AND status=open AND title=\"->{recipient}]\"");
    let output = std::process::Command::new(bd_path())
        .args(["query", &query, "--json", "-n", "0", "-q"])
        .env("BEADS_DIR", beads_dir())
        .env("BD_ACTOR", "poller")
        .env("PATH", path_env())
        .output();
    // parse as Vec<BeadsMessage>
}
```

### Message Delivery Format

Messages are injected into the agent's tmux window formatted as:

```markdown
# Message from glados
_1711324800000_

Full message content here.
Can span multiple lines.
```

The timestamp is milliseconds since Unix epoch (used as a reference, not parsed).

### Routing Rules

| Target | Delivery Method |
|--------|----------------|
| Any agent name (glados, wheatley, peppy, izzy) | BEADS message bus |
| Any spiderling name | BEADS message bus |
| `operator` | File-based (mailbox + chat log) |
| `warroom` | File-based (War Room mechanics) |

The `send_message` MCP tool enforces this routing:

```typescript
// Operator and warroom still use file-based delivery
if (target === "operator" || target === "warroom") {
  const filepath = store.sendMessage(AGENT_NAME, target, message);
  return { content: [{ type: "text", text: `Message sent to ${target}.` }] };
}

// All agent-to-agent messages go through BEADS
const result = await createMessage(AGENT_NAME, target, message);
```

---

## 6. Artifact Storage

### How Artifacts Work

Artifacts are not stored in a separate table. They are embedded directly in a task's `notes` field as specially formatted lines:

```
artifact:<type>:<value>
```

### Writing an Artifact

```typescript
// In beads.ts
export async function storeArtifact(
  taskId: string,
  type: string,
  value: string,
): Promise<string> {
  const artifactLine = `artifact:${type}:${value}`;
  return runBd(["update", taskId, "--notes", artifactLine, "--json"]);
}
```

CLI equivalent:
```bash
bd update aperture-6dw --notes "artifact:file:src/main.ts" --json
bd update aperture-6dw --notes "artifact:pr:https://github.com/org/repo/pull/42" --json
bd update aperture-6dw --notes "artifact:url:http://localhost:3001" --json
bd update aperture-6dw --notes "artifact:note:Found that X was already implemented" --json
```

Because `bd update --notes` **appends** to the notes field (not replaces), you can call it multiple times and each artifact line accumulates.

### Artifact Types

| Type | Example value | UI behavior |
|------|---------------|-------------|
| `file` | `src/components/Foo.tsx` | Shows with "Open" button that calls `openFile()` |
| `pr` | `https://github.com/org/repo/pull/42` | Shows as clickable link |
| `url` | `http://localhost:3001` | Shows as clickable link |
| `note` | `Found that X was already done` | Shows as text note |
| `session` | `session-abc123` | Shows as text (reference to another agent session) |

### Parsing Artifacts (Frontend)

```typescript
function parseArtifacts(notes: string): Artifact[] {
  if (!notes) return [];
  return notes
    .split("\n")
    .filter((l) => l.startsWith("artifact:"))
    .map((l) => {
      const [, type, ...rest] = l.split(":");
      return { type, value: rest.join(":") };
      // rest.join(":") handles URLs which contain colons
    });
}
```

### Example Notes Field

After multiple `store_artifact` calls:

```
Claimed task. Starting to read source files.
Found that poller.rs handles delivery.
artifact:file:docs/implementations/03-beads-message-bus.md
artifact:note:Dolt schema verified against live database
```

---

## 7. TypeScript Wrappers (`mcp-server/src/beads.ts`)

### Environment Setup

```typescript
const BEADS_DIR = resolve(homedir(), ".aperture", ".beads");
const BD_PATH = process.env.BD_PATH ?? "bd";

function getActor(): string {
  return process.env.BD_ACTOR ?? process.env.AGENT_NAME ?? "unknown";
}

function bdEnv(): NodeJS.ProcessEnv {
  return {
    ...process.env,
    BEADS_DIR,
    BD_ACTOR: getActor(),
    PATH: `/opt/homebrew/bin:/usr/local/bin:${process.env.PATH ?? ""}`,
  };
}
```

### Core Runner

```typescript
export function runBd(args: string[]): Promise<string> {
  return new Promise((resolve, reject) => {
    execFile(BD_PATH, args, { env: bdEnv(), timeout: 30000 }, (err, stdout, stderr) => {
      if (err) {
        reject(new Error(stderr || err.message));
      } else {
        resolve(stdout.trim());
      }
    });
  });
}
```

Key points:
- Uses `execFile` (not `exec`) — safer, no shell injection
- 30-second timeout
- Rejects on non-zero exit with stderr as error message
- Returns trimmed stdout

### All Exported Functions

```typescript
createTask(title, priority, description?)  → bd create ... --json
updateTask(id, flags)                      → bd update <id> [--key value]... --json
closeTask(id, reason)                      → bd close <id> --reason <reason> --json
queryTasks(mode, id?)                      → bd show/ready/list --json
storeArtifact(taskId, type, value)         → bd update <id> --notes "artifact:type:value" --json
searchTasks(label?)                        → bd list [--label <label>] --json
createMessage(from, to, content)           → bd create "[from->to] preview" --type message -d content --json
getUnreadMessages(recipient)               → bd query 'type=message AND status=open AND title="->recipient]"' --json -n 0
markMessageRead(messageId)                 → bd close <id> --reason delivered --json
```

---

## 8. Rust Wrappers (`src-tauri/src/beads.rs`)

The Rust side exposes two Tauri commands used by the frontend panel:

```rust
fn bd_cmd() -> Command {
    let mut c = Command::new("bd");
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let current_path = std::env::var("PATH").unwrap_or_default();
    c.env("PATH", format!("/opt/homebrew/bin:/usr/local/bin:{}", current_path));
    c.env("BEADS_DIR", format!("{}/.aperture/.beads", home));
    c
}

// List all tasks including closed (for the panel)
#[tauri::command]
pub fn list_beads_tasks() -> Result<serde_json::Value, String> {
    bd_cmd().args(["list", "--json", "--all"]).output()
    // parse JSON → serde_json::Value
}

// Update a task's status (used by panel UI buttons)
#[tauri::command]
pub fn update_beads_task_status(task_id: String, status: String) -> Result<(), String> {
    bd_cmd().args(["update", &task_id, "--status", &status, "--quiet"]).output()
}
```

Note: Rust wrappers don't set `BD_ACTOR` — they run without an actor, which is fine for read-heavy panel operations.

---

## 9. MCP Tools (`mcp-server/src/index.ts`)

The MCP server exposes BEADS operations as tools that AI agents call. Each tool maps directly to a `beads.ts` function.

### Tool Definitions

| MCP Tool | Calls | Parameters |
|----------|-------|------------|
| `send_message` | `createMessage()` | `to: string`, `message: string` |
| `mark_as_read` | `markMessageRead()` | `message_id: string` |
| `get_messages` | `getUnreadMessages()` | _(none)_ |
| `create_task` | `createTask()` | `title`, `priority: 0-4`, `description?` |
| `update_task` | `updateTask()` | `id`, `claim?`, `status?`, `description?`, `notes?` |
| `close_task` | `closeTask()` | `id`, `reason` |
| `query_tasks` | `queryTasks()` | `mode: "list"\|"ready"\|"show"`, `id?` |
| `store_artifact` | `storeArtifact()` | `task_id`, `type: "file"\|"pr"\|"session"\|"url"\|"note"`, `value` |
| `search_tasks` | `searchTasks()` | `label?` |

### Agent Identity

Each MCP server process is started with `AGENT_NAME` and `AGENT_ROLE` environment variables:

```typescript
const AGENT_NAME = process.env.AGENT_NAME;  // e.g. "glados", "spider-doc-beads"
const agentRole = process.env.AGENT_ROLE ?? "agent";  // "orchestrator" or "agent"
```

The `BD_ACTOR` is set from `AGENT_NAME` via `beads.ts`'s `getActor()`.

### Sending Messages: Full Flow

```typescript
server.tool("send_message", ..., async ({ to, message }) => {
  const target = to.toLowerCase().trim();

  // Validate recipient (permanent agents + active spiderlings)
  if (!isValidRecipient(target)) { return error; }

  // Special routing: operator and warroom use file-based delivery
  if (target === "operator" || target === "warroom") {
    const filepath = store.sendMessage(AGENT_NAME, target, message);
    return { content: [{ type: "text", text: `Delivered to: ${filepath}` }] };
  }

  // All other agents: BEADS
  const result = await createMessage(AGENT_NAME, target, message);
  const parsed = JSON.parse(result);
  const msgId = parsed.id ?? "unknown";
  return { content: [{ type: "text", text: `Message sent via BEADS (${msgId}).` }] };
});
```

### Getting Messages: Full Flow

```typescript
server.tool("get_messages", ..., async () => {
  const result = await getUnreadMessages(AGENT_NAME!);
  const messages = JSON.parse(result);

  const formatted = messages.map((m: any) => {
    const titleMatch = m.title?.match(/\[(.+?)->(.+?)\]/);
    const from = titleMatch?.[1] ?? "unknown";
    return `[${m.id}] From ${from}: ${m.description ?? "(no content)"}`;
  }).join("\n\n");

  return { content: [{ type: "text", text: formatted }] };
});
```

---

## 10. Query Patterns

All the `bd` query patterns Aperture actually uses, with their TypeScript/Rust equivalents:

### List all open tasks

```bash
bd list --json
```
→ TypeScript: `queryTasks("list")`

### List all tasks including closed

```bash
bd list --json --all
```
→ Rust: `list_beads_tasks()` (uses `--all`)

### Ready tasks (claimable, no blockers)

```bash
bd ready --json
```
→ TypeScript: `queryTasks("ready")`

### Show single task

```bash
bd show aperture-6dw --json
```
→ TypeScript: `queryTasks("show", "aperture-6dw")`

### Tasks by label

```bash
bd list --json --label frontend
```
→ TypeScript: `searchTasks("frontend")`

### Unread messages for recipient

```bash
bd query 'type=message AND status=open AND title="->glados]"' --json -n 0
```
→ TypeScript: `getUnreadMessages("glados")`
→ Rust: `query_unread_messages("glados")`

The `-n 0` flag disables the default 50-item limit — important for message delivery!

### Query by status and priority

```bash
bd query 'status=open AND priority<=1' --json
```

### Query by assignee

```bash
bd list --json --assignee spider-doc-beads
```

---

## 11. Background Poller (`src-tauri/src/poller.rs`)

The poller is a Rust background thread that runs every 5 seconds and handles:

1. **Spawn requests** — reads `~/.aperture/mailbox/_spawn/*.json`, spawns spiderlings
2. **Kill requests** — reads `~/.aperture/mailbox/_kill/*`, kills spiderlings
3. **War Room messages** — handles file-based war room turn mechanics
4. **Operator messages** — moves agent→human messages to chat log
5. **BEADS message delivery** — queries and delivers inter-agent messages

### BEADS Message Delivery Loop

```rust
// For each running agent + spiderling:
for (agent_name, window_id) in &agents {
    let messages = query_unread_messages(&agent_name);

    for msg in &messages {
        let sender = parse_sender_from_title(&msg.title);
        let content = msg.description.as_deref().unwrap_or("(no content)");

        // Format as markdown
        let formatted = format!(
            "# Message from {}\n_{}_\n\n{}\n",
            sender, timestamp_millis, content
        );

        // Write to temp file and inject via tmux
        let tmp_path = format!("/tmp/aperture-msg-{}.md", msg.id);
        fs::write(&tmp_path, &formatted);
        let cmd = format!("cat '{}' && rm '{}'", tmp_path, tmp_path);
        tmux::tmux_send_keys(window_id.clone(), cmd);

        // Mark as delivered
        mark_message_read(&msg.id);
    }
}
```

### Sender Parsing

```rust
fn parse_sender_from_title(title: &str) -> String {
    // Extract "glados" from "[glados->wheatley] Hello"
    if let Some(start) = title.find('[') {
        if let Some(arrow) = title.find("->") {
            return title[start + 1..arrow].to_string();
        }
    }
    "unknown".to_string()
}
```

---

## 12. Frontend Integration (`src/components/BeadsPanel.ts`)

### Component Structure

```typescript
export function createBeadsPanel(container: HTMLElement) {
  // Renders:
  // - Search input
  // - Status filter (All / Open / Closed)
  // - Objective filter (from Kanban click events)
  // - Task list with expand/collapse

  // Polls bd every 3 seconds via Tauri command
  const interval = setInterval(poll, 3000);

  return { destroy() { clearInterval(interval); }, refresh: poll };
}
```

### Data Flow

```
Tauri command: list_beads_tasks()
  → bd list --json --all
  → [BeadsTask[], ...]
  → renderFiltered()
  → DOM update
```

### BeadsTask Interface

```typescript
interface BeadsTask {
  id: string;
  title: string;
  description?: string;
  status: string;           // "open" | "in_progress" | "blocked" | "closed"
  priority?: number;        // 0-4
  notes?: string;           // contains artifact:type:value lines
  owner?: string;
  created_at?: string;
  closed_at?: string;
  close_reason?: string;
}
```

### Status Colors

```typescript
const statusColor =
  t.status === "closed"      ? "var(--accent-green)"   :
  t.status === "in_progress" ? "var(--accent-orange)"  :
                               "var(--text-secondary)";
```

### Priority Colors

```typescript
const PRIORITY_COLORS: Record<number, string> = {
  0: "var(--accent-red)",      // Critical
  1: "var(--accent-orange)",   // High
  2: "var(--accent-blue)",     // Normal
  3: "var(--text-secondary)",  // Low
  4: "#555",                   // Backlog
};
```

### Artifact Rendering

The panel renders artifacts differently by type:

- `file` → shows path + **Open** button (calls `commands.openFile(path)`)
- `pr` / `url` → shows as clickable `<a href>` link
- `note` → shows as plain text
- other → shows type + value as text

### Objective Filter Integration

The panel listens for a custom DOM event `objective-selected` dispatched by the Kanban board. When received, it filters tasks to only show those in the objective's `task_ids` array:

```typescript
window.addEventListener("objective-selected", (e: CustomEvent<Objective>) => {
  objectiveFilter = e.detail;
  renderFiltered();
});
```

---

## 13. Schema

The underlying Dolt/SQL schema for BEADS issues (as reflected in the JSON API):

| Field | Type | Description |
|-------|------|-------------|
| `id` | string | Scoped ID, e.g. `aperture-6dw` (prefix-hash) |
| `title` | string | Short title (messages use `[from->to] preview` format) |
| `description` | text | Full description / message body |
| `notes` | text | Append-only notes; artifacts embedded as `artifact:type:value` lines |
| `status` | enum | `open`, `in_progress`, `blocked`, `deferred`, `closed` |
| `priority` | int | 0 (highest) to 4 (lowest) |
| `issue_type` | string | `task`, `bug`, `feature`, `message`, `epic`, `chore`, etc. |
| `assignee` | string | Agent/user name who owns the issue |
| `owner` | string | Creator's email/identity |
| `created_at` | timestamp | ISO 8601 UTC |
| `created_by` | string | `BD_ACTOR` at creation time |
| `updated_at` | timestamp | ISO 8601 UTC |
| `closed_at` | timestamp | ISO 8601 UTC, null if open |
| `close_reason` | text | Reason from `bd close --reason` |
| `dependency_count` | int | Number of issues this blocks |
| `dependent_count` | int | Number of issues blocking this |
| `comment_count` | int | Number of comments |

### Message-Specific Fields

Messages reuse the issue schema. The `type=message` flag distinguishes them:

- `issue_type: "message"`
- `title: "[sender->recipient] preview"` — required format for routing
- `description` — full message content
- `status: "open"` = unread/undelivered; `status: "closed"` = read/delivered

---

## 14. Permanent Recipients

The system defines a set of permanent agent names that are always valid message targets:

```typescript
const PERMANENT_RECIPIENTS = ["glados", "wheatley", "peppy", "izzy", "operator", "warroom"];
```

These names are hardcoded in `index.ts`. Spiderling names are dynamic and loaded from the active spiderlings registry.

---

## 15. Rebuild Checklist

To rebuild BEADS from scratch on a new machine:

1. **Install Dolt** — `curl -L https://github.com/dolthub/dolt/releases/latest/download/install.sh | bash`
2. **Install bd** — `curl ... ~/.local/bin/bd` (get from existing installation or build from source)
3. **Create database directory** — `mkdir -p ~/.aperture/.beads`
4. **Init Dolt** — `cd ~/.aperture/.beads && dolt init`
5. **Create `metadata.json`**:
   ```json
   {"database":"dolt","jsonl_export":"issues.jsonl","backend":"dolt","dolt_mode":"server","dolt_database":"beads_aperture"}
   ```
6. **Create `config.yaml`** — set `listener.host: 127.0.0.1` and `listener.port: 3307`
7. **Start Dolt server** — `cd ~/.aperture/.beads && dolt sql-server --port 3307 --host 127.0.0.1 &`
8. **Init BEADS** — `BEADS_DIR=~/.aperture/.beads bd init`
9. **Verify** — `BEADS_DIR=~/.aperture/.beads bd list --json` should return `[]`
10. **Set env vars for agents** — `BD_ACTOR=agent_name`, `BEADS_DIR=~/.aperture/.beads`

---

## 16. Key Design Decisions

| Decision | Rationale |
|----------|-----------|
| Messages as issues | Reuses existing infrastructure; no separate message broker to maintain |
| `status=open` = unread | Natural fit — open issues are "pending", closed issues are "done" |
| Title format `[from->to]` | Enables simple `bd query title="->"` routing without custom fields |
| Notes as artifact store | Append-only; no schema migration needed for new artifact types |
| Poller delivers via tmux | Agents run in tmux windows; `cat` injection is the universal delivery mechanism |
| File fallback for operator/warroom | These are human-facing; file-based delivery integrates with the Chat panel |
| `BD_ACTOR` from `AGENT_NAME` | Automatic audit trail without extra configuration |
| `-n 0` on message queries | Prevents missing messages when > 50 accumulate |
