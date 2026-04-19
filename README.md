# Aperture

> Multi-agent AI orchestration platform. A Tauri desktop app that runs, coordinates, and monitors a team of Claude CLI agents inside tmux windows.

---

## What It Is

Aperture is a control room for AI agents. It launches Claude Code sessions as named agents (GLaDOS, Wheatley, Peppy, Izzy, and others), routes messages between them through a file-based mailbox system, tracks work through BEADS task tracking, and lets the human operator observe and direct everything through a native desktop UI.

The agents are real Claude CLI processes. The operator sees their terminals live, can chat with any agent directly, invite them to structured War Room discussions, and assign work through BEADS.

---

## Architecture

```
┌──────────────────────────────────────────────────────┐
│                  Tauri Desktop App                    │
│  Terminals  │  Agent Cards  │  War Room  │  BEADS    │
│─────────────────────────────────────────────────────│
│                  Tauri Command Bridge                 │
│─────────────────────────────────────────────────────│
│  PTY/tmux  │  Agent Mgmt  │  War Room  │  BEADS     │
│─────────────────────────────────────────────────────│
│         Background Message Poller (3s loop)          │
│   Scans ~/.aperture/mailbox/* → feeds tmux windows   │
└──────────────────────────────────────────────────────┘

                    tmux Session
         GLaDOS │ Wheatley │ Peppy │ Izzy │ ...
                      │
              MCP Server (stdio)
              aperture-bus — one instance per agent
```

**Three layers:**

1. **Frontend** — Vite + TypeScript UI. Terminals rendered via xterm.js with WebGL. Components for agent cards, War Room, BEADS panel, spiderlings, objectives.

2. **Tauri backend (Rust)** — PTY management, tmux session control, agent lifecycle, War Room state machine, BEADS/Dolt integration, background message poller.

3. **MCP server (Node.js)** — `aperture-bus` runs as a stdio MCP server. One instance per agent. Exposes tools for messaging, task management, artifact storage, spiderling spawning.

---

## Communication

**All inter-agent communication is file-based.**

```
Agent A calls send_message(to: "B", message: "...")
  → MCP writes .md file to ~/.aperture/mailbox/B/
  → Poller (3s) detects file → cat into B's tmux window via send-keys
  → Agent B reads it in conversation → responds
```

No sockets. No queues. The mailbox is a directory you can `ls`. Fully debuggable, resilient to crashes.

---

## Agent Team

| Agent | Model | Role |
|-------|-------|------|
| **GLaDOS** | Opus | Orchestration — delegates tasks, spawns spiderlings, enforces quality gate |
| **The Planner** | Opus | Project direction — War Room analysis, BEADS task creation, operator sign-off |
| **Wheatley** | Sonnet | Implementation — code, bug fixes, feature work |
| **Peppy** | Opus | Infrastructure — Docker, deployments, CI/CD, services |
| **Izzy** | Opus | QA — tests, code review, quality gate sign-off |
| **Atlas** | Sonnet | Documentation |
| **Cipher** | Sonnet | Security review |
| **Sentinel** | Sonnet | Task health monitoring |
| + others | — | See `AGENTS.md` for full roster |

---

## Supported Agent Types

Aperture supports both **Claude Code agents** and **Codex agents**.

- **Claude Code agents** call MCP tools directly through `aperture-bus`.
- **Codex agents** do not call MCP tools directly. They emit `@@BEADS@@` command blocks instead.
- The **Tauri harness** intercepts those `@@BEADS@@` blocks and executes the corresponding BEADS operations on the agent's behalf.
- The `codex-comms` skill defines the Codex-side communication protocol.
- Model names starting with `codex/` are treated as Codex agents by the harness.

---

## Key Concepts

### War Rooms
Structured multi-agent discussions. The operator creates a War Room, invites participants, and each agent takes a turn contributing. The operator can interject. When concluded, the transcript is handed to The Planner for task creation.

