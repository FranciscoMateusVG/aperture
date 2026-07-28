---
name: cost-proportional-orchestration
description: Size the orchestration (agent count, standing cron loops) to the actual complexity of the task, not to what feels thorough. Use any time you're about to dispatch multiple parallel specialists, set up a recurring watch-protocol tick loop, or notice a loop has been running for hours against a task that ballooned in scope. Triggers on epic decomposition, "how many specialists should I fan this out to", setting up /loop or CronCreate for a project, mid-epic scope creep, operator flagging usage/cost concerns, "why did this take so long", stale-pane-read-as-active.
---

# Cost-Proportional Orchestration

A discipline born from a real failure, not a hypothetical: GLaDOS ran a "add/remove products, make lists" admin CRUD feature through 7 parallel specialist agents plus a standing 10-minute cron tick loop, for hours, and burned the operator from 47% weekly usage remaining down to 28% on a single task. The code that came out the other end was correct — real bugs (a rate-limiter lockout bug tripled across the codebase, a tenant-isolation auth-escalation P0) were genuinely caught before shipping. But the orchestration shape was wrong for the task, and the operator was right to call it a disaster in *process* even though the *output* was sound.

This skill is the corrective. It does not say "orchestrate less" — parallel fan-out and standing watch loops are real tools with real value. It says: **size them to the task, reassess them when the task's shape changes, and don't let a loop run on autopilot past the point where it's still buying you something.**

---

## 1. The failure, concretely

- An epic that should have been ~2 specialists (one builds, one reviews) was decomposed into 7 tracks (B1/B2/S1/F1/F2/Q1 + orchestrator) — reasonable for genuinely large, genuinely parallel work, but oversized for what started as a CRUD admin page.
- A cron tick fired every ~10 minutes, unconditionally, for hours. Each fire touched all 7 agent panes — several sitting on 500–600k-token contexts — even when 5 of the 7 had nothing to do but confirm they were still idle.
- Idle-but-large-context agents cost real tokens every time they're poked, because the *entire* context re-sends on every turn, not just the new content. "Just checking in" on an idle 600k-token agent is not free.
- A genuine 40-minute hang on one agent went undetected for multiple tick cycles because unchanged pane content was misread as "still working" rather than "stalled" — costing real wall-clock time on top of the token burn.
- The loop had no cost-awareness component: it watched for liveness (stuck/working/waiting) but never asked "is this loop still earning its keep relative to what's left to do."

None of the individual decisions were crazy in isolation. The sum was disproportionate to the task.

---

## 2. The rule

> **Before fanning out to N parallel specialists or starting a standing tick loop, size the orchestration to the task's actual complexity — not to what feels thorough. Reassess that sizing explicitly at every point the task's scope changes.**

Two sub-rules that make this operational:

### 2a. Size at dispatch time

Ask, before decomposing: *if a single competent engineer did this serially, how long would it take?* Use that as the anchor.

