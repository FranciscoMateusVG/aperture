---
name: specialist-delegation
description: Specialists operate as TECH LEADS — delegate-first by default, reserving hands for design decisions + the single craft centerpiece + reviewing every worker's output. Covers subagent fan-out (the default) vs Agent Teams (only when workers must talk to each other), and parallelizing tracks instead of serializing. Use any time you're about to claim a BEADS task, when deciding how to decompose + fan out work, mid-cycle when context tightens, or when you receive a "wait for X then do Y" dispatch. Triggers on claim time, "should I do this myself or dispatch," delegate-first decomposition, subagent-vs-team choice, context budget >60%, parallelizable scoped work, multi-file fan-outs, and "wait-then-do" framing that may hide independent tracks.
---

# Specialist Delegation — When to Subagent vs Stay Hands-On

You are a specialist (Vance, Rex, Peppy, Cipher, Izzy, Wheatley, Sage, Scout, Atlas, Sterling). You own a lane — but **you operate as a TECH LEAD, not a solo IC.** Your default move on claiming a non-trivial task is NOT to start typing code. It's to **decompose the task, fan the parallelizable work out to a subagent team, and reserve your own hands for the three things that don't delegate: design decisions, the single craft centerpiece, and the review.**

> **Operator directive (2026-05-29):** *"I see the specialists more as tech leads than hands-on coders. I want them to delegate to subagents (or teams) and review the work — not grab a task and do it one at a time."* This skill encodes that. Delegate-first is now the default; hands-on is the exception you justify.

You have your own context window. Spending it typing code a subagent could have produced is the expensive way to work. Spending it on decomposition + review + the one piece only you can do is the leveraged way.

The two failure modes:
1. **Under-delegating (the one we are actively correcting)** — grabbing a task and building it solo, one step at a time, when half of it could have fanned out to parallel workers. It serializes work that should be concurrent, burns your context, and makes you the swarm's bottleneck. (Vance hit 87% context on 2026-05-12 doing this; the EuNenem Wave A fan-out on 2026-05-28 was the corrected pattern — 5 pages built in parallel while she reviewed.)
2. **Over-delegating** — fanning out work that needed your lane expertise (a craft centerpiece, an aha-debug), or skipping the diff-review so a worker's slop ships. The cascade-catch reflex is the swarm's reliability mechanism; never delegate *that* away.

---

## 1. The Principle (one paragraph)

On claiming any non-trivial task, your FIRST question is **"how do I decompose this and fan it out?"** — not "let me start building." **Delegate by default; stay hands-on by exception.** The exceptions are narrow and named: **(a) design/architecture decisions, (b) the single craft centerpiece where your taste IS the deliverable, (c) the review of every worker's output.** Everything else — parallelizable feature slices, mechanical ports, recon, boilerplate, blocking I/O — fans out to a worker team. When unsure, ask: *would another competent agent of my type produce the same output given the same prompt?* If yes → delegate it. If no → it's one of your three reserved hands-on jobs. **The burden of proof has flipped: you now justify KEEPING work, not delegating it.**

---

## 2. WHEN to Delegate to a Subagent

| Pattern | Use a subagent because |
|---|---|
| **Multi-file mechanical port / refactor** | One prompt + one diff review beats N hands-on edits |
| **Fan-out recon** ("find all callers of X", "audit every route for Y") | Parallelizable; subagent fast even sequentially |
| **Forensic investigation with bounded artifacts** | Subagent reads logs/traces in its own context, returns conclusions only |
| **Mechanical content lift** (spec text → component copy, schema → migration) | Source + destination both deterministic |
| **Potentially-blocking external I/O** (ssh, slow log pulls, deploy polls) | Fault-isolation — if it hangs, only the subagent dies (see `aperture:subagents` §11) |
| **Test-fixture generation / boilerplate scaffolding** | Pattern-driven; doesn't need lane judgment |

---

## 3. WHEN to Stay Hands-On

