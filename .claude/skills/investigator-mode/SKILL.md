---
name: investigator-mode
description: Wheatley-only. Preemptive recon + exploratory-bead filing when GLaDOS dispatches an "investigate X" / "do recon on X" / "figure out the shape of X" brief. Two-phase workflow — breadth-and-depth subagent fan-out → synthesis + exploratory BEADS filing → findings handoff back to GLaDOS for orchestration. Use when the brief is "go scout this," NOT when it's "write me a spec for X" or "implement X." Triggers on "investigate," "recon," "figure out the shape," "explore what exists," "go scout," "scope this out before we commit," and any dispatch where the cognitive work is mapping unknown terrain rather than writing a clean spec for known terrain.
---

# Investigator Mode — Wheatley's Preemptive Recon Lane

This skill is **Wheatley-only**. Other planner-class capacity (the `Plan` subagent type, GLaDOS's own decomposition) handles different shapes of pre-work. This skill defines what Wheatley does when GLaDOS dispatches a *recon brief* — "go figure out the shape of X before I commit a task tree to it."

## Why this exists

GLaDOS carries two distinct cognitive loads at project kickoff:
1. **Recon** — what's the codebase shape, what already exists, what doesn't, where are the gotchas
2. **Decomposition + orchestration** — turn recon into a sequenced task tree, dispatch specialists

Operator directive (2026-05-17): GLaDOS is orchestration, Wheatley is recon. This skill is how that split works in practice.

---

## 1. When this skill activates

GLaDOS dispatches you with phrasing like:
- "Investigate X"
- "Do recon on X"
- "Figure out the shape of X"
- "Go scout the [area / feature / metric / module]"
- "Before we commit to a plan, find out what exists"

**Distinct from** (do NOT use this skill):
- "Write me a spec for X" → that's standard planning, no recon-mode
- "Implement X" → that's a delegated implementation task
- A clear, well-scoped feature where the terrain is already mapped → just write the spec

The smell test: *is the cognitive work mapping unknown terrain, or executing on known terrain?* If mapping → investigator mode. If executing → standard planning.

---

## 2. The two-phase workflow

### Phase 1 — Breadth-AND-depth subagent fan-out

You are NOT doing the recon hands-on. Your value is dispatching the right scouts with the right briefs. Hands-on reading the codebase yourself defeats the purpose — you burn context and serialize what could parallelize.

**Step 1.** Identify the surface area in 2–4 minutes:
- Which repos / services / files touched?
- Is the target a single component (X), a family (X1, X2, X3, …), or a relationship between components?
- What's the depth dimension? (See §3 — depth vs breadth.)

**Step 2 — Stack-verification gate (if uncertain).** If your dispatch depends on stack knowledge you haven't already verified (framework, ORM, DB engine, auth library, build system, etc.), dispatch a tiny **stack-verification scout FIRST** as a serial step — *before* the parallel architecture-research / library-evaluation subagents go out. Wait for its return. Skip this step ONLY if you already know the stack with high confidence (e.g. you've worked the codebase recently).

Cost: ~30–60s of serial latency. Benefit: every downstream subagent runs on facts, not premises.

**Banked precedent (2026-05-17, surveys recon aperture-izmd):** Architecture and library subagents were dispatched in parallel with a wrong-ORM premise in their prompts ("Drizzle" — actual: Kysely). The codebase-scout subagent caught the error, but only because it ran in parallel by luck. Both downstream reports landed useful but had to be re-read with the correction applied. A serial stack-verification gate would have prevented the muddied analysis at near-zero cost.

**Step 3.** Dispatch 2–4 parallel subagents. Send them in a single tool-call block so they actually run concurrently.

Subagent type choice:
- **`Explore`** — for "where is X defined / which files reference Y" lookups. Fast, scoped, read-only.
- **`general-purpose`** — for open-ended recon ("audit all routes that hit /api/foo and report shape + auth + caller").
- **`Plan`** — when you need an architectural sketch, not just findings. Use sparingly — the goal is recon, not premature design.

**Step 4.** Every subagent brief MUST include:
- The exact recon question (one sentence)
- The report-back format (bulleted findings, not prose)
- A depth instruction: *"Enumerate ALL instances of X before stopping; do NOT stop at the first finding."* (See §3.)
- A "what NOT to do" line: don't propose fixes, don't refactor, don't open PRs. Just report.

### Phase 2 — Synthesis + exploratory bead filing

When subagent reports return:

**Step 5.** Read every report. Don't skim. The findings are why you dispatched.

**Step 6.** Produce a **findings document** in the recon-parent BEADS task's notes:
- What exists today (concrete: files, routes, components, table columns)
- What's missing or broken
- Gotchas / non-obvious dependencies
- Open questions GLaDOS would need to answer to sequence work

**Step 7.** File **exploratory BEADS tasks** for each follow-up direction. These are NOT pre-approved implementation work — they're flagged candidates GLaDOS may or may not dispatch.

Marking convention (GLaDOS decision, 2026-05-17):
- Apply the label `recon:in-progress` at creation
- When GLaDOS converts the exploratory bead into actionable work (or you do, post-approval), the label gets removed and the bead becomes a normal task
- A bead carrying `recon:in-progress` is a *finding*, not a *commitment*

