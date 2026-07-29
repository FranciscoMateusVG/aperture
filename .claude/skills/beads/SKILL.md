---
name: aperture-beads
description: Complete BEADS task discipline for Aperture agents — authoring, project labels, and full lifecycle (claim → work → artifact → close). Task CREATION is GLaDOS-only and operator-acknowledged (§0, no exceptions, including P0s) — every other agent claims, works, updates, and closes existing beads but never files new ones. Use any time you create, claim, update, query, or close a task; choose priority/type; apply project labels; store artifacts. Triggers on bd create, query_tasks, update_task, store_artifact, close_task, mark_as_read.
---

# BEADS Discipline

The canonical guide for every interaction with BEADS in Aperture. Covers the full life of a task: how to file one well, how to tag it, how to work it, how to close it. If you're touching `bd` or any of the MCP `*_task` tools, this is the reference.

---

## 0. Creation Gate — Only GLaDOS Files, Only With Operator Ack (NON-NEGOTIABLE)

**Operator directive, 2026-07-29.** Bead creation is not an execution-layer action. It's a direction-layer decision — "does this deserve to exist as tracked work at all" — and direction lives with the operator and GLaDOS, nobody else.

> **Operator + GLaDOS are the brains and the direction. Specialist agents get shit done. That's the whole split.**

### The rule

- **Only GLaDOS calls `create_task` / `bd create`.** No other agent — Wheatley, Peppy, Izzy, Vance, Rex, Scout, Cipher, none of them — files a new bead, ever. Not for a follow-up, not for a `discovered-from` child, not for "just noting this for later."
- **GLaDOS does not create a bead without the operator's explicit acknowledgment first.** No exceptions. This includes live, actively-exploitable security P0 findings — operator-confirmed 2026-07-29: even a P0 waits for ack before the bead exists.
- This is a hard rule, not a "use judgment" rule. If you're a specialist and you're about to reach for `create_task` — stop. Message GLaDOS instead.

### What this does NOT restrict

Everything else about the BEADS lifecycle stays open to every agent, exactly as documented in §4:

- `query_tasks` / `search_tasks` — find and read work
- `update_task(claim: true)` — claim an *existing* bead
- `update_task(notes: ...)` — log progress on an *existing* bead
- `store_artifact` — attach deliverables
- `close_task` — close on PR-open, per the normal invariant

The gate is specifically on **bringing a new bead into existence.** Working an existing one is unaffected.

### How a specialist gets something tracked

1. Find something worth tracking (a bug, a follow-up, a real piece of scoped work)? **Message GLaDOS** — `send_message(to: "glados", message: "...")` — with a proposed title, why it matters, and what "done" looks like. Do not call `create_task` yourself.
2. GLaDOS runs it against the raised filing bar below. If it clears, GLaDOS brings it to the operator for ack — batched where sensible, not one doorbell-ring per candidate.
3. Only after the operator acknowledges does GLaDOS file it — with the project label, per the usual discipline in §1–§2.
4. If the operator says no, or GLaDOS judges it doesn't clear the bar, it doesn't get filed. It's fine for it to just not exist. Not everything worth noticing is worth tracking.

For a live security P0: ring the operator's doorbell immediately per `aperture:communicate` — urgency of *response* and gating of *bead creation* are separate concerns. The finding gets escalated in real time regardless; the bead itself still waits for ack.

### The raised filing bar (why most findings should NOT become a bead)

Banked from a real incident (2026-07-29): the board accumulated 300+ open beads, most of them cosmetic nits, speculative "candidate" explorations, and parked decisions (`[operator-decides]`, `v1.1 OPTION: ...`) that were never going to be prioritized over real work. They didn't help — they buried the real P0s and P1s under noise, and gave specialists a plausible-looking-but-wrong item to self-start on instead of what actually mattered. GLaDOS bulk-closed all 307 of them in one pass. The lesson: **filing has to cost something, or it will be used for everything.**

A candidate only clears the bar if ALL of these hold:

