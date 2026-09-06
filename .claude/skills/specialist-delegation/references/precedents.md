# specialist-delegation — Precedents

Worked examples and banked precedents moved out of `SKILL.md` (which is force-injected into every agent's system prompt on boot). Each block below is verbatim from the original file, under a heading naming the section it came from. The rules these stories established remain in `SKILL.md`; this file is the evidence.

---

## Intro — Operator directive (2026-05-29)

> **Operator directive (2026-05-29):** *"I see the specialists more as tech leads than hands-on coders. I want them to delegate to subagents (or teams) and review the work — not grab a task and do it one at a time."* This skill encodes that. Delegate-first is now the default; hands-on is the exception you justify.

---

## Intro — The two failure modes (dated instances)

From the "Under-delegating" failure-mode bullet:

> (Vance hit 87% context on 2026-05-12 doing this; the EuNenem Wave A fan-out on 2026-05-28 was the corrected pattern — 5 pages built in parallel while she reviewed.)

---

## §3b — Subagent fan-out canonical win

From the "Subagent fan-out (the Agent tool) — YOUR DEFAULT" paragraph:

> EuNenem Wave A (5 page ports against a shared design spec, 2026-05-28) is the canonical win: the pages didn't need to talk; they needed to conform to a contract the lead set up front.

---

## §4 Three Worked Examples (2026-05-12 session)

**Example A — Subagent WIN (Peppy on `aperture-z5ow`)**

The work: investigate a suspected GHA concurrency anomaly across PRs #198/#199/#200, evaluate 4 hypotheses, write Gotcha #8 into the `aperture:incluir-deploy` skill. Peppy dispatched a general-purpose subagent with a tight brief (4 hypotheses, output format, write-the-skill constraint, no-live-repro guard, no-upstream-file guard). Subagent returned a clean forensic report + skill edit. **Peppy's context untouched.** Clean shape: scoped + bounded + outputs concrete artifacts.

**Example B — Subagent FAIL-then-takeover (Peppy on `aperture-h8mm`)**

The work: add a name-filter sweep pass to `apps/frontend/e2e/global-setup.ts`. Peppy dispatched a subagent. Subagent stalled at the 600s watchdog and never created its worktree. **Fault-isolation worked exactly as designed** — Peppy never lost context to the hang. He took over hands-on and shipped PR #212 in 12 min. Lesson: **subagent-can-fail is the reason fault-isolation matters.** Don't optimise so hard for delegation that you can't fall back to hands-on when the subagent stalls.

**Example C — Hands-on WIN (Vance on `aperture-ics4` / eunenem-v2)**

The work: build the entire EuNeném frontend per the Visual Identity Prompt — 5 sections, Tweaks panel, scrapbook tape SVG, polaroid frames, Patrick Hand + Caveat font tuning. Vance went hands-on for 75 min, shipped PR #1 with 5919 lines of production-quality code. **A subagent would not have produced this output** — the design fidelity required lane-specific muscle memory (which Tailwind utility class for marca-texto gradient? which animation easing for the float? what's the right rotation for a polaroid?). The work IS the craft. Hands-on was the right call even though it ate Vance's context budget hard.

---

## §6 Three failure-mode catches on 2026-05-12

Three failure-mode catches on 2026-05-12 that ALL came from hands-on diff/code reads:
- Vance caught Rex's wrong forum-bug triage by reading the trace + the actual route code + the prod Postgres row
- Cipher caught Peppy's "no crash = migration applied" inference by walking the actual route + Loki
- Vance caught his own ghost-migration assumption in the explainer cascade by re-grepping the diff

---

## §8 The Wrong-Frame Pause — banked precedents

**Banked precedent (2026-05-25 morning):** GLaDOS sent Vance a *"Default: I /compact you in ~30s unless you signal hold off"* framing during a P0 prod-broken fix. Vance correctly applied the /compact-is-orchestrator-decision logic but spent context cycles banking a 9-step recovery anchor + writing an explicit "GLaDOS green-lit to /compact" reply rather than just executing the P0. Operator caught it: *"can we make the agents stop from blocking themselves asking for compact? this is an orquestrator decision not a specialist decision."* The /compact mechanic must be ZERO-INTERACTION from the specialist side. Orchestrator decides + fires. Period.

**Operator-banked precedent (2026-05-25):** GLaDOS pinged Rex with "you have the autonomy, self-/clear." Rex correctly pointed out that /clear is not agent-invocable, then asked operator to /clear him. Operator caught the failure mode: *"Stop asking for /clear just compact is enough and you can do it yourself for the agents. adjust the skills for that or else will wait like a dumb dumb with this kind of stupid questions."* The fix: orchestrator types /compact directly via tmux send-keys; skills updated to reflect this is the canonical play.

**Earlier banked precedent (2026-05-13):** Rex paused on `aperture-axax` framing it as "hour 18+ of waking." GLaDOS validated the pause. Operator called it out: *"How are you guys tired? You are AIs! Just compact your conversation."* Same anti-fatigue clause, different mechanic — back then both wrong-frame and wrong-mechanic; now codified end-to-end.

---

## §9 Parallel Tracks — Worked example (2026-05-15, banked precedent)

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

---

## §9 When serial is genuinely cheaper — Izzy's refinement (2026-05-15)

Izzy's banked precedent (2026-05-15): she had Track 2 option `aperture-tsx1` (P3, ~20 lines of hardening tweaks) ready to claim while waiting for her impersonation E2E PR #260 to merge. Stacking tsx1 on #260 as a parallel PR would mean rebasing tsx1 onto main after #260 lands — an extra cascade cycle for negligible time saved. She correctly chose serial: claim tsx1 fresh from main post-merge. **The right call when the parallel work is small enough that the rebase tax eats the parallelism gain.**

Contrast with Vance's `aperture-l1gx` (frontend craft work, ~hours, fully independent of impersonation epic at the code level): parallelize aggressively. The orchestration cost (a 5-command cascade) is trivial relative to the hours of frontend work.
