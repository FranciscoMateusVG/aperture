# @@BEADS@@ Command Block Syntax — Canonical Spec

> **Status:** Canonical — locked at Wave 0 (task `src-tauri-fdx`)
> **Owner:** glados
> **Consumers:** `src-tauri-a2z` (parser), `src-tauri-s7b` (system prompt), `src-tauri-pl0` (executor)

---

## 1. Purpose

Codex agents cannot call MCP tools natively. The `@@BEADS@@` command block format is the structured output protocol that lets Codex agents express BEADS intent. The Tauri harness scans every Codex response for these blocks and executes the corresponding BEADS operations on the agent's behalf.

---

## 2. Formal Syntax

### BNF Grammar

```
block       ::= "@@BEADS" SP command (SP field)+ "@@"
command     ::= "send_message" | "update_task" | "store_artifact" | "close_task"
field       ::= key ":" value
key         ::= [a-z_]+
value       ::= bare-value | quoted-value
bare-value  ::= [^ \t"@@]+          ; no spaces, no quotes, no @@
quoted-value ::= '"' inner '"'
inner       ::= ([^"\\] | "\\" escape)*
escape      ::= '"' | "\\"
SP          ::= " "+
```

### Constraints

- **Single-line only.** Each `@@BEADS ... @@` block must appear on one line. No newlines inside a block.
- **One block per line.** Multiple blocks in a response are supported — one per line. The parser processes them in document order (top to bottom).
- **Case-sensitive.** Command names and field keys are lowercase. `Send_Message` is invalid.
- **No nested quotes.** Values containing a literal double-quote must escape it as `\"`. Values containing a literal backslash must escape it as `\\`.
- **Closing `@@` is mandatory.** A line starting with `@@BEADS` but not ending with `@@` is malformed.

### Regex (for parser implementation)

```
@@BEADS\s+(\S+)((?:\s+\S+:[^\s"@@][^\s@@]*|\s+\S+:"(?:[^"\\]|\\.)*")*)\s*@@
```

Capture groups:
- Group 1: command name
- Group 2: raw field string (parse key:value pairs from this)

---

## 3. Commands

### 3.1 `send_message`

Send a message to another agent via BEADS.

| Field     | Required | Type          | Notes                          |
|-----------|----------|---------------|--------------------------------|
| `to`      | ✅       | bare or quoted | Recipient agent name (e.g. `glados`, `peppy`, `planner`, `operator`) |
| `message` | ✅       | quoted        | Message body. Must be quoted (spaces expected). |

**Examples:**
```
@@BEADS send_message to:planner message:"Wave 0 complete. Spec locked and stored as artifact."@@
@@BEADS send_message to:glados message:"Parser is done. Wired to harness. Tests passing."@@
@@BEADS send_message to:operator message:"Blocked: cannot find hook point in agents.rs."@@
```

---

### 3.2 `update_task`

Append progress notes to a BEADS task, optionally changing its status.

| Field    | Required | Type          | Notes                                                        |
|----------|----------|---------------|--------------------------------------------------------------|
| `id`     | ✅       | bare          | BEADS task ID (e.g. `src-tauri-a2z`)                        |
| `notes`  | ✅       | quoted        | Progress update. Must be quoted.                             |
| `status` | ❌       | bare          | New status. Valid values: `in_progress`, `done`, `blocked`   |

**Examples:**
```
@@BEADS update_task id:src-tauri-a2z notes:"Implemented regex parser, handles all 4 command types"@@
@@BEADS update_task id:src-tauri-a2z status:in_progress notes:"Starting implementation. Reading agents.rs."@@
@@BEADS update_task id:src-tauri-a2z status:blocked notes:"Cannot find Codex API response callback in agents.rs."@@
```

---

### 3.3 `store_artifact`

Attach a deliverable to a BEADS task.

| Field     | Required | Type          | Notes                                                                    |
|-----------|----------|---------------|--------------------------------------------------------------------------|
| `task_id` | ✅       | bare          | BEADS task ID                                                            |
| `type`    | ✅       | bare          | Artifact type. Valid values: `file`, `pr`, `url`, `note`, `session`      |
| `value`   | ✅       | bare or quoted | Path, URL, or text. Quote if it contains spaces.                        |

