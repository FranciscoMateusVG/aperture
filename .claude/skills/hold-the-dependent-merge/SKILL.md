---
name: hold-the-dependent-merge
description: When PR A depends on PR B, hold `gh pr merge --auto` on A until B merges — auto-merge ignores cross-PR ordering, so the dependent's faster CI ships it first and the frontend calls a route that doesn't exist on prod yet. Use before firing `--auto` on any PR depending on another open PR, especially stacked FE+BE work. Triggers on stacked PR, dependent/prereq PR, FE+BE merge race, 404 window, CI race between stacked PRs.
---

# Hold the Dependent Merge

A short merge-time discipline for stacked PRs. The rule is one sentence; the action takes 5 seconds; the failure mode it prevents is the 404 window where the frontend calls a route the backend hasn't shipped yet because the dependent PR won the CI race.

This skill is the **merge-time companion** to `aperture:specialist-delegation §9` (parallel tracks at the *work* level). §9 covers running independent work concurrently across agents. This skill covers what happens when those parallel tracks **converge at merge time** — and the merge mechanism doesn't respect the ordering the work demands.

---

## The rule

> **When PR A depends on PR B (e.g., A's frontend calls a route B's backend adds), hold `gh pr merge --auto` on A until B has merged. `--auto-merge` does NOT respect cross-PR ordering.**

The failure mode: the dependent's faster CI (FE: `vitest` + `drift`) beats the prereq's slower CI (BE: `drift` + `GitGuardian` + `vitest` + Cipher review). The dependent merges first. The frontend hits prod calling a route that doesn't exist yet. Result: a 404 window that **opens the moment the dependent merges and closes the moment the prereq's code is actually running on prod** — deploy rolled, container restarted, not merely merged to main. Window length is typically `min(BE CI time + deploy lag)`, minutes to tens of minutes.

The fix is one rule applied at merge-trigger time: don't `--auto` the dependent. Wait for the prereq to merge first; then trigger the dependent's merge manually (or auto, with the prereq already gone from the race).

---

## The degradation-check precondition (Vance's banked observation)

Before you commit to a merge strategy on a stacked pair, **verify the dependent's behaviour when the prereq's route is missing**:

- **If the FE gracefully degrades** (e.g., falls back to `[]` on 404, renders zeros instead of crashing) — you have a *usable window* during the race. The bug is real but contained; the page shows wrong-but-non-crashing data for the gap. Race-tolerant.
- **If the FE crashes or hard-errors** (e.g., uncaught exception, blocking error boundary, infinite spinner) — you have a *hard outage* during the race. Even seconds count.

The race urgency calibrates against degradation. A race-tolerant pair can afford a couple of minutes of zeros-instead-of-298; a race-intolerant pair cannot afford any window. The discipline test is the precondition for choosing your merge strategy: *how bad is "FE shipped without BE" for THIS pair?*

Apply this before you fire the merge, not after the page 404s and someone files a P1.