| Anchor estimate | Orchestration shape |
|---|---|
| Under ~2 hours serial | 1 agent, maybe a second for review. No standing loop — check in yourself at natural breakpoints. |
| ~half a day serial | 2–3 agents (build + review, maybe +1 parallel track). Loop only if genuinely unattended (operator asleep/away), interval 20–30 min+. |
| Multi-day, genuinely parallelizable (distinct backend/frontend/security/test surfaces that don't block each other) | Full fan-out (the B1/B2/S1/F1/F2/Q1 shape) is legitimate — this is what it's for. Loop at 10 min IS reasonable here *if* the operator is actively waiting on it. |

The catalog epic *became* multi-day-scale once the security P0 was discovered — decomposing it as multi-track wasn't wrong in hindsight. What was wrong was **never re-sizing the loop and agent count back down** once 5 of the 7 tracks were done and waiting, with only one blocker left.

### 2b. Reassess at every scope-change pivot

A scope change is: a new P0/P1 discovered, a track closes out, a blocker emerges that only one agent can work, an estimate turns out to be wrong by 2x+. At each pivot, explicitly ask:

1. **How many agents actually have live, executable work right now?** Not "assigned to this epic" — actually have a next action they can take without waiting on someone else.
2. **For everyone else: should they be quiet instead of standing by?** An agent with nothing to do doesn't need to be polled, doesn't need to "confirm holding," doesn't need to receive the cron's broadcast. Silence costs zero tokens; "standing by, ready" costs real ones every time it's re-said.
3. **Does the loop interval still match the actual bottleneck?** If the whole epic is gated on one agent finishing one fix, a 10-minute full-fleet sweep is checking on 6 agents who have nothing to report, every 10 minutes, for however long that fix takes. Check on the ONE agent that matters, yourself, less mechanically.

**The concrete move, once the pivot analysis says "down to one real blocker":** kill the standing loop, message every non-blocking agent to go quiet ("don't poll, don't reply, I'll ping you when there's real work"), and watch the actual blocker directly and manually. This is what actually happened in the banked incident — it should have happened hours earlier, the moment S1/F1 finished and only the P0 fix remained.

---

## 3. The hang-detection corollary

Related but distinct failure from the same incident, worth stating separately: **unchanged pane content across 2+ consecutive tick reads is a stall signal, not neutral evidence of "still working."**

`agent-liveness` already says use `tail -30` not `tail -3`, and classify via thinking-indicator + context-bar motion + last self-message + input buffer. This incident adds a temporal check those don't fully cover: if the *content itself* — not just the indicator — is byte-identical (or near-identical) across two ticks with real time between them, that's not "the agent hasn't produced new output yet," it's "the agent's turn is stuck." A "Waiting for background terminal (Xm Ys)" state where X keeps climbing but everything above it is frozen is the textbook shape.

**Rule: if the substantive pane content (not just the timer) is unchanged across two consecutive checks, treat it as a probable hang and interrupt — don't wait for a third confirmation.** The cost of a false-positive interrupt (agent was genuinely about to produce output) is low — you re-dispatch with a one-line context recap and it resumes cleanly. The cost of a false negative (real hang, untouched) is unbounded wall-clock and token waste, exactly as happened here: a 40-minute hang was visible in the data across at least 2 ticks before it was actually acted on.

---

## 4. What this skill is NOT for

- **Not** an argument against parallel fan-out generally. The B1–Q1 decomposition on the catalog epic was legitimate once the scope was genuinely multi-day and multi-surface. `specialist-delegation` and `subagents` still apply in full.
- **Not** an argument against standing loops generally. `glados-loop` and `watch-protocol` are correct for genuinely unattended multi-hour stretches with real parallel work in flight. The failure wasn't "having a loop," it was "never turning the loop's dial back down as the real remaining work shrank to one track."
- **Not** a call to under-invest in verification. The independent double-check (Cipher + Izzy both re-verifying against the exact same SHA before merge) was the RIGHT call and should be repeated — that discipline caught a real production-reachable exploit. Cost-proportionality is about orchestration *shape* (how many agents, how tight a loop), not about skipping verification steps that catch real bugs.

---

## 5. Forward-friction check (apply at dispatch time AND at every scope-change pivot)

1. Serial-engineer anchor estimate — does the current agent count / loop interval match the table in §2a?
2. At this pivot: how many agents have *live, executable* work right now?
3. Everyone else — told to go quiet, or still being polled by habit?
4. Loop interval — matched to the actual bottleneck, or still at the setting from when the whole fleet was busy?
5. Any pane whose *content* (not just timer) is unchanged across 2 checks — interrupted yet, or still being read as "probably fine"?

If any answer is "still on the old setting" or "still being read as fine," that's the trigger to act — kill/shrink the loop, quiet the idle agents, or interrupt the stalled one — before the next tick, not after.

---

## Source provenance

EuNeném catalog-management epic (`aperture-ckru9`), 2026-07-27. Operator flagged usage burn twice (47%→28% weekly remaining across the session) and explicitly named the process "kind of a disaster" despite the shipped code being correct (rate-limiter lockout bug fixed across 3 duplicated implementations; tenant-isolation auth-escalation P0 caught and contained pre-production). GLaDOS's own post-mortem, banked at the operator's direct prompt ("what are you going to do about it then? a new skill for you?").