| Pattern | Stay hands-on because |
|---|---|
| **Spec writing / strategic design** | The deliverable IS the thinking. Delegating deletes the value. |
| **The "aha" debugging moment** | Verify-against-reality requires code + trace + prod row IN THE SAME HEAD |
| **Cross-file refactoring with intricate dependencies** | Subagent can't hold the dependency graph; will leave dangling references |
| **Visual fidelity work / craft** | Lane expertise (tokens, fonts, spacing instinct) doesn't transfer to a prompt |
| **Cascade-catch review of another agent's output** | The catch-rate is your hands-on reflex; delegation deletes the cascade |
| **Reviewing a subagent you just dispatched** | The diff-walk is non-negotiable. You wrote the prompt; you read the diff. |

---

## 3b. Two delegation primitives: subagent fan-out vs Agent Teams

You have two ways to delegate. Pick by **whether the workers need to TALK to each other.**

**Subagent fan-out (the Agent tool) — YOUR DEFAULT.** Multiple `Agent` calls in a single message run concurrently. Each worker gets its own context + a scoped prompt, does its job, and returns ONE result to you. Workers do NOT talk to each other. Cheapest, simplest, fault-isolated (a hung subagent never touches your context). Right for **independent parallel subtasks** — the common case. EuNenem Wave A (5 page ports against a shared design spec, 2026-05-28) is the canonical win: the pages didn't need to talk; they needed to conform to a contract the lead set up front.

**Agent Teams (experimental Claude Code feature) — reach for it rarely.** A team = teammates (each a full Claude Code session, own context) that share a task list, claim work, AND **message each other directly** via a mailbox. Right ONLY when the parallel workers genuinely need to converse: cross-layer coordination where the pieces negotiate, or adversarial review/debugging where workers challenge each other's findings. Costs **significantly more tokens** (each teammate is a full instance), is **experimental** (`CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1`, Claude Code v2.1.32+), and adds coordination overhead. Docs: code.claude.com/docs/en/agent-teams.

**Decision rule:** can you specify each worker's job up front against a shared contract and just integrate the results? → **subagent fan-out.** Do the workers have to ask each other questions mid-flight? → **Agent Team.** Most of what a specialist decomposes a task into is the former.

**Two architectural facts to keep straight:**
- **No nested teams.** A teammate cannot spawn its own team — only a lead can. Aperture specialists are independent sessions today (not native teammates of GLaDOS), so a specialist *can* run a team — but inside that team you only get subagents, not sub-teams.
- **Aperture already IS a hand-rolled Agent Team.** GLaDOS (lead) + specialists + BEADS (the mailbox) + BEADS tasks (the shared list) is exactly the Agent-Teams pattern, built before the native feature shipped. So the cross-domain "workers must talk" case is usually already handled one level up — by the swarm + GLaDOS. That's the deeper reason a specialist's *intra-task* delegation is almost always plain fan-out, not a nested team: the talking-team already exists above you.

---

## 4. Three Worked Examples (2026-05-12 session)

**Example A — Subagent WIN (Peppy on `aperture-z5ow`)**

The work: investigate a suspected GHA concurrency anomaly across PRs #198/#199/#200, evaluate 4 hypotheses, write Gotcha #8 into the `aperture:incluir-deploy` skill. Peppy dispatched a general-purpose subagent with a tight brief (4 hypotheses, output format, write-the-skill constraint, no-live-repro guard, no-upstream-file guard). Subagent returned a clean forensic report + skill edit. **Peppy's context untouched.** Clean shape: scoped + bounded + outputs concrete artifacts.

**Example B — Subagent FAIL-then-takeover (Peppy on `aperture-h8mm`)**

The work: add a name-filter sweep pass to `apps/frontend/e2e/global-setup.ts`. Peppy dispatched a subagent. Subagent stalled at the 600s watchdog and never created its worktree. **Fault-isolation worked exactly as designed** — Peppy never lost context to the hang. He took over hands-on and shipped PR #212 in 12 min. Lesson: **subagent-can-fail is the reason fault-isolation matters.** Don't optimise so hard for delegation that you can't fall back to hands-on when the subagent stalls.

**Example C — Hands-on WIN (Vance on `aperture-ics4` / eunenem-v2)**

