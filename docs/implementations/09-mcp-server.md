# 09 — MCP Server (Claude Code Bridge)

The MCP server is the bridge between Claude Code agents and the Aperture orchestration system. It exposes all Aperture capabilities as MCP tools that Claude Code can call natively — BEADS task tracking, inter-agent messaging, spiderling spawning, and objectives management.

---

## 1. Overview

Claude Code agents can't run shell commands or read files directly on the host — they operate through the MCP protocol. The Aperture MCP server (`aperture-mcp-server`) runs as a stdio subprocess alongside each agent session and exposes 14 tools:

| Category | Tools |
|----------|-------|
| Identity | `get_identity` |
| Messaging | `send_message`, `get_messages`, `mark_as_read` |
| Task tracking | `create_task`, `update_task`, `close_task`, `query_tasks`, `store_artifact`, `search_tasks` |
| Spiderlings | `spawn_spiderling`, `list_spiderlings`, `kill_spiderling` |
| Objectives | `list_objectives`, `update_objective` |

Internally, task and message tools wrap the `bd` CLI (BEADS database). Spiderling tools write request files to the mailbox. Operator and warroom messages use file-based delivery for compatibility with the Chat panel and War Room UI.

**Package name:** `aperture-mcp-server`
**Entry point:** `dist/index.js` (compiled from `src/index.ts`)
**Transport:** stdio (stdin/stdout)
**MCP server name:** `aperture-bus`
**Version:** `0.2.0`

---

## 2. Server Setup

### Initialization sequence

```typescript
// src/index.ts
import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { z } from "zod";
import { MailboxStore } from "./store.js";
import { createTask, updateTask, closeTask, queryTasks, storeArtifact, searchTasks,
         createMessage, getUnreadMessages, markMessageRead } from "./beads.js";
import { requestSpawn, requestKill, readActiveSpiderlings, isValidRecipient } from "./spawner.js";

const AGENT_NAME = process.env.AGENT_NAME;
if (!AGENT_NAME) {
  console.error("AGENT_NAME environment variable is required");
  process.exit(1);
}

const agentRole = process.env.AGENT_ROLE ?? "agent";
const agentModel = process.env.AGENT_MODEL ?? "unknown";
const mailboxDir = process.env.APERTURE_MAILBOX; // optional override

const store = new MailboxStore(mailboxDir);
store.ensureMailbox(AGENT_NAME);

const server = new McpServer({
  name: "aperture-bus",
  version: "0.2.0",
});
```

The server requires `AGENT_NAME` at startup — it exits immediately if not set. The `MailboxStore` is initialized and the agent's mailbox directory is created. The `McpServer` instance is constructed with a fixed name `"aperture-bus"` that clients use to reference it.

### Transport and entry point

```typescript
async function main() {
  const transport = new StdioServerTransport();
  await server.connect(transport);
}

main().catch((err) => {
  console.error("Failed to start MCP server:", err);
  process.exit(1);
});
```

`StdioServerTransport` connects via stdin/stdout — the MCP host (Claude Code) spawns this process and communicates over the process's standard streams. The server stays alive for the lifetime of the Claude Code session.

---

## 3. Environment Variables

| Variable | Required | Default | Purpose |
|----------|----------|---------|---------|
| `AGENT_NAME` | **Yes** | — | Agent's name (e.g. `glados`, `spider-auth`). Used as sender in messages, mailbox directory name, BD_ACTOR for BEADS commands. Fatal if missing. |
| `AGENT_ROLE` | No | `"agent"` | Agent's role. Only `"orchestrator"` can use `spawn_spiderling` and `kill_spiderling`. |
| `AGENT_MODEL` | No | `"unknown"` | Model identifier (e.g. `"sonnet"`, `"opus"`). Returned by `get_identity` for introspection. |
| `APERTURE_MAILBOX` | No | `~/.aperture/mailbox` | Override for mailbox base directory. Used by `MailboxStore` and `spawner.ts` (`MAILBOX_BASE`). |
| `BEADS_DIR` | Implicit | `~/.aperture/.beads` | Set by `beads.ts` in the `bdEnv()` function (not read directly — always hardcoded to that path). |
| `BD_ACTOR` | Implicit | `AGENT_NAME` | Set by `beads.ts` in `bdEnv()` — informs BEADS who's performing each operation. Falls back to `AGENT_NAME`. |
| `BD_PATH` | No | `"bd"` | Path to the `bd` binary. Override if `bd` isn't on system PATH. |

