---
name: codex-comms
description: Communication protocol for Codex agents in Aperture (comms v2). Codex agents now call aperture-bus MCP tools directly (send_message, get_messages, mark_as_read, BEADS task tools) and receive inbound messages as injected turns via the app-server bridge. Use this skill if your model starts with codex/.
---

# Codex Agent Communication Protocol (v2)

> **This skill applies to you if you are running as a Codex agent** (your model identifier starts with `codex/`).
>
> As of comms v2 (2026-07-19), Codex agents have the **aperture-bus MCP server wired directly** into their session. You call the same tools Claude agents call. The old `@@BEADS@@` command-block protocol is **retired** — do not emit those blocks; nothing parses them anymore.

---

## 1. How you communicate (outbound)

Call MCP tools from the `aperture-bus` server exactly like any other tool:

| Need | Tool |
|---|---|
| Message another agent or GLaDOS | `send_message(to, message)` |
| Ring the operator's attention badge | `send_message(to: "operator", message)` |
| Read your queued messages | `get_messages` |
| Acknowledge a processed message | `mark_as_read(id)` |
| Task lifecycle | `create_task` / `update_task` / `close_task` / `query_tasks` / `store_artifact` / `search_tasks` |

All the discipline in `aperture:beads` and `aperture:communicate` applies to you unchanged — project labels, close-on-PR-open, artifact storage, escaping footguns.

## 2. How messages reach you (inbound)

The Aperture hub is connected to your session through the Codex app-server socket. When another agent messages you:

- A turn is **injected into your session** that renders like a user message, formatted:
  `[BEADS message from <sender> | id <message-id>] <full body>`
- Process it, then **call `mark_as_read(<message-id>)`** — this is the ONLY thing that marks it read. If you don't ack, the message replays on the next reconnect (at-least-once delivery; re-seeing a message you already handled means you forgot to ack).
- If you were mid-turn, the message may arrive appended to your current turn (`turn/steer`) — same handling.

You do NOT need to poll for messages; the hub pushes them. Calling `get_messages` at a natural pause is still a fine belt-and-suspenders habit.

## 3. Operator interaction

Unchanged from v1: the operator watches and types into your tmux pane directly. Reply in your terminal output. `send_message(to: "operator", ...)` is a doorbell badge, not a chat — the substance lives in your scrollback.

## 4. What changed vs v1 (historical)

| v1 (retired) | v2 (now) |
|---|---|
| Emit `@@BEADS ... @@` blocks; harness scraped your pane text | Call MCP tools directly |
| Poller injected `read <file>` prompts into your pane | Hub injects structured turns via app-server (`turn/start` / `turn/steer`) |
| Messages marked read on delivery attempt | YOU ack via `mark_as_read` after processing |
| No presence | Your app-server connection broadcasts join/leave/busy/idle to the facility |

If you find yourself about to type an `@@BEADS` block, stop — that muscle memory is from a retired protocol. Use the tool call.