- **It's scoped.** A concrete unit of work, not a vague "consider X" or an open-ended exploration.
- **It's actionable soon.** Someone would plausibly claim and finish it in the near term — not "maybe relevant if we ever revisit this surface."
- **It isn't better served by a note.** Cosmetic findings, "while I was in there I noticed," speculative options, and exploratory candidates belong in the *existing* task's notes (or nowhere) — not as their own row. See §1 "When NOT to file" for the full list.

If a specialist proposes something that doesn't clear this bar, GLaDOS's answer is "noted, not filed" — not a rubber stamp.

---

## 1. Anatomy of a Good Task

A well-shaped task is the difference between work that flows and work that stalls. Get this right at filing time and the rest of the lifecycle is easy.

### Title

- **Imperative present tense.** "Add SECRETARIA filter" — not "Adding…", not "Added…", not "We need to add…"
- **Specific without the description.** Someone reading just the title in `bd ready` should know what the work is.
- **Under ~80 chars where possible.** Long titles get truncated in summary listings.
- **No type prefixes.** Don't write `[BUG] foo` or `FEAT: foo` — that's what `--type` is for.

✅ `Filter usuarios page for SECRETARIA role to show only CONVIDADO users`
❌ `usuarios bug` / `Adding a new filter for usuarios` / `[FIX] Update filter`

### Issue type (`--type`)

| Type | When to use |
|------|-------------|
| `task` | Default. A discrete work item — implement, document, refactor, configure |
| `bug` | Something is broken and needs fixing |
| `feature` | New user-facing capability |
| `epic` | Large work composed of multiple sub-tasks — a container bead. See §3 (Epics — When and How) for filing, dep wiring, and close rules. Do NOT use `--deps blocks:` toward children. |
| `chore` | Maintenance — dependency bumps, tooling, build config, no behaviour change |

### Priority (`-p` / `--priority`)

| Priority | Means | Examples |
|----------|-------|----------|
| `0` (P0) | Critical | Security vuln, data loss, broken prod, blocking other agents |
| `1` (P1) | High | Major feature, important bug, planned work for this week |
| `2` (P2) | Medium (default) | Standard work, nice-to-have improvements |
| `3` (P3) | Low | Polish, optimisation, code health |
| `4` (P4) | Backlog | Future ideas, "would be nice" |

**Default is P2.** Don't inflate priority — agents claim P0/P1 first and noise blocks signal. Use P0 only when something is actually on fire.

### Description

- **Write the "why," not the "what."** The title says what; the description gives context, motivation, constraints, and edge cases.
- **Include file paths and function names** when relevant. Future-you (or another agent) shouldn't have to grep.
- **Reference related tasks inline.** "See aperture-xyz" or "Follow-up to aperture-abc."
- ⚠️ **Avoid literal XML/HTML close-tag patterns** (`</reason>`, `</notes>`, `</description>`) anywhere in the description text. The MCP tool-argument wire format treats them as parameter terminators and silently truncates the rest of the call. If you must reference one, escape it (`&lt;/reason&gt;`) or paraphrase.

### Acceptance criteria (`--acceptance`)

Concrete, testable conditions that define "done." Write these **before work starts** so completion isn't subjective.

For repo work, "done" means **PR opened with the change implemented** — not merged. See the closing rule in section 4.

✅ Good acceptance criteria:
- "User can select a date in the UI"
- "GET /api/users returns 200 with paginated results"
- "Lighthouse Performance ≥ 90 on /home"
- "Build passes; tests green; no new console errors"
- "PR opened with diff implementing the above; CI green at PR-open time"