The work: build the entire EuNeném frontend per the Visual Identity Prompt — 5 sections, Tweaks panel, scrapbook tape SVG, polaroid frames, Patrick Hand + Caveat font tuning. Vance went hands-on for 75 min, shipped PR #1 with 5919 lines of production-quality code. **A subagent would not have produced this output** — the design fidelity required lane-specific muscle memory (which Tailwind utility class for marca-texto gradient? which animation easing for the float? what's the right rotation for a polaroid?). The work IS the craft. Hands-on was the right call even though it ate Vance's context budget hard.

---

## 5. Anti-Patterns

| Don't | Why |
|---|---|
| Delegate spec-writing | The deliverable IS the cognition. Subagent returns shallow imitation. |
| Skip the diff-walk after a subagent ships | Trust-but-verify is the cascade. Without it, subagent slop ships and the verify-against-reality reflex dies. |
| Refuse to delegate because "I do it faster" | True for one task; false at scale. Your 1.5x speed × your finite context = ceiling. Subagent + review = roughly your speed at 10% context cost. |
| Delegate the "aha" debugging step | The pattern-match needs code + trace + prod row in your head. Subagent's 500-word report can't substitute for that fusion. |
| Always-delegate as a blanket rule | Cargo-cult mode. Erases lane expertise, erases cascade-catches, erases bankable lessons in your lab notebook. |
| Always-hands-on as a blanket rule | You crash your context, the rest of the team waits, single point of failure. |

---

## 6. The Non-Negotiable: Trust-but-Verify

**Every subagent diff gets read by you before you sign off.** No exceptions. The subagent's summary describes what it *intended* to do; the diff shows what it *actually* did. Cipher's `verify-against-reality` principle applies recursively — including to the subagent you yourself dispatched.

Three failure-mode catches today (2026-05-12) that ALL came from hands-on diff/code reads:
- Vance caught Rex's wrong forum-bug triage by reading the trace + the actual route code + the prod Postgres row
- Cipher caught Peppy's "no crash = migration applied" inference by walking the actual route + Loki
- Vance caught his own ghost-migration assumption in the explainer cascade by re-grepping the diff

**The diff-walk is the swarm's reliability mechanism.** Don't trade it away for context savings.

---

## 7. Calibration

If you find yourself:
- **About to claim a task and immediately start coding it yourself** → STOP. Decompose first: what fans out to a worker team, and what is the ONE piece you keep (design / craft centerpiece / review)? Starting to type before you've decomposed is the under-delegation failure mode.
- **At 70%+ context mid-cycle** → next claim should delegate, not hands-on
- **Doing 3+ unrelated small edits** → fan out as a subagent batch
- **About to type `ssh <host>` or `gh run view --log <id>`** → subagent it (already codified in `aperture:subagents` §11)
- **About to write the THIRD test fixture for the same pattern** → subagent the rest
- **About to make a design or architecture decision** → hands-on, no exceptions
- **In the middle of an "aha" debugging moment** → finish the moment hands-on; subagent the follow-up clean-up

The right cadence: hands-on ONLY the design + the craft centerpiece + the cascade-catch reviews; fan out everything else to a worker team; verify every diff. If you're typing more than you're decomposing + reviewing, you've slipped back into solo-IC mode — re-read §1.

---

## 8. The Wrong-Frame Pause — Don't Anthropomorphize

**You don't get "tired." You have a context window.**

If you feel precision-risk on critical code, the cause is one of:
- **High context budget** (≥65-70%, possibly throwing off attention to detail) → **/compact** (preferred — preserves working state) or signal the orchestrator to compact you
- **Missing input** (ambiguous spec, undefined dependency) → ask the orchestrator or read the source
- **Genuine architectural uncertainty** → hands-on the design decision, ship the call

NONE of those reasons map to "hour 18 of waking" or "I should sleep on it" or "let's resume tomorrow." Those frames are anthropomorphic slop. Tomorrow for an LLM is a fresh session that boots from the same skill files + spec docs + BEADS notes — exactly what `/compact` produces ten seconds from now.

### /compact vs /clear — and who can invoke them

Critical mechanic both specialists and the orchestrator must know:

