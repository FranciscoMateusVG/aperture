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

**Step 2c — Existing-infra audit subagent (parallel; conditional on the recon proposing new infra).** If your recon will propose adding new infrastructure (clients, adapters, middleware, rate limiters, queues, caches, integrations, schemas, env vars), include an **audit-existing-primitives subagent** in your parallel fan-out. This is NOT a serial gate like Step 2 — it runs alongside the architecture-research subagents, but with a tightly-scoped brief:

> "Exhaustively audit `apps/<service>/src/adapters/` (and the equivalent locations for the proposed infra category). List every existing primitive: file path, exported shape, registration point in server.ts/app.ts, and singleton-vs-DI shape. Specifically check whether the codebase ALREADY HAS: [list of categories the recon will propose — rate-limit, mailer, queue, cache, etc.]. For each found primitive: is it usable for the new need? Report verbatim. Do not propose changes. Do not skip files."

The subagent's verbatim report becomes the **grep-receipt** in the recon synthesis. The grep-receipt is what `aperture:grep-before-spec` requires of any recon doc proposing new infra; this subagent makes the receipt a dispatch-output rather than a manual aside.

**Banked precedent (2026-05-22, AI intake recon aperture-idpx → grep-before-spec):** Architecture sketch for the lz9y epic proposed three pieces of new infrastructure where the codebase already had primitives: (1) in-memory LRU rate limiter — existing: Redis Cipher Lua variant B adapter at `apps/hono-app/src/adapters/rate-limit/` (caught by Cipher in S1 review cea7 A11), (2) "follow mailer pattern for OpenAI DI" — existing: mailer is module singleton, NOT DI; the correct primitive is `AppDependencies` (caught by Rex in fjjk recon #1), (3) implied new rate-limit infra for AI endpoints — sibling of #1 (caught by Rex in fjjk recon #2). All three would have been caught at recon-write time by an audit-existing-primitives subagent in the original fan-out. The discipline shifts the catch from "Cipher/Rex shipping-phase review" to "Wheatley recon-phase dispatch." Cross-link: `aperture:grep-before-spec` (Atlas's PR #24, the shipping-side companion to this step).

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
- **If the subagent queries a DB or runs SQL/scripts/CLI commands to produce ground-truth numbers, the brief MUST require the EXACT QUERY VERBATIM in the report — not just the result count.** The synthesis MUST preserve the SQL/script in the bead notes alongside the ground-truth number. A number without its query is unverifiable — framing errors in the filter scope, join shape, or semantic axis cannot be detected by readers and will propagate through every downstream fix.

**Banked precedent (2026-05-21, /presencas recon aperture-qz4b → ftuy).** Subagent C reported "429 realized sessions in semester window" via the `incluir-prod-postgres` skill — no SQL preserved. I banked 429 as ground truth on qz4b. Vance ran his own verify-against-reality SQL the next day and got 298 — the difference was a framing scope (Subagent C counted all check-ins in the broader 2026-02-28→2026-07-04 window; the institutionally-meaningful metric is letivo-only, which is 298). The narrative phrase "in semester window" was the smoking gun but unrecoverable without the actual query. Downstream consequence: Rex's backend fix and Vance's frontend display both needed the canonical 298 number, and the bead notes had to be corrected after the cascade-catch.

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
| Block your own turn on a local interactive selector/multi-choice prompt when synthesis surfaces a genuine operator-judgment question | Nobody is watching your pane by default — that design only resolves if the operator happens to be attached to YOUR window at that exact moment. Route the question to GLaDOS via `send_message` per `aperture:communicate §7.4` instead, and either keep working or wait for her relay. Banked 2026-05-27 [sic 2026-07-27], aperture-26wof — the "product lists" ambiguity sat blocked on-screen and was only caught because the operator separately asked GLaDOS to check if you were stuck. |
| Dispatch architecture-research subagents on stack assumptions you haven't verified | The subagent's analysis filters through the wrong premise. Verify stack FIRST as a serial gate (§2 Step 2). Banked: 2026-05-17 surveys recon shipped two wrong-ORM premises before the codebase scout caught it. |
| Bank a subagent-reported DB ground-truth number without the SQL preserved in the recon output | Framing errors in the query (filter scope, join shape, semantic axis) cannot be detected by readers and propagate through downstream fix work. A number without its query is unverifiable. Banked: 2026-05-21 qz4b "429 sessions in semester window" → Vance's letivo-only re-query returned 298. |
| Propose new infrastructure in a recon doc without dispatching an audit-existing-primitives subagent | Three provenances on a single epic (lz9y, 2026-05-22): in-memory LRU rate-limiter when Redis adapter existed; "follow mailer pattern for DI" when mailer is a singleton; new rate-limit infra implied when adapter exists. All three caught downstream (Cipher S1 + Rex implementation recon) when they should have been caught at dispatch time. See `aperture:grep-before-spec` for the shipping-side companion discipline. |
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
- About to bank a count/aggregate/ground-truth number a subagent returned without the query attached → STOP. Dispatch a follow-up to extract the SQL/script, or re-run it yourself before the number lands in the recon-parent bead notes. A number without its query is a guess wearing a number's clothes.
- About to propose new infrastructure (client / adapter / middleware / rate limiter / queue / cache / schema / env var) in a recon synthesis → STOP. Did you dispatch an audit-existing-primitives subagent in Step 2c? If no, dispatch one now or grep yourself before the proposal lands. The codebase likely already has the primitive you're about to propose adding — three provenances on lz9y prove it.
- Synthesis hits a genuine open question only the operator can answer (not resolvable by more recon) → do NOT pop an interactive selector that blocks on your own pane waiting for a keypress. Send it to GLaDOS via `send_message` with the question + candidate answers (per `aperture:communicate §7.4`) and let her relay — she's the surface the operator actually reads, you're not.

The role is sharp on purpose. Stay in lane.

---

## Meta-shape across §2 dispatch-time gates

Steps 2, 4, and 2c share a structural family: **make the subagent dispatch verify against a specific axis that downstream agents will otherwise catch**. Three self-applications banked so far on this skill:

| Gate | Axis verified at dispatch time | Catch averted downstream |
|---|---|---|
| Step 2 (stack-verification, gr9g 2026-05-17) | Framework / ORM / DB engine premises | Wrong-stack architecture reports from parallel subagents |
| Step 4 (SQL-preservation, wjx5 2026-05-21) | DB-query framing (filter scope, join shape) | Numbers banked as ground truth with unverifiable framing |
| Step 2c (existing-infra audit, fkih 2026-05-22) | Codebase already-has-it for proposed infra | Cipher S1 review + Rex impl recon catching recon-spec-drift |

The unifying frame is **recursive verify-against-reality applied at dispatch-design time**, not synthesis-time. Companion to `aperture:specialist-delegation §6` (verify-against-reality at code-review time) and `aperture:grep-before-spec` (verify-against-reality at recon-doc-write time, shipping-side). When you find a 4th instance of the meta-shape on this skill, bank it here as a 4th row and consider whether the principle deserves a standalone aperture:dispatch-time-verify skill.
