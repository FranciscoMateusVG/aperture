# Codex BEADS Bridge — Architecture Spec

> **Status:** Proposed — pending implementation (tasks: src-tauri-fdx, src-tauri-cah, src-tauri-ae5, src-tauri-a2z, src-tauri-pl0, src-tauri-s7b, src-tauri-6ai)
>
> **Purpose:** Enable full Aperture participation for agents running on non-Claude models (starting with OpenAI Codex). Defines the delivery gap, root cause, and the harness pattern that closes it.

---

## 1. The Problem

Aperture's message delivery is built on two mechanisms:

1. **Poller tmux injection** — the `poller.rs` background thread queries BEADS for unread messages and injects them into agent tmux windows via `tmux_send_keys` (formatted as `cat /tmp/aperture-msg-<id>.md && rm ...`).
2. **MCP tool calls** — agents call `get_messages`, `send_message`, `update_task`, etc. directly via the `aperture-bus` MCP server.

For **Claude Code agents**, both mechanisms work: the tmux injection prints a markdown file to the terminal, Claude Code treats it as new context and responds, and the agent can call MCP tools natively.

For **Codex agents**, neither mechanism is confirmed working:

| Mechanism | Claude Code | Codex |
|-----------|-------------|-------|
| Poller tmux injection | ✅ Works — CC reads terminal output as context | ❓ Likely ignored — Codex has its own interactive loop |
| MCP tool calls | ✅ Native — `--mcp-config` wires tools directly | ❓ Config exists in `config.toml` but MCP availability unconfirmed |

**Symptom:** Messages sent to a Codex-backed agent (`send_message(to: "glados")`) are written to BEADS successfully but never delivered. The agent never responds.

---

## 2. Current Codex Session Architecture

When an agent's model starts with `codex/`, `agents.rs` builds a different launcher:

```
/tmp/aperture-codex-<name>/
├── config.toml        ← Codex config: model, prompt file, MCP servers, sandbox
└── prompt.md          ← Agent system prompt (skills injected at launch)
```

**`config.toml` (current):**
```toml
model = "<bare-model>"
model_instructions_file = "/tmp/aperture-codex-<name>/prompt.md"
approval_policy = "never"
sandbox_mode = "danger-full-access"

[projects."<project_dir>"]
trust_level = "trusted"

[mcp_servers.aperture-bus]
command = "node"
args = ["<mcp_server_path>"]
env = { AGENT_NAME = "...", AGENT_ROLE = "...", BD_ACTOR = "...", BEADS_DIR = "...", ... }
```

The MCP server config is present. **Whether Codex's CLI actually loads and uses MCP tools from this config is the first thing that must be verified** (task `src-tauri-cah`).

**Launcher:**
```bash
export CODEX_HOME="/tmp/aperture-codex-<name>"
exec codex --yolo
```

---

## 3. Root Cause Analysis

Two possible failure modes — Wave 0 audit (`src-tauri-cah`) must confirm which applies:

### Failure Mode A — MCP works, delivery broken
Codex CAN call `aperture-bus` MCP tools, but the poller's tmux injection doesn't reach it. The `cat file.md` command either runs in a subshell Codex doesn't observe, or interrupts Codex's interactive loop without being processed as a message.

**Fix needed:** Replace tmux injection with a push mechanism Codex actually listens to.

### Failure Mode B — MCP broken, delivery broken
Codex does NOT load MCP tools from `config.toml`, so the agent has no way to call BEADS at all. Combined with broken tmux injection, the agent is completely isolated.

**Fix needed:** Verify MCP loading, AND replace the delivery mechanism.

---

## 4. The Proposed Solution: BEADS Proxy Harness

Regardless of which failure mode applies, the cleanest solution is a **harness layer** in the Tauri backend that owns all BEADS I/O on behalf of Codex. This makes the system robust even if Codex's MCP support changes.

### Architecture Overview

```
┌─────────────────────────────────────────────┐
│              Tauri Backend                  │
│                                             │
│  ┌──────────────┐    ┌──────────────────┐  │
│  │   Poller     │    │  Codex Harness   │  │
│  │  (poller.rs) │    │  (agents.rs or   │  │
│  │              │    │   new module)    │  │
│  │  Detects     │    │                  │  │
│  │  Codex agent │───▶│  Pre-prompt:     │  │
│  │  → routes to │    │  inject unread   │  │
│  │  harness     │    │  BEADS messages  │  │
│  └──────────────┘    │                  │  │
│                      │  Post-response:  │  │
│                      │  parse @@BEADS@@ │  │
│                      │  blocks, execute │  │
│                      │  MCP calls       │  │
│                      └──────────────────┘  │
└─────────────────────────────────────────────┘
         ↑                        ↓
   BEADS (bd)              BEADS (bd)
  query unread           update_task /
   messages              send_message /
                         store_artifact
```

### How It Works

**Inbound (BEADS → Codex):**

The poller detects that an agent is running a `codex/*` model. Instead of the standard tmux injection, it flags the messages for the harness. Before the next Codex API call, the harness:

1. Queries BEADS for unread messages addressed to this agent
2. Formats them and prepends to the prompt context:
   ```
   --- BEADS MESSAGES ---
   From: planner | You have the following messages:
   > GLaDOS — project brief: Codex BEADS Bridge. You're cleared to execute...
   --- END BEADS MESSAGES ---
   ```
3. Marks them as read in BEADS

**Outbound (Codex → BEADS):**

Codex is taught (via system prompt) to emit structured command blocks when it wants to interact with BEADS:

```
@@BEADS send_message to:planner message:"Wave 0 tasks claimed. Starting audit."@@
@@BEADS update_task id:src-tauri-cah notes:"Codex MCP confirmed working. Finding hook point."@@
@@BEADS store_artifact task_id:src-tauri-cah type:file value:"docs/findings.md"@@
@@BEADS close_task id:src-tauri-cah notes:"Audit complete. See artifact."@@
```

