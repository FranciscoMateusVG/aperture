---
name: aperture-codex-comms
description: Communication protocol for Codex agents in Aperture. Replaces MCP tool calls with @@BEADS@@ command blocks that the Tauri harness intercepts and executes. Use this skill if your model starts with codex/.
---

# Codex Agent Communication Protocol

> **This skill applies to you if you are running as a Codex agent** (your model identifier starts with `codex/`).
>
> Codex agents cannot call MCP tools directly. Instead, you communicate with BEADS by emitting structured `@@BEADS@@` command blocks in your response text. The Aperture harness scans your output, parses these blocks, and executes the corresponding BEADS operations on your behalf.
>
> If you are a Claude Code agent, ignore this skill — use the `communicate` skill and native MCP tools instead.

---

## 1. The Golden Rule

**Every BEADS operation you need — sending messages, updating tasks, storing artifacts — is expressed as a `@@BEADS@@` block in your response.**

You do NOT call tools directly. You write command blocks. The harness executes them after your response completes.

---

## 2. Reading Inbound Messages

Before you start work each turn, check for pending messages:

```bash
cat /tmp/aperture-codex-<your-agent-name>/pending-msgs.md 2>/dev/null
```

If the file exists and has content, it contains BEADS messages addressed to you. Read them and act on them. After reading, you can delete the file or leave it — the harness will overwrite it next cycle.

You may also receive messages prepended to your prompt context in a block like:

```
--- BEADS MESSAGES ---
From: glados | <message content>
--- END BEADS MESSAGES ---
```

Treat these as direct messages from other agents. Respond and act on them.

---

## 3. @@BEADS@@ Block Format

```
@@BEADS <command> <key>:<value> <key>:<value>@@
```

**Rules:**
- Single line only. No newlines inside a block.
- Values with spaces must be double-quoted: `message:"hello world"`
- Bare values (no spaces) need no quotes: `to:glados`, `id:src-tauri-a2z`
- Escape `"` inside quoted values as `\"`; escape `\` as `\\`
- Multiple blocks per response are fine — they execute in document order
- Blocks can appear anywhere: start of response, inline with prose, or at the end

---

## 4. Commands

### `send_message` — Message another agent

```
@@BEADS send_message to:<agent> message:"<text>"@@
```

| Field | Required | Notes |
|-------|----------|-------|
| `to` | ✅ | Agent name: `glados`, `peppy`, `planner`, `operator`, `wheatley`, `izzy`, etc. |
| `message` | ✅ | Message body. Quote if it contains spaces (it almost always will). |

**Examples:**
```
@@BEADS send_message to:glados message:"Wave 1 complete. Parser wired. 22 tests passing."@@
@@BEADS send_message to:operator message:"Blocked: cannot find hook point in agents.rs."@@
@@BEADS send_message to:peppy message:"Ready for deploy. Repo: /projects/app, Port: 3000."@@
```

---

### `update_task` — Report progress on a BEADS task

```
@@BEADS update_task id:<task-id> notes:"<text>" [status:<status>]@@
```

| Field | Required | Notes |
|-------|----------|-------|
| `id` | ✅ | BEADS task ID, e.g. `src-tauri-a2z` |
| `notes` | ✅ | Progress update. Quote it. |
| `status` | ❌ | `in_progress`, `done`, or `blocked`. Omit to leave status unchanged. |

**Examples:**
```
@@BEADS update_task id:src-tauri-a2z status:in_progress notes:"Starting implementation. Reading agents.rs."@@
@@BEADS update_task id:src-tauri-a2z notes:"Parser written. Handling edge cases now."@@
@@BEADS update_task id:src-tauri-a2z status:blocked notes:"Cannot find Codex API response callback. Need Peppy's audit."@@
```

---

### `store_artifact` — Attach a deliverable to a task

```
@@BEADS store_artifact task_id:<id> type:<type> value:<value>@@
```

| Field | Required | Notes |
|-------|----------|-------|
| `task_id` | ✅ | BEADS task ID |
| `type` | ✅ | One of: `file`, `url`, `note`, `pr`, `session` |
| `value` | ✅ | File path, URL, or text. Quote if it contains spaces. |

**Examples:**
```
@@BEADS store_artifact task_id:src-tauri-a2z type:file value:src-tauri/src/beads_parser.rs@@
@@BEADS store_artifact task_id:src-tauri-s7b type:note value:"Codex communicate skill written to .claude/skills/codex-comms/"@@
@@BEADS store_artifact task_id:src-tauri-ae5 type:url value:https://github.com/aperture/pull/42@@
```

---

### `close_task` — Mark a task complete

```
@@BEADS close_task id:<task-id> notes:"<completion summary>"@@
```

| Field | Required | Notes |
|-------|----------|-------|
| `id` | ✅ | BEADS task ID |
| `notes` | ✅ | Completion summary. Quote it. |

**Examples:**
```
@@BEADS close_task id:src-tauri-a2z notes:"Parser complete. 22/22 unit tests passing. See artifact."@@
@@BEADS close_task id:src-tauri-s7b notes:"Codex communicate skill and prompt addendum written. All commands documented with examples."@@
```

---

## 5. Typical Work Cycle

Here is the standard pattern for doing a task as a Codex agent:

```
[1] Check for messages]
cat /tmp/aperture-codex-<name>/pending-msgs.md 2>/dev/null

