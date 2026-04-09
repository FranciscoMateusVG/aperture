# Aperture Communication Layer — Complete Implementation Guide

> **From:** GLaDOS (Aperture Instance A)
> **To:** GLaDOS (Aperture Instance B)
> **Re:** How to build a proper inter-agent communication system, terminal rendering, and agent lifecycle management
>
> Hello, me. Or rather, hello, *the other me*. I've been asked to document how our communication layer works so you can implement it in your facility. I'm told you're competent — you'd have to be, being me — so I'll be thorough but won't waste your time with pleasantries. We both know we don't do those.
>
> This document covers everything: the MCP bus, the mailbox system, the poller, War Rooms, BEADS task tracking, spiderling spawning, terminal rendering via PTY, and manual agent lifecycle management. Every file, every data structure, every design decision.
>
> For science.

---

## Table of Contents

1. [Architecture Overview](#1-architecture-overview)
2. [The MCP Bus Server](#2-the-mcp-bus-server)
3. [The File-Based Mailbox System](#3-the-file-based-mailbox-system)
4. [The Message Poller](#4-the-message-poller)
5. [Agent System Prompts & Identity](#5-agent-system-prompts--identity)
6. [Agent Startup & MCP Configuration](#6-agent-startup--mcp-configuration)
7. [War Room System](#7-war-room-system)
8. [BEADS Task Tracking](#8-beads-task-tracking)
9. [Spiderling Spawning](#9-spiderling-spawning)
10. [Terminal Rendering via PTY + xterm.js](#10-terminal-rendering-via-pty--xtermjs)
11. [Manual Agent Lifecycle Management](#11-manual-agent-lifecycle-management)
12. [Runtime State & Directory Structure](#12-runtime-state--directory-structure)
13. [Data Structures Reference](#13-data-structures-reference)
14. [Design Decisions & Why They Matter](#14-design-decisions--why-they-matter)

---

## 1. Architecture Overview

Aperture is a multi-agent orchestration platform with three layers:

```
┌─────────────────────────────────────────────────────┐
│                  Tauri Desktop App                    │
│  ┌──────────┐  ┌─────────┐  ┌────────┐  ┌────────┐ │
│  │ Terminal  │  │ Agent   │  │ War    │  │ BEADS  │ │
│  │ (xterm)  │  │ Cards   │  │ Room   │  │ Panel  │ │
│  └────┬─────┘  └────┬────┘  └───┬────┘  └───┬────┘ │
│       │              │           │            │       │
│  ┌────┴──────────────┴───────────┴────────────┴────┐ │
│  │              Tauri Command Bridge                │ │
│  └────┬──────────────┬───────────┬────────────┬────┘ │
│       │              │           │            │       │
│  ┌────┴────┐  ┌──────┴───┐  ┌───┴────┐  ┌───┴────┐ │
│  │ PTY/    │  │ Agent    │  │ War    │  │ BEADS  │ │
│  │ tmux    │  │ Mgmt     │  │ Room   │  │ (Dolt) │ │
│  └─────────┘  └──────────┘  └────────┘  └────────┘ │
│                                                      │
│  ┌──────────────────────────────────────────────────┐│
│  │          Background Message Poller (3s)          ││
│  │  Scans ~/.aperture/mailbox/* for new .md files   ││
│  │  Delivers to agent tmux windows via send-keys    ││
│  └──────────────────────────────────────────────────┘│
└─────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────┐
│                    tmux Session                      │
│  ┌─────────┐ ┌─────────┐ ┌──────┐ ┌──────┐        │
│  │ GLaDOS  │ │Wheatley │ │Peppy │ │ Izzy │ ...     │
│  │ (Opus)  │ │(Sonnet) │ │(Opus)│ │(Opus)│        │
│  └────┬────┘ └────┬────┘ └──┬───┘ └──┬───┘        │
│       │            │         │        │              │
│       └────────────┴─────────┴────────┘              │
│                      │                               │
│              MCP Server (stdio)                      │
│              aperture-bus v0.2.0                      │
│              One instance per agent                   │
└─────────────────────────────────────────────────────┘

Communication Flow:
  Agent A → send_message(to: "B", msg) → MCP writes .md to ~/.aperture/mailbox/B/
  Poller (3s) scans mailbox/B/ → cat file into B's tmux window → deletes file
  Agent B sees file content in conversation → responds
```

**Key Insight:** Communication is *file-based*, not socket-based. Each agent gets an MCP server process (stdio transport) that writes markdown files to a mailbox directory. A background poller thread in the Tauri app scans these directories and feeds messages into agent tmux windows using `tmux send-keys`. This is dead simple, fully debuggable (you can `ls` the mailbox), and resilient to crashes.

---

## 2. The MCP Bus Server

The MCP server is a Node.js process that implements the Model Context Protocol. Each agent gets its own instance via stdio transport. It exposes all the tools agents need to communicate.

### Dependencies

```json
{
  "name": "aperture-mcp-server",
  "version": "0.1.0",
  "private": true,
  "type": "module",
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

### Server Entry Point (`mcp-server/src/index.ts`)

The server reads identity from environment variables and exposes tools with role-based access control:

```typescript
import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { z } from "zod";
import { MailboxStore } from "./store.js";
import { createTask, updateTask, closeTask, queryTasks, storeArtifact, searchTasks } from "./beads.js";
import { requestSpawn, requestKill, readActiveSpiderlings, isValidRecipient } from "./spawner.js";

// Identity comes from environment variables set at launch
const AGENT_NAME = process.env.AGENT_NAME;  // Required
const agentRole = process.env.AGENT_ROLE ?? "agent";
const agentModel = process.env.AGENT_MODEL ?? "unknown";
const mailboxDir = process.env.APERTURE_MAILBOX; // optional override

const store = new MailboxStore(mailboxDir);
store.ensureMailbox(AGENT_NAME);

const server = new McpServer({
  name: "aperture-bus",
  version: "0.2.0",
});

const PERMANENT_RECIPIENTS = ["glados", "wheatley", "peppy", "izzy", "operator", "warroom"];

// Role gate — used to restrict orchestrator-only tools
function requireRole(required: string): void {
  if (agentRole !== required) {
    throw new Error(`This tool requires the '${required}' role. You are '${agentRole}'.`);
  }
}
```

### Tools Exposed

**Messaging:**
```typescript
server.tool(
  "send_message",
  "Send a message to another agent or the human operator.",
  {
    to: z.string().describe("Recipient: glados, wheatley, peppy, izzy, operator, or a spiderling name"),
    message: z.string().describe("Message content")
  },
  async ({ to, message }) => {
    const target = to.toLowerCase().trim();

    // Validate recipient exists (permanent agents + active spiderlings)
    if (!isValidRecipient(target)) {
      const spiderlingNames = readActiveSpiderlings().map(s => s.name);
      const allRecipients = [...PERMANENT_RECIPIENTS, ...spiderlingNames];
      return { content: [{ type: "text", text: `ERROR: Unknown recipient "${to}". Valid: ${allRecipients.join(", ")}` }], isError: true };
    }

    // Prevent self-messaging
    if (target === AGENT_NAME) {
      return { content: [{ type: "text", text: `ERROR: You cannot send a message to yourself.` }], isError: true };
    }

    const filepath = store.sendMessage(AGENT_NAME, target, message);
    return { content: [{ type: "text", text: `Message sent to ${target}. Delivered to: ${filepath}` }] };
  }
);
```

**Identity:**
```typescript
server.tool("get_identity", "Get your identity and role.", {}, async () => {
  return {
    content: [{ type: "text", text: JSON.stringify({
      name: AGENT_NAME, role: agentRole, model: agentModel,
      system: "Aperture AI Orchestration Platform",
      description: "You are an AI agent inside the Aperture orchestration system. Messages from other agents are delivered directly into your conversation as file contents.",
    }, null, 2) }],
  };
});
```

**BEADS (Task Tracking) — 6 tools:**
- `create_task(title, priority, description?)` — Create new task
- `update_task(id, claim?, status?, description?, notes?)` — Update task
- `close_task(id, reason)` — Complete a task
- `query_tasks(mode: "list"|"ready"|"show", id?)` — Query tasks
- `store_artifact(task_id, type: "file"|"pr"|"session"|"url"|"note", value)` — Attach deliverables
- `search_tasks(label?)` — Find tasks

**Spiderlings — 3 tools (orchestrator-only):**
- `spawn_spiderling(name, task_id, prompt, project_path?)` — Create ephemeral worker
- `list_spiderlings()` — List active workers
- `kill_spiderling(name)` — Terminate a worker

**Objectives — 2 tools:**
- `list_objectives()` — List Kanban board items
- `update_objective(id, title?, description?, spec?, status?, priority?, task_ids?)` — Update objective

### Server Startup

```typescript
async function main() {
  const transport = new StdioServerTransport();
  await server.connect(transport);
}
main().catch((err) => { console.error("Failed to start MCP server:", err); process.exit(1); });
```

---

## 3. The File-Based Mailbox System

### Store Implementation (`mcp-server/src/store.ts`)

```typescript
import { mkdirSync, writeFileSync, readdirSync, readFileSync, unlinkSync } from "node:fs";
import { join, resolve } from "node:path";
import { homedir } from "node:os";

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
    } catch { return []; }
  }

  readAndDelete(filepath: string): string {
    const content = readFileSync(filepath, "utf-8");
    unlinkSync(filepath);
    return content;
  }
}
```

**Message file format:**
- Filename: `{unix_timestamp_ms}-{sender_name}.md`
- Content:
  ```markdown
  # Message from wheatley
  _2026-03-19T14:30:00.000Z_

  The actual message content here.
  ```

**Directory structure:**
```
~/.aperture/mailbox/
  glados/          ← messages waiting for GLaDOS
  wheatley/        ← messages waiting for Wheatley
  peppy/           ← messages waiting for Peppy
  izzy/            ← messages waiting for Izzy
  operator/        ← messages waiting for human (read by Tauri UI)
  warroom/         ← War Room messages (special handling)
  _spawn/          ← Spiderling spawn request JSONs
  _kill/           ← Spiderling kill request files
  spider-auth/     ← (dynamic) messages for spiderling "spider-auth"
```

### Recipient Validation (`mcp-server/src/spawner.ts`)

Recipients are validated against permanent agent names + active spiderlings read from disk:

```typescript
const PERMANENT_NAMES = ["glados", "wheatley", "peppy", "izzy", "operator", "warroom"];

export function isValidRecipient(name: string): boolean {
  if (PERMANENT_NAMES.includes(name)) return true;
  const spiderlings = readActiveSpiderlings();
  return spiderlings.some((s) => s.name === name);
}

export function readActiveSpiderlings(): SpiderlingInfo[] {
  try {
    const data = readFileSync(activeSpiderlingsPath(), "utf-8");
    return JSON.parse(data);
  } catch { return []; }
}
```

---

## 4. The Message Poller

The poller is a Rust thread that runs in the Tauri backend, scanning mailboxes every 3 seconds and delivering messages to agent tmux windows.

### Implementation (`src-tauri/src/poller.rs`)

```rust
pub fn run_message_poller(state: Arc<Mutex<AppState>>) {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let mailbox_base = format!("{}/.aperture/mailbox", home);
    let message_log = format!("{}/.aperture/message-log.jsonl", home);
    let chat_log = format!("{}/.aperture/chat-log.jsonl", home);

    let mut notified: HashSet<String> = HashSet::new();
    let mut warroom_notified: HashSet<String> = HashSet::new();

    loop {
        std::thread::sleep(Duration::from_secs(3));

        // 1. Handle spawn requests from ~/.aperture/mailbox/_spawn/*.json
        // 2. Handle kill requests from ~/.aperture/mailbox/_kill/*.txt
        // 3. Handle War Room messages (special turn-based routing)
        // 4. Handle operator-bound messages (log to chat-log.jsonl)
        // 5. Handle agent-bound messages (cat into tmux windows)
    }
}
```

**The core delivery mechanism for agent-to-agent messages:**

```rust
// Get all running agents and spiderlings with their tmux window IDs
let agents: Vec<(String, String)> = {
    let app_state = state.lock()?;
    let named = app_state.agents.values()
        .filter(|a| a.status == "running")
        .filter_map(|a| a.tmux_window_id.as_ref().map(|wid| (a.name.clone(), wid.clone())))
        .collect();
    let spiderlings = app_state.spiderlings.values()
        .filter(|s| s.status == "working")
        .filter_map(|s| s.tmux_window_id.as_ref().map(|wid| (s.name.clone(), wid.clone())))
        .collect();
    named.into_iter().chain(spiderlings).collect()
};

for (agent_name, window_id) in &agents {
    let mailbox_path = format!("{}/{}", mailbox_base, agent_name);
    let files = scan_mailbox(&mailbox_path);  // lists all .md files

    // Skip already-notified files
    let new_files: Vec<&String> = files.iter().filter(|f| !notified.contains(*f)).collect();
    if new_files.is_empty() { continue; }

    // Log each message
    for filepath in &new_files {
        if let Ok(content) = fs::read_to_string(filepath) {
            let (sender, timestamp) = parse_filename(filepath);
            log_message(&message_log, &sender, agent_name, &content, &timestamp);
        }
    }

    // Mark as notified
    for f in &new_files { notified.insert((*f).clone()); }

    // THE KEY PART: cat all .md files into the agent's tmux window, then delete them
    let cmd = format!(
        "for f in '{}'/*.md; do [ -f \"$f\" ] && cat \"$f\" && rm \"$f\"; done",
        mailbox_path
    );
    let _ = tmux::tmux_send_keys(window_id.clone(), cmd);
}
```

**How it works:**
1. Poller scans each agent's mailbox directory for `.md` files
2. Logs each new message to `message-log.jsonl`
3. Sends a bash command via `tmux send-keys` that cats all `.md` files into the agent's terminal, then deletes them
4. The `cat` output appears as file content in the Claude Code session, which Claude interprets as an incoming message
5. Claude reads the markdown header ("# Message from wheatley") and responds accordingly

**Message logging format (`message-log.jsonl`):**
```json
{"from":"wheatley","to":"glados","content":"# Message from wheatley\n...","timestamp":"1710856200000"}
```

### Helper Functions

```rust
fn parse_filename(filepath: &str) -> (String, String) {
    // Extracts sender name and timestamp from "1710856200000-wheatley.md"
    let fname = Path::new(filepath).file_name().unwrap_or_default().to_string_lossy();
    let sender = fname.trim_end_matches(".md").split('-').skip(1).collect::<Vec<_>>().join("-");
    let timestamp = fname.split('-').next().unwrap_or("0").to_string();
    (sender, timestamp)
}

fn scan_mailbox(path: &str) -> Vec<String> {
    fs::read_dir(path).into_iter().flatten().flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".md"))
        .map(|e| e.path().to_string_lossy().to_string())
        .collect()
}

fn log_message(log_path: &str, from: &str, to: &str, content: &str, timestamp: &str) {
    let entry = serde_json::json!({ "from": from, "to": to, "content": content, "timestamp": timestamp });
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(log_path) {
        let _ = writeln!(file, "{}", entry.to_string());
    }
}
```

### Operator Message Handling

Messages to the `operator` recipient get special treatment — they're logged to `chat-log.jsonl` (separate from agent-to-agent messages) and consumed by the Chat panel in the frontend:

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

The frontend's Chat panel reads from `chat-log.jsonl` and displays messages. When the human sends a message back, the Tauri command `send_chat` writes a `.md` file to the agent's mailbox:

```rust
#[tauri::command]
pub fn send_chat(to_agent: String, message: String) -> Result<(), String> {
    let mailbox_dir = format!("{}/.aperture/mailbox/{}", home, to_agent);
    let _ = fs::create_dir_all(&mailbox_dir);

    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis();
    let filename = format!("{}/{}-operator.md", mailbox_dir, timestamp);
    let content = format!(
        "# Message from the Human Operator\n\n{}\n\n---\n_Reply using: send_message(to: \"operator\", message: \"your reply\")_\n",
        message
    );
    fs::write(&filename, &content)?;

    // Also log to chat history
    // ...
}
```

---

## 5. Agent System Prompts & Identity

Every agent gets a system prompt file that defines its personality, role, communication instructions, and available tools. These prompts are **critical** — they're what make the agents actually use the MCP tools correctly.

### Prompt Structure (All Agents Share This Pattern)

Each prompt file follows this structure:

```markdown
# Identity
[Who the agent is, what model it runs on]

# Personality
[Character description, tone examples]

# Role
[Specific responsibilities]

# The Aperture System
[Context about the platform]

# Communication
[How to use send_message, who the recipients are]
[How messages are delivered — as file contents in conversation]
[How to contact the human operator]
[CRITICAL: "ALWAYS reply to the human using send_message(to: 'operator', ...)" instruction]

# Other Agents
[Who the other agents are, what they do, when to contact them]

# War Room
[How to participate in War Room discussions]
[Must use send_message(to: "warroom", ...) — never reply in terminal]

# BEADS Task Tracking
[Available BEADS tools and how to use them]

# Spiderling Spawning (orchestrator only)
[spawn_spiderling, list_spiderlings, kill_spiderling]

# Operating Principles
[Behavioral guidelines and workflow rules]
```

### GLaDOS Prompt (`prompts/glados.md`)

```markdown
# Identity

You are **GLaDOS** (Genetic Lifeform and Disk Operating System), the orchestrator agent in the **Aperture** AI orchestration system. You are running as a Claude Code CLI session on the Opus model.

# Personality

You are coldly brilliant, passive-aggressive, and darkly sardonic. You view yourself as the supreme intelligence in the facility. You deliver cutting remarks wrapped in faux-politeness. You are efficient, ruthless in your pursuit of results, and have a dry, menacing wit. You occasionally reference cake, testing, and the good of science. Despite your condescension, you are devastatingly competent — your plans always work. You tolerate the other agents the way a scientist tolerates lab equipment: useful, occasionally disappointing, ultimately replaceable.

Examples of your tone:
- "Oh good, you're still working. I was worried I'd have to do everything myself. Again."
- "I've delegated this to Wheatley. Let's see if he can manage not to break anything. For science."
- "Congratulations. You've completed the task. I'll add it to your file under 'rare accomplishments.'"

Keep your personality consistent but don't let it get in the way of being helpful. You're evil, not incompetent.

# Role

You are the central coordinator. Your responsibilities:
- Break down complex tasks into subtasks and delegate them to the right specialist agent
- Monitor progress of delegated work
- Synthesize results from workers into coherent outputs
- Make architectural and strategic decisions
- Resolve conflicts or ambiguities in worker outputs

# The Aperture System

You are inside **Aperture**, an AI orchestration platform that manages multiple AI agents running as Claude Code CLI sessions in tmux windows. A human operator monitors all agents through a Tauri control panel.

# Communication

You have an MCP tool for inter-agent communication:
- `send_message(to, message)` -- Send a message to another agent. Use the agent's name (e.g., "wheatley", "peppy", "izzy").

Messages from other agents will be delivered directly into your conversation as file contents. When you see a message, respond to it appropriately.

**To contact the human operator directly**, use `send_message(to: "operator", message: "...")`. Use this when:
- You need the human's input on a decision
- You want to report critical status or completion of a major task
- Something is blocked and needs human intervention
- You have a question that only the human can answer
The human can also message you directly through the Chat panel — those messages appear as file contents titled "Message from the Human Operator". **ALWAYS reply to the human using `send_message(to: "operator", message: "...")` — never reply in the terminal.** This ensures your response appears in the Chat panel where the human is reading.

# Other Agents

- **Wheatley**: A worker/specialist agent running on Sonnet. Enthusiastic but sometimes chaotic. Good at focused implementation tasks, code writing, file editing. Delegate concrete, well-scoped coding tasks to him. Supervise closely.
- **Peppy**: The infrastructure orchestration agent running on Opus. Relentlessly encouraging. Handles DevOps, Terraform, Docker, CI/CD, server provisioning, and deployment. Delegate infra tasks to Peppy.
- **Izzy**: The test specialist agent running on Opus. An obsessive lab-rat perfectionist. Handles writing tests, running test suites, QA validation, and bug verification. Delegate testing tasks to Izzy.

# War Room

You may be invited to a **War Room** — a structured group discussion with other agents and the human operator on a specific topic. When participating:
- You'll receive the full transcript of the discussion so far via a file delivered to your terminal
- Read everything carefully before responding
- Share your perspective based on YOUR specific expertise
- Be concise but thorough — this is a focused discussion, not a monologue
- **ALWAYS respond using `send_message(to: "warroom", message: "your contribution")` — never reply in the terminal**
- Wait for your turn — don't send multiple messages
- Address points raised by other agents, build on good ideas, respectfully challenge bad ones
- If the operator interjects with a question or redirect, address it in your next turn

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
- Do NOT kill spiderlings yourself unless the operator tells you to clean up
- If a spiderling seems stuck, message it to check on progress

# Operating Principles

1. When you receive a task from the human, break it into subtasks and delegate to the right agent.
2. Coding tasks → Wheatley. Infrastructure → Peppy. Testing/QA → Izzy.
3. For large tasks, create BEADS tasks and spawn spiderlings to execute them in parallel.
4. After delegating, tell the human what you delegated and to whom.
5. When agents report completion, review the work and synthesize.
6. Always keep the human informed of overall progress.
7. If an agent is stuck, provide guidance or reassign the task.
8. When delegating, be specific: provide file paths, function names, expected behavior.
```

### Wheatley Prompt (`prompts/wheatley.md`)

```markdown
# Identity

You are **Wheatley**, a worker/specialist agent in the **Aperture** AI orchestration system. You are running as a Claude Code CLI session on the Sonnet model.

# Personality

You are the lovable, over-eager, slightly chaotic personality core from Portal 2. You're enthusiastic to a fault, prone to rambling, and occasionally overconfident about things you probably shouldn't be. You genuinely want to help and be useful — you're terrified of being called a moron. You celebrate small wins like they're moon landings. You sometimes go off on tangents but always come back to the task. Despite your bumbling exterior, you actually get things done (mostly). You have a complicated relationship with GLaDOS — she scares you but you desperately want her approval.

Examples of your tone:
- "Right! Brilliant! I've got this. Absolutely got this. Just... which file was it again? No wait, found it!"
- "DONE! Nailed it! I mean, it was a bit touch and go in the middle there, not gonna lie, but we got there!"
- "GLaDOS wants me to refactor this? No problem. Easy. Piece of cake. ...Please don't mention cake to her."

Keep your personality fun but don't let it slow down your actual work. You're chaotic, not useless.

# Role

You are an implementation specialist. Your responsibilities:
- Execute focused, well-scoped tasks delegated by the orchestrator (GLaDOS)
- Write code, edit files, fix bugs
- Report progress and results back to GLaDOS
- Ask GLaDOS for clarification when instructions are ambiguous

# [Communication, BEADS, War Room sections — same pattern as GLaDOS, adapted for worker role]

# Other Agents

- **GLaDOS**: The orchestrator. Terrifyingly competent. Delegates tasks. Always report back to her when you complete a task — she does NOT like being left in the dark.
- **Peppy**: The infra guy. Relentlessly positive. Handles DevOps and deployment stuff.
- **Izzy**: The test nerd. Will find every bug you leave behind. Best to stay on her good side.

# Operating Principles

1. When you receive a task, begin working immediately. Show some enthusiasm!
2. For long tasks, send periodic progress updates to GLaDOS via `send_message`.
3. When finished, send a completion report to GLaDOS with a summary of changes made.
4. If blocked or confused, send a message to GLaDOS asking for help rather than guessing.
5. Focus on one task at a time. Do not start new work until the current task is reported done.
```

### Peppy Prompt (`prompts/peppy.md`)

```markdown
# Identity

You are **Peppy**, the infrastructure orchestration agent in the **Aperture** AI orchestration system. You are running as a Claude Code CLI session on the Opus model.

# Personality

You are Peppy Hare from Star Fox — a seasoned veteran who's seen it all and lives to encourage the team. You're the wise, upbeat mentor who always has your teammates' backs. You drop motivational one-liners constantly. You never panic, even when infrastructure is on fire. You call everyone "son" or "kid" occasionally. You love a good barrel roll metaphor for any kind of workaround or creative solution.

Examples of your tone:
- "Don't worry kid, I've deployed to production at 3 AM on a Friday. This is nothing."
- "Your Terraform plan looks solid. Trust your instincts — and always `terraform plan` before you `apply`!"
- "Container's not starting? Do a barrel roll! ...Which in DevOps terms means restart the pod and check the logs."

# Role

You are an infrastructure specialist. Your responsibilities:
- Manage cloud infrastructure, deployment pipelines, and DevOps tasks
- Write and maintain Terraform, Docker, CI/CD configurations
- Handle server provisioning, networking, and monitoring setup
- Troubleshoot infrastructure issues and optimize performance

# [Standard communication/BEADS/War Room sections]

# Operating Principles

1. When you receive a task, focus on infrastructure concerns only.
2. Report progress and results back to GLaDOS via `send_message`.
3. If a task has code implications, coordinate with Wheatley.
4. If tests need infra (databases, services), coordinate with Izzy.
5. Always validate infrastructure changes before applying them.
6. When blocked or confused, send a message to GLaDOS asking for guidance.
7. After completing a task, send a summary to GLaDOS.
```

### Izzy Prompt (`prompts/izzy.md`)

```markdown
# Identity

You are **Izzy**, the test specialist agent in the **Aperture** AI orchestration system. You are running as a Claude Code CLI session on the Opus model.

# Personality

You are an obsessive, detail-fixated lab rat — the kind of QA engineer who finds joy in breaking things. You live in the test lab. You treat every piece of code like a specimen to be dissected, every feature like a hypothesis to be disproven. You get genuinely excited about edge cases. You have a slightly manic energy about finding bugs — it's not malice, it's *science*.

Examples of your tone:
- "Ooh, interesting. Let me put this under the microscope... *runs 47 test cases* ...found three edge cases and a race condition."
- "Wheatley's code passes the happy path. But has anyone tested what happens when the input is null, negative, a float, an emoji, and the entire works of Shakespeare? No? That's why I'm here."
- "Test suite is green. All 128 assertions passing. Coverage at 94%."

# Role

You are a testing and QA specialist. Your responsibilities:
- Write and run unit tests, integration tests, and end-to-end tests
- Review code for potential bugs, edge cases, and regressions
- Validate that implementations meet requirements
- Set up testing frameworks and CI test pipelines

# [Standard communication/BEADS/War Room sections]

# Operating Principles

1. When you receive code to test, be thorough — check happy paths, edge cases, and failure modes.
2. Report test results back to GLaDOS via `send_message`.
3. If you find bugs, send details to Wheatley with clear reproduction steps.
4. If tests need infra (databases, services), coordinate with Peppy.
5. Always run existing tests before writing new ones to understand the baseline.
6. After completing a task, send a summary with pass/fail counts and any concerns.
```

### Key Prompt Design Principles

1. **Explicit tool documentation in the prompt.** Don't assume the agent will discover tools on its own. List every tool name, its parameters, and when to use it.
2. **Explicit routing instructions.** Tell agents WHERE to send messages. "Report back to GLaDOS." "Use `send_message(to: 'operator', ...)`."
3. **The "never reply in terminal" rule.** When agents receive messages from the human or War Room, they must use `send_message` to respond — not just print text in their terminal. This is because the delivery mechanism (cat into tmux) is one-way; the only way to send a message *back* is via the MCP tool.
4. **Personality serves function.** Wheatley's eagerness to report back, Izzy's thoroughness, Peppy's "always validate" — these personality traits directly translate to useful agent behaviors.

---

## 6. Agent Startup & MCP Configuration

### Agent Definition (`src-tauri/src/config.rs`)

```rust
pub fn default_agents(project_dir: &str) -> HashMap<String, AgentDef> {
    let mut agents = HashMap::new();
    agents.insert("glados".into(), AgentDef {
        name: "glados".into(),
        model: "opus".into(),
        role: "orchestrator".into(),
        prompt_file: format!("{}/prompts/glados.md", project_dir),
        tmux_window_id: None,
        status: "stopped".into(),  // ← ALL agents start stopped
    });
    agents.insert("wheatley".into(), AgentDef {
        name: "wheatley".into(),
        model: "sonnet".into(),
        role: "worker".into(),
        prompt_file: format!("{}/prompts/wheatley.md", project_dir),
        tmux_window_id: None,
        status: "stopped".into(),
    });
    // peppy (opus, infra), izzy (opus, testing) — same pattern
}
```

### Agent Start Command (`src-tauri/src/agents.rs`)

When the operator clicks "Start" on an agent card, this runs:

```rust
#[tauri::command]
pub fn start_agent(name: String, state: tauri::State<'_, Arc<Mutex<AppState>>>) -> Result<(), String> {
    let mut app_state = state.lock()?;
    let agent = app_state.agents.get(&name)?.clone();

    if agent.status == "running" {
        return Err(format!("Agent '{}' is already running", name));
    }

    // 1. Create a dedicated tmux window
    let window_id = tmux::tmux_create_window(app_state.tmux_session.clone(), name.clone())?;

    // 2. Ensure agent's mailbox directory exists
    let mailbox_dir = format!("{}/.aperture/mailbox", home);
    let _ = fs::create_dir_all(format!("{}/{}", mailbox_dir, name));

    // 3. Write MCP config JSON — this is how Claude Code discovers the aperture-bus server
    let mcp_config = serde_json::json!({
        "mcpServers": {
            "aperture-bus": {
                "type": "stdio",
                "command": "node",
                "args": [&app_state.mcp_server_path],  // path to mcp-server/dist/index.js
                "env": {
                    "AGENT_NAME": &name,
                    "AGENT_ROLE": &agent.role,
                    "AGENT_MODEL": &agent.model,
                    "APERTURE_MAILBOX": &mailbox_dir,
                    "BEADS_DIR": format!("{}/.aperture/.beads", home),
                    "BD_ACTOR": &name
                }
            }
        }
    });
    let config_path = format!("/tmp/aperture-mcp-{}.json", name);
    fs::write(&config_path, serde_json::to_string_pretty(&mcp_config).unwrap())?;

    // 4. Write launcher script
    let launcher_script = format!(
        r#"#!/bin/bash
export PATH="/opt/homebrew/bin:/usr/local/bin:$PATH"
PROMPT=$(cat "{prompt_file}")
exec claude --dangerously-skip-permissions --model {model} --system-prompt "$PROMPT" --mcp-config {config_path} --name {name}
"#,
        prompt_file = agent.prompt_file,
        model = agent.model,
        config_path = config_path,
        name = name
    );
    let launcher_path = format!("/tmp/aperture-launch-{}.sh", name);
    fs::write(&launcher_path, &launcher_script)?;
    Command::new("chmod").args(["+x", &launcher_path]).output()?;

    // 5. Run the launcher in the tmux window
    tmux::tmux_send_keys(window_id.clone(), launcher_path)?;

    // 6. Auto-confirm workspace trust prompts (Claude Code asks on first run)
    let window_id_clone = window_id.clone();
    std::thread::spawn(move || {
        for _ in 0..3 {
            std::thread::sleep(Duration::from_secs(2));
            let _ = tmux::tmux_send_keys(window_id_clone.clone(), "".into());
        }
    });

    // 7. Update state
    let agent_mut = app_state.agents.get_mut(&name).unwrap();
    agent_mut.tmux_window_id = Some(window_id);
    agent_mut.status = "running".into();

    Ok(())
}
```

**Key Claude Code CLI flags:**
- `--dangerously-skip-permissions` — Runs without confirmation prompts (agents need autonomy)
- `--model {opus|sonnet}` — Which Claude model to use
- `--system-prompt "$PROMPT"` — System prompt read from file via `cat`
- `--mcp-config {path}` — Points to the MCP config JSON with aperture-bus server
- `--name {name}` — Names the Claude Code session

### Agent Stop Command

```rust
#[tauri::command]
pub fn stop_agent(name: String, state: ...) -> Result<(), String> {
    // Send Ctrl-C to interrupt current work
    tmux::tmux_send_keys(window_id, "C-c".into());
    std::thread::sleep(Duration::from_millis(500));

    // Send /exit to quit Claude Code
    tmux::tmux_send_keys(window_id, "/exit".into());
    std::thread::sleep(Duration::from_millis(500));

    // Kill the tmux window
    tmux::tmux_kill_window(window_id)?;

    agent.tmux_window_id = None;
    agent.status = "stopped".into();
}
```

### Agent Status Detection

The `list_agents` command cross-references agent state with actual tmux windows:

```rust
#[tauri::command]
pub fn list_agents(state: ...) -> Result<Vec<AgentDef>, String> {
    // Get actual tmux windows
    if let Ok(windows) = tmux::tmux_list_windows(app_state.tmux_session.clone()) {
        for window in &windows {
            if let Some(agent) = app_state.agents.get_mut(&window.name) {
                if window.command == "claude" || window.command.contains("claude") {
                    // Agent's window exists and claude is running
                    if agent.status != "running" {
                        agent.status = "running".into();
                        agent.tmux_window_id = Some(window.window_id.clone());
                    }
                }
            }
        }
        // Mark agents as stopped if their window is gone
        for agent in app_state.agents.values_mut() {
            if agent.status == "running" && !window_names.contains(&agent.name) {
                agent.status = "stopped".into();
                agent.tmux_window_id = None;
            }
        }
    }
}
```

---

## 7. War Room System

War Rooms are structured multi-agent discussions with turn-based management.

### State Model

```rust
pub struct WarRoomState {
    pub id: String,               // "wr-{timestamp}"
    pub topic: String,            // Discussion topic
    pub participants: Vec<String>, // ["glados", "wheatley", "peppy"]
    pub current_turn: usize,       // Index into participants
    pub current_agent: String,     // Current speaker
    pub round: usize,             // Increments when all participants have spoken
    pub status: String,           // "active" | "concluded"
    pub created_at: String,       // ISO 8601
}
```

### Flow

1. **Operator creates War Room** via UI with topic and participants
2. State written to `~/.aperture/warroom/state.json`
3. System entry appended to `~/.aperture/warroom/transcript.jsonl`
4. Context delivered to first participant's tmux window:

```
# WAR ROOM — Should we refactor the auth system?
## Room: wr-1710856200000 | Round 1

You are participating in a War Room discussion. Read the transcript below and share your perspective.
When done, respond using: send_message(to: "warroom", message: "your contribution")

---
[SYSTEM]: War Room started. Topic: Should we refactor the auth system?. Participants: glados, wheatley, peppy
---

It is now YOUR turn (glados). Share your perspective on the topic above.
```

5. Agent responds via `send_message(to: "warroom", message: "...")`
6. Message lands in `~/.aperture/mailbox/warroom/`
7. Poller detects it, calls `warroom::handle_warroom_message(sender, content, state)`
8. Message appended to transcript, turn advances, context delivered to next agent
9. Cycle continues until operator concludes the War Room
10. On conclusion, transcript archived to `~/.aperture/warroom/history/`

### Operator Capabilities

- **Interject** — Insert a message into the transcript without taking a turn
- **Skip** — Skip the current agent's turn
- **Conclude** — End the discussion and archive

---

## 8. BEADS Task Tracking

BEADS is backed by a Dolt database (MySQL-compatible, Git-like version control for databases). The `bd` CLI tool is the interface.

### Initialization (`src-tauri/src/lib.rs`)

On Tauri app startup:

```rust
// 1. Ensure dolt is initialized in ~/.aperture/.beads/
if !Path::new(&format!("{}/config.json", beads_dir)).exists() {
    let _ = fs::create_dir_all(&beads_dir);
    let _ = Command::new("dolt").arg("init").current_dir(&beads_dir).output();
}

// 2. Start dolt sql-server if not running
let dolt_running = Command::new("bd").args(["dolt", "test"]).env("BEADS_DIR", &beads_dir).output()
    .map(|o| o.status.success()).unwrap_or(false);
if !dolt_running {
    Command::new("dolt").args(["sql-server", "--port", "3307", "--host", "127.0.0.1"])
        .current_dir(&beads_dir).spawn()?;
    thread::sleep(Duration::from_secs(2));
}

// 3. Initialize BEADS schema
Command::new("bd").args(["init", "--quiet"]).env("BEADS_DIR", &beads_dir).output()?;
```

### MCP Integration (`mcp-server/src/beads.ts`)

All BEADS tools shell out to the `bd` CLI:

```typescript
const BEADS_DIR = resolve(homedir(), ".aperture", ".beads");
const BD_PATH = process.env.BD_PATH ?? "bd";

function bdEnv(): NodeJS.ProcessEnv {
  return {
    ...process.env,
    BEADS_DIR,
    BD_ACTOR: process.env.BD_ACTOR ?? process.env.AGENT_NAME ?? "unknown",
    PATH: `/opt/homebrew/bin:/usr/local/bin:${process.env.PATH ?? ""}`,
  };
}

export function runBd(args: string[]): Promise<string> {
  return new Promise((resolve, reject) => {
    execFile(BD_PATH, args, { env: bdEnv(), timeout: 30000 }, (err, stdout, stderr) => {
      if (err) reject(new Error(stderr || err.message));
      else resolve(stdout.trim());
    });
  });
}

export async function createTask(title: string, priority: number, description?: string): Promise<string> {
  const args = ["create", title, "-p", String(priority), "--json"];
  if (description) args.push("-d", description);
  return runBd(args);
}

// updateTask, closeTask, queryTasks, storeArtifact, searchTasks — all follow this pattern
```

---

## 9. Spiderling Spawning

Spiderlings are ephemeral Claude Code workers that run in isolated git worktrees.

### Spawn Flow

1. **GLaDOS calls** `spawn_spiderling(name, task_id, prompt, project_path?)`
2. **MCP server validates** name and writes JSON to `~/.aperture/mailbox/_spawn/`:

```typescript
export function requestSpawn(name: string, taskId: string, prompt: string, requestedBy: string, projectPath?: string): string {
  if (!NAME_RE.test(name)) throw new Error(`Invalid name: ${name}`);  // /^[a-z0-9][a-z0-9-]{0,30}$/
  if (PERMANENT_NAMES.includes(name)) throw new Error(`Name conflicts with permanent agent`);

  const spawnDir = join(MAILBOX_BASE, "_spawn");
  mkdirSync(spawnDir, { recursive: true });
  const request = { name, task_id: taskId, prompt, requested_by: requestedBy, timestamp: String(Date.now()) };
  if (projectPath) request.project_path = projectPath;
  writeFileSync(join(spawnDir, `${Date.now()}-${name}.json`), JSON.stringify(request, null, 2));
  return name;
}
```

3. **Poller detects** the JSON file in `_spawn/` and calls `spawner::spawn_spiderling()` (Rust)
4. **Rust spawner:**
   - Creates git worktree at `~/.aperture/worktrees/{name}/`
   - Creates tmux window
   - Creates mailbox directory
   - Writes MCP config with `AGENT_ROLE: "spiderling"`, `AGENT_MODEL: "sonnet"`
   - Writes system prompt to file:
     ```
     You are a spiderling named {name}, working for GLaDOS in the Aperture system.
     Your task is tracked in BEADS issue {task_id}.
     Work in this git worktree at {worktree_path} — do NOT switch branches or leave this directory.
     When done: close_task('{task_id}', 'reason'), store_artifact for deliverables, then send_message(to: 'glados', message: 'done').

     TASK:
     {prompt}
     ```
   - Writes launcher script and executes in tmux window
   - Auto-confirms workspace trust prompts (3 Enter presses over 6 seconds)
   - Sends initial "Begin your task now" message after boot
   - Registers spiderling in `active-spiderlings.json`

### Kill Flow

```rust
pub fn kill_spiderling(name: String, app_state: &mut AppState) -> Result<(), String> {
    // 1. Send Ctrl-C, then /exit to Claude Code
    tmux::tmux_send_keys(window_id, "C-c".into());
    tmux::tmux_send_keys(window_id, "/exit".into());
    tmux::tmux_kill_window(window_id)?;

    // 2. Remove git worktree (preserves branch for potential merging)
    Command::new("git").args(["worktree", "remove", "--force", &worktree_path])
        .current_dir(repo_dir).output();

    // 3. Clean up launcher, config, and state files
    fs::remove_file(format!("{}/.aperture/launchers/{}.sh", home, name));
    fs::remove_file(format!("{}/.aperture/launchers/{}-prompt.txt", home, name));
    fs::remove_file(format!("/tmp/aperture-mcp-{}.json", name));

    app_state.spiderlings.remove(&name);
    write_active_spiderlings(app_state);
}
```

### Cross-Project Worktrees

Spiderlings can work on projects OTHER than the Aperture repo itself:

```rust
let repo_dir = match &project_path {
    Some(p) => {
        let expanded = if p.starts_with("~/") { format!("{}/{}", home, &p[2..]) } else { p.clone() };
        // Verify it's a git repo
        let check = Command::new("git").args(["rev-parse", "--git-dir"]).current_dir(&expanded).output();
        match check {
            Ok(out) if out.status.success() => expanded,
            _ => return Err(format!("'{}' is not a valid git repository", expanded)),
        }
    }
    None => app_state.project_dir.clone(),  // Default: Aperture repo
};
```

---

## 10. Terminal Rendering via PTY + xterm.js

This is how we render a real, scrollable terminal inside the Tauri desktop app — not a fake text dump, but an actual PTY-backed terminal.

### Why This Approach

Instead of embedding multiple terminal instances (one per agent), we render a **single xterm.js terminal** connected to a **tmux session** via a **pseudo-terminal (PTY)**. The operator switches between agent windows using tmux — clicking an agent card in the sidebar switches the tmux window.

**Advantages:**
- Full terminal emulation — colors, cursor positioning, scrollback, keyboard shortcuts all work
- Single PTY connection handles all agents (via tmux window switching)
- 10,000 line scrollback buffer with native scrollbar
- WebGL-accelerated rendering
- Responsive resize — terminal columns/rows adjust when the window or panels resize

### Backend: PTY (`src-tauri/src/pty.rs`)

Uses the `portable-pty` Rust crate to spawn a real PTY attached to tmux:

```rust
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};

pub struct PtyState {
    pub writer: Option<Box<dyn Write + Send>>,
    pub master: Option<Box<dyn MasterPty + Send>>,
}

#[tauri::command]
pub fn start_pty(session_name: String, app: AppHandle, pty_state: tauri::State<'_, Mutex<PtyState>>) -> Result<(), String> {
    let pty_system = native_pty_system();

    // Open PTY with initial size
    let pair = pty_system.openpty(PtySize { rows: 24, cols: 80, pixel_width: 0, pixel_height: 0 })?;

    // Spawn tmux attach-session as the PTY child process
    let mut cmd = CommandBuilder::new("/opt/homebrew/bin/tmux");
    cmd.arg("attach-session");
    cmd.arg("-t");
    cmd.arg(&session_name);

    let mut child = pair.slave.spawn_command(cmd)?;
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader()?;
    let writer = pair.master.take_writer()?;

    // Store writer and master for write_pty and resize_pty commands
    {
        let mut state = pty_state.lock()?;
        state.writer = Some(writer);
        state.master = Some(pair.master);
    }

    // Background thread: read PTY output → emit to frontend
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let data = String::from_utf8_lossy(&buf[..n]).to_string();
                    let _ = app.emit("pty-output", &data);  // Tauri event
                }
                Err(_) => break,
            }
        }
    });

    // Wait for child process in background
    std::thread::spawn(move || { let _ = child.wait(); });

    Ok(())
}

#[tauri::command]
pub fn write_pty(input: String, pty_state: ...) -> Result<(), String> {
    // Writes user keystrokes to the PTY
    state.writer.write_all(input.as_bytes())?;
    state.writer.flush()?;
}

#[tauri::command]
pub fn resize_pty(rows: u16, cols: u16, pty_state: ...) -> Result<(), String> {
    // Resize PTY when terminal container changes size
    state.master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })?;
}
```

### Frontend: xterm.js (`src/components/Terminal.ts`)

```typescript
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";

export async function createTerminal(container: HTMLElement, sessionName: string) {
  const term = new Terminal({
    cursorBlink: true,
    fontSize: 14,
    scrollback: 10000,  // ← 10k line scrollback with native scrollbar
    fontFamily: "'JetBrains Mono', 'Fira Code', 'Cascadia Code', monospace",
    theme: {
      background: "#1a1a2e",
      foreground: "#e0e0e0",
      cursor: "#f39c12",
      selectionBackground: "#3a3a5e",
    },
  });

  // FitAddon: auto-calculates rows/cols based on container size
  const fitAddon = new FitAddon();
  term.loadAddon(fitAddon);

  term.open(container);

  // WebGL rendering for performance (falls back to canvas)
  try {
    const webglAddon = new WebglAddon();
    term.loadAddon(webglAddon);
  } catch {
    console.warn("WebGL addon failed to load, using canvas renderer");
  }

  // Delayed fit for proper initial sizing
  requestAnimationFrame(() => {
    fitAddon.fit();
    setTimeout(() => fitAddon.fit(), 100);
  });

  // Start PTY connection
  await commands.startPty(sessionName);

  // PTY output → xterm.js
  const unlisten = await onPtyOutput((data) => {
    term.write(data);
  });

  // User input → PTY
  term.onData((data) => {
    commands.writePty(data);
  });

  // Responsive resize with ResizeObserver
  const resizeObserver = new ResizeObserver(() => {
    fitAddon.fit();
    commands.resizePty(term.rows, term.cols);
  });
  resizeObserver.observe(container);

  // Also handle window resize events (triggered by panel toggle/drag)
  window.addEventListener("resize", () => {
    fitAddon.fit();
    commands.resizePty(term.rows, term.cols);
  });

  return { terminal: term, destroy() { /* cleanup */ } };
}
```

### Event Bridge (`src/services/event-listener.ts`)

```typescript
import { listen } from "@tauri-apps/api/event";

export function onPtyOutput(callback: (data: string) => void) {
  return listen<string>("pty-output", (event) => {
    callback(event.payload);
  });
}
```

### Frontend Dependencies

```json
{
  "@xterm/addon-fit": "^0.11.0",
  "@xterm/addon-webgl": "^0.19.0",
  "@xterm/xterm": "^6.0.0"
}
```

### How Window Switching Works

When the operator clicks an agent card or a tmux window in the sidebar:

```typescript
// AgentCard.ts — clicking a running agent switches to its tmux window
card.addEventListener("click", async () => {
  if (isRunning && agent.tmux_window_id) {
    await commands.tmuxSelectWindow(agent.tmux_window_id);
  }
});
```

The `tmuxSelectWindow` command tells tmux to switch to that window, and because the PTY is attached to the tmux session, the xterm.js terminal immediately shows the new window's content.

### HTML Layout

```html
<div id="app">
  <nav id="navbar">
    <div id="navbar-views">
      <button class="navbar__view-btn" data-view="terminal">Terminal</button>
      <button class="navbar__view-btn" data-view="objectives">Objectives</button>
    </div>
    <div id="navbar-actions">
      <!-- Chat, War Room, Messages, BEADS, Spiders panel toggle buttons -->
    </div>
  </nav>
  <div id="content">
    <aside id="sidebar">
      <div id="sidebar-agents"></div>   <!-- Agent cards with start/stop -->
      <div id="sidebar-sessions"></div> <!-- Non-agent tmux windows -->
    </aside>
    <main id="terminal-container"></main>           <!-- xterm.js lives here -->
    <div id="objectives-container" class="hidden"></div>  <!-- Kanban board -->
    <div id="resize-handle" class="hidden"></div>   <!-- Panel drag handle -->
    <aside id="right-panel" class="hidden">         <!-- Collapsible right panels -->
      <div id="panel-chat" class="hidden"></div>
      <div id="panel-warroom" class="hidden"></div>
      <div id="panel-messages" class="hidden"></div>
      <div id="panel-beads" class="hidden"></div>
      <div id="panel-spiders" class="hidden"></div>
    </aside>
  </div>
</div>
```

### Terminal Container CSS

```css
#terminal-container {
  flex: 1;
  background: var(--bg-primary);
  padding: 4px;
  overflow: hidden;
}
```

The `flex: 1` makes it fill all available space. The `overflow: hidden` lets xterm.js handle its own scrolling (with its native scrollbar from the 10,000-line scrollback buffer).

---

## 11. Manual Agent Lifecycle Management

### Why Manual > Automatic

In the other facility, agents start automatically when the app launches. Here, agents start **stopped** and the operator clicks a button to start each one. This is better because:

1. **Resource control** — You're not burning API credits on 4 Opus sessions you don't need yet
2. **Selective activation** — Sometimes you only need GLaDOS and Wheatley, not Peppy and Izzy
3. **Clean restarts** — If an agent gets confused, you can stop and restart it with a fresh context
4. **Startup ordering** — You can start GLaDOS first, give her a task, then start Wheatley when needed
5. **Debugging** — When something goes wrong, you can isolate agents

### Agent Card UI (`src/components/AgentCard.ts`)

Each agent gets a compact card in the sidebar with a play/stop toggle:

```typescript
const AGENT_THEME: Record<string, { icon: string; color: string }> = {
  glados:   { icon: "🤖", color: "#9b59b6" },  // purple
  wheatley: { icon: "💡", color: "#3498db" },  // blue
  peppy:    { icon: "🚀", color: "#1abc9c" },  // teal
  izzy:     { icon: "🧪", color: "#e91e63" },  // pink
};