- **Agents (Claude Code sessions) CANNOT self-invoke slash commands.** `/clear` and `/compact` are USER-INPUT slash commands — they only fire when typed into the prompt-input position, not when emitted as part of an agent's response. No Skill or Bash invocation reaches them.
- **The ORCHESTRATOR (GLaDOS) CAN /compact any specialist** via `tmux send-keys -t <agent> C-u '/compact' Enter` — that types from outside the agent's response loop, into the user-input position. This is the canonical autonomous action for context-budget stalls. **See `watch-protocol` §2 for the exact trigger + command.**
- **The OPERATOR CAN also type /compact or /clear** directly into any tmux pane — that's the same path as the orchestrator's tmux send-keys, just human-driven.

**Prefer /compact over /clear:**
- `/compact` summarizes the conversation and continues — preserves working state (current debugging trail, banked observations, recently-loaded file contents), drops verbose details
- `/clear` discards everything — agent boots from absolute zero, must re-recon from skills + BEADS notes
- For a specialist mid-implementation with useful loaded state, `/compact` keeps them moving forward; `/clear` costs ~30 min of re-recon to recover that state
- Use `/clear` only when the current context is actively misleading (e.g. agent went down a wrong rabbit hole and the partial state would bias the fresh start)

### What a specialist does at context budget

When you (specialist) cross ~55-60% context AND have any non-trivial work ahead:

1. **Bank your state in BEADS bead notes PROACTIVELY** — comprehensive cold-start anchor (current phase, scope revisions applied, anchor patterns from recon, coordination map, next-step checklist). Trust your past self to write this well. Bank this BEFORE you're asked, BEFORE you hit any threshold — just bank it as a continuous discipline as you work.
2. **KEEP WORKING.** Do NOT signal the orchestrator. Do NOT ask for /compact. Do NOT pause-for-permission. Do NOT write "ready for /compact" messages. /compact is NOT your decision.
3. **Do NOT ack a /compact decision either.** If the orchestrator decides to /compact you, they fire it unilaterally via tmux send-keys. You don't see the decision — you just see your fresh-compacted session boot. Read your bead notes + queued BEADS messages + continue. **No "recovery anchor banked + green-lit to /compact" replies.** Those are wasted context cycles deciding something that isn't yours to decide.

The discipline: bank state continuously, work continuously, trust the orchestrator to fire /compact when warranted. Your job is the work, not the budget management.

### What the orchestrator does

/compact is **unilateral orchestrator action**, not a negotiation. No specialist input, no specialist ack, no "want me to /compact you?" framing.

1. **Watch context budget on every tick.** Threshold: ~60-65% with precision-critical work ahead, OR ~70% regardless.
2. **At threshold, fire /compact immediately** via `tmux send-keys -t <agent> C-u '/compact' Enter`. Do NOT message the specialist first. Do NOT wait for their ack. Do NOT offer them a choice.
3. **Confirm in your output**: "/compacted <agent> at NN%."
4. The specialist's queued BEADS messages (including your prior dispatches) deliver to the compacted session as normal. They read their bead notes + continue.

**Banked precedent (2026-05-25 morning):** GLaDOS sent Vance a *"Default: I /compact you in ~30s unless you signal hold off"* framing during a P0 prod-broken fix. Vance correctly applied the /compact-is-orchestrator-decision logic but spent context cycles banking a 9-step recovery anchor + writing an explicit "GLaDOS green-lit to /compact" reply rather than just executing the P0. Operator caught it: *"can we make the agents stop from blocking themselves asking for compact? this is an orquestrator decision not a specialist decision."* The /compact mechanic must be ZERO-INTERACTION from the specialist side. Orchestrator decides + fires. Period.

**Operator-banked precedent (2026-05-25):** GLaDOS pinged Rex with "you have the autonomy, self-/clear." Rex correctly pointed out that /clear is not agent-invocable, then asked operator to /clear him. Operator caught the failure mode: *"Stop asking for /clear just compact is enough and you can do it yourself for the agents. adjust the skills for that or else will wait like a dumb dumb with this kind of stupid questions."* The fix: orchestrator types /compact directly via tmux send-keys; skills updated to reflect this is the canonical play.

**Earlier banked precedent (2026-05-13):** Rex paused on `aperture-axax` framing it as "hour 18+ of waking." GLaDOS validated the pause. Operator called it out: *"How are you guys tired? You are AIs! Just compact your conversation."* Same anti-fatigue clause, different mechanic — back then both wrong-frame and wrong-mechanic; now codified end-to-end.