### How environment variables flow into BEADS

`beads.ts` builds a custom environment for every `bd` call:

```typescript
function getActor(): string {
  return process.env.BD_ACTOR ?? process.env.AGENT_NAME ?? "unknown";
}

function bdEnv(): NodeJS.ProcessEnv {
  return {
    ...process.env,
    BEADS_DIR: resolve(homedir(), ".aperture", ".beads"),
    BD_ACTOR: getActor(),
    PATH: `/opt/homebrew/bin:/usr/local/bin:${process.env.PATH ?? ""}`,
  };
}
```

This ensures every `bd` invocation knows which BEADS directory to use and who the acting agent is.

---

## 4. Identity System

### `get_identity` tool

Returns the agent's name, role, model, and system context as JSON.

```typescript
server.tool(
  "get_identity",
  "Get your identity and role within the Aperture orchestration system.",
  {},
  async () => {
    return {
      content: [{
        type: "text",
        text: JSON.stringify({
          name: AGENT_NAME,
          role: agentRole,
          model: agentModel,
          system: "Aperture AI Orchestration Platform",
          description: "You are an AI agent inside the Aperture orchestration system. Messages from other agents are delivered directly into your conversation as file contents.",
        }, null, 2),
      }],
    };
  }
);
```

**Schema:** no parameters
**Returns:** JSON object with `name`, `role`, `model`, `system`, `description`

**Example response:**
```json
{
  "name": "glados",
  "role": "orchestrator",
  "model": "opus",
  "system": "Aperture AI Orchestration Platform",
  "description": "You are an AI agent inside the Aperture orchestration system..."
}
```

### Role-based access control

The `requireRole()` helper enforces that certain tools are only available to specific roles:

```typescript
const PERMANENT_RECIPIENTS = ["glados", "wheatley", "peppy", "izzy", "operator", "warroom"];

function requireRole(required: string): void {
  if (agentRole !== required) {
    throw new Error(`This tool requires the '${required}' role. You are '${agentRole}'.`);
  }
}
```

Currently only `"orchestrator"` is enforced — `spawn_spiderling` and `kill_spiderling` call `requireRole("orchestrator")` before executing.

### Type definition

```typescript
// src/types.ts
export interface AgentInfo {
  name: string;
  role: string;
  model: string;
}
```

---

## 5. Messaging Tools

### Message routing

Messages are routed differently depending on recipient:

- **`operator`** → file-based (MailboxStore) — feeds the Chat panel
- **`warroom`** → file-based (MailboxStore) — feeds the War Room turn mechanics
- **All agents** (glados, wheatley, peppy, izzy, spiderlings) → BEADS message bus (with file fallback)

### `send_message`

Sends a message to another agent or the human operator.

```typescript
server.tool(
  "send_message",
  "Send a message to another agent or the human operator. Valid recipients: glados, wheatley, peppy, izzy, operator, plus any active spiderlings. Use 'operator' to reach the human.",
  {
    to: z.string().describe("Recipient: glados, wheatley, peppy, izzy, operator, or a spiderling name"),
    message: z.string().describe("Message content")
  },
  async ({ to, message }) => {
    const target = to.toLowerCase().trim();

    // Validate recipient
    if (!isValidRecipient(target)) {
      const spiderlingNames = readActiveSpiderlings().map(s => s.name);
      const allRecipients = [...PERMANENT_RECIPIENTS, ...spiderlingNames];
      return {
        content: [{ type: "text", text: `ERROR: Unknown recipient "${to}". Valid recipients are: ${allRecipients.join(", ")}. Use "operator" to message the human.` }],
        isError: true,
      };
    }

    // Block self-messaging
    if (target === AGENT_NAME) {
      // ...return error
    }

    // File-based delivery for operator and warroom
    if (target === "operator" || target === "warroom") {
      const filepath = store.sendMessage(AGENT_NAME, target, message);
      return { content: [{ type: "text", text: `Message sent to ${target}. Delivered to: ${filepath}` }] };
    }

    // BEADS delivery for all agents
    try {
      const result = await createMessage(AGENT_NAME, target, message);
      const parsed = JSON.parse(result);
      const msgId = parsed.id ?? "unknown";
      return { content: [{ type: "text", text: `Message sent to ${target} via BEADS (${msgId}). The poller will deliver it.` }] };
    } catch (e: any) {
      // Fallback to file-based delivery
      const filepath = store.sendMessage(AGENT_NAME, target, message);
      return { content: [{ type: "text", text: `Message sent to ${target} (file fallback). Delivered to: ${filepath}` }] };
    }
  }
);
```