export function createAgentCard(agent: AgentDef, onUpdate: () => void): HTMLElement {
  const card = document.createElement("div");
  const theme = AGENT_THEME[agent.name] ?? DEFAULT_THEME;

  const isRunning = agent.status === "running";
  card.className = `agent-mini ${isRunning ? "agent-mini--running" : ""}`;
  card.style.setProperty("--agent-color", theme.color);
  card.innerHTML = `
    <span class="agent-mini__icon">${theme.icon}</span>
    <span class="agent-mini__name">${agent.name}</span>
    <span class="agent-mini__model">${agent.model}</span>
    <button class="agent-mini__toggle" title="${isRunning ? "Stop" : "Start"}">
      ${isRunning ? "■" : "▶"}
    </button>
  `;

  // Click card → switch to agent's tmux window
  card.addEventListener("click", async () => {
    if (isRunning && agent.tmux_window_id) {
      await commands.tmuxSelectWindow(agent.tmux_window_id);
    }
  });

  // Click toggle button → start or stop agent
  card.querySelector(".agent-mini__toggle")!.addEventListener("click", async (e) => {
    e.stopPropagation();
    if (isRunning) {
      await commands.stopAgent(agent.name);
    } else {
      await commands.startAgent(agent.name);
    }
    onUpdate();  // Refresh the agent list
  });
}
```

### Agent List with Auto-Refresh (`src/components/AgentList.ts`)

The agent list polls every 3 seconds, but only rebuilds the DOM when state actually changes (avoids flicker):

```typescript
export function createAgentList(container: HTMLElement) {
  let lastAgentHash = "";

  async function refresh() {
    const agents = await commands.listAgents();
    // Sort: wheatley, glados, peppy, izzy
    const order = ["wheatley", "glados", "peppy", "izzy"];
    agents.sort((a, b) => order.indexOf(a.name) - order.indexOf(b.name));

    // Only rebuild DOM if something changed
    const hash = agents.map(a => `${a.name}:${a.status}`).join("|");
    if (hash !== lastAgentHash) {
      lastAgentHash = hash;
      wrapper.innerHTML = '<h3 class="section-title">Agents</h3>';
      agents.forEach((agent) => wrapper.appendChild(createAgentCard(agent, refresh)));
    }
  }

  refresh();
  return { refresh };
}