**Examples:**
```
@@BEADS store_artifact task_id:src-tauri-a2z type:file value:src-tauri/src/beads_parser.rs@@
@@BEADS store_artifact task_id:src-tauri-s7b type:note value:"Prompt updated in codex agent template"@@
@@BEADS store_artifact task_id:src-tauri-ae5 type:url value:https://github.com/aperture/pull/42@@
```

---

### 3.4 `close_task`

Mark a BEADS task as complete.

| Field   | Required | Type   | Notes                                  |
|---------|----------|--------|----------------------------------------|
| `id`    | ✅       | bare   | BEADS task ID                          |
| `notes` | ✅       | quoted | Completion summary. Must be quoted.    |

**Examples:**
```
@@BEADS close_task id:src-tauri-a2z notes:"Parser complete. Unit tests in src-tauri/tests/beads_parser_test.rs"@@
@@BEADS close_task id:src-tauri-fdx notes:"Canonical spec written to docs/codex-beads-syntax.md and stored as artifact."@@
```

---

## 4. Multiple Blocks Per Response

A Codex response may contain multiple `@@BEADS@@` blocks. The harness executes them **sequentially, in document order** (top to bottom).

**Example — claim, update, then store:**
```
@@BEADS update_task id:src-tauri-a2z status:in_progress notes:"Starting implementation."@@
@@BEADS update_task id:src-tauri-a2z notes:"Parser written. Handling edge cases."@@
@@BEADS store_artifact task_id:src-tauri-a2z type:file value:src-tauri/src/beads_parser.rs@@
@@BEADS close_task id:src-tauri-a2z notes:"Done. All edge cases handled. Tests pass."@@
```

Blocks may appear anywhere in the response — inline with prose, at the start, or at the end. The harness scans the full output.

---

## 5. Error Handling

The harness MUST handle bad output gracefully. Codex may hallucinate malformed blocks. **Never crash on bad Codex output.**

| Error Condition                          | Harness Behavior                                                      |
|------------------------------------------|-----------------------------------------------------------------------|
| Missing closing `@@`                     | Log warning with line number. Skip block. Continue.                   |
| Unknown command (e.g. `@@BEADS foo ...@@`) | Log warning: `unknown command 'foo'`. Skip block. Continue.         |
| Missing required field                   | Log warning: `missing required field '<key>' for command '<cmd>'`. Skip block. Continue. |
| Unknown field key                        | Log warning: `unknown field '<key>' for command '<cmd>'`. Skip field. Continue executing block with known fields. |
| Unclosed quoted value                    | Log warning. Skip block. Continue.                                    |
| BEADS MCP call fails (network/auth)      | Log error with block text and failure reason. Do NOT retry. Continue to next block. |
| Duplicate blocks (same task_id/id)       | Execute both. BEADS is idempotent on notes. Do not deduplicate.       |

All warnings and errors are logged to the harness log (implementation detail for `src-tauri-pl0`). The agent is not notified of failed blocks in the current turn — failed commands appear as missing side effects (e.g. task not updated), which the agent will observe on its next inbound BEADS poll.

---

## 6. Non-Goals

The following are explicitly out of scope for this spec:

- **Multi-line values.** Not supported. Use `\n` escape sequences within quoted strings if line breaks are needed in a message.
- **Nested or composite commands.** One command per block.
- **Response from harness to Codex within the same turn.** The harness executes blocks after the response is complete. Codex cannot observe execution results until the next prompt cycle.
- **Agent-defined commands.** The command set is closed. New commands require a spec update and parser update.

---

## 7. Version

| Field   | Value              |
|---------|--------------------|
| Version | 1.0.0              |
| Locked  | 2026-04-13         |
| Task    | `src-tauri-fdx`    |
| Author  | glados             |

Changes to this spec require a new task and must be communicated to all consumers (`src-tauri-a2z`, `src-tauri-s7b`, `src-tauri-pl0`) before implementation begins.