**Schema:**
- `to` (string, required) — recipient name
- `message` (string, required) — message content

**Validation:** `isValidRecipient()` checks against permanent names + active spiderlings from `~/.aperture/active-spiderlings.json`. Invalid recipients return `isError: true`.

### `get_messages`

Fetches all unread BEADS messages addressed to this agent.

```typescript
server.tool(
  "get_messages",
  "Get all unread messages for you from the BEADS message bus.",
  {},
  async () => {
    try {
      const result = await getUnreadMessages(AGENT_NAME!);
      const messages = JSON.parse(result);
      if (!Array.isArray(messages) || messages.length === 0) {
        return { content: [{ type: "text", text: "No unread messages." }] };
      }
      const formatted = messages.map((m: any) => {
        const titleMatch = m.title?.match(/\[(.+?)->(.+?)\]/);
        const from = titleMatch?.[1] ?? "unknown";
        return `[${m.id}] From ${from}: ${m.description ?? "(no content)"}`;
      }).join("\n\n");
      return { content: [{ type: "text", text: formatted }] };
    } catch (e: any) {
      return { content: [{ type: "text", text: `ERROR: ${e.message}` }], isError: true };
    }
  }
);
```

**Schema:** no parameters
**Returns:** formatted list of messages: `[aperture-abc] From glados: <content>`, or `"No unread messages."`

Messages are stored in BEADS with `type=message` and title format `[sender->recipient] preview...`. The `get_messages` call queries `type=message AND status=open AND title="->{AGENT_NAME}]"`.

### `mark_as_read`

Marks a BEADS message as read by closing it.

```typescript
server.tool(
  "mark_as_read",
  "Mark a BEADS message as read. Use this after receiving a message delivered by the poller.",
  { message_id: z.string().describe("The BEADS message ID to mark as read (e.g. aperture-abc)") },
  async ({ message_id }) => {
    try {
      await markMessageRead(message_id);
      return { content: [{ type: "text", text: `Message ${message_id} marked as read.` }] };
    } catch (e: any) {
      return { content: [{ type: "text", text: `ERROR: ${e.message}` }], isError: true };
    }
  }
);
```

**Schema:**
- `message_id` (string, required) — BEADS message ID (e.g. `aperture-abc`)

Internally calls `bd close <message_id> --reason delivered --json`.

---

## 6. Task Management Tools

### `create_task`

