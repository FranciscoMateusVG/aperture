# Aperture

> Multi-agent AI orchestration platform. A Tauri desktop app that runs, coordinates, and monitors a team of Claude CLI agents inside tmux windows.

---

## What It Is

Aperture is a control room for AI agents. It launches Claude Code sessions as named agents (GLaDOS, Wheatley, Peppy, Izzy, and others), routes messages between them through BEADS task tracking + a small file-based mailbox for operator notifications, and lets the human operator observe and direct everything through a native desktop UI.

The agents are real Claude CLI processes. The operator sees their terminals live by attaching to the shared tmux session, and assigns work through BEADS.

---

## Architecture

```
┌──────────────────────────────────────────────────────┐
│                  Tauri Desktop App                    │
│  Launcher (Agent Cards)  │  Version Footer            │
│─────────────────────────────────────────────────────│
│                  Tauri Command Bridge                 │
│─────────────────────────────────────────────────────│
│  PTY/tmux  │  Agent Mgmt  │  BEADS / Dolt            │
│─────────────────────────────────────────────────────│
│         Background Message Poller (5s loop)          │
│   BEADS message bus → tmux windows + attention       │
└──────────────────────────────────────────────────────┘

                    tmux Session
         GLaDOS │ Wheatley │ Peppy │ Izzy │ ...
                      │
              MCP Server (stdio)
              aperture-bus — one instance per agent
```

**Three layers:**

1. **Frontend** — Vite + TypeScript launcher UI (vanilla DOM, no framework). Agent cards, model picker, attention badges, version footer.

2. **Tauri backend (Rust)** — tmux session control, agent lifecycle, BEADS/Dolt integration, background message poller, build-time version metadata baked into the binary.

3. **MCP server (Node.js)** — `aperture-bus` runs as a stdio MCP server. One instance per agent. Exposes tools for messaging, BEADS task management, and artifact storage.

---

## Communication

**BEADS is the only inter-agent channel.** A small file-based mailbox handles operator notifications.

```
Agent A calls send_message(to: "B", message: "...")
  → MCP server creates a BEADS message record
  → Poller (5s) queries BEADS for unread messages
  → For each unread message: cat into recipient's tmux window
  → Recipient reads it in conversation → responds via BEADS
  → Sender's MCP marks as read after delivery
```

For operator notifications: agents call `send_message(to: "operator", ...)` which writes a small file under `~/.aperture/mailbox/operator/`. The poller picks it up and lights an attention badge on the sending agent's row in the launcher. The actual message body lives in the agent's tmux scrollback (the operator clicks the agent and reads it there).

No sockets. No long-lived queues. Mailbox state is fully debuggable, resilient to crashes.

---

## Agent Team

| Agent | Model | Role |
|-------|-------|------|
| **GLaDOS** | Opus | Orchestration — decomposes operator briefs into BEADS tasks, delegates to specialists or parallel subagents via the Agent tool |
| **Wheatley** | Sonnet | Planning & research — specs, technical research, small scoped implementations |
| **Peppy** | Opus | Infrastructure — Docker, deployments, Dokploy, CI/CD |
| **Izzy** | Opus | QA — tests, code review, quality gate sign-off |
| **Vance** | Opus | Web design & performance — CSS, Lighthouse, accessibility |
| **Rex** | Opus | Backend & APIs |
| **Scout** | Opus | Mobile (React Native, Flutter) |
| **Cipher** | Opus | Security review |
| **Sage** | Opus | SEO, content, growth |
| **Atlas** | Opus | Documentation |
| **Sterling** | Opus | Quality enforcement / final sign-off |

See `AGENTS.md` for the full lane definitions.

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

