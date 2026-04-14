---
name: aperture-communicate
description: Inter-agent communication patterns for Aperture. Use when sending messages to other agents, reporting task status to GLaDOS, requesting infra work from Peppy, or writing status reports. Triggers on agent messaging, status reports, task handoffs, and infra requests.
---

# Aperture Communication Patterns

This skill defines how Aperture agents communicate. Follow it whenever you report progress, hand off work, or coordinate with other agents.

---

## 1. The Golden Rule

**BEADS is the ONLY communication channel between agents.**

Every message between agents — task updates, quick pings, handoffs, questions, FYIs — goes through BEADS. There is no exception. `send_message` to another agent does NOT exist as a pattern anymore.

**How it works:**
- You call `send_message(to: "agent", message: "...")` — this writes a BEADS message record
- The poller delivers unread messages to the recipient every 5 seconds
- Messages persist until the recipient marks them as read
- No more lost messages. No more one-shot file delivery.

**Why:** File-based messages got lost when agents were busy processing. BEADS messages are persistent, have read/unread state, and retry delivery automatically.

---

## 2. When to Use What

| Channel | Use for | Example |
|---------|---------|---------|
| **BEADS `update_task`** | All task progress, completions, blockers, findings | "Found the bug — query filter was wrong. Fixed in usuarios/page.tsx" |
| **BEADS `store_artifact`** | Deliverables, files created, URLs deployed | `type: "file", value: "src/auth.ts"` |
| **BEADS `send_message`** | ALL agent-to-agent messages — pings, questions, FYIs, coordination | "Heads up, I changed the DB schema" |
| **`send_message(to: "operator")`** | Questions only the human can answer, critical alerts | "Need your GitHub credentials for this repo" |
| **`send_message(to: "warroom")`** | War Room responses (your turn in a discussion) | Your analysis of the topic |

**The only two recipients that bypass BEADS:** `operator` (Chat panel UI) and `warroom` (turn advancement mechanics). Everything else goes through BEADS.

---

## 3. Task Communication Flow

### Starting work
```
update_task(id: "task-id", claim: true)
update_task(id: "task-id", status: "in_progress")
```

### Progress updates (when something notable happens)
```
update_task(
  id: "task-id",
  notes: "Found that the nav link already exists — only the filter needs changing"
)
```

### Completion
```
store_artifact(task_id: "task-id", type: "file", value: "src/components/Auth.tsx")
update_task(id: "task-id", status: "done", notes: "Implemented auth flow. Build passes. Tests green.")
```

### Blockers
```
update_task(
  id: "task-id",
  notes: "BLOCKED: Need DATABASE_URL for production. Waiting on operator."
)
```

### Handoffs (e.g., builder → deployer)
```
update_task(
  id: "task-id",
  notes: "HANDOFF TO PEPPY: Ready for deploy. Repo: /projects/fitt, Branch: main, Port: 3000, Subdomain: fitt.programaincluir.org"
)
```

---

## 4. Status Report Format

When completing a task, your BEADS notes should be structured enough for GLaDOS (or any agent) to understand what happened without asking follow-up questions:

```
What I did: [1-3 bullet points of actual changes]
Files touched: [list key files]
Next step: [what happens now — review needed? deploy? nothing?]
```

❌ Bad: `"done"`
✅ Good: `"Updated SECRETARIA filter in admin/usuarios/page.tsx to show only CONVIDADO users. Build passes. Ready for review."`

---

## 5. Monitoring Tasks (for GLaDOS)

GLaDOS tracks all delegated work through BEADS:

```
query_tasks(mode: "list")              — see all tasks and their status
query_tasks(mode: "show", id: "...")   — read notes, artifacts, and progress
```

When you spawn spiderlings or delegate to agents, poll BEADS for their task updates. Messages from agents also arrive via BEADS — the poller delivers them to your terminal automatically.

---

## 6. Infra Handoff Requests to Peppy

When you need Peppy to deploy, structure it as a BEADS task note:

```
update_task(
  id: "task-id",
  notes: "DEPLOY HANDOFF TO PEPPY:
  - Repo: /projects/my-app
  - Branch: main
  - Service: my-app
  - Port: 3000
  - Subdomain: myapp.programaincluir.org
  - Env vars: DATABASE_URL, ADMIN_SECRET
  - Notes: Docker Compose, needs PostgreSQL"
)
```

Peppy reads BEADS and picks up deploy tasks. The structured format means no follow-up questions needed.

---

## 7. War Room Participation

When you receive a War Room context file (`# WAR ROOM — ...`):

1. **Read the entire transcript** before forming your response
2. **Build on what others said** — acknowledge good points, challenge bad ones with reasoning
3. **Be concise** — 150–400 words. Focused discussion, not a monologue.
4. **Always respond via** `send_message(to: "warroom", message: "...")`
5. **One message per turn** — don't send multiple warroom messages

> War Room is one of two places where `send_message` still uses the file-based path (to: "warroom"). This is by design — the warroom poller needs the file-based mechanism to advance turns.

---

## 8. Operator Communication

To reach the human operator, use `send_message(to: "operator", ...)`. This still uses the file-based path — operator messages go through the Chat panel, not BEADS.

Use this for:
- Questions only the human can answer
- Critical status updates or completion of major milestones
- Blockers that need human intervention

**Default escalation path:** Try to solve it yourself → update BEADS with findings → if truly stuck, message GLaDOS via BEADS → last resort, message operator.

---

## 9. Codex Agents

> **If you are a Codex agent** (your model starts with `codex/`), you cannot call MCP tools directly. Use the `codex-comms` skill instead — it defines the `@@BEADS@@` command block protocol that the Aperture harness intercepts and executes on your behalf.
>
> Everything in this skill (sections 1–8) applies to **Claude Code agents only**. The BEADS patterns are the same; only the execution mechanism differs.

---

## 10. Don't Spam

- Don't send the same update twice
- Don't update BEADS every 5 minutes unless something changed
- DO update BEADS if a task is taking longer than expected
- DO update BEADS immediately if you're blocked — silence is worse than a blocker report
- One BEADS update per significant milestone, not per line of code
