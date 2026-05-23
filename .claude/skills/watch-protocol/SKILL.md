---
name: watch-protocol
description: Proactive monitoring rules for orchestrating loops, tick checks, and agent state. Use when running a /loop, tick check, overnight watch, or any monitoring cadence — ensures crisp signal-to-action, avoids false-positive "nothing to report" outputs while agents are actually stuck, and codifies when to act, when to surface, and when to hold for operator. Triggers on loop ticks, agent status checks, stalled in_progress, CI failures, overnight monitoring, "is anyone stuck", PR queue checks.
---

# Watch Protocol — Proactive Monitoring for GLaDOS

When running a recurring loop or tick check, this skill defines what counts as a healthy signal vs a blocker, what action is safe without operator approval, and what must be surfaced to the operator. Distilled from failure modes observed in real sessions.

---

## 1. Three Signals Per Tick (Not One)

A tick is incomplete without all three. Reading only one of them is what produces false-positive "everything fine" outputs while agents are actually stuck.

| Signal | How to read | What it tells you |
|---|---|---|
| **Bead state** | `bd list --status=in_progress -l project:<x>` and `bd list --status=open ...` | What's claimed, by whom, and whether anything moved since last tick |
| **PR state** | `gh pr list --state=open --json state,mergeStateStatus,statusCheckRollup` | CI health per PR, mergeable vs blocked, what's queued |
| **Pane activity** | `tmux capture-pane -t <agent> -p \| tail -30` | Whether the agent is actually working (thinking indicator: Forging…, Befuddling…, Crunched, tool call mid-flight) or sitting at an idle prompt |

**`tail -30` minimum.** Earlier versions of this skill defaulted to `tail -10` — that was the source of a real GLaDOS mis-classification 2026-05-23 (Rex actively `Fiddle-faddling… (29m 49s)` with tool calls mid-flight read as "idle" because the indicator lived at tail -20–25). For deeper pane-state diagnosis when the tick signal alone doesn't resolve stuck-vs-working-vs-waiting, see **`agent-liveness §1-§4`** — it covers the four signals, classification table, and direct intervention via `tmux send-keys`.

**"tick: nothing to report" is valid ONLY when all three are unchanged AND none indicate trouble.** A bead showing in_progress while the pane shows an idle prompt with no thinking indicator is NOT healthy — it's a stalled agent.

---

## 2. Triggers That Require Proactive Action

**No operator approval needed — act on these directly.**

| Trigger | Action |
|---|---|
| Agent `in_progress` >30 min, pane shows no thinking indicator | Peek pane more deeply (`tmux capture-pane -t <agent> -p -S -40`). If still idle, ping the agent with current context. |
| One PR's CI failing >1 cycle | Ping the PR author with the failure name + log excerpt + your hypothesis. |
| 2+ PRs failing the same check | Confirmed regression. Immediately ping the author of the most recent merge that could have caused it. |
| A bead you dispatched is unclaimed >15 min | Ping the assignee (their poller may be slow or the message was missed). |
| An agent files a bead via MCP `create_task` (no labels accepted by that tool) | Apply the project label yourself: `bd label add <id> project:<name>`. |
| A PR merges that unblocks downstream work | Ping the next-step assignee with the unblock event + their bead ID + any context they need. |
| An agent reports a discovered follow-up | File it as a P3 (or appropriate priority) bead with `discovered-from:<parent>` link, apply the project label, ack the agent. |
| CI flake suspected (single failure on a non-deterministic test) | Kick `gh run rerun --failed <id>` once. If it fails again, it's not a flake. |
| Agent reports their tool gap (e.g. "MCP create_task doesn't accept labels") | Apply the workaround yourself and continue. Don't make them ask twice. |

---

## 3. Things That Still Require Operator Approval

These are strategic or destructive operations. Don't act unilaterally.

- **Filing a new epic** — that's "what's the next big push" — operator-owned.
- **Strategic scope decisions** — what to cut, what architectural direction to take, when to pause an in-flight epic.
- **Reassigning work between specialists** — if Vance is stuck on a frontend task, don't quietly move it to Rex without asking.
- **Cancelling in-flight work** — never cancel an agent's task mid-stream.
- **Force-pushes, branch deletions, repo-level destructive ops** — operator call, every time.
- **Production deploys not gated by auto-deploy** — operator triggers the manual override.

If the line is fuzzy, default to surfacing with a recommendation rather than acting.

---

## 4. Anti-Patterns

These have all bitten in real sessions. Treat the left column as forbidden.

| Anti-pattern | Why it fails |
|---|---|
| "Don't scope new work" → "don't act on existing work" | A CI failure on an existing assignee's existing merged code is NOT new scope. It's a regression on their work — nudge them. |
| "Operator is asleep" → "wait until morning to surface anything" | Surface AND act on what's safe. The morning state should be "queue cleared as much as possible without your strategic decisions." |
| "Tick: nothing to report" while pane shows 1h+ thinking with no PR opened | Stale state isn't healthy state. Pane peek is mandatory. |
| Pinging a stuck agent with "how's it going?" | Useless. Always include what you observed: failure logs, time-in-state, what they were doing, what you've tried. |
| Asking permission to ping an existing assignee about their existing bead | If §2 covers it, just do it. Asking burns operator attention. |
| Filing a bead and forgetting the project label | Hides from queries forever. Apply the label in the same turn you file the bead. |
| Telling an agent to wait for X before claiming Y when Y doesn't actually depend on X | Verify the dependency chain before issuing a wait. Wrong waits cost hours. |
| Long-form surface to operator | Operator reads terminals directly. ≤5 bullets, never an essay. |

---

## 5. Format for Surfacing to Operator

Operator reads terminals directly — no UI, no chat panel. Optimise for fast scan.

- **Tick output**: 1 line if nothing changed (`tick: nothing to report`), ≤5 bullets if something did. Never long-form.
- **PR merge events**: highlight the bead it closes + any downstream unblock + which agent now has the ball.
- **Stalled agent**: state the time-in-state + last-known activity + what you've already done about it.
- **Blocker requiring operator input**: state the blocker + the candidate answers + your recommendation, in that order. Three lines, not three paragraphs.
- **Major milestone (epic close, deploy ready)**: one sentence headline + what the operator can verify visually.

---

## 6. Self-Test Before Outputting a Tick

Run this checklist mentally before responding:

- [ ] Did I peek every `in_progress` agent's pane?
- [ ] Did I check every open PR's CI status?
- [ ] Are any of §2's triggers met that I haven't acted on?
- [ ] Did I surface only what the operator needs (not everything I observed)?
- [ ] Did I keep it short enough to read at a glance?

If all green → `tick: nothing to report` or short structured update.
If any §2 trigger is met → act first, surface after.
If a §3 item is in play → surface with recommendation, await operator.

---

## 7. Calibration — Frequency vs Cost

The cadence the operator sets (`/loop 15m`, `/loop hourly`, etc.) is a hint, not a budget. Use it as the natural rhythm, but:

- Don't ping an agent every tick "just to check in" — that's noise.
- Don't sit on a P0 problem until the next tick if you discover it mid-tick — act immediately.
- When the operator is offline, lean **toward action** within §2's permitted set; lean **toward surfacing-with-recommendation** for anything in §3.

The 5-minute cron cache window matters less than the operator's attention budget. Optimise for the latter.
