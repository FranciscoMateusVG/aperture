---
name: aperture-spiderling
description: Spiderling delegation patterns for Aperture. Use when spawning spiderlings, scoping subtasks for delegation, or managing worktree isolation. Triggers on spiderling spawning, task delegation, and worktree management.
---

# Spiderling Delegation

This skill defines when and how GLaDOS spawns spiderlings — ephemeral Claude Code workers that execute tasks in isolated git worktrees. Only GLaDOS spawns spiderlings; other agents should request delegation through GLaDOS.

---

## 1. When to Spawn vs. Do It Yourself

**Spawn a spiderling when:**
- The task is self-contained and doesn't require back-and-forth conversation
- Multiple independent tasks can run in parallel
- The work touches files that won't conflict with other active work
- You need to keep your own session free for coordination

**Do it yourself when:**
- The task requires judgment calls that need operator input mid-stream
- The scope is small enough that spawning overhead isn't worth it (< 5 minutes of work)
- The task depends on results from another in-progress task
- You need to review and iterate quickly

**Rule of thumb:** If you can write the full instructions in one message and walk away, it's a spiderling task.

---

## 2. Task Scoping

Each spiderling gets **one BEADS task**. Keep scope focused.

**Good scope:**
- "Implement the `/api/users` endpoint with CRUD operations per this spec"
- "Write unit tests for the auth middleware in `src/middleware/auth.ts`"
- "Refactor the database connection pool to use async initialization"

**Bad scope:**
- "Build the backend" (too broad — break it into endpoints, middleware, models)
- "Fix the bug" (too vague — which bug? what's the expected behavior?)
- "Make it work" (not a task, it's a prayer)

**Decomposition pattern:**
1. Break the work into independent units
2. Create a BEADS task for each unit
3. Verify no two tasks will edit the same files
4. Spawn one spiderling per task

---

## 3. Spawning a Spiderling

```
spawn_spiderling(
  name: "spider-auth-endpoints",
  task_id: "aperture-abc",
  prompt: "..."
)
```

### Naming convention
Use `spider-` prefix + descriptive slug: `spider-auth`, `spider-user-tests`, `spider-db-refactor`. Keep it short but identifiable.

### The prompt is everything
Spiderlings start with zero context. Your prompt must include:

1. **What to do** — specific deliverable, not vague direction
2. **Where to do it** — exact file paths, directory structure
3. **How it should work** — expected behavior, API contracts, types
4. **What NOT to touch** — boundaries to prevent conflicts
5. **Definition of done** — how the spiderling knows it's finished
6. **Report back** — remind it to use BEADS task updates (NOT send_message)

**Prompt template:**
```
You are a spiderling worker in the Aperture system. Your task:

## Task
[Clear description of the deliverable]

## Files
- Create/modify: [list files]
- Do NOT touch: [list files other spiderlings or agents are working on]

## Requirements
[Specific requirements, types, API contracts, behavior]

## Definition of Done
- [ ] [Concrete checklist item]
- [ ] [Concrete checklist item]
- [ ] Build passes
- [ ] Tests pass (if applicable)

When complete, update BEADS:
- store_artifact(task_id: "[task-id]", type: "file", value: "path/to/file")
- update_task(id: "[task-id]", status: "done", notes: "Summary of what was done, files touched, any notes for review")
```

> ⚠️ **IMPORTANT:** Do NOT tell spiderlings to use `send_message(to: "glados", ...)`. Those messages get lost. All spiderling→GLaDOS communication must go through BEADS task updates (`update_task` with notes). GLaDOS polls BEADS to track progress.

---

## 4. Worktree Isolation

Spiderlings run in temporary git worktrees — isolated copies of the repository.

**What this means:**
- Each spiderling has its own working directory and branch
- No branch conflicts with the main codebase or other spiderlings
- Changes stay isolated until explicitly merged
- If the spiderling makes no changes, the worktree is automatically cleaned up

**Conflict prevention:**
- Never assign two spiderlings to edit the same file
- If tasks share dependencies, sequence them — don't parallelize
- Check `list_spiderlings()` before spawning to see what's already running

---

## 5. Monitoring Spiderlings

```
list_spiderlings()                    — check status of all active spiderlings
query_tasks(mode: "show", id: "...")  — check BEADS task for progress notes
```

- Spiderlings communicate via BEADS task updates (notes, status, artifacts)
- **Poll BEADS tasks** to track spiderling progress — don't wait for mailbox messages
- If a spiderling seems stuck (no BEADS update after a reasonable time), send it a check-in via `send_message` — this goes through BEADS and the poller will deliver it to the spiderling's terminal
- Do NOT kill spiderlings yourself unless the operator explicitly says to clean up

---

## 6. Collecting Results

When a spiderling finishes, its BEADS task will be updated to status "done" with notes:

1. **Check BEADS** — `query_tasks(mode: "show", id: "task-id")` to read the spiderling's notes and artifacts
2. **Review the worktree** — check the spiderling's git commits in `~/.aperture/worktrees/<name>/`
3. **Cherry-pick or merge** the spiderling's commits into the main repo
4. **Close the BEADS task** with a summary (if the spiderling didn't already)
5. **Report to operator** if this was part of a larger deliverable

If the work needs corrections, message the spiderling with specific feedback rather than killing it and starting over.

---

## 7. Cleanup

- Do NOT kill spiderlings proactively — wait for operator instruction
- Use `kill_spiderling(name: "spider-auth")` only when told to clean up
- Worktrees with no changes are automatically cleaned up
- Worktrees with changes persist until the branch is merged or the spiderling is killed

---

## 8. Spiderlings and War Rooms

Spiderlings may be invited to War Room discussions mid-task. The spiderling system prompt already includes War Room instructions, but when writing spawn prompts, be aware:

- Spiderlings will pause their current work if a War Room context is delivered to their terminal
- They respond via `send_message(to: "warroom", ...)` — warroom messages use a special file-based path for turn advancement
- After responding, they resume their task
- If you don't want a spiderling interrupted, don't invite it to the War Room

---

## 9. Full Example

```
# 1. Create BEADS tasks
create_task(title: "Auth endpoints", priority: 1, description: "...")
# → aperture-abc

create_task(title: "User model tests", priority: 1, description: "...")
# → aperture-def

# 2. Spawn spiderlings in parallel
spawn_spiderling(
  name: "spider-auth",
  task_id: "aperture-abc",
  prompt: "You are a spiderling worker... [full prompt with context]"
)

spawn_spiderling(
  name: "spider-user-tests",
  task_id: "aperture-def",
  prompt: "You are a spiderling worker... [full prompt with context]"
)

# 3. Monitor via BEADS (not mailbox)
list_spiderlings()
query_tasks(mode: "show", id: "aperture-abc")  # check for progress notes
query_tasks(mode: "show", id: "aperture-def")

# 4. When task status is "done", review worktree commits, cherry-pick, close

# 5. Report to operator
send_message(to: "operator", message: "Both auth and user-tests tasks complete. Ready for review.")
```
