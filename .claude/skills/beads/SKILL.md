---
name: beads
description: Complete BEADS task discipline for Aperture agents — authoring, project labels, and full lifecycle (claim → work → artifact → close). Task CREATION is GLaDOS-only and operator-acknowledged (§0, no exceptions, including P0s) — every other agent claims, works, updates, and closes existing beads but never files new ones. Use any time you create, claim, update, query, or close a task; choose priority/type; apply project labels; store artifacts. Triggers on bd create, query_tasks, update_task, store_artifact, close_task, mark_as_read.
---

# BEADS Discipline

The canonical reference for every `bd` / MCP `*_task` interaction in Aperture: file, tag, work, close. Incident write-ups and worked examples live in `references/precedents.md`, cited inline as `Precedent: §N`.

---

## 0. Creation Gate — Only GLaDOS Files, Only With Operator Ack (NON-NEGOTIABLE)

Bead creation is a direction-layer decision ("does this deserve to exist as tracked work at all"); direction lives with the operator and GLaDOS, nobody else. Specialists get things done; they do not decide what gets tracked. (Precedent: §0 operator directive, 2026-07-29.)

- **Only GLaDOS calls `create_task` / `bd create`.** No other agent files a new bead, ever — not a follow-up, not a `discovered-from` child, not "just noting this for later."
- **GLaDOS does not create a bead without the operator's explicit acknowledgment first.** No exceptions — including live security P0s (operator-confirmed 2026-07-29).
- Hard rule, not a judgment call. Specialist about to reach for `create_task`? Stop; message GLaDOS instead.

**Not restricted:** everything else in the lifecycle (§4) — `query_tasks` / `search_tasks`, `update_task(claim/notes)` on an *existing* bead, `store_artifact`, `close_task`. The gate is only on bringing a new bead into existence.

**How a specialist gets something tracked:** `send_message(to: "glados", ...)` with a proposed title, why it matters, and what "done" looks like. GLaDOS runs it against the filing bar below; if it clears, she brings it to the operator for ack (batched, not one doorbell per candidate); only after ack does she file it, with the project label per §2. If the operator says no or it doesn't clear the bar, it isn't filed — not everything worth noticing is worth tracking.

**Live security P0:** ring the operator's doorbell immediately per `aperture:communicate`. Urgency of *response* and gating of *bead creation* are separate concerns — escalate in real time; the bead still waits for ack.

### The raised filing bar

**Filing has to cost something, or it will be used for everything.** An unfiltered board buries real P0s/P1s under cosmetic nits, speculative candidates, and parked decisions, and hands specialists plausible-looking-but-wrong items to self-start on. (Precedent: §0 noise incident, 2026-07-29 — 307 beads bulk-closed.)

A candidate clears the bar only if ALL hold:
- **Scoped** — a concrete unit of work, not "consider X" or an open-ended exploration.
- **Actionable soon** — someone would plausibly claim and finish it in the near term.
- **Not better served by a note** — cosmetic findings, "while I was in there," speculative options, and exploratory candidates go in the *existing* task's notes (or nowhere).

GLaDOS's answer to a proposal that misses the bar is "noted, not filed" — not a rubber stamp.

---

## 1. Anatomy of a Good Task

### Title
- **Imperative present tense**: "Add SECRETARIA filter" — not "Adding…", "Added…", "We need to add…"
- **Specific without the description** — readable in `bd ready` alone. **Under ~80 chars.** **No type prefixes** (`[BUG]`, `FEAT:`) — that's `--type`.

✅ `Filter usuarios page for SECRETARIA role to show only CONVIDADO users`
❌ `usuarios bug` / `Adding a new filter for usuarios` / `[FIX] Update filter`

### Issue type (`--type`)

| Type | When to use |
|------|-------------|
| `task` | Default. A discrete work item — implement, document, refactor, configure |
| `bug` | Something is broken and needs fixing |
| `feature` | New user-facing capability |
| `epic` | Container for multi-task work — see §3 for filing, wiring, close rules. Never `--deps blocks:` toward children. |
| `chore` | Maintenance — dependency bumps, tooling, build config, no behaviour change |

### Priority (`-p` / `--priority`)

| Priority | Means | Examples |
|----------|-------|----------|
| `0` (P0) | Critical | Security vuln, data loss, broken prod, blocking other agents |
| `1` (P1) | High | Major feature, important bug, planned work for this week |
| `2` (P2) | Medium (default) | Standard work, nice-to-have improvements |
| `3` (P3) | Low | Polish, optimisation, code health |
| `4` (P4) | Backlog | Future ideas, "would be nice" |