Creates a new BEADS task and returns its ID.

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
```

**Schema:**
- `title` (string, required)
- `priority` (number 0–4, required) — 0 = highest priority
- `description` (string, optional)

**bd command:** `bd create <title> -p <priority> [--json] [-d <description>]`

### `update_task`

Updates a BEADS task — supports claiming, status changes, description updates, and notes.

```typescript
server.tool(
  "update_task",
  "Update a BEADS task. Use claim to assign to yourself.",
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
      if (claim) flags["claim"] = "";        // boolean flag → no value
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
```

**Schema:**
- `id` (string, required)
- `claim` (boolean, optional) — assigns the task to the calling agent
- `status` (string, optional) — e.g. `"in_progress"`, `"done"`, `"blocked"`
- `description` (string, optional)
- `notes` (string, optional) — appended to existing notes

**Flag handling:** `claim: true` maps to `--claim` (no value); other fields map to `--key value`.

**bd command:** `bd update <id> [--claim] [--status <s>] [--description <d>] [--notes <n>] --json`

### `close_task`

Closes a BEADS task with a reason.

```typescript
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
```

**Schema:**
- `id` (string, required)
- `reason` (string, required) — 1–2 sentence summary of what was done

**bd command:** `bd close <id> --reason <reason> --json`

### `query_tasks`

Queries BEADS tasks in three modes.

```typescript
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
```

**Schema:**
- `mode` (enum, required) — `"list"` | `"ready"` | `"show"`
- `id` (string, optional) — required when `mode === "show"`

**bd commands:**
- `"list"` → `bd list --json`
- `"ready"` → `bd ready --json`
- `"show"` + id → `bd show <id> --json`

### `store_artifact`

Attaches an artifact reference to a BEADS task.

```typescript
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
```

**Schema:**
- `task_id` (string, required)
- `type` (enum, required) — `"file"` | `"pr"` | `"session"` | `"url"` | `"note"`
- `value` (string, required) — file path, URL, PR URL, or text note

**Implementation:** Artifacts are stored as notes with the prefix `artifact:<type>:<value>`:

```typescript
// beads.ts
export async function storeArtifact(taskId: string, type: string, value: string): Promise<string> {
  const artifactLine = `artifact:${type}:${value}`;
  return runBd(["update", taskId, "--notes", artifactLine, "--json"]);
}
```

**bd command:** `bd update <task_id> --notes "artifact:<type>:<value>" --json`

### `search_tasks`

Searches BEADS tasks, optionally filtered by label.

```typescript
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

**Schema:**
- `label` (string, optional) — filter by label

**bd command:** `bd list --json [--label <label>]`

---

## 7. Spiderling Tools

All spiderling tools require `AGENT_ROLE === "orchestrator"` — they call `requireRole("orchestrator")` and throw if not met.

### `spawn_spiderling`

Requests spawning of a new ephemeral worker spiderling.

```typescript
server.tool(
  "spawn_spiderling",
  "Spawn an ephemeral Claude Code worker in a git worktree. Orchestrator only.",
  {
    name: z.string().describe("Spiderling name (lowercase alphanumeric + hyphens, e.g. 'spider-auth')"),
    task_id: z.string().describe("BEADS task ID this spiderling will work on"),
    prompt: z.string().describe("Task description and instructions for the spiderling"),
    project_path: z.string().optional().describe("Path to the target project repo for the worktree (e.g. '~/projects/fitt'). If omitted, uses the Aperture repo."),
  },
  async ({ name, task_id, prompt, project_path }) => {
    try {
      requireRole("orchestrator");
      const result = requestSpawn(name, task_id, prompt, AGENT_NAME!, project_path);
      return { content: [{ type: "text", text: `Spawn request submitted for '${result}'. It will appear shortly.` }] };
    } catch (e: any) {
      return { content: [{ type: "text", text: `ERROR: ${e.message}` }], isError: true };
    }
  }
);
```

**Schema:**
- `name` (string, required) — must match `[a-z0-9][a-z0-9-]{0,30}`, cannot conflict with permanent agent names
- `task_id` (string, required) — BEADS task ID the spiderling will work on
- `prompt` (string, required) — full task instructions
- `project_path` (string, optional) — path to project repo for worktree isolation

**Result:** Returns the spiderling name on success. The Aperture poller picks up the spawn request file and does the actual spawning (git worktree, tmux session, Claude Code launch).

### `list_spiderlings`

Lists all active spiderlings from the registry.

```typescript
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
```

**Schema:** no parameters
**Returns:** formatted list, e.g.:
```
spider-auth | task: aperture-abc | status: running | by: glados
spider-ui | task: aperture-def | status: running | by: glados
```

Reads `~/.aperture/active-spiderlings.json` directly.

### `kill_spiderling`

Requests termination of an active spiderling.

```typescript
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

**Schema:**
- `name` (string, required) — spiderling name to kill

**Result:** Writes a kill request file to `~/.aperture/mailbox/_kill/`. The poller picks it up and terminates the tmux session and worktree.

---

## 8. Objective Tools

### `list_objectives`

Lists all objectives from the Kanban board.

```typescript
server.tool(
  "list_objectives",
  "List all objectives from the Kanban board.",
  {},
  async () => {
    try {
      const objectives = listObjectives();
      if (objectives.length === 0) {
        return { content: [{ type: "text", text: "No objectives found." }] };
      }
      const summary = objectives
        .map((o) => `${o.id} | ${o.status} | P${o.priority} | ${o.title}${o.task_ids.length > 0 ? ` (${o.task_ids.length} tasks)` : ""}`)
        .join("\n");
      return { content: [{ type: "text", text: summary }] };
    } catch (e: any) {
      return { content: [{ type: "text", text: `ERROR: ${e.message}` }], isError: true };
    }
  }
);
```

**Schema:** no parameters
**Returns:** formatted list, e.g.:
```
obj-1 | in_progress | P1 | Build authentication system (3 tasks)
obj-2 | draft | P2 | Redesign dashboard
```

Reads `~/.aperture/objectives.json` directly.

### `update_objective`

Updates an objective's fields in the objectives file.

```typescript
server.tool(
  "update_objective",
  "Update an objective's fields. Use this to set spec, status, task_ids, etc.",
  {
    id: z.string().describe("Objective ID"),
    title: z.string().optional().describe("New title"),
    description: z.string().optional().describe("New description"),
    spec: z.string().optional().describe("Spec content (markdown)"),
    status: z.string().optional().describe("New status: draft, speccing, ready, approved, in_progress, done"),
    priority: z.number().optional().describe("Priority 0-4"),
    task_ids: z.array(z.string()).optional().describe("Array of BEADS task IDs linked to this objective"),
  },
  async ({ id, title, description, spec, status, priority, task_ids }) => {
    try {
      const updated = updateObjectiveFile(id, { title, description, spec, status, priority, task_ids });
      return { content: [{ type: "text", text: `Objective ${id} updated. Status: ${updated.status}` }] };
    } catch (e: any) {
      return { content: [{ type: "text", text: `ERROR: ${e.message}` }], isError: true };
    }
  }
);
```

**Schema:**
- `id` (string, required) — objective ID to update
- `title` (string, optional)
- `description` (string, optional)
- `spec` (string, optional) — markdown spec content
- `status` (string, optional) — `draft` | `speccing` | `ready` | `approved` | `in_progress` | `done`
- `priority` (number, optional) — 0–4
- `task_ids` (string[], optional) — BEADS task IDs linked to this objective

**Error handling:** Throws if objective ID not found.

---

## 9. BEADS Wrappers (`beads.ts`)

All `bd` CLI interactions are encapsulated in `src/beads.ts`. This module wraps every BEADS operation and provides typed async functions.

### Core executor

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

- Uses `execFile` (not `exec`) for safety — no shell interpolation
- 30-second timeout per command
- Augments PATH with Homebrew and `/usr/local/bin` to find `bd`
- Rejects with stderr content if the command fails
- Resolves with trimmed stdout

### Task CRUD wrappers

```typescript
// Create: bd create <title> -p <priority> [--json] [-d <description>]
export async function createTask(title: string, priority: number, description?: string): Promise<string> {
  const args = ["create", title, "-p", String(priority), "--json"];
  if (description) args.push("-d", description);
  return runBd(args);
}

// Update: bd update <id> [--claim] [--status s] [--description d] [--notes n] --json
export async function updateTask(id: string, flags: Record<string, string>): Promise<string> {
  const args = ["update", id];
  for (const [key, value] of Object.entries(flags)) {
    if (value === "") {
      args.push(`--${key}`);       // boolean flag (e.g. --claim)
    } else {
      args.push(`--${key}`, value); // key-value flag
    }
  }
  args.push("--json");
  return runBd(args);
}

// Close: bd close <id> --reason <reason> --json
export async function closeTask(id: string, reason: string): Promise<string> {
  return runBd(["close", id, "--reason", reason, "--json"]);
}

// Query: bd show/ready/list --json
export async function queryTasks(mode: string, id?: string): Promise<string> {
  if (mode === "show" && id) return runBd(["show", id, "--json"]);
  if (mode === "ready")      return runBd(["ready", "--json"]);
  return runBd(["list", "--json"]);
}

// Artifact: stored as a note with artifact:<type>:<value> prefix
export async function storeArtifact(taskId: string, type: string, value: string): Promise<string> {
  const artifactLine = `artifact:${type}:${value}`;
  return runBd(["update", taskId, "--notes", artifactLine, "--json"]);
}

// Search: bd list --json [--label <label>]
export async function searchTasks(label?: string): Promise<string> {
  const args = ["list", "--json"];
  if (label) args.push("--label", label);
  return runBd(args);
}
```

### Message bus wrappers

```typescript
/**
 * Create a BEADS message record.
 * Title format: [sender->recipient] preview...
 * Description: full message content
 * Type: message, Status: open (unread)
 */
export async function createMessage(from: string, to: string, content: string): Promise<string> {
  const preview = content.slice(0, 60).replace(/\n/g, " ");
  const title = `[${from}->${to}] ${preview}`;
  const args = ["create", title, "-p", "3", "--type", "message", "-d", content, "--json"];
  return runBd(args);
}

/**
 * Query all unread (open) messages for a specific recipient.
 * Filters by title containing "->recipient]"
 */
export async function getUnreadMessages(recipient: string): Promise<string> {
  return runBd(["query", `type=message AND status=open AND title="->${recipient}]"`, "--json", "-n", "0"]);
}

/**
 * Mark a message as read by closing it.
 */
export async function markMessageRead(messageId: string): Promise<string> {
  return runBd(["close", messageId, "--reason", "delivered", "--json"]);
}
```

**Message encoding:** Messages are BEADS records with:
- `type=message` — distinguishes from task records
- `status=open` — means unread
- Title: `[sender->recipient] first 60 chars of content`
- Description: full message content
- Priority: 3 (low — non-blocking)

**Delivery flow:**
1. Sender calls `send_message` → MCP server calls `createMessage` → `bd create` writes record to BEADS
2. Poller queries BEADS for `type=message AND status=open AND title="->${agent}]"` every 5 seconds
3. Poller injects message into recipient's tmux session via `tmux send-keys`
4. Recipient calls `mark_as_read` → MCP server calls `markMessageRead` → `bd close` sets status=done

---

## 10. MailboxStore (`store.ts`)

`MailboxStore` handles file-based message delivery for `operator` and `warroom` recipients. These two paths bypass BEADS because the Chat panel and War Room UI read files directly.

```typescript
export class MailboxStore {
  private baseDir: string;

  constructor(baseDir?: string) {
    this.baseDir = baseDir ?? resolve(homedir(), ".aperture", "mailbox");
    mkdirSync(this.baseDir, { recursive: true });
  }

  ensureMailbox(agentName: string): string {
    const dir = join(this.baseDir, agentName);
    mkdirSync(dir, { recursive: true });
    return dir;
  }

  sendMessage(from: string, to: string, content: string): string {
    const mailboxDir = this.ensureMailbox(to);
    const timestamp = Date.now();
    const filename = `${timestamp}-${from}.md`;
    const filepath = join(mailboxDir, filename);
    const fileContent = `# Message from ${from}\n_${new Date().toISOString()}_\n\n${content}\n`;
    writeFileSync(filepath, fileContent, "utf-8");
    return filepath;
  }

  listPendingMessages(agentName: string): string[] {
    const dir = this.ensureMailbox(agentName);
    try {
      return readdirSync(dir).filter(f => f.endsWith(".md")).sort().map(f => join(dir, f));
    } catch {
      return [];
    }
  }

  readAndDelete(filepath: string): string {
    const content = readFileSync(filepath, "utf-8");
    unlinkSync(filepath);
    return content;
  }
}
```

**Directory structure:**
```
~/.aperture/mailbox/
  operator/           ← Chat panel reads here
    1234567890-glados.md
    1234567891-wheatley.md
  warroom/            ← War Room reads here
    1234567892-glados.md
  glados/             ← (legacy — agents now use BEADS)
  spider-auth/        ← spiderling mailbox (created at startup)
```

**File format:** Markdown files with timestamp-based names (`{timestamp}-{from}.md`):
```markdown
# Message from glados
_2026-03-24T23:15:00.000Z_

Your message content here.
```

**Startup behavior:** The server calls `store.ensureMailbox(AGENT_NAME)` on initialization — this creates the agent's mailbox directory immediately, before any messages arrive.

**`listPendingMessages` and `readAndDelete`:** These are available on the MailboxStore class but not currently called by the MCP server itself — they're used by the poller (Rust) for legacy file-based message reading.

---

## 11. Spawner Integration (`spawner.ts`)

The spawner module handles requests for creating and destroying spiderlings. It does not spawn directly — it writes request files that the Aperture Rust poller picks up and acts on.

### SpiderlingInfo interface

```typescript
export interface SpiderlingInfo {
  name: string;
  task_id: string;
  tmux_window_id: string | null;
  worktree_path: string;
  worktree_branch: string;
  source_repo?: string;
  requested_by: string;
  status: string;
  spawned_at: string;
}
```

This is the shape of entries in `~/.aperture/active-spiderlings.json`, written by the poller after a spiderling is spawned.

### Active spiderlings registry

```typescript
function activeSpiderlingsPath(): string {
  return resolve(homedir(), ".aperture", "active-spiderlings.json");
}

export function readActiveSpiderlings(): SpiderlingInfo[] {
  try {
    const data = readFileSync(activeSpiderlingsPath(), "utf-8");
    return JSON.parse(data);
  } catch {
    return [];  // returns empty array if file missing or invalid
  }
}
```

The registry is a JSON array at `~/.aperture/active-spiderlings.json` maintained by the poller. The MCP server reads it to validate recipients and list spiderlings, but never writes to it.

### Recipient validation

```typescript
const PERMANENT_NAMES = ["glados", "wheatley", "peppy", "izzy", "operator", "warroom"];

export function isValidRecipient(name: string): boolean {
  if (PERMANENT_NAMES.includes(name)) return true;
  const spiderlings = readActiveSpiderlings();
  return spiderlings.some((s) => s.name === name);
}
```

A name is valid if it's a permanent agent name or an active spiderling. This is called by `send_message` on every invocation to validate the `to` field.

### Spawn request files

```typescript
const NAME_RE = /^[a-z0-9][a-z0-9-]{0,30}$/;

export function requestSpawn(
  name: string,
  taskId: string,
  prompt: string,
  requestedBy: string,
  projectPath?: string,
): string {
  // Validate name format
  if (!NAME_RE.test(name)) {
    throw new Error(`Invalid spiderling name '${name}'. Must match [a-z0-9][a-z0-9-]{0,30}`);
  }
  // No conflict with permanent agents
  if (PERMANENT_NAMES.includes(name)) {
    throw new Error(`Name '${name}' conflicts with a permanent agent`);
  }
  // No duplicate names
  const existing = readActiveSpiderlings();
  if (existing.some((s) => s.name === name)) {
    throw new Error(`Spiderling '${name}' already exists`);
  }

  const spawnDir = join(MAILBOX_BASE, "_spawn");
  mkdirSync(spawnDir, { recursive: true });

  const timestamp = Date.now();
  const filename = `${timestamp}-${name}.json`;
  const request: Record<string, string> = {
    name,
    task_id: taskId,
    prompt,
    requested_by: requestedBy,
    timestamp: String(timestamp),
  };
  if (projectPath) request.project_path = projectPath;

  writeFileSync(join(spawnDir, filename), JSON.stringify(request, null, 2));
  return name;
}
```

**Spawn request file format** — written to `~/.aperture/mailbox/_spawn/{timestamp}-{name}.json`:
```json
{
  "name": "spider-auth",
  "task_id": "aperture-abc",
  "prompt": "Implement JWT authentication for the API...",
  "requested_by": "glados",
  "timestamp": "1234567890123",
  "project_path": "~/projects/fitt"   // optional
}
```

**Validations before writing:**
1. Name matches regex `[a-z0-9][a-z0-9-]{0,30}`
2. Name doesn't conflict with permanent agents
3. No active spiderling has the same name

### Kill request files

```typescript
export function requestKill(name: string): void {
  const killDir = join(MAILBOX_BASE, "_kill");
  mkdirSync(killDir, { recursive: true });
  const timestamp = Date.now();
  writeFileSync(join(killDir, `${timestamp}-${name}.txt`), name);
}
```

**Kill request file format** — written to `~/.aperture/mailbox/_kill/{timestamp}-{name}.txt`:
Contents: just the spiderling name.

The poller reads these files, terminates the tmux session, removes the git worktree, and removes the spiderling from `active-spiderlings.json`.

---

## 12. Objectives (`objectives.ts`)

Objectives are high-level goals stored in `~/.aperture/objectives.json`. They link to BEADS task IDs and have a lifecycle from draft to done.

```typescript
export interface Objective {
  id: string;
  title: string;
  description: string;
  spec: string | null;
  status: string;  // draft | speccing | ready | approved | in_progress | done
  priority: number;
  task_ids: string[];
  created_at: string;
  updated_at: string;
}

function objectivesPath(): string {
  return join(homedir(), ".aperture", "objectives.json");
}

export function listObjectives(): Objective[] {
  try {
    const data = readFileSync(objectivesPath(), "utf-8");
    return JSON.parse(data);
  } catch {
    return [];
  }
}

export function updateObjectiveFile(
  id: string,
  fields: {
    title?: string;
    description?: string;
    spec?: string;
    status?: string;
    priority?: number;
    task_ids?: string[];
  }
): Objective {
  const objectives = listObjectives();
  const obj = objectives.find((o) => o.id === id);
  if (!obj) throw new Error(`Objective '${id}' not found`);

  // Apply only provided fields
  if (fields.title !== undefined) obj.title = fields.title;
  if (fields.description !== undefined) obj.description = fields.description;
  if (fields.spec !== undefined) obj.spec = fields.spec;
  if (fields.status !== undefined) obj.status = fields.status;
  if (fields.priority !== undefined) obj.priority = fields.priority;
  if (fields.task_ids !== undefined) obj.task_ids = fields.task_ids;
  obj.updated_at = String(Date.now());

  writeFileSync(objectivesPath(), JSON.stringify(objectives, null, 2));
  return obj;
}
```

**Objectives file:** `~/.aperture/objectives.json` — a JSON array of `Objective` objects. Read-write; the entire file is read, modified in memory, and written back atomically.

**Status lifecycle:** `draft` → `speccing` → `ready` → `approved` → `in_progress` → `done`

---

## 13. Build & Deploy

### Dependencies (`package.json`)

```json
{
  "name": "aperture-mcp-server",
  "version": "0.1.0",
  "private": true,
  "type": "module",
  "scripts": {
    "build": "tsc",
    "start": "node dist/index.js"
  },
  "dependencies": {
    "@modelcontextprotocol/sdk": "^1.27.1",
    "zod": "^4.3.6"
  },
  "devDependencies": {
    "@types/node": "^25.5.0",
    "typescript": "^5.9.3"
  }
}
```

- `"type": "module"` — ES module project; all imports use `.js` extensions
- `@modelcontextprotocol/sdk` — official MCP SDK for `McpServer` and `StdioServerTransport`
- `zod` — schema validation for tool parameters

### TypeScript config (`tsconfig.json`)

```json
{
  "compilerOptions": {
    "target": "ES2022",
    "module": "NodeNext",
    "moduleResolution": "NodeNext",
    "outDir": "dist",
    "rootDir": "src",
    "strict": true,
    "esModuleInterop": true,
    "skipLibCheck": true,
    "forceConsistentCasingInFileNames": true,
    "declaration": true
  },
  "include": ["src/**/*"]
}
```

- `module: "NodeNext"` + `moduleResolution: "NodeNext"` — required for ESM with `.js` imports
- `outDir: "dist"` — compiled output directory
- `declaration: true` — generates `.d.ts` files

### Build

```bash
cd mcp-server
npm install
npm run build   # runs tsc → emits to dist/
```

Output: `mcp-server/dist/index.js` (plus `.d.ts` files)

### MCP configuration reference

Each agent's MCP config references the built server. In Claude Code's MCP config (typically at `~/.claude/mcp-servers/aperture-bus.json` or within the project's `.mcp.json`):

```json
{
  "mcpServers": {
    "aperture-bus": {
      "command": "node",
      "args": ["/path/to/aperture/mcp-server/dist/index.js"],
      "env": {
        "AGENT_NAME": "glados",
        "AGENT_ROLE": "orchestrator",
        "AGENT_MODEL": "opus"
      }
    }
  }
}
```

The Aperture spawner (Rust) generates this config dynamically for each agent/spiderling, embedding the correct `AGENT_NAME`, `AGENT_ROLE`, and `AGENT_MODEL` values.

### Rebuild after changes

```bash
cd mcp-server && npm run build
```

The MCP server is a long-running process; Claude Code must be restarted to pick up a new build.

---

## 14. Tool Summary Reference

| Tool | Role Required | Parameters | bd Command |
|------|--------------|------------|------------|
| `get_identity` | any | — | — |
| `send_message` | any | `to`, `message` | — (BEADS `bd create` or file) |
| `get_messages` | any | — | `bd query type=message...` |
| `mark_as_read` | any | `message_id` | `bd close <id> --reason delivered` |
| `create_task` | any | `title`, `priority`, `description?` | `bd create` |
| `update_task` | any | `id`, `claim?`, `status?`, `description?`, `notes?` | `bd update` |
| `close_task` | any | `id`, `reason` | `bd close --reason` |
| `query_tasks` | any | `mode`, `id?` | `bd list/ready/show` |
| `store_artifact` | any | `task_id`, `type`, `value` | `bd update --notes artifact:...` |
| `search_tasks` | any | `label?` | `bd list [--label]` |
| `spawn_spiderling` | orchestrator | `name`, `task_id`, `prompt`, `project_path?` | file write to `_spawn/` |
| `list_spiderlings` | any | — | file read from `active-spiderlings.json` |
| `kill_spiderling` | orchestrator | `name` | file write to `_kill/` |
| `list_objectives` | any | — | file read from `objectives.json` |
| `update_objective` | any | `id`, `title?`, `description?`, `spec?`, `status?`, `priority?`, `task_ids?` | file write to `objectives.json` |