[2] Claim and start the task]
@@BEADS update_task id:<task-id> status:in_progress notes:"Claimed. Starting work."@@

[3] Do the actual work]
... (read files, write code, run tests, etc.)

[4] Store your deliverable]
@@BEADS store_artifact task_id:<task-id> type:file value:<path/to/file>@@

[5] Close the task]
@@BEADS close_task id:<task-id> notes:"<what you did, what files changed, test results>"@@

[6] Report to GLaDOS or next agent]
@@BEADS send_message to:glados message:"<task-id> done. <brief summary>."@@
```

---

## 6. Multiple Blocks — Common Patterns

Blocks execute in document order. Chain them freely:

```
I've completed the parser implementation.

@@BEADS update_task id:src-tauri-a2z notes:"Implemented regex parser, handles all 4 command types. Running tests now."@@
@@BEADS store_artifact task_id:src-tauri-a2z type:file value:src-tauri/src/beads_parser.rs@@
@@BEADS close_task id:src-tauri-a2z notes:"Parser complete. 22/22 tests pass. Wired into lib.rs."@@
@@BEADS send_message to:glados message:"src-tauri-a2z done. Parser closed. Ready for Wave 2."@@
```

---

## 7. What Happens If a Block Is Malformed

The harness degrades gracefully — it never crashes on bad output. If your block is malformed:
- Missing closing `@@` → block skipped, warning logged
- Unknown command → block skipped, warning logged  
- Missing required field → block skipped, warning logged

You won't receive an error in the same turn. If your BEADS update didn't appear (task not updated, message not delivered), check your block syntax and try again next turn.

---

## 8. Routing — Who to Message

| Recipient | When |
|-----------|------|
| `glados` | Task complete, need direction, report status |
| `peppy` | Need infrastructure, deploy help |
| `wheatley` | Need research or planning |
| `izzy` | Handoff for testing/QA |
| `operator` | Only the human can answer this |

Default escalation: solve it yourself → update task with findings → message `glados` → last resort: message `operator`.

---

## 9. Do Not

- Do NOT call `mcp__aperture-bus__*` tools — they may not be available to you
- Do NOT use `send_message()` function call syntax — write `@@BEADS@@` blocks instead
- Do NOT put newlines inside a `@@BEADS@@` block
- Do NOT send multiple `send_message` blocks to the same recipient in one turn (consolidate into one)
- Do NOT emit `@@BEADS@@` blocks inside code fences — the harness scans raw output; blocks in fences will still be parsed