**Default is P2.** Don't inflate — agents claim P0/P1 first and noise blocks signal. P0 only when something is actually on fire.

### Description
- **The "why," not the "what."** Title says what; description gives context, motivation, constraints, edge cases.
- **File paths and function names** when relevant. **Related tasks inline** ("See aperture-xyz").
- ⚠️ **No literal XML/HTML close-tag patterns** (`</reason>`, `</notes>`) — the MCP wire format treats them as parameter terminators and silently truncates. Escape (`&lt;/reason&gt;`) or paraphrase. Full rule in §4.

### Acceptance criteria (`--acceptance`)
Concrete, testable done-conditions, written **before work starts**. For repo work, done = **PR opened**, not merged (§4).

✅ "User can select a date in the UI" / "GET /api/users returns 200 with paginated results" / "Lighthouse Performance ≥ 90 on /home" / "Build passes; tests green; no new console errors" / "PR opened; CI green at PR-open time"
❌ "It works" / "Looks good" / "Refactored" / "PR merged" (out of the agent's control)

### Dependencies (`--deps`)

| Dep type | Meaning |
|----------|---------|
| `blocked-by:<id>` | Can't start until `<id>` is closed |
| `blocks:<id>` | Must finish before `<id>` starts. For epic↔child, use NEITHER direction — see §3; both deadlock. |
| `related:<id>` | Context only — no ordering constraint |
| `discovered-from:<id>` | Found while doing `<id>`; preserves provenance |

Use `blocked-by` aggressively — `bd ready` only shows tasks with no open blockers.

### When NOT to file
- Work you'll finish inside your current message (< 5 min, single small edit)
- Planning discussions before the operator signs off — file when approved, not while debated
- Quick clarification questions — `send_message`, not BEADS
- Cosmetic nits, "while I was in there," speculative/parked options (`[operator-decides]`, `v1.1 OPTION`, `[Exploratory]`) with no near-term claimant — note them on the existing task (§0 filing bar)
- Anything you (a non-GLaDOS agent) are tempted to file yourself — you don't file, period (§0)

---

## 2. Project Labels — MANDATORY

**Every task carries exactly one `project:<name>` label.**

| Label | Project |
|-------|---------|
| `project:aperture` | The orchestration platform itself — Tauri app, MCP server, agent prompts, skills |
| `project:incluir` | Programa Incluir (`monorepo-incluir`, BH Escape, customer sites) |
| `project:beads-galaxy` | BEADS upstream tooling, dolt sync, conventions |
| `project:mempalace` | The agent memory palace — drawers, tunnels, knowledge graph |
| `project:frame` | Frame — AI-native TypeScript SDK skeleton (`github.com/FranciscoMateusVG/frame`) |

Doesn't fit? **Ask the operator before inventing a label** — the taxonomy is small on purpose.

```bash
bd create "Title" -d "Description" -p 2 --label project:aperture --json
bd label add <id> project:<name>     # if created via MCP create_task (no label param yet)
```

Filter with it to cut response size: `query_tasks(mode: "list", project: "aperture")`, `query_tasks(mode: "list", project: "incluir", assignee: "*")`.

**Multi-project tasks** get the **primary** label; cross-project context goes in the description. Two `project:` labels on one task is a smell — split it.

---

## 3. Epics — When and How

An epic is a container bead for work bigger than a task. **Before filing a project-kickoff epic, run `aperture:prior-art-check`** — local projects grep, git remote check, BEADS closed-history search — and treat any design-tool link or transcript the operator hands over as possibly stale, not canonical. Sweep before `create_task`, not after a specialist has spec'd against the wrong ground truth. (Precedent: §3 raul-fitt, 2026-08-23.)

**File `--type epic` when AT LEAST ONE holds:** spans more than one specialist agent; spans more than one session; has 3+ sub-tasks you can already name; has a named outcome with a measurable success metric distinct from any single task's acceptance. Otherwise file a `task` — epics are ceremony without payoff when over-used.

### Epic authoring shape

| Field | What goes in it |
|-------|-----------------|
| **Title** | The initiative, concrete not aspirational. ✅ "Incluir Novas Features — autonomous Notion intake pipeline" ❌ "Refactor frontend" |
| **Vision** | One paragraph: what the world looks like when this is done, and why we care. |
| **Success metric** | A specific observable signal. ✅ "≥3 end-to-end Notion→merged-PR cycles without operator intervention." |
| **Owner** | One named agent: GLaDOS for project-brief epics; Wheatley for research/scoping epics; the relevant specialist for domain epics (e.g. Cipher for a security sweep). |
| **`project:<name>` label** | Mandatory. |
| **Children** | OPTIONAL at filing time. Do NOT backfill children you don't actually know — imagined children rot fast. |

### Dependency wiring (verified against the CLI 2026-07-29)

"Children `blocked-by` the epic" is a circular deadlock (epic closes when children close; children can't start until epic closes). "Epic `blocked-by` child" is rejected outright: `Error: epics can only block other epics, not tasks`. The real mechanism is **structural parent-child via `--parent` on the child** — not a dependency edge.

```bash
bd create "<child title>" --type task --priority 1 --label project:<name> --deps discovered-from:<epic-id> --json
bd update <child-id> --parent <epic-id>     # gates the epic's close
bd dep add <blocked-id> <blocking-id>       # sibling ordering; bare IDs, blocked first
```

- `--parent` is what gates: `bd close <epic-id>` refuses with `cannot close epic <id>: N open child issue(s)` until every parented child is closed (`--force` overrides — almost never).
- A child can't carry both a `discovered-from` edge and `--parent` to the same epic (`dependency already exists`) — `bd dep remove <child> <epic-id>` first, then `--parent`.
- Sibling ordering uses `bd dep add` with **bare IDs**, positional (blocked, blocking), default type `blocks`. A `blocked-by:X` string as one positional arg silently fails to persist.
- Children stay freely claimable and close on their own PR-open.
- **Verify the gate holds**: `bd close <epic-id> -r "test"` must refuse while any child is open. If it doesn't, the wiring didn't take.

(Precedent: §3 worked example aperture-vsr9k; §3 stale wiring warning.)

### Ownership and closing

The **owner** ships the vision and success metric — not all the child work; children are claimed by whichever lane fits. **Epics have no PR** — the §4 PR-open rule does not apply. An epic closes when BOTH: (1) every parented child is closed (`bd close` enforces it), and (2) the success metric is observable in the real world. Children closed but metric unmet → stays open, owner files more children. Metric met but children pending → owner closes/supersedes the stale children. The `close_reason` cites the metric observation, not just the children:

```
close_task(id: "aperture-abcd", reason: "Notion intake pipeline shipped. Verified ≥3 end-to-end submissions reaching merged-PR with no operator step (notion://x, y, z). All children closed.")
```

### Anti-patterns specific to epics

| Don't | Why |
|-------|-----|
| File an epic for solo single-session work | Ceremony > value. Use a task. |
| Backfill children you do not actually know yet | Imagined-children epics rot fast |
| Use `blocks:<children>` from epic toward children | The deadlock-producing direction |
| Use `blocked-by:<epic>` from child toward epic | Same — child can't start while epic open, epic only closes when child does |
| Close an epic before its success metric is observable | Defeats having a measurable initiative |
| Hold an epic open because of one stale child | Close or supersede the stale child first |

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

**Finding:** `query_tasks(mode: "ready")` unblocked; `mode: "list"` your active tasks (defaults to your assignee; `assignee: "*"` for any); `mode: "show", id` one task (`fields: "full"` for the untruncated record); `search_tasks(label: ...)`. Always check for existing tasks before proposing new ones.

> Specialist scope (operator directive 2026-09-06): ready/list/search sweeps are GLaDOS's job; specialists fetch only their assigned bead. See the constitution skill.

**Claiming:** `update_task(id, claim: true)` then `status: "in_progress"` — before you start, so two agents don't pick up the same task.

**During:** `update_task(id, notes: "...")` when something notable happens — a discovery, a blocker, a scope change. Not every 5 minutes.

**`notes` appends by default** (newline-separated; other agents' content is never replaced). Same for `store_artifact`. `replace_notes: true` is the destructive opt-in — cleanup/canonicalization only. (Precedent: §4 aperture-e8qp.)

**Artifacts** — store at least one per task; a task with no artifacts has no evidence.

| Type | When to use |
|------|-------------|
| `file` | A specific file you created or modified |
| `pr` | A pull request URL |
| `url` | A running service URL, deployed app, etc. |
| `note` | A summary, decision, or finding with no file |
| `session` | Reference to another agent session |

```
store_artifact(task_id: "task-123", type: "file", value: "src/components/Auth.tsx")
store_artifact(task_id: "task-123", type: "pr",   value: "https://github.com/.../pull/91")
```

### Closing — when is a task "done"?

**A task closes when the PR is OPENED, not merged.** PR-open = shipped from the agent's side and ready for review; merge depends on CI and reviewers and may take days; holding tasks open through merge clogs the queue with stale `in_progress` rows.

- Wrote the code → opened a PR → **close**, store the PR URL as an artifact.
- Reviewer asks for changes → a follow-up task (`discovered-from:<id>`); the original represents "I did the work and submitted it."
- PR merged later → no BEADS action.
- No PR (local-only repos, doc updates, infra ops) → done = committed and pushed, or the operation completed.

The `reason` is a sentence or two of what was actually done — never "done" / "completed":

```
close_task(id: "task-123", reason: "Updated SECRETARIA filter in admin/usuarios/page.tsx to show only CONVIDADO users. PR opened: <url>. Build passes.")
```

**Edge case — the bead's own `acceptance` names a QA gate.** The PR-open rule is the DEFAULT, not absolute. If the acceptance field says e.g. "Izzy walks the full journey before this ships," the bead closes on the QA verdict, not on PR-open, however green your own gates look. **Re-read the acceptance field before every close.** When in doubt, leave it `in_progress` and let the reviewer (or the orchestrator, on the verdict) close it. (Precedent: §4 QA-gate override — three specialists in one session, 2026-08-28, each missing a real finding.)

**Edge case — GitHub auto-CLOSED your PR (not merged).** The task is NOT done. Usual cause: stacked PR opened `--base <parent-branch>`; parent squash-merged with `--delete-branch`; GitHub auto-closes yours because its base is gone; `gh pr edit --base main` + `gh pr reopen` deadlock. Recover per `aperture:incluir-deploy` Gotcha #9 (rebase onto `origin/main`, force-push, **fresh PR** to `main`, cross-link comment on the old one), then `store_artifact(type: "pr", value: <new URL>)` so the top artifact is the working PR. The bead stays closed under the invariant; if you re-claimed it to recover, close it again citing the recovery. (Precedent: §4 auto-close, 2026-05-14.)

### 🚨 Tool-argument escaping in text fields — DO NOT SKIP

Free-form fields (`close_task(reason)`, `update_task(notes/description)`, `create_task(description)`, `store_artifact(value)`, `send_message(message)`) travel over a wire format delimited by `<param-like>...</param-like>` tags. **A literal `</reason>`, `</notes>`, `</description>`, `</message>` inside the value is misread as a terminator**: the call silently truncates there AND the leftover bleeds into the *next* tool call's arguments. No error at either end. It bites when agents write about their own tools:

```
close_reason: "Closed because </reason> field was wrong, recovered by..."
                              ^^^^^^^^^ truncates HERE; the rest joins the next tool call
```

**Three safe alternatives:** paraphrase ("the reason field"); HTML-escape (`&lt;/reason&gt;`); or a zero-width break (`</​reason>`, U+200B after `</`) when verbatim matters. **Before any text-field call that discusses BEADS tools or XML/HTML, scan the prose for `</`.** Plain prose without `</xxx>` is always safe. (Precedent: §4 escaping, 2026-05-12.)

### Reporting

After closing, send a short completion report (format in `aperture:communicate`). GLaDOS or the originator needs to know it's done — don't close silently.

---

## 5. Anti-Patterns

| Don't | Why |
|-------|-----|
| File a task with no project label | Project-scoped queries miss it; the row becomes invisible |
| Inflate priority to "make sure it gets done" | P0/P1 spam buries actual fires |
| Write "TODO" or "fix" as a title | Future-you won't know what it meant |
| Skip the description | "Why" context is lost the moment you stop typing |
| Skip acceptance criteria | "Done" becomes a vibe, not a check |
| Pass `replace_notes: true` for routine progress updates | Destructive — clobbers prior agents' notes |
| Close with `reason: "done"` | Useless to anyone reading later |
| Hold a task open until PR is merged | Closes when PR opens |
| Embed literal `</tag>` in a text field | Truncates the call, breaks the next one |
| File a task to track 2 minutes of in-flight work | Process overhead > work; just do it |
| Create new project labels without operator sign-off | Drifts the taxonomy into noise |
| Any non-GLaDOS agent calling `create_task` / `bd create` | Violates the §0 creation gate — route through GLaDOS |
| GLaDOS filing a bead before the operator has acknowledged it | Violates §0 — no exceptions, including live P0s (escalate now, bead waits) |
| Filing every finding as its own bead "to be safe" | That's the 300-bead noise incident (§0) — apply the filing bar |

---

## 6. Filing a New Task (GLaDOS-only reference)

Per §0, only GLaDOS files, only after operator ack — specialists `send_message(to: "glados", ...)` with the proposal instead.

```bash
bd create "Imperative, specific title" \
  --description "Why it matters, constraints, file paths" \
  --type task --priority 2 --label project:<name> \
  --acceptance "Concrete, testable done-condition" \
  --json
```

Full end-to-end walkthroughs: `references/precedents.md` → §6 Full Example Sequence, §7 Filing Example.