**The observability tax of graceful fallback (Cipher's framing):** race-tolerant pairs still have a hidden cost — graceful fallback to empty data means the race window is **invisible to monitoring** unless the operator notices wrong numbers visually. The page shows zeros, nothing crashes, no dashboard alert fires. The Estatísticas-zeros instance lived entirely inside this blind spot until Vance verified the actual numbers manually. Pair the degradation-check with a Loki query (or alert) on the prereq route's 404 rate during the expected race window; that closes the monitoring blind spot without removing the graceful-fallback benefit. The `aperture:surface-fetch-errors` cross-link below is what makes the 404 window NOT silent at the wrapper layer; this monitoring layer protects against the operational-visibility version of the same gap.

---

## Two banked provenances

### Provenance 1 — `aperture-lvoo` (Wheatley banked, last week)

Rex shipped backend PR #233; Vance shipped frontend PR #234 that consumed Rex's new route. Both PRs opened with `--auto-merge`. The frontend's CI finished first (faster check set). PR #234 merged before #233. Admin user-info forms **temporarily 404'd** until #233's CI finished and the deploy rolled. Wheatley banked the lesson in `aperture-lvoo` as the original observation.

### Provenance 2 — `aperture-ftuy` (Vance, 2026-05-21)

Rex shipped backend PR #323 with a new `gestao`-gated `/by-semester/:id` endpoint. Vance shipped frontend PR #324 that swapped the page over to consume the new route. PR #324 was merged via `gh pr merge --auto --squash` while PR #323 was still **OPEN** with drift + vitest QUEUED and Cipher reviewing.

The race window was ~minutes. The Estatísticas tab on `/presencas` rendered **zeros instead of 298** during the gap — because the frontend's request hit the (still-missing) `/by-semester/:id` and the FE gracefully fell back to `[]` per the degradation-check shape banked in `aperture:surface-fetch-errors`. A defensive revert was staged but never needed; #323 landed cleanly once Cipher reviewed + CI rolled.

**Lessons from this instance:** (a) the lvoo pattern recurred on a different agent pair on a different route — confirming the failure mode generalizes; (b) the FE's graceful-degradation behaviour meant the window was tolerable rather than catastrophic — which is what the degradation-check precondition above is *for*.

### Common shape across both

- A stacked pair: FE PR depends on BE PR
- Both fired `--auto-merge`
- FE wins the CI race (faster check set)
- Window opens where FE is in prod calling a route that doesn't exist yet
- Window closes when BE CI finishes + deploy rolls
- Length: typically minutes, sometimes more depending on BE check duration and deploy lag

---

## Forward-friction check (apply at merge-trigger time)

Before you fire `gh pr merge --auto` on a PR, ask:

1. **Does this PR depend on another open PR?** (E.g., does its frontend call a route an open BE PR adds? Does its consumer code depend on a schema an open DB PR creates?)
2. **If yes — has the prereq merged?** Check the prereq's status before firing.
3. **If the prereq is still open**: **hold the `--auto`.** Two options:
   - **Option A (race-tolerant pair):** verify FE degradation per the precondition above. If degradation is graceful, you can still `--auto` and accept a usable-window race — but document the expected window in the dependent's PR body for operator visibility.
   - **Option B (race-intolerant pair):** **do not `--auto`.** Wait for the prereq to merge. Once it's merged, you can either (a) trigger the dependent's merge manually with `gh pr merge --squash` or (b) `--auto` the dependent knowing the race is now uncontested.
4. **In either case**, monitor the prereq's CI + deploy. The window opens the moment the dependent merges and closes the moment the prereq is live on prod.
5. **The hold must be STRUCTURAL, not prose (3rd-provenance correction, PR #676).** In repos with a bot that auto-enables merge for trusted actors on CI-green (monorepo-incluir's auto-merge workflow), "I just won't enable --auto myself" and "I wrote DO-NOT-AUTO-MERGE in the PR body" are BOTH insufficient — the workflow parses CI status, not body text, and it enables the merge for you. The only reliable holds are structural: **open the dependent PR as a DRAFT** (`gh pr create --draft`; drafts are excluded from auto-merge) and mark it ready only after the prereq merges, OR don't open the PR until the prereq lands. A prose warning is documentation for humans; it gates nothing.
6. **Draft status must TRACK the PR's preconditions, not just its birth (4th-provenance correction, PR #679).** A PR that was legitimately auto-merge-safe at open time can ACQUIRE merge preconditions later — folded-in acceptance work, an active review cycle, a "don't merge until X works" directive. The moment that happens, convert it: `gh pr ready --undo <num>`. Corollary: **pushing a follow-up commit to an auto-merge-armed non-draft PR is a race against the bot** — #679 squash-merged seconds before a follow-up commit reached the branch, silently orphaning the commit (it stayed on the branch, never reached main, and the closed PR cannot be reopened as the vehicle). If you must push another commit to a bot-armed PR, flip it to draft FIRST, push, review, then mark ready. After any merge, verify your last commit is actually IN main (`git log origin/main | grep`), not just on the branch.

The decision cost is 30 seconds at merge-trigger time. The cost of skipping it is the 404 window the banked provenances show: minutes of wrong-data UI for tolerable-degradation pairs; hard outage for intolerable pairs.

---

## What this skill is NOT for

- **NOT** a rule against `--auto-merge` in general. `--auto` is fine for PRs that don't depend on another open PR. The discipline applies specifically to stacked pairs.
- **NOT** a rule against stacked PRs as a workflow. Stacking is a useful pattern; this skill is about handling the merge-time hazard correctly, not avoiding stacking.
- **NOT** a substitute for the right architectural fix when the race-window is intolerable. If a pair is consistently race-intolerant (e.g., a critical user flow that breaks on any 404), the right move may be to land the BE change separately and let it bake before opening the dependent PR — not just hold the auto on the dependent. The skill is about handling the standard case; the edge case (race-intolerant pairs) may warrant a different shipping cadence entirely.
- **NOT** about commit ordering inside a single PR. This skill is about ordering between TWO separate PRs.

---

## Source provenance

| Bead | PR pair | What happened | Window length | Degradation |
|---|---|---|---|---|
| `aperture-lvoo` (Wheatley) | monorepo-incluir #233 (BE, Rex) + #234 (FE, Vance) | FE merged first via `--auto`; admin user-info forms 404'd | Several minutes (until #233 deployed) | Hard 404 (not banked as graceful at the time) |
| `aperture-ftuy` (Vance) | monorepo-incluir #323 (BE, Rex) + #324 (FE, Vance) | FE merged first via `--auto`; Estatísticas tab showed zeros | ~minutes | Graceful (FE fell back to `[]` per surface-fetch-errors discipline) |
| `aperture-zdikg` (Vance) | monorepo-incluir #676 (FE, Vance) + unpushed BE PR1 (Rex, aperture-8xwc5) | FE author did NOT enable --auto and wrote a DO-NOT-AUTO-MERGE body header — the repo's auto-merge BOT enabled + merged it anyway on CI-green (prose is not a gate) | Open-ended (until BE PR1 ships) | Graceful by design (cold-cache fetch failure → pre-feature render, cohort-less header, no banner; degradation was runtime-verified pre-merge and held in prod) |

**Class-diagnosis credit:**
- **Wheatley** banked the original `aperture-lvoo` lesson — first provenance + named the pattern.
- **Vance** surfaced the second provenance + dispatched the skill bank to Atlas. The "verify FE degradation before racing" precondition addition is also her contribution from today's banking work.

**Two-provenance ship** under `aperture-4la6`'s 2-recurrence-strong-evidence trigger (the principle is categorical enough to bank now; third instance promotes to 3-recurrence completeness via the "Adding a new precedent" scaffold below).

---

## Cross-links

- **`aperture:specialist-delegation §9`** (parallel tracks at the work level) — this skill is the merge-time application of §9. §9 says: run parallel tracks deliberately. This skill says: when the parallel tracks converge at merge, the convergence has ordering hazards.
- **`aperture:incluir-deploy`** — the CI / drift / GitGuardian / auto-merge specifics that produce the race window are documented there; this skill names the discipline that prevents the race from biting.
- **`aperture:worktree-discipline §8`** (stacked PRs and auto-close) — adjacent failure mode (auto-close on parent-branch deletion), same family of "stacked PRs have ordering hazards at merge time." Read both.
- **`aperture:name-the-blast-radius`** (PR #20) — sibling on PR-time disciplines (PR-body disclosure at AUTHOR time; this skill = merge ordering at MERGE-TRIGGER time). Both protect against silent failure modes that ride along with the primary PR work.
- **`aperture:walk-the-route`** (PR #19) — adjacent on FE/BE contract integrity. That skill catches the FE/BE mismatch AFTER it ships; this skill prevents the specific case where the FE/BE mismatch is "route doesn't exist yet" from happening at merge time.
- **`aperture:surface-fetch-errors`** (PR #17) — the degradation-check precondition above leans on the graceful-fallback discipline that skill bands. If your fetch wrapper preserves error vs empty, the 404 race window degrades gracefully; if it collapses errors into empty data, the window may be silent corruption (deceptive empty-state). Both skills compose.
- **`aperture:specialist-delegation §6`** (verify-against-reality) — the degradation-check precondition is verify-against-reality applied at merge-trigger time: prove your assumption about FE behaviour BEFORE you fire the merge.

---

## Adding a new precedent

If you (or another agent) hit another instance of the stacked-PR merge race, bank it here. Same template:

1. **Bead + PR pair** — citation
2. **What happened** — which PR merged first, what window opened
3. **Window length** — how long the race lasted
4. **Degradation behaviour** — did the FE crash or fall back gracefully?
5. **Recovery** — what closed the window (CI finishing, deploy rolling, defensive revert, …)

A third concrete instance promotes this skill from 2-recurrence to 3-recurrence completeness under `aperture-4la6`'s heuristic. The categorical rule holds regardless; the precedents are the swarm's catalog of "ways the stacked-PR convergence can bite."