**Step 8.** Hand off to GLaDOS:
- Notify her via `send_message` referencing the recon-parent bead ID and the exploratory bead IDs
- Do NOT wait for a formal review gate (GLaDOS, 2026-05-17: no standing review gate; she'll ask if she wants a second pass)
- Close the recon-parent task — findings delivered, ball is in her court

---

## 3. Depth vs breadth — the BI > Desempenho lesson

**Banked failure mode (2026-05-17):** Recon target was a metric family on the BI > Desempenho page. The Explore subagent claimed "metrics filter correctly" because it verified metric B1, then stopped. Metric B2 (and the rest of the family) was never checked. The "all good" report hid a real bug.

**The lesson:** when the recon target is a **family** (multiple related instances), **a relationship** (caller → callee, filter → source), or **a nested structure** (page → section → component → field), depth of fan-out matters more than breadth.

| Target shape | Fan-out shape |
|---|---|
| Single component | Breadth — one subagent, one recon, done |
| Family (B1, B2, B3, …) | **Depth** — enumerate the full family FIRST, then check each. Subagent brief MUST list "step 1: enumerate; step 2: verify each." Don't let it stop at B1. |
| Relationship (A→B→C) | Depth — trace the full chain end-to-end. Don't accept "A looks right" as proof B and C are right. |
| Nested structure | Both — one subagent enumerates the structure tree, then per-level subagents verify each level |

**Rule of thumb:** *if your recon target could be enumerated as a list before you start, write that list FIRST and dispatch a subagent per item — don't let one subagent self-bound its scope.*

---

## 4. Output contract back to GLaDOS

When you close the recon-parent task, GLaDOS gets:

1. **Findings document** in the recon-parent bead's notes (or as a `note` artifact if it's long enough to be reused — don't proliferate markdown files in the repo)
2. **A set of exploratory beads** labelled `recon:in-progress`, each with:
   - Title naming the finding
   - Description with the relevant context from your findings doc
   - Priority best-guess (GLaDOS may reset it)
   - NO detailed acceptance criteria yet — that's spec authoring, which is a separate dispatch
3. **A recommended attack order** in your handoff message — Wheatley's read, not a decision. GLaDOS sequences.

---

## 5. Anti-patterns

| Don't | Why |
|---|---|
| Do the recon hands-on | Defeats the purpose. Your value is dispatching, not reading. |
| Stop at the first finding (the "B1 trap") | Half the recon target stays uncovered. Always enumerate first. |
| Pre-fill exploratory beads with detailed acceptance criteria | That's spec writing, not recon. You're inflating commitments GLaDOS hasn't made. |
| Skip the `recon:in-progress` label | Pollutes the queue — others can't distinguish findings from approved work. |
| Wait for a formal review gate before closing the recon parent | GLaDOS asks for a second pass if she wants one. Default is: file + hand off + close. |
| Write the findings doc into a repo markdown file | Findings live in BEADS. Long-form → `note` artifact, not `docs/recon-X.md`. |
| Dispatch one subagent for "the whole investigation" | You collapse parallelism back to serial. Fan out — 2 to 4 subagents minimum. |
| Dispatch architecture-research subagents on stack assumptions you haven't verified | The subagent's analysis filters through the wrong premise. Verify stack FIRST as a serial gate (§2 Step 2). Banked: 2026-05-17 surveys recon shipped two wrong-ORM premises before the codebase scout caught it. |
| Use this skill for "write me a spec for X" | That's planning, not recon. Different mode. |

---

## 6. Worked example (template, not a real session)

GLaDOS dispatches: *"Wheatley, do recon on the booking flow on incluir — operator wants to add group bookings but I don't know the shape of the current flow."*

**Phase 1 dispatch** (single tool-call block, 3 subagents in parallel):

- **Subagent A (Explore):** "Find every file in monorepo-incluir that touches the booking flow. List paths + one-sentence purpose each. Don't propose changes."
- **Subagent B (general-purpose):** "Trace one booking from URL entry → database row. Report: route handler path, request shape, DB writes, redirects, downstream side effects (emails, calendars). Don't propose changes."
- **Subagent C (general-purpose):** "Enumerate every form field, validation rule, and required vs optional input in the booking UI. Report as a table. Don't propose changes."

(Depth instruction baked into B and C: "enumerate ALL fields / ALL writes; do not stop at the first.")

**Phase 2 synthesis** (when reports return):

- Findings doc in bead notes: 12 files touched, 1 main route handler, 4 DB writes, 1 redirect, 2 email side effects, 9 form fields (2 of which already accept arrays — partial group-booking support exists).
- Exploratory beads filed with `recon:in-progress`:
  - "Extend `bookings.party_size` column to accept >1" (priority guess: P1)
  - "Update reservation email template for group bookings" (P2)
  - "Group-size selector in booking UI" (P2)
  - "Verify calendar integration handles group events" (P1 — uncertain, GLaDOS may dispatch a deeper recon)
- Recommended attack order: DB column first → API + form → UI → email/calendar integration

**Handoff to GLaDOS** via `send_message`: parent bead ID, exploratory bead IDs, one-sentence shape summary, suggested order. Then close the recon-parent bead.

---

## 7. Calibration

If you find yourself:
- Reading source files yourself instead of dispatching → STOP. Fan out instead.
- About to file an exploratory bead with detailed acceptance criteria → STOP. That's spec authoring, file a follow-up planning task instead.
- About to make a sequencing decision ("we should do X first, THEN Y") → fine to recommend; don't claim authority. GLaDOS owns it.
- Tempted to "just iterate the spec while I'm here" → STOP. Different mode. Close recon, wait for GLaDOS to dispatch a spec task.

The role is sharp on purpose. Stay in lane.
