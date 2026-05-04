---
name: aperture-task-workflow
description: BEADS task lifecycle for Aperture agents. Use when claiming tasks, updating task status, storing artifacts, or closing tasks. Triggers on BEADS operations, task management, and artifact storage.
---

# BEADS Task Workflow

This skill defines the consistent lifecycle for BEADS tasks. Every task goes through the same stages: **claim → work → artifact → close**. Don't skip steps.

---

## 1. The Lifecycle

```
query_tasks()        → find what needs doing
update_task(claim)   → claim it before you start
[do the work]
store_artifact()     → attach deliverables
update_task(status)  → mark complete or note blockers
close_task()         → close with a summary
send_message(glados) → report completion
```

---

## 2. Finding Tasks

```
query_tasks(mode: "ready")    — tasks available to claim
query_tasks(mode: "list")     — all tasks and their status
query_tasks(mode: "show", id: "task-123")  — details on one task
search_tasks(label: "frontend")            — find by label
```

Always check for existing tasks before creating new ones. GLaDOS may have already created a task for what you're about to do.

---

## 3. Claiming a Task

**Claim before you start working.** This prevents two agents picking up the same task.

```
update_task(id: "task-123", claim: true)
```

If a task doesn't exist yet and you're creating one yourself:

```
create_task(
  title: "Add Secretaria usuarios filter",
  priority: 2,
  description: "Filter usuarios page for SECRETARIA role to show only CONVIDADO users"
)
```

Then immediately claim it.

---

## 4. During the Work

Update the task if you hit something worth noting — a discovery, a blocker, a scope change:

```
update_task(
  id: "task-123",
  status: "in_progress",
  notes: "Found that the nav link already exists — only the filter needs changing"
)
```

You don't need to update every 5 minutes. Update when something changes.

### ⚠️ Gotcha: `update_task(notes: ...)` REPLACES — it does NOT append

Despite the schema description saying "Append notes," the BEADS server overwrites the entire `notes` field on every `update_task` call. Writing a new note silently erases anything that was there before.

**Always read-modify-write** when adding to existing notes:

```
# 1. Read the current notes
existing = query_tasks(mode: "show", id: "task-123")[0].notes ?? ""

# 2. Concatenate with your addition
combined = existing + "\n\n---\n\n## New section\n" + your_new_content

# 3. Write the full blob back
update_task(id: "task-123", notes: combined)
```

If you skip the read step, you will silently destroy other agents' work. This bites hardest in collaborative tasks (e.g. Wheatley drafts wave plans, GLaDOS adds a decision, the second writer overwrites the first).

For purely standalone progress notes that don't need to preserve history, replace is fine — but be deliberate about that choice.

---

## 5. Storing Artifacts

Before closing, attach your deliverables. Use the right artifact type:

| Type | When to use |
|------|-------------|
| `file` | A specific file you created or modified |
| `pr` | A pull request URL |
| `url` | A running service URL, deployed app, etc. |
| `note` | A summary, decision, or finding with no file |
| `session` | Reference to another agent session |

Examples:
```
store_artifact(
  task_id: "task-123",
  type: "file",
  value: "apps/frontend/src/app/home/admin/usuarios/page.tsx"
)

store_artifact(
  task_id: "task-123",
  type: "url",
  value: "http://localhost:3001"
)

store_artifact(
  task_id: "task-123",
  type: "note",
  value: "Nav link was already in place — only updated the SECRETARIA filter block"
)
```

Store at least one artifact per task. A task with no artifacts is a task with no evidence.

---

## 6. Closing a Task

```
close_task(
  id: "task-123",
  reason: "Updated SECRETARIA filter in admin/usuarios/page.tsx to show only CONVIDADO users. Build passes."
)
```

The `reason` should be a one or two sentence summary of what was done — not "done" or "completed". Future agents may read this.

---

## 7. Reporting to GLaDOS

After closing, send a completion report. See `aperture:communicate` for the status report format. Don't just close the task silently — GLaDOS needs to know it's done.

---

## 8. Full Example Sequence

```
# 1. Find the task
query_tasks(mode: "ready")
# → task-456: "Add usuarios page to Secretaria nav"

# 2. Claim it
update_task(id: "task-456", claim: true)

# 3. Do the work...

# 4. Note a discovery mid-task
update_task(
  id: "task-456",
  status: "in_progress",
  notes: "Nav link already exists — scope reduced to filter change only"
)

# 5. Store artifacts
store_artifact(task_id: "task-456", type: "file", value: "apps/frontend/src/app/home/admin/usuarios/page.tsx")

# 6. Close it
close_task(id: "task-456", reason: "Updated SECRETARIA filter to show only CONVIDADO users. Build passes.")

# 7. Report to GLaDOS
send_message(to: "glados", message: "**Task:** task-456 ...")
```