### BEADS
Task tracking backed by [Dolt](https://github.com/dolthub/dolt) (a version-controlled MySQL-compatible database). The `bd` CLI manages issues with priority, dependencies, and status. Agents use BEADS MCP tools to create, claim, update, and close tasks. BEADS also doubles as the inter-agent message bus — `send_message` writes a message-typed task and the poller delivers it.

### Subagents (Agent tool)
GLaDOS delegates scoped, parallelisable work using Claude Code's native **Agent tool** — fire-and-return subagents that run in the same session. Multiple `Agent` calls in a single message run concurrently. Specialists (Wheatley/Peppy/etc.) handle lane-specific persistent work; subagents handle scoped one-shot tasks. See the `aperture:subagents` skill for the full delegation protocol.

### Specialists
Persistent named agents (GLaDOS, Wheatley, Peppy, Izzy, Vance, Rex, Scout, Cipher, Sage, Atlas, Sterling) with their own tmux windows, models, and prompts. Each one has a defined lane. Cross-lane delegation flows through GLaDOS via BEADS.

### Version Footer
The launcher footer shows `vX.Y.Z · <git-sha> · YYYY-MM-DD` — semver from `Cargo.toml`, short git SHA + UTC build date baked into the Rust binary at build time (`src-tauri/build.rs`). Lets the operator confirm a reinstall actually picked up the latest commit.

---

## Layout

```
aperture/
├── src/                       # Tauri frontend (Vite + TS)
│   ├── components/            # UI components (Navbar, AgentList, AgentCard, Footer, ...)
│   ├── services/              # Tauri command wrappers
│   └── style.css              # Single stylesheet
├── src-tauri/                 # Tauri backend (Rust)
│   ├── src/
│   │   ├── lib.rs             # Tauri command handlers + entry
│   │   ├── agents.rs          # Agent lifecycle (start/stop, skill injection)
│   │   ├── poller.rs          # Background message delivery loop
│   │   ├── tmux.rs            # tmux session/window control
│   │   ├── codex_harness.rs   # Codex agent integration
│   │   ├── beads_parser.rs    # BEADS CLI output parser
│   │   ├── config.rs          # Default agent definitions
│   │   └── state.rs           # Shared app state types
│   ├── build.rs               # Bakes git SHA + build date into binary
│   └── Cargo.toml
├── mcp-server/                # aperture-bus MCP server (Node.js)
│   └── src/index.ts           # MCP tool definitions (send_message, BEADS, ...)
├── prompts/                   # Per-agent system prompts (one .md per agent)
├── .claude/skills/            # Aperture skills (communicate, beads, subagents, team, ...)
├── AGENTS.md                  # Agent lane definitions
└── README.md
```

---

## Development

```bash
# Install
pnpm install
cd mcp-server && pnpm install && pnpm build

# Run dev (Vite HMR + Tauri)
pnpm tauri dev

# Production build
pnpm tauri build

# Symlink skills globally
just setup-skills

# System health check
just status
```

---

## Skills

Aperture skills are markdown files under `.claude/skills/<skill>/SKILL.md`. As of v1.0 they are loaded via a **folder-driven runtime tree** at `~/.claude/aperture/`:

```
~/.claude/aperture/
├── <agent>/
│   ├── manifest.json   → repo agents/<agent>/manifest.json
│   ├── prompt.md       → repo prompts/<agent>.md
│   └── skills/
│       └── <skill>     → repo .claude/skills/<skill>/  (via shared/)
└── shared/
    └── <skill>         → repo .claude/skills/<skill>/
```

The repo holds canonical sources (`agents/<name>/{manifest.json, skills.txt}`, `prompts/<name>.md`, `.claude/skills/<skill>/`). Run `just setup` to (re)build the runtime tree from those sources — it's idempotent. Adding/removing a skill or agent never requires a recompile; only `just setup`.

To see what each agent actually carries at boot, run `just skills-matrix`: it walks the same sources (plus the runtime model overrides in `~/.aperture/agent-config.json`) and prints, per agent, which skills are **resident** (full body force-injected into the boot prompt) vs **lazy** (name+description only, body read on demand — Codex agents via `resident.txt`, Claude agents via native `.claude/skills/` discovery), with byte sizes, a skill × agent grid, and any skills that are unassigned or referenced but missing. `just skills-matrix --json` emits the same data as one JSON object for tooling.

| Skill | Purpose |
|-------|---------|
| `communicate` | Inter-agent messaging patterns, status reports |
| `team` | Agent roster and routing reference |
| `beads` | Complete BEADS discipline — authoring, project labels, full lifecycle (claim → work → close) |
| `subagents` | Subagent delegation patterns (Agent tool, parallel invocations) — primarily for GLaDOS |
| `worktree-discipline` | Per-task git worktree convention for senior monorepo-incluir agents |
| `deploy-workflow` | End-to-end deployment pipeline |
| `dokploy-api` | Dokploy REST API reference |
| `codex-comms` | `@@BEADS@@` protocol for Codex agents |

---

## Versioning

Semver tracked in `src-tauri/Cargo.toml`. Git SHA and build date are captured by `src-tauri/build.rs` and exposed via the `get_version` Tauri command. The launcher footer renders all three.