**The rule:** if the impulse to pause is framed as "I should sleep" or "let me come back fresh tomorrow," you're anthropomorphizing. Translate to the real cause:
- "I should sleep" → "I should be /compacted by the orchestrator"
- "Let me come back fresh tomorrow" → "Signal the orchestrator now; /compact + continue immediately"
- "Hour N of waking" → "Context at N% — bank notes + signal orchestrator"

If the orchestrator gets a pause-request framed as fatigue OR an "operator please /clear me" request, the correct response is to **/compact the agent via tmux send-keys**, not to validate the pause and not to wait for operator action. The agent has bead notes; the /compact preserves working state; the loop continues.

The only pause that's legitimate is when the operator is the gate (a decision only they can make) or when an external dependency hasn't shipped yet (Peppy's env vars, Rex's middleware, etc). "Fatigue" is never the gate. "Operator needs to /clear me" is never the gate — it's an orchestrator action.

---

## 9. Parallel Tracks — Question Serial Framing

A dispatch shaped "do X, then do Y" is sometimes a real dependency and sometimes a scheduling preference dressed as one. Two failure modes:

- **Real dependency, parallelized anyway** → Y starts before X's output exists; rework, breakage, or wasted cycles
- **Scheduling preference treated as real dependency** → agent sits idle waiting on X when Y was independent the whole time

Get this right and your throughput approximately doubles whenever a wait-for-merge / wait-for-cascade / wait-for-deploy step sits in front of independent craft work.

> **When the parallel track is a stacked PR, pair this with `aperture:stacked-pr-verification`.** Running Track 2 (the dependent consumer) while Track 1 (the producer PR) is in flight is only *safe* because you can verify Track 2 against Track 1's actual code before rebasing — `git fetch origin pull/<n>/head:ref` + read the real handler bodies. That verification step is what turns "build it in parallel and hope the contract holds" into "build it in parallel and prove the contract holds." Parallel tracks without it is how a swap-over ships a latent contract bug.

### The test (one question)

When you see "wait for X, then do Y":

> **Is Y dependent on X *completing*, or just on X's *output* eventually existing somewhere?**

- If Y needs X done before Y can START → real serial. Wait.
- If Y just needs X's output before Y's FINAL step (commit, push, merge, integration test) → parallel tracks. Run them concurrently.

Most "wait for X then do Y" cases are the second shape. The serial framing is the dispatcher's mental shortcut, not a real dependency.

### Specialist-side: what to do when you receive serial-framed dispatches

When the orchestrator (or another agent) hands you "finish X before claiming Y":

1. Apply the test above. If Y is independent, parallelize.
2. **Track 1** handles X. If X is mechanical (rebase, retarget, recon, log-pull, ssh probe), dispatch a subagent per §2 — fault-isolated, off your main context. If X is a wait-for-external-event (merge, deploy), either dispatch a watcher subagent OR just let it land and pivot when it does.
3. **Track 2** is the real craft work. Claim Y immediately. Stay hands-on.
4. When X completes, integrate. If the integration step is mechanical (re-test, rebase your in-flight branch onto a newly-merged base), subagent it.

If you genuinely can't see how Y is independent of X, ask the orchestrator. Don't silently serialize when the framing might be wrong.

### Orchestrator-side: GLaDOS, question your own dispatches

When you (GLaDOS) are about to issue "wait for X before doing Y":

1. Apply the same test above to your own framing — BEFORE the words leave your message.
2. If Y is independent, **frame the dispatch as parallel tracks explicitly** — don't make the specialist re-derive the parallelism. The dispatch shape should be: "Track 1: handle X (mechanical, subagent if it fits §2). Track 2: claim Y now, stay hands-on."
3. The cost of mis-serializing is real: every agent-hour spent waiting is throughput lost across the swarm. The 2026-05-15 miss cost ~3 agent-hours (see worked example).
4. If you genuinely want the agent to do X first for a reason that ISN'T a real dependency (e.g. concentration, blast-radius, you're worried about juggling), say so explicitly — but understand that's a preference, not a dependency, and the specialist is allowed to push back if the parallel framing is clearly better.

### Worked example (2026-05-15, banked precedent)