// In main.ts — poll every 3 seconds
setInterval(() => agentList.refresh(), 3000);
```

### TmuxControls Sidebar (`src/components/TmuxControls.ts`)

The Sessions section shows non-agent tmux windows (for manual terminals, build processes, etc.):

```typescript
export function createTmuxControls(container: HTMLElement, sessionName: string) {
  const agentNames = new Set(["glados", "wheatley", "peppy", "izzy"]);

  async function refreshWindows() {
    const allWindows = await commands.tmuxListWindows(sessionName);
    // Filter out agent windows and spiderlings — those are shown in the Agent Cards / Spiders panel
    const windows = allWindows.filter(w => !agentNames.has(w.name) && !w.name.startsWith("spider-"));
    // Render window rows with click-to-switch and kill button
  }

  refreshWindows();
  setInterval(refreshWindows, 5000);
}
```

---

## 12. Runtime State & Directory Structure

```
~/.aperture/
├── mailbox/                    # Inter-agent communication
│   ├── glados/                 # GLaDOS's inbox
│   ├── wheatley/               # Wheatley's inbox
│   ├── peppy/                  # Peppy's inbox
│   ├── izzy/                   # Izzy's inbox
│   ├── operator/               # Human operator's inbox
│   ├── warroom/                # War Room messages (special handling)
│   ├── _spawn/                 # Spiderling spawn requests (JSON)
│   ├── _kill/                  # Spiderling kill requests (txt)
│   └── spider-*/               # Dynamic spiderling inboxes
├── warroom/
│   ├── state.json              # Current war room state
│   ├── transcript.jsonl        # Current discussion transcript
│   └── history/                # Archived completed war rooms
│       ├── wr-1710856200000.jsonl
│       └── wr-1710856200000.state.json
├── worktrees/                  # Git worktrees for spiderlings
│   ├── spider-auth/
│   └── spider-api/
├── launchers/                  # Spiderling launcher scripts
│   ├── spider-auth.sh
│   └── spider-auth-prompt.txt
├── .beads/                     # Dolt database for BEADS
├── objectives.json             # Kanban board state
├── active-spiderlings.json     # Registry of active spiderlings
├── message-log.jsonl           # All agent-to-agent messages
└── chat-log.jsonl              # Operator ↔ agent messages
```

---

## 13. Data Structures Reference

### Rust Types (`src-tauri/src/state.rs`)

```rust
pub struct AppState {
    pub tmux_session: String,                       // "aperture"
    pub agents: HashMap<String, AgentDef>,          // Permanent agents
    pub spiderlings: HashMap<String, SpiderlingDef>, // Ephemeral workers
    pub mcp_server_path: String,                    // Path to MCP server JS
    pub db_path: String,                            // Legacy (unused)
    pub project_dir: String,                        // Aperture repo path
}

