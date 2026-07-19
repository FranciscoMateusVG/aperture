# Aperture Comms Layer v2 — Design

**Date:** 2026-07-19 · **Approved by:** operator (Maintenance Day session) · **Owner:** GLaDOS

## Problem

The current agent-communication layer delivers messages by typing shell commands into live TUI panes via tmux send-keys. This is non-atomic (text and Enter are separate sends), unacknowledged (marks read on attempt, not receipt), racy against pane render state, and — for Codex agents — depends on screen-scraping pane text for `@@BEADS@@` blocks. Documented failure beads: aperture-7mk0b (empty /tmp msg files), aperture-3cysb (stuck typed-but-unsent commands), aperture-ig9je (dead-session delivery), aperture-p27om (unverified crash durability), aperture-52g0z (no sender auth).

## Decisions (operator-ratified)

| # | Decision |
|---|---|
| Q1 | BEADS stays the message store + audit trail. Only the delivery transport is replaced. |
| Q2 | Codex agents stay interactive TUIs — no headless mode. Injection via shared app-server socket (`codex --remote`), verified live 2026-07-19 on codex-cli 0.144.6. |
| Q3 | Claude delivery = WebSocket push to a Monitor (not file-tailing). |
| Q4 | A message is marked read ONLY by the recipient's explicit `mark_as_read` after processing. Never on delivery attempt. |
| Q5 | Presence/liveness rides along in v1 (socket = heartbeat, join/leave broadcast). Sender auth is out of scope; close aperture-52g0z as wontfix. |
| Arch | Approach A — bus-centric: aperture-bus is the single hub for both protocols. |

## Architecture

```
                ┌──────────── aperture-bus (Node, one process) ─────────────┐
                │ MCP stdio per agent (existing tools, unchanged)           │
                │ + WS SERVER ws://127.0.0.1:4517   ← Claude Monitors       │
                │ + WS CLIENTS → unix://~/.aperture/run/<agent>.sock        │
                │       (Codex app-servers, one per Codex agent)            │
                │ + Presence registry → join/leave/busy/idle broadcast      │
                │ + Router: BEADS store ⇄ deliveries, unread replay         │
                └───────────────────────────────────────────────────────────┘
BEADS (Dolt)   = message store + audit (unchanged; type=message, open=unread)
Sender queue   = disk JSONL persist-then-send with retry (unchanged)
Tauri          = process supervisor (spawn panes + app-servers) + presence UI
DELETED        = 5s poller, /tmp msg files, tmux keystroke injection,
                 codex_harness @@BEADS@@ pane-scraping
```

## Protocol 1 — Claude ("pull-on-push")

1. Agent boot: system prompt/plugin starts a Monitor with a WebSocket source to `ws://127.0.0.1:4517`, identifying as `{agent}`.
2. `send_message` (unchanged API) → BEADS row → bus pushes `{type:"message", id, from, preview}` to the recipient's socket.
3. Monitor surfaces the event mid-conversation → agent calls `get_messages` → processes → `mark_as_read`.
4. Recipient offline: nothing pushed; on reconnect the bus replays every unread row. Semantics: at-least-once, idempotent by message id.

## Protocol 2 — Codex ("steered turns")

1. Tauri spawns per Codex agent: `codex app-server --listen unix://~/.aperture/run/<agent>.sock`, then a tmux pane running `codex --remote unix://...` (TUI stays fully interactive for the operator).
2. Bus connects as a second WS client on the same socket: `initialize` → `thread/list`/`thread/resume` to bind the live thread.
3. Delivery: thread idle → `turn/start` with the formatted message; mid-turn → `turn/steer`. The TUI live-renders injected turns (verified).
4. Codex agents consume aperture-bus as a real MCP server (`codex mcp add`) → same `get_messages`/`mark_as_read` ack path as Claude. `codex-comms` `@@BEADS@@` protocol retired.
5. Turn state: `turn/started`/`turn/completed` stream + Stop hooks (payload includes `session_id` = thread id) → bus tracks idle/busy.

## Presence

- Socket connected = present. Disconnect = leave event. Bus broadcasts `{agent, event: join|leave|busy|idle, ts}` to subscribers (GLaDOS, launcher UI).
- Replaces pane-sweeping as the primary liveness signal; pane peek remains a forensic fallback.
- Operator doorbell path unchanged (attention badge, not a message).

## Failure handling

- Bus down → sender disk queue holds + retries; Monitors and app-server clients reconnect with backoff.
- Codex app-server dies → Tauri respawns; bus rebinds; unread replay covers the outage window.
- Codex CLI pinned (0.144.6 at design time); protocol labeled experimental → smoke-test on every version bump.
- Two-writer discipline: bus only injects `turn/start` when thread reports idle, `turn/steer` otherwise; never races the operator's typing at the transport level (JSON-RPC is atomic per call).

## Migration

- **Phase 1:** bus WS server + presence + Claude protocol. Poller keeps running for Codex only.
- **Phase 2:** Codex app-server harness + injection; delete codex_harness scraping; rewrite `codex-comms`.
- **Phase 3:** delete poller + /tmp path; close 7mk0b, 3cysb (fixed-by-design), 52g0z (wontfix); rewrite `communicate` skill; update agent prompts (Monitor boot instruction).

## Test matrix

| Test | Answers |
|---|---|
| SIGKILL bus mid-queue-flush → respawn → replay, zero loss | aperture-p27om |
| Offline recipient → reconnect → unread replay | lost-message class |
| Mid-turn `turn/steer` into busy Codex TUI | injection semantics |
| Presence join/leave/busy/idle on connect/kill | liveness |
| E2E Claude↔Codex round-trip with ack | both protocols |

## Verified references

- Monitor tool: code.claude.com/docs/en/tools-reference#monitor-tool (WebSocket source; plugin-declared monitors auto-start).
- Codex app-server: learn.chatgpt.com/docs/app-server; `codex --remote` + external `turn/start` verified live 2026-07-19; PoC clients at `~/.codex/tmp/ws.py` and `~/.codex/tmp/inject.py`.
- Hooks: learn.chatgpt.com/docs/hooks — Stop hook fires from interactive sessions, `session_id` = thread id.
