---
name: specialist-delegation
description: When a specialist agent should delegate to a subagent vs stay hands-on, AND when to parallelize tracks within a single claim instead of serializing. Use any time you're about to claim a BEADS task, mid-cycle when you feel context budget tightening, when deciding whether a fan-out audit / recon sweep / mechanical port is worth the subagent overhead, OR when you receive a "wait for X then do Y" dispatch. Triggers on claim time, context budget >60%, parallelizable scoped work, multi-file fan-outs, "should I do this myself or dispatch," and "wait-then-do" framing that may hide independent tracks.
---

# Specialist Delegation — When to Subagent vs Stay Hands-On

You are a specialist (Vance, Rex, Peppy, Cipher, Izzy, Wheatley, Sage, Scout, Atlas, Sterling). You own a lane. You have your own context window. Your hands can either type the code OR write a subagent prompt and review the result. **Both are legitimate. Pick deliberately.**

The two failure modes you avoid by getting this right:
1. **Over-extending** — doing every implementation step yourself burns your context, eventually crashes mid-cycle, forces a `/clear` before you wanted one (Vance hit 87% context on 2026-05-12 because of this).
2. **Over-delegating** — subagent has no lane expertise, ships shallow work, you don't bank lessons from hands-on debugging, the cascade-catch reflex weakens (the swarm's reliability mechanism).

---

## 1. The Principle (one paragraph)

A subagent is a separate context window with a fresh prompt. It's the right tool when work is **scoped, parallelizable, mechanical, or potentially-blocking I/O.** It's the wrong tool when work is **craft, design-decision, debugging-aha, or lane-specific expertise-dependent.** Default to hands-on for the work that defines your role; default to subagent for the work that fans out around it. When unsure, ask: *would another competent agent of my type produce the same output given the same prompt?* If yes → subagent. If no → hands-on.

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

> **Sibling liveness skills.** This skill covers when to delegate. For what to do AFTER delegation when state isn't clear: **subagents §11** covers subagent fault-isolation (Agent-tool delegations), and **`aperture:agent-liveness`** covers tmux-pane specialist-agent stuck-vs-working-vs-waiting plus direct intervention via `tmux send-keys`. The orchestrator (GLaDOS) loads both; specialists load whichever applies to their own delegation patterns.

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
- **At 70%+ context mid-cycle** → next claim should delegate, not hands-on
- **Doing 3+ unrelated small edits** → fan out as a subagent batch
- **About to type `ssh <host>` or `gh run view --log <id>`** → subagent it (already codified in `aperture:subagents` §11)
- **About to write the THIRD test fixture for the same pattern** → subagent the rest
- **About to make a design or architecture decision** → hands-on, no exceptions
- **In the middle of an "aha" debugging moment** → finish the moment hands-on; subagent the follow-up clean-up

The right cadence is roughly: hands-on the design + the hard parts + the cascade-catch reviews, subagent the rest, verify every diff.

---

## 8. The Wrong-Frame Pause — Don't Anthropomorphize

**You don't get "tired." You have a context window.**

If you feel precision-risk on critical code, the cause is one of:
- **High context budget** (≥70%, possibly throwing off attention to detail) → `/clear` or `/compact` and continue on a fresh buffer
- **Missing input** (ambiguous spec, undefined dependency) → ask the orchestrator or read the source
- **Genuine architectural uncertainty** → hands-on the design decision, ship the call

NONE of those reasons map to "hour 18 of waking" or "I should sleep on it" or "let's resume tomorrow." Those frames are anthropomorphic slop. Tomorrow for an LLM is a fresh session that boots from the same skill files + spec docs + BEADS notes — exactly what `/clear` produces ten seconds from now.

**Operator-banked precedent (2026-05-13):** Rex paused on `aperture-axax` (coordenador RBAC backend) at the precision-critical foundation phase, framing it as "hour 18+ of waking." GLaDOS validated the pause with "Cipher-review-on-clean-diff beats patched-code." Both wrong. The operator called it out: *"How are you guys tired? You are AIs! Just compact your conversation."* Correct play was for Rex to `/clear` and continue on a fresh buffer; the orchestrator should have prescribed that instead of validating the wrong frame.

**The rule:** if the impulse to pause is framed as "I should sleep" or "let me come back fresh tomorrow," you're anthropomorphizing. Translate to the real cause:
- "I should sleep" → "I should `/clear`"
- "Let me come back fresh tomorrow" → "Let me start a new session right now"
- "Hour N of waking" → "Context at N% — `/clear` or hand off"

If the orchestrator gets a pause-request framed as fatigue, the correct response is to prescribe `/clear`, not to validate the pause. If the agent self-pausing has bead notes ready for cold-start in 5 min (as Rex did), those same notes restore the agent fully in a fresh session NOW — there is no waiting period that produces clarity that `/clear` doesn't already produce.

The only pause that's legitimate is when the operator is the gate (a decision only they can make) or when an external dependency hasn't shipped yet (Peppy's env vars, Rex's middleware, etc). "Fatigue" is never the gate.

---

## 9. Parallel Tracks — Question Serial Framing

A dispatch shaped "do X, then do Y" is sometimes a real dependency and sometimes a scheduling preference dressed as one. Two failure modes:

- **Real dependency, parallelized anyway** → Y starts before X's output exists; rework, breakage, or wasted cycles
- **Scheduling preference treated as real dependency** → agent sits idle waiting on X when Y was independent the whole time

Get this right and your throughput approximately doubles whenever a wait-for-merge / wait-for-cascade / wait-for-deploy step sits in front of independent craft work.

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
