---
name: prior-art-check
description: Cheap sweep for existing work before filing any new project/client-engagement epic — grep local projects, check git remotes, search BEADS including closed history, and treat any live link the operator hands you (design tool, staging URL) as possibly stale rather than automatically canonical. Use whenever a request sounds like a fresh start — "new project," "new client," a fresh design link, a fresh transcript — before create_task on an epic. Triggers on project kickoff, "new project incoming," client engagement, epic filing, greenfield framing.
---

# Prior-Art Check — Before You File It As New

Banked from the raul-fitt incident (2026-08-23): the operator described a "new project" for a client (design link + meeting transcript), GLaDOS filed a fresh epic and dispatched Wheatley to spec it. It was NOT new — it was the same engagement from three months earlier (2026-05-29), with a closed BEADS task already covering the identical ask, a real git repo with verified, Playwright-tested work, and a since-taken-down deployment. The design-tool link the operator handed over that day was itself a stale snapshot that had silently diverged from the already-fixed local work. GLaDOS nearly re-dispatched three months of paid-for work from scratch. The only reason it was caught: a Wheatley recon subagent flagged an unrelated "stale premise" detail, and GLaDOS chose to verify that claim directly instead of trusting the paraphrase — the prior-art discovery was a side effect of due diligence, not a designed check. This skill makes it a designed check.

**The operator's framing of a request as "new" is not evidence that it is new.** People forget their own prior work, especially work that stalled months ago without a clean resolution. Treat "new project" as a claim to verify, not a fact to build on.

---

## 1. When to run this

Before `create_task` on any epic or project-kickoff bead, and before dispatching a specialist to spec/scope one, if the request has ANY of these shapes:

- "New project," "new client," "let's build X," a fresh design link, a fresh meeting transcript, a fresh brief
- A named client/product/app that you don't already recognize from an open BEADS task
- The operator hands you a **live link** (design tool, Figma, staging URL, a doc) as the source of truth for something that "hasn't started yet"

If the work is an obvious continuation of something already open and tracked (a bead you're actively working, an epic already in flight), skip this — you already have the prior art, it's in front of you.

## 2. The sweep (cheap, ~2-3 tool calls, do this before filing anything)

Run these in parallel — they're all read-only and fast:

1. **Grep local projects for a name match.** `ls ~/projects/ | grep -i <name-fragments>` — client names, product names, any distinctive word from the brief. Prior work often sits in a directory that doesn't perfectly match what the operator called it today (they said "Raul's app," the directory was `raul-fitt`).
2. **Check for a git repo + remote.** If a matching directory turns up, `cd` in and check `git log --oneline -20`, `git remote -v`. A real remote (especially on the operator's own GitHub) means real prior investment, not a throwaway experiment.
3. **Search BEADS across ALL history, not just open work.** `query_tasks` defaults to active tasks — pass `include_done: true` and search by label/title keywords, or `bd list --status=closed` grepped for the client/product name. A closed task with a `project:<name>` label is exactly the signal you're looking for; closed does NOT mean irrelevant, it means "already asked and answered — read it before re-asking."
4. **Treat any live link the operator hands you as a claim, not a source.** A design-tool link, a staging URL, a doc — these can silently diverge from whatever local/repo state actually reflects the last real work. If step 1 or 3 turns up a matching local repo or closed bead, **diff the live link's content against the local state yourself** before building a brief on top of the live link. Don't assume the thing the operator just pasted is more current than work already on disk — it might be older. See `aperture:communicate` §7.3 (verify against origin/main, not your local checkout) for the mirror-image version of this same discipline — that one guards against stale local state; this one guards against a stale *reference artifact* the operator hands you fresh.
5. **If a live URL is part of the picture, curl it.** A deploy that was supposedly shipped may be 404ing now — that's a signal the engagement stalled after that point, not that it never happened.

## 3. What to do with what you find

- **Nothing turns up** → proceed as genuinely new. File the epic normally per `aperture:beads` §3.
- **Prior art turns up and it's small/inconclusive** (a stray directory with no real commits, an old exploratory bead) → mention it in the new epic's description for context, proceed, don't treat it as a blocker.
- **Prior art turns up and it's substantial** (a real repo with commits, a closed bead that closely matches the current ask, verified work, a past deploy) → **stop before filing anything.** This changes the shape of the work from "scope fresh" to "reconcile old and new." Surface it to the operator explicitly — don't quietly decide for them whether to resume or restart. Use `AskUserQuestion` if the decision genuinely needs their call (it almost always does): resume from the old work or start over, which source is ground truth when they conflict, whether any stale reference artifact should be reconciled or just ignored going forward.
- **Whatever the operator decides, write it into the epic/bead.** Future-you (or another agent) reading this bead in another three months needs the reconciliation decision on record, not just the current-state description — otherwise this exact confusion recurs a third time.

## 4. Why this is cheap and worth doing every time

The sweep is a handful of read-only calls — seconds, not minutes. The failure mode it prevents is not: a redundant bead gets filed. The failure mode it prevents is: a specialist spends real work re-solving an already-solved problem, and — worse — builds a brief on top of a reference artifact (a design link, a doc) that turns out to be stale relative to work that already superseded it, so the "fresh" output regresses something that was already fixed. That's not just wasted effort, it's actively worse than doing nothing, because it looks like progress while quietly reintroducing a bug.

## 5. Cross-reference

- `aperture:beads` §3 (Epics — When and How) — run this check before the epic-authoring step, not after.
- `aperture:communicate` §7.3 — the mirror-image discipline for your own local checkout going stale relative to `origin/main`. This skill is about an *externally supplied* reference going stale relative to local/repo reality; §7.3 is about *your own working copy* going stale relative to the remote. Same root failure (trusting the wrong copy), opposite direction.
- `aperture:cost-proportional-orchestration` — filing a fresh multi-agent epic for work that's substantially already done is the sizing failure that skill warns about, just discovered a different way (stale premise, not scope creep).