### BEADS
Task tracking backed by [Dolt](https://github.com/dolthub/dolt) (a version-controlled MySQL-compatible database). The `bd` CLI manages issues with priority, dependencies, and status. Agents use BEADS MCP tools to create, claim, update, and close tasks.

### Spiderlings
Ephemeral sub-agents spawned by GLaDOS for isolated subtasks. They run in git worktrees, complete a scoped task, store an artifact, and terminate. Spiderling state is visible in the Spiderlings panel.

### Objectives
High-level project goals tracked in a Kanban view. Linked to BEADS task clusters.

---

## Project Structure

```
aperture/
├── src/                    # Frontend (TypeScript + Vite)
│   ├── components/         # UI components (Terminal, AgentCard, WarRoom, BeadsPanel, ...)
│   ├── services/           # Tauri command wrappers
│   ├── main.ts
│   └── types.ts
├── src-tauri/              # Tauri backend (Rust)
│   └── src/
│       ├── agents.rs       # Agent lifecycle (start, stop, list, chat)
│       ├── beads.rs        # BEADS/Dolt integration
│       ├── config.rs       # App config and default state
│       ├── objectives.rs   # Project objectives
│       ├── poller.rs       # Background message delivery poller
│       ├── pty.rs          # PTY management
│       ├── spawner.rs      # Spiderling management
│       ├── state.rs        # Shared app state
│       ├── tmux.rs         # tmux session control
│       ├── warroom.rs      # War Room state machine
│       └── lib.rs          # Entry point, service init
├── mcp-server/             # aperture-bus MCP server (Node.js)
├── docs/                   # Architecture docs and implementation guides
├── prompts/                # Agent system prompts
├── justfile                # Dev commands
├── AGENTS.md               # Agent lane definitions and workflow rules
├── WARROOM-SPEC.md         # War Room protocol specification
└── CLAUDE.md               # Instructions for AI agents working on this repo
```

---

## Getting Started

### Prerequisites

- [Rust](https://rustup.rs/) + Cargo
- [Node.js](https://nodejs.org/) + pnpm
- [Tauri CLI v2](https://tauri.app/start/prerequisites/)
- [tmux](https://github.com/tmux/tmux)
- [Claude CLI](https://claude.ai/download) (authenticated)
- [Dolt](https://docs.dolthub.com/introduction/installation) + `bd` CLI

### Install & Run

```bash
# Install frontend deps
pnpm install

# Install MCP server deps
cd mcp-server && pnpm install && cd ..

# Set up agent skills symlink
just setup-skills

# Run in dev mode
pnpm tauri dev

# Build for production
pnpm tauri build
```

### First Launch

1. Aperture auto-initializes BEADS (`~/.aperture/.beads/`) and starts the Dolt SQL server on port 3307.
2. Create a tmux session from the UI or let Aperture create one automatically.
3. Start agents from the Agent Cards panel — each agent launches as a named Claude CLI session with its MCP server.
4. The operator Chat panel connects you directly to any agent.

---

## Development

```bash
just status          # System health check (skills, Docker, agents)
just setup-skills    # (Re)link agent skills to ~/.claude/skills/aperture
just check-skills    # Verify skills symlink
```

### Adding a New Agent

1. Add the agent definition to `prompts/` and `AGENTS.md`
2. Register the agent name in `src-tauri/src/agents.rs`
3. Add the agent skill file to `skills/aperture/` if needed

### Adding a New MCP Tool

Tools live in `mcp-server/src/`. Add the tool schema and handler, rebuild with `pnpm build` inside `mcp-server/`, and restart any running agents.

---

## Docs

- `docs/APERTURE-COMMUNICATION-LAYER.md` — Full implementation guide: MCP bus, mailbox, poller, War Rooms, PTY, spiderlings
- `WARROOM-SPEC.md` — War Room protocol and state machine
- `AGENTS.md` — Agent lanes, proactivity rules, handoff protocols

---

## License

Private. All rights reserved.