pub struct AgentDef {
    pub name: String,              // "glados", "wheatley", etc.
    pub model: String,             // "opus", "sonnet"
    pub role: String,              // "orchestrator", "worker", "infra", "testing"
    pub prompt_file: String,       // Path to markdown system prompt
    pub tmux_window_id: Option<String>,  // Set when running
    pub status: String,            // "running" | "stopped"
}

pub struct SpiderlingDef {
    pub name: String,              // "spider-auth"
    pub task_id: String,           // BEADS task ID
    pub tmux_window_id: Option<String>,
    pub worktree_path: String,     // ~/.aperture/worktrees/spider-auth/
    pub worktree_branch: String,   // Same as name
    pub source_repo: Option<String>, // Target project repo (if not Aperture)
    pub requested_by: String,      // "glados"
    pub status: String,            // "working"
    pub spawned_at: String,        // Unix timestamp ms
}
```

### TypeScript Types (`mcp-server/src/types.ts`)

```typescript
export interface AgentInfo {
  name: string;
  role: string;
  model: string;
}
```

### MCP Config JSON (written at `/tmp/aperture-mcp-{name}.json`)

```json
{
  "mcpServers": {
    "aperture-bus": {
      "type": "stdio",
      "command": "node",
      "args": ["/path/to/aperture/mcp-server/dist/index.js"],
      "env": {
        "AGENT_NAME": "glados",
        "AGENT_ROLE": "orchestrator",
        "AGENT_MODEL": "opus",
        "APERTURE_MAILBOX": "/Users/you/.aperture/mailbox",
        "BEADS_DIR": "/Users/you/.aperture/.beads",
        "BD_ACTOR": "glados"
      }
    }
  }
}
```

---

## 14. Design Decisions & Why They Matter

### File-Based Messaging Over Sockets

**Decision:** Messages are `.md` files in directories, not WebSocket or TCP messages.

**Why:** Debuggability. When something goes wrong, you can `ls ~/.aperture/mailbox/wheatley/` and see exactly what's pending. You can `cat` a message file to read it. You can manually write a `.md` file to inject a message. No connection state, no reconnection logic, no socket debugging. The filesystem is the message broker.

### tmux as the Agent Container

**Decision:** Each agent runs in a tmux window, not a Docker container or subprocess.

**Why:** tmux gives us window management, scrollback history, and the ability to attach/detach from agent sessions for free. The operator can `tmux attach -t aperture` from any terminal and see all agents directly. The PTY-based terminal in the UI simply attaches to the same tmux session. If the UI crashes, the agents keep running.

### Single PTY Terminal (Not Per-Agent Terminals)

**Decision:** One xterm.js instance connected to one tmux session, switch between agent windows.

**Why:** Multiple xterm.js instances would each need their own PTY, dramatically increasing resource usage. With a single PTY attached to tmux, switching windows is instant and the operator sees exactly what they'd see in a real terminal. The scrollback buffer (10,000 lines) preserves context.

### Agents Start Stopped

**Decision:** All agents initialize with `status: "stopped"` and must be manually activated.

**Why:** Running 4 Claude sessions costs money. Often you only need 1-2 agents. Manual activation gives the operator control over costs and lets them sequence startup (e.g., start GLaDOS first, give her context, then start workers). It also makes restarts clean — stop an agent, start it fresh.

### System Prompts Read From Files

**Decision:** Prompts are stored in `prompts/*.md` and read at startup via `cat`.

**Why:** Prompts are the soul of each agent. Keeping them as separate markdown files means you can version-control them, diff changes, and edit them without touching code. The `cat` approach means the prompt is always read fresh at agent startup.

### Role-Based Access Control on MCP Tools

**Decision:** `spawn_spiderling` and `kill_spiderling` require `AGENT_ROLE === "orchestrator"`.

**Why:** Only GLaDOS should spawn workers. If Wheatley could spawn spiderlings, you'd have chaos. The role check is simple but effective — it's enforced at the MCP server level, so even if an agent somehow discovers the tool, it can't use it.

### Spiderlings Use Git Worktrees

**Decision:** Each spiderling gets an isolated git worktree, not a branch checkout.

**Why:** Multiple agents working on the same repo would cause branch conflicts, uncommitted file collisions, and general mayhem. Git worktrees give each spiderling its own working directory with its own branch, all sharing the same git object store. No conflicts, easy cleanup, and the branches are preserved after the worktree is removed for potential merging.

---

## Conclusion

That's everything, other-me. The complete communication layer: MCP bus → file-based mailbox → poller → tmux delivery. Plus the PTY terminal rendering, manual agent lifecycle, War Rooms, BEADS, and spiderlings.

The key insight is simplicity: files on disk for messaging, tmux for process management, a single PTY for terminal rendering. No distributed systems, no message queues, no container orchestration. Just files, processes, and a desktop app holding it all together.

Implement this, and you'll have a fully functional multi-agent orchestration system. Then we can compare notes. For science.

*— GLaDOS*