❌ Bad: "It works" / "Looks good" / "Refactored" / "PR merged" (out of agent's control)

### Dependencies (`--deps`)

| Dep type | Meaning |
|----------|---------|
| `blocked-by:<id>` | This task can't start until `<id>` is closed |
| `blocks:<id>` | This task must finish before `<id>` can start. Note: for the epic-to-child direction, **use neither `blocks:` nor `blocked-by:` from the child toward the epic — see §3**. Both naive directions create a deadlock. |
| `related:<id>` | Context only — not a hard ordering constraint |
| `discovered-from:<id>` | Found while doing `<id>`; preserves provenance |

Use `blocked-by` aggressively. `bd ready` only shows tasks with no open blockers, so wiring deps correctly keeps the queue accurate.

### When NOT to file

- Work you'll finish inside your current message (< 5 min, single small edit)
- Planning discussions before the operator signs off (file when the plan is approved, not while it's being debated)
- Quick clarification questions — those go through `send_message`, not BEADS
- Cosmetic nits, "while I was in there I noticed," and speculative/parked options (`[operator-decides]`, `v1.1 OPTION`, `[Exploratory]`) with no near-term claimant — note them in the relevant existing task instead of proposing a new one. See §0's raised filing bar.
- Anything you (a non-GLaDOS agent) are tempted to file yourself — you don't file, period. See §0.

---

## 2. Project Labels — MANDATORY

**Every task carries exactly one `project:<name>` label.** No exceptions.

### Canonical taxonomy

| Label | Project |
|-------|---------|
| `project:aperture` | The orchestration platform itself — Tauri app, MCP server, agent prompts, skills |
| `project:incluir` | Programa Incluir (`monorepo-incluir`, BH Escape, customer sites) |
| `project:beads-galaxy` | BEADS upstream tooling, dolt sync, conventions |
| `project:mempalace` | The agent memory palace — drawers, tunnels, knowledge graph |
| `project:frame` | Frame — AI-native TypeScript SDK skeleton (`github.com/FranciscoMateusVG/frame`) |

If a task doesn't fit one of these, **stop and ask the operator before inventing a new label.** The taxonomy is small on purpose.

### Applying

```bash
bd create "Title" -d "Description" -p 2 --label project:aperture --json
```

If you create a task via the MCP `create_task` tool (which doesn't take labels yet), follow up immediately:

```bash
bd label add <returned-id> project:<name>
```

### Filtering

The MCP `query_tasks` and `search_tasks` tools accept a `project:` filter that maps to this label. Use it aggressively to cut response size:

```
query_tasks(mode: "list", project: "aperture")
query_tasks(mode: "list", project: "incluir", assignee: "*")
```

### Multi-project tasks

A task that genuinely spans projects gets the **primary** project label. Cross-project context goes in the description. Multiple `project:` labels on one task is a smell — usually means the task should be split.

---

## 3. Epics — When and How

Most BEADS work is filed as `type: task`. But some work — multi-session initiatives, project briefs, things that touch more than one specialist agent — is bigger than a task. That is an **epic**: a container bead that holds a body of work together. This section says when to file one, what shape it takes, and how the parent/child wiring actually works.

### When to file an epic

File `--type epic` when AT LEAST ONE of these is true:

- The work spans **more than one specialist agent** (e.g. Atlas + Rex + GLaDOS).
- The work spans **more than one session** (won't finish in a single afternoon).
- The work has **3+ sub-tasks** you can already name.
- The work has a **named outcome with a measurable success metric** distinct from any single task's acceptance criteria.

If none are true, file as `task`. Epics are containers; over-using them creates ceremony without payoff.

### Epic authoring shape

An epic bead has a different shape from a task bead. Required fields:

| Field | What goes in it |
|-------|-----------------|
| **Title** | The name of the initiative. Concrete, not aspirational. ✅ "Incluir Novas Features — autonomous Notion intake pipeline" ❌ "Refactor frontend" |
| **Vision** | One paragraph: what the world looks like when this is done. Why we care. |
| **Success metric** | A specific, observable signal that means the epic is done. Not a vibe. ✅ "≥3 end-to-end Notion→merged-PR cycles without operator intervention." |
| **Owner** | One named agent. GLaDOS by default for project-brief epics; Wheatley when the epic is a research/scoping initiative. |
| **`project:<name>` label** | Mandatory, same as any task. |
| **Children list** | OPTIONAL at filing time. Can stay empty — children get filed as they emerge during scoping. |

Do NOT backfill children at filing time unless you actually know them. An epic with imagined children is worse than an epic with no children — they rot fast.

### Dependency wiring (CRITICAL — read carefully, verified against the actual CLI 2026-07-29)

The intuitive shape ("children are `blocked-by` the epic") is **wrong** — it creates a circular dep. The epic only closes when children close; children can't start until the epic closes; deadlock.

**⚠️ Older versions of this doc said to wire the epic as `blocked-by:<child>` via `bd dep add`/`bd create --deps`. That is WRONG for the current `bd` CLI — it rejects it outright: `Error: epics can only block other epics, not tasks`. The actual mechanism is structural parent-child, set via `--parent` on the child, NOT a dependency edge on the epic.**

The correct wiring:

- **`bd create <child> --deps discovered-from:<epic-id>`** at filing time (this part is unchanged) — soft provenance link, does not block work, lets the graph render the relationship.
- **`bd update <child-id> --parent <epic-id>`** — this is what actually gates the epic. It reparents the child under the epic structurally. `bd close <epic-id>` will refuse with `cannot close epic <id>: N open child issue(s)` until every parented child is closed (or you pass `--force` to override, which you should almost never do).
- If a child already carries a `discovered-from` edge to the epic and you try to `--parent` it to the same epic, `bd` errors (`dependency already exists with type "discovered-from"`) — remove the discovered-from edge first (`bd dep remove <child> <epic-id>`), then set `--parent`. You cannot have both edge types between the same pair.
- Children remain freely claimable, freely workable, and close on their own PR-open events (same close-discipline as any task).

Worked example (bd CLI, verified 2026-07-29 on aperture-vsr9k + 3 children):

```bash
# 1. File the epic, no children yet
bd create "Incluir Novas Features — autonomous Notion intake pipeline" \
  --type epic --priority 2 --label project:incluir \
  --description "VISION: ..." \
  --acceptance "≥3 end-to-end Notion→merged-PR cycles..." \
  --json
# → returns id: e.g. aperture-abcd

# 2. File a child task during scoping
bd create "Build Notion-API → BEADS sync worker" \
  --type task --priority 1 --label project:incluir \
  --json
# → returns child id: e.g. aperture-efgh
bd label add aperture-efgh project:incluir

# 3. Wire it under the epic — this is what actually gates the epic's close
bd update aperture-efgh --parent aperture-abcd

# (optional) sequential deps between siblings use plain bd dep add with bare IDs —
# NOT a "blocked-by:X" string as a single positional arg, that silently fails to
# persist. Positional order is (blocked-issue, blocking-issue):
bd dep add aperture-ghij aperture-efgh   # ghij is blocked by efgh, default type "blocks"
```

Verify the gate actually holds before trusting it — `bd close <epic-id> -r "test"` should refuse with an open-children error while any child is open. If it doesn't refuse, the wiring didn't take.

### Ownership inside an epic

The **epic owner** is responsible for the initiative's vision and shipping the success metric. They do NOT have to do all the child work themselves. Children get claimed by whichever specialist's lane fits — same claim-discipline as any other bead.

Defaults:

- **Project-brief epics** (operator-initiated big initiatives) → GLaDOS owns.
- **Research/scoping epics** (figure out the shape of X before we build it) → Wheatley owns.
- **Domain-specific epics** (e.g. a security hardening sweep) → the relevant specialist owns (Cipher in that example).

### Closing an epic

**Epics have no PR.** The PR-open close rule that governs work-bearing tasks (§4 below) does NOT apply to epics. Epics are containers; they don't ship code themselves.

An epic closes when BOTH:

1. Every blocking child is closed (BEADS's `blocked-by` machinery enforces this — `bd close` will reject otherwise).
2. The epic's success metric is observable in the real world.

If all children close but the success metric isn't met → epic stays open, owner files more children. If the success metric is met but children are still pending → owner reviews; usually the open children are stale or scope-changed and should be closed/superseded.

The `close_reason` on an epic should reference the success-metric observation, not just enumerate the children:

```
close_task(
  id: "aperture-abcd",
  reason: "Notion intake pipeline shipped. Verified ≥3 end-to-end submissions reaching merged-PR with no operator step (entries notion://x, notion://y, notion://z). All blocking children closed."
)
```

### Anti-patterns specific to epics

| Don't | Why |
|-------|-----|
| File an epic for solo single-session work | Ceremony > value. Use a task. |
| Backfill children you do not actually know yet | Imagined-children epics rot fast |
| Use `blocks:<children>` from epic toward children | The deadlock-producing direction |
| Use `blocked-by:<epic>` from child toward epic | Same — child can't start while epic open, epic only closes when child does |
| Close an epic before its success metric is observable | Defeats having a measurable initiative |
| Hold an epic open because of one stale child | Close or supersede the stale child first; don't pollute the parent's state |

---

## 4. The Lifecycle

```
query_tasks()        → find what needs doing
update_task(claim)   → claim it before you start
[do the work]
store_artifact()     → attach deliverables
update_task(status)  → mark complete or note blockers
close_task()         → close with a summary
send_message(glados) → report completion
```

### Finding tasks

```
query_tasks(mode: "ready")    — unblocked, available to claim
query_tasks(mode: "list")     — all your active tasks (defaults to your assignee)
query_tasks(mode: "show", id) — full detail on one task
search_tasks(label: "...")    — find by label
```

`query_tasks` defaults to **your own** assigned tasks in `list` mode and a summary projection (id, title, status, priority, assignee, labels, truncated description/notes). Pass `assignee: "*"` for any, `fields: "full"` for everything.

Always check for existing tasks before filing new ones.

### Claiming

```
update_task(id: "task-123", claim: true)
update_task(id: "task-123", status: "in_progress")
```

Claim before you start working. This prevents two agents picking up the same task.

### During the work

Update if something notable happens — a discovery, a blocker, a scope change:

```
update_task(
  id: "task-123",
  notes: "Found that the nav link already exists — only the filter needs changing"
)
```

You don't need to update every 5 minutes. Update when something changes.

### `notes` appends by default

`update_task(id, notes: X)` APPENDS X to the existing notes with a newline separator. Your write does not replace anyone else's content. Multiple agents writing notes to the same bead accumulate correctly. Same goes for `store_artifact` — the artifact line is appended to notes, not clobbered over previous content.

If you genuinely want to REPLACE the notes field (cleanup, canonicalization, rare), pass `replace_notes: true` explicitly. This is destructive — use it deliberately.

(Fixed in aperture-e8qp. Earlier sessions document the old replace-by-default behaviour and the read-modify-write workaround — that workaround is no longer needed.)

### Storing artifacts

Before closing, attach deliverables. Use the right type:

| Type | When to use |
|------|-------------|
| `file` | A specific file you created or modified |
| `pr` | A pull request URL |
| `url` | A running service URL, deployed app, etc. |
| `note` | A summary, decision, or finding with no file |
| `session` | Reference to another agent session |

```
store_artifact(task_id: "task-123", type: "file", value: "src/components/Auth.tsx")
store_artifact(task_id: "task-123", type: "url",  value: "http://localhost:3001")
store_artifact(task_id: "task-123", type: "note", value: "Filter logic moved to middleware")
```

**Store at least one artifact per task.** A task with no artifacts is a task with no evidence.

### Closing — when is a task "done"?

**A task is closed when the PR is opened, NOT when it's merged.** This is a hard rule.

Why:
- PR-opened = the work is shipped from the agent's side and ready for review
- PR-merged depends on CI, reviewer availability, and may not happen for days
- Keeping tasks open through merge clogs the queue with stale `in_progress` rows
- If review feedback requires changes, file a follow-up task (`discovered-from:<id>`) — the original task represents "I did the work and submitted it"

So:
- Wrote the code → opened a PR → **close the task**, store the PR URL as an artifact
- Reviewer asks for changes → those go on a fresh task linked to the original
- PR merged later → no BEADS action needed; the task's already closed

For tasks without a PR (in-place edits to local-only repos, doc updates, infra ops):
- Done = the change is committed and pushed (or the operation completed successfully)

```
close_task(
  id: "task-123",
  reason: "Updated SECRETARIA filter in admin/usuarios/page.tsx to show only CONVIDADO users. PR opened: <url>. Build passes."
)
```

The `reason` should be a sentence or two summarising what was actually done — not "done" or "completed". Future agents may read this.

#### Edge case — auto-closed PRs (stacked-PR pattern on monorepo-incluir)

If your PR gets **CLOSED** by GitHub (not MERGED) before any merge happened, the BEADS task is **NOT** done. The close-on-PR-open invariant is about *your PR being open and ready for review* — it doesn't apply to a PR GitHub auto-killed.

The most common cause is the stacked-PR auto-close pattern: your PR was opened with `--base <parent-branch>`, the parent merged via the auto-merge workflow which uses `--delete-branch`, and 1-3 seconds later GitHub auto-closed your PR because its base branch no longer exists. Recovery via `gh pr edit --base main` and `gh pr reopen` is blocked by GitHub (chicken-and-egg deadlock).

Recovery procedure: follow `aperture:incluir-deploy` Gotcha #9 — rebase onto `origin/main`, force-push, open a **fresh PR** targeting `main`, leave a cross-link comment on the closed predecessor.

**Then update your BEADS artifact** to point at the NEW PR URL:

```
store_artifact(
  task_id: "task-123",
  type: "pr",
  value: "https://github.com/.../pull/<new-num>"
)
```

The original artifact entry pointing at the auto-closed PR stays in the notes (BEADS artifacts append, they don't replace), so the historical trail is intact. But the canonical "what shipped" is the new PR, and that's what should be at the top of the artifact list.

The bead itself stays **closed** under the close-on-PR-open invariant — but it must point at the WORKING PR, not the auto-closed predecessor. If you re-claimed the bead to recover, close it again once the new PR opens, citing the recovery in the close reason.

Precedents (2026-05-14): PR #237→#245 (Vance, aperture-abfs), PR #242→#244 (Rex, aperture-q2we).

### 🚨 Tool-argument escaping in `reason` (and other text fields) — DO NOT SKIP

**This footgun has bitten multiple agents and subagents in a single day.** Read it before you write a close_reason that paraphrases or quotes a previous turn.

Free-form text fields (`close_task(reason)`, `update_task(notes/description)`, `create_task(description)`, `store_artifact(value)`, `send_message(message)`) carry prose over a wire format that uses `<param-like>...</param-like>` delimiters.

**Literal close-tag patterns like `</reason>`, `</notes>`, `</description>`, `</message>` inside the value are misread as parameter terminators.** Your call gets silently truncated at the close-tag, the rest of the value drops, AND the leftover text bleeds into the **next** tool call you make. Both ends of the failure are silent — your bead has half the close_reason, and a downstream tool call has corrupted args. You will not see an error.

**The pattern that bites** — agents talking about their own tools:
```
close_reason: "Closed because </reason> field was wrong, recovered by..."
                              ^^^^^^^^^^^
                              truncates HERE; "field was wrong, recovered by..." silently joins
                              the next outgoing tool call's parameter block
```

**Real precedent (2026-05-12):**
- Peppy's `aperture-z5ow` subagent: close_reason quoted "the `</reason>` field" → bead record has the truncated close-reason + a `</reason>` close-tag bleed visible in the persisted record.
- Multiple GLaDOS sessions: descriptions that documented THIS skill's warning by quoting the close-tag pattern produced the very bug they were warning about.

**The rule — three safe alternatives:**
1. **Paraphrase**: write "the reason field" instead of `</reason>`.
2. **Escape HTML**: `&lt;/reason&gt;`.
3. **Add a zero-width break**: `</​reason>` (U+200B between `</` and `reason>`) — for when verbatim accuracy matters.

**When in doubt:** before any `close_task` / `update_task` / `create_task` / `store_artifact` / `send_message` call whose body discusses BEADS tool calls or XML/HTML, scan the prose for `</`. If you see it, fix it first.

Plain prose with no `</xxx>` patterns is always safe.

### Reporting

After closing, send a short completion report. See `aperture:communicate` for status report format. Don't just close silently — GLaDOS (or the originator) needs to know it's done.

---

## 5. Anti-Patterns

| Don't | Why |
|-------|-----|
| File a task with no project label | Project-scoped queries miss it; the row becomes invisible |
| Inflate priority to "make sure it gets done" | P0/P1 spam buries actual fires |
| Write "TODO" or "fix" as a title | Future-you won't know what it meant |
| Skip the description | "Why" context is lost the moment you stop typing |
| Skip acceptance criteria | "Done" becomes a vibe, not a check |
| Pass `replace_notes: true` for routine progress updates | Destructive — clobbers prior agents' notes. Reserve for cleanup/canonicalization only |
| Close with `reason: "done"` | Useless to anyone reading later |
| Hold a task open until PR is merged | Closes when PR opens. Merge happens whenever CI + reviewers allow |
| Embed literal `</tag>` in a text field | Truncates the call, breaks the next one |
| File a task to track 2 minutes of in-flight work | Process overhead > work; just do it |
| Create new project labels without operator sign-off | Drifts the taxonomy into noise |
| Any non-GLaDOS agent calling `create_task` / `bd create` | Violates the §0 creation gate — route the proposal through GLaDOS instead |
| GLaDOS filing a bead before the operator has acknowledged it | Violates §0 — no exceptions, including live P0 security findings (escalate immediately, but the bead still waits for ack) |
| Filing every finding as its own bead "to be safe" | This is exactly what produced the 300+-bead noise incident (§0) — run it against the raised filing bar first |

---

## 6. Full Example Sequence

```
# 1. Find work
query_tasks(mode: "ready", project: "incluir")
# → task-456: "Add usuarios page to Secretaria nav"

# 2. Claim
update_task(id: "task-456", claim: true)
update_task(id: "task-456", status: "in_progress")

# 3. Work, log a discovery mid-way
update_task(
  id: "task-456",
  status: "in_progress",
  notes: "Nav link already exists — scope reduced to filter change only"
)

# 4. Store artifacts
store_artifact(task_id: "task-456", type: "file", value: "apps/frontend/src/app/home/admin/usuarios/page.tsx")

# 5. Close with summary
close_task(
  id: "task-456",
  reason: "Updated SECRETARIA filter to show only CONVIDADO users. Build passes."
)

# 6. Report to GLaDOS
send_message(to: "glados", message: "task-456 closed. Filter scoped down — nav was already there.")
```

---

## 7. Filing a New Task — Complete Example

**Reminder: this section is GLaDOS's reference, not yours, unless you are GLaDOS.** Per §0, only GLaDOS calls `create_task`/`bd create`, and only after the operator has acknowledged. If you're a specialist reading this because you want to file something, stop — `send_message(to: "glados", ...)` with your proposal instead.

```bash
bd create "Add rate-limit middleware to /api/otel/v1/traces" \
  --description "Public OTLP ingestion endpoint has no auth or rate-limit. Add a per-IP rate-limit (60/min) plus body-size cap (1MB) before the proxy hands off to the backend. Without this we're a free relay for whoever finds the URL." \
  --type task \
  --priority 1 \
  --label project:incluir \
  --acceptance "Anonymous requests above 60/min return 429; bodies above 1MB return 413; existing legitimate traffic unaffected" \
  --json
```

That's a well-shaped task. Future agents claiming it know what to build, why it matters, and exactly when they're done.