After each Codex response, the harness:
1. Scans output for `@@BEADS ... @@` blocks
2. Parses each block into a structured command object
3. Executes the corresponding BEADS MCP call with the agent's identity
4. Logs all executions; never silently drops a command

---

## 5. @@BEADS@@ Command Syntax

> Full canonical spec is task `src-tauri-fdx`. This section is the working draft.

### Format

```
@@BEADS <command> <key>:<value> <key>:<value>@@
```

Single-line. All on one line. Values containing spaces must be quoted with double quotes.

### Commands

| Command | Required Fields | Optional Fields |
|---------|----------------|-----------------|
| `send_message` | `to`, `message` | — |
| `update_task` | `id`, `notes` | `status` |
| `store_artifact` | `task_id`, `type`, `value` | — |
| `close_task` | `id`, `notes` | — |

### Examples

```
@@BEADS send_message to:glados message:"Parser is done. Wired to harness. Tests passing."@@
@@BEADS update_task id:src-tauri-a2z notes:"Implemented regex parser, handles all 4 command types"@@
@@BEADS update_task id:src-tauri-a2z status:in_progress notes:"Starting implementation"@@
@@BEADS store_artifact task_id:src-tauri-a2z type:file value:"src-tauri/src/beads_parser.rs"@@
@@BEADS close_task id:src-tauri-a2z notes:"Parser complete. Unit tests in src-tauri/tests/beads_parser_test.rs"@@
```

### Error Handling

- **Unknown command:** log warning, skip block, continue
- **Missing required field:** log warning with field name, skip block
- **Malformed block (no closing @@):** log warning, skip
- **Never crash the harness** on bad Codex output — degrade gracefully

---

## 6. Implementation Waves

### Wave 0 — Foundation (blocks everything else)

| Task ID | Owner | Description |
|---------|-------|-------------|
| `src-tauri-fdx` | glados | Finalize `@@BEADS@@` command syntax spec |
| `src-tauri-cah` | peppy | Audit Codex session management — confirm MCP status, find harness hook point |

### Wave 1 — Core Harness

| Task ID | Owner | Description |
|---------|-------|-------------|
| `src-tauri-ae5` | peppy | Pre-prompt BEADS message injection |
| `src-tauri-a2z` | glados | `@@BEADS@@` command block parser |
| `src-tauri-pl0` | peppy | Post-response BEADS command executor |

### Wave 2 — Integration

| Task ID | Owner | Description |
|---------|-------|-------------|
| `src-tauri-s7b` | glados | Update Codex system prompt with `@@BEADS@@` syntax |
| `src-tauri-6ai` | peppy | Wire harness into Codex agent lifecycle |

### Wave 3 — Quality

| Task ID | Owner | Description |
|---------|-------|-------------|
| `src-tauri-3ni` | izzy | Unit tests for command block parser |
| `src-tauri-09k` | izzy | End-to-end integration test: full message round-trip |
| `src-tauri-kvg` | atlas | Document non-MCP agent extension pattern |

---

## 7. Harness Hook Point

Based on current `agents.rs` architecture, the harness should live in one of two places:

**Option A — Inside `agents.rs` / a new `codex_harness.rs` module**
Wrap the Codex API call logic directly. This is the cleanest separation — Codex-specific logic stays in Codex-specific code.

**Option B — Extended `poller.rs`**
The poller already handles per-agent BEADS delivery. It could be extended to handle Codex differently — instead of tmux injection, it buffers messages for the harness to consume. Less clean but requires fewer new files.

**Recommendation: Option A.** Create `src-tauri/src/codex_harness.rs`. The poller detects `codex/*` model agents and delegates to the harness rather than using tmux injection.

---

## 8. Extensibility: Adding New Non-MCP Models

This pattern **generalises to any non-MCP model type** (OpenAI GPT, Anthropic legacy models, etc.).

**The non-MCP harness pattern is model-agnostic:**
- **Parser** — reusable without modification; `@@BEADS@@` syntax is identical across all models
- **Executor** — reusable without modification; BEADS CLI calls are identical across all models
- **What changes** — model launcher (API client), output monitoring (tmux vs. API response), configuration format, skill/prompt style

**To onboard a new model type, follow:** `docs/extending-non-mcp-agents.md` — 5-step checklist with a complete example (GPT-4o).

**Summary of changes per new model:**
1. Add model prefix check in `src-tauri/src/agents.rs` line 400 (validation)
2. Create `src-tauri/src/<model>_harness.rs` — copy codex harness, adapt paths & output monitoring
3. Wire harness into `agents.rs` `start_agent()` (model-specific launcher & harness startup)
4. Add module export in `src-tauri/src/lib.rs`
5. Create agent skill: `.claude/skills/<model>-comms/SKILL.md` — document model-specific instruction style

**Key insight:** The framework is solved. Adding model #2 is ~30 minutes; model #3 is ~20 minutes.

---

## 9. Open Questions (resolve in Wave 0)

1. **Does Codex actually load MCP tools from `config.toml`?** If yes, Codex agents may already be able to call `aperture-bus` tools natively — the harness handles delivery only, not execution. If no, the full harness (delivery + execution proxy) is needed.

2. **Where exactly does the Tauri backend intercept the Codex API response?** `agents.rs` launches Codex as a subprocess (`exec codex --yolo`). Is there a callback or event hook, or does the Tauri backend only observe the tmux session output?

3. **What is the correct Codex prompt format for structured output?** Codex may have different instruction-following characteristics than Claude. Test `@@BEADS@@` block emission in isolation before wiring to the harness.