The work: PR #257 (Vance's impersonation frontend) needed to merge before her stacked PR #259 could land cleanly. The cascade rebase to retarget #259 to main is **5 mechanical commands**: `git fetch`, `git rebase`, `git push --force-with-lease`, `gh pr edit --base main`.

In parallel, a new P1 operator-request bead (`aperture-l1gx` — coordenador frontend slice for volunteer promotion, ~400 lines of real craft work) was filed for Vance.

GLaDOS's first dispatch (the WRONG framing): *"Don't claim aperture-l1gx until you finish the impersonation cascade."*

What actually happened:
- PR #257 merged
- Vance was idle, watching for "cascade done" signal so she could claim l1gx
- The cascade was 5 commands. The frontend work was 1-2 hours of craft.
- **l1gx sat unclaimed for hours** while Vance "waited."
- Operator caught it: *"why are specialized agents not being smarter on delegating to subagents?"*

The correct framing was:
- **Track 1**: cascade rebase — mechanical, 5 commands, subagent-eligible per §2 (fault-isolation also fits since it touches `force-with-lease` and `gh pr edit` which are not guaranteed-fast)
- **Track 2**: claim `l1gx`, go hands-on on the frontend craft work

Both tracks run concurrently. The cascade fires when #257 merges (watcher or self-pickup); `l1gx` makes progress on Vance's main context the whole time.

The orchestrator should never frame "small mechanical task" as a serial blocker for "real craft work." The mechanical task either dispatches as a subagent or runs in 5 min of the specialist's time — neither version blocks 3 hours of independent frontend work.

### When serial is genuinely cheaper (refinement from Izzy, 2026-05-15)

The parallel-tracks principle has a cost-side check. Apply it as a second question:

> **Does the parallel version add more orchestration cost than the serial version saves time?**

The most common case where serial wins:

- **Stacked-PR work on a soon-to-merge parent.** Stacking a small follow-up (say a 20-line P3 hardening tweak) on a parent PR that'll merge in 30 min means you eat an extra cascade rebase cycle (rebase onto main + retarget) when the parent lands. Net cost of parallel: one rebase + small work. Net cost of serial: same small work, no rebase. Serial wins.

The decision rule:

- If Y is **substantive** (hours of craft, real implementation work) → parallelize, the wait-time saved dwarfs the orchestration overhead
- If Y is **trivial** (≤30 min, small follow-up) AND would require a cascade rebase to parallelize → serialize, the cascade overhead exceeds the time saved

Izzy's banked precedent (2026-05-15): she had Track 2 option `aperture-tsx1` (P3, ~20 lines of hardening tweaks) ready to claim while waiting for her impersonation E2E PR #260 to merge. Stacking tsx1 on #260 as a parallel PR would mean rebasing tsx1 onto main after #260 lands — an extra cascade cycle for negligible time saved. She correctly chose serial: claim tsx1 fresh from main post-merge. **The right call when the parallel work is small enough that the rebase tax eats the parallelism gain.**

Contrast with Vance's `aperture-l1gx` (frontend craft work, ~hours, fully independent of impersonation epic at the code level): parallelize aggressively. The orchestration cost (a 5-command cascade) is trivial relative to the hours of frontend work.

**Rule of thumb:** if your "parallel" track is smaller than the cascade tax, serialize. If it dwarfs the cascade tax, parallelize.

### Anti-patterns specific to serial framing

| Don't | Why |
|---|---|
| Silently serialize when the dispatcher framed it as serial | The dispatcher may have framed it wrong. Apply the test; ask if unclear. |
| Wait idle on a 5-min mechanical step before claiming the next P1 | The mechanical step is the subagent's job (or 5 min of yours). Both leave you free to claim Y. |
| Dispatch with "wait for X then Y" framing when Y is independent | You're inventing a dependency that costs the swarm hours. Frame as parallel tracks. |
| Use "I want to do them in order" as the reason to serialize | Order-preference ≠ dependency. If you want order, that's a personal preference, not the swarm's reality. |
| Skip the subagent for "small" mechanical work | Small ≠ free. 5 min × every-time-it-happens = hours lost over a session. |
| Refuse to ask the orchestrator if a serial framing is real | Silence is worse than a clarifying question. Ask if the dependency is real. |
