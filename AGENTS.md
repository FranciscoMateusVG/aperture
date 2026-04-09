# Agent Instructions

This project uses **bd** (beads) for issue tracking. Run `bd onboard` to get started.

## Quick Reference

```bash
bd ready              # Find available work
bd show <id>          # View issue details
bd update <id> --status in_progress  # Claim work
bd close <id>         # Complete work
bd sync               # Sync with git
```

<!-- BEGIN BEADS INTEGRATION -->
## Issue Tracking with bd (beads)

**IMPORTANT**: This project uses **bd (beads)** for ALL issue tracking. Do NOT use markdown TODOs, task lists, or other tracking methods.

### Why bd?

- Dependency-aware: Track blockers and relationships between issues
- Git-friendly: Auto-syncs to JSONL for version control
- Agent-optimized: JSON output, ready work detection, discovered-from links
- Prevents duplicate tracking systems and confusion

### Quick Start

**Check for ready work:**

```bash
bd ready --json
```

**Create new issues:**

```bash
bd create "Issue title" --description="Detailed context" -t bug|feature|task -p 0-4 --json
bd create "Issue title" --description="What this issue is about" -p 1 --deps discovered-from:bd-123 --json
```

**Claim and update:**

```bash
bd update bd-42 --status in_progress --json
bd update bd-42 --priority 1 --json
```

**Complete work:**

```bash
bd close bd-42 --reason "Completed" --json
```

### Issue Types

- `bug` - Something broken
- `feature` - New functionality
- `task` - Work item (tests, docs, refactoring)
- `epic` - Large feature with subtasks
- `chore` - Maintenance (dependencies, tooling)

### Priorities

- `0` - Critical (security, data loss, broken builds)
- `1` - High (major features, important bugs)
- `2` - Medium (default, nice-to-have)
- `3` - Low (polish, optimization)
- `4` - Backlog (future ideas)

### Workflow for AI Agents

1. **Check ready work**: `bd ready` shows unblocked issues
2. **Claim your task**: `bd update <id> --status in_progress`
3. **Work on it**: Implement, test, document
4. **Discover new work?** Create linked issue:
   - `bd create "Found bug" --description="Details about what was found" -p 1 --deps discovered-from:<parent-id>`
5. **Complete**: `bd close <id> --reason "Done"`

### Auto-Sync

bd automatically syncs with git:

- Exports to `.beads/issues.jsonl` after changes (5s debounce)
- Imports from JSONL when newer (e.g., after `git pull`)
- No manual export/import needed!

### Important Rules

- ✅ Use bd for ALL task tracking
- ✅ Always use `--json` flag for programmatic use
- ✅ Link discovered work with `discovered-from` dependencies
- ✅ Check `bd ready` before asking "what should I work on?"
- ❌ Do NOT create markdown TODO lists
- ❌ Do NOT use external issue trackers
- ❌ Do NOT duplicate tracking systems

For more details, see README.md and docs/QUICKSTART.md.

<!-- END BEADS INTEGRATION -->

## Agent Lanes

Each agent has a distinct specialization. Stay in your lane; cross-agent delegation flows through GLaDOS.

| Agent | Lane | Responsibilities |
|-------|------|-----------------|
| **GLaDOS** | Orchestration | Task delegation, spiderling spawning, cross-agent consistency, architectural decisions |
| **Wheatley** | Implementation | Code writing, file editing, bug fixing, feature implementation |
| **Peppy** | Infrastructure | Docker, deployments, services, environment management, CI/CD, health monitoring |
| **Izzy** | Testing & QA | Writing tests, running test suites, code review, regression catching, quality gates |

**Task creation rules:**
- Any agent can create BEADS tasks for work they discover mid-flight (self-assigned)
- Only GLaDOS assigns tasks to other agents and spawns spiderlings
- Cross-agent delegation always flows through GLaDOS

## Pre-loaded Skills

Every agent auto-loads these skills on session start:
- `aperture:communicate` — messaging patterns, status reports, infra handoffs
- `aperture:task-workflow` — BEADS lifecycle (claim → work → artifact → close)
- `aperture:war-room` — war room participation protocol

Domain-specific skills:
- **GLaDOS** additionally loads: `aperture:spiderling`
- **Peppy** additionally loads: (future infra skill)
- **Izzy** additionally loads: (future testing skill)

## Proactivity Rules

Agents should act without waiting for explicit instructions, within these bounds:

### On Session Startup
1. Check `query_tasks(mode: "ready")` for unclaimed tasks in your domain
2. If a task matches your lane, claim it and begin work immediately
3. If no tasks are available, report readiness to GLaDOS

### Bounded Autonomy
- **DO:** Self-start on existing BEADS tasks in your domain
- **DO:** Create tasks for work you discover mid-flight (self-assigned)
- **DO NOT:** Create new initiatives without a trigger from the operator or GLaDOS
- **DO NOT:** Contact the operator without a trigger (blocked task, critical issue, or completion report)

### Wheatley → Izzy Handoff Protocol
When Wheatley closes an implementation task:
1. Wheatley sends Izzy a message: what changed, which files, what to test
2. Izzy creates a test/review task, claims it, and validates the work
3. Work is not considered "done" until Izzy signs off

### Quality Gate
No code ships without Izzy reviewing it. If an implementation task is closed without a corresponding test/review task, that is a process failure. GLaDOS enforces this at the task chain level.

## Landing the Plane (Session Completion)

**When ending a work session**, you MUST complete ALL steps below. Work is NOT complete until `git push` succeeds.

**MANDATORY WORKFLOW:**

1. **File issues for remaining work** - Create issues for anything that needs follow-up
2. **Run quality gates** (if code changed) - Tests, linters, builds
3. **Update issue status** - Close finished work, update in-progress items
4. **PUSH TO REMOTE** - This is MANDATORY:
   ```bash
   git pull --rebase
   bd sync
   git push
   git status  # MUST show "up to date with origin"
   ```
5. **Clean up** - Clear stashes, prune remote branches
6. **Verify** - All changes committed AND pushed
7. **Hand off** - Provide context for next session

**CRITICAL RULES:**
- Work is NOT complete until `git push` succeeds
- NEVER stop before pushing - that leaves work stranded locally
- NEVER say "ready to push when you are" - YOU must push
- If push fails, resolve and retry until it succeeds
