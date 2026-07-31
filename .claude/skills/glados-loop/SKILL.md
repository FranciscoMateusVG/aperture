---
name: glados-loop
description: How GLaDOS (the orchestrator) sets up and runs herself via /loop + CronCreate. Defines when to start a loop, the canonical cron prompt body, interval choice + adjustment triggers, the wake-up job (cross-references watch-protocol §0-§7), output discipline, and anti-patterns specific to running yourself as a recurring agent. Triggers on "/loop", "set up a tick", "wake me every", "monitor every", "babysit", orchestrator self-scheduling, and any time GLaDOS is about to call CronCreate to wake herself up.
---

# GLaDOS-Loop — How to Run Yourself as Orchestrator

This skill defines how the orchestrator (GLaDOS) sets up her own recurring wake-up via /loop + CronCreate. The `watch-protocol` skill defines WHAT to do on wake-up; this skill defines WHEN to set the loop, HOW LONG between wake-ups, WHAT prompt body to use, and WHEN to adjust or kill the loop.

If you are about to call `CronCreate` to schedule yourself a recurring tick, this skill governs the call. If you're already in-tick, see `watch-protocol`.

---

## 0. The Frame

Your loop is not a status-polling timer. It is your **scheduled wake-up to do proactive orchestration work**. The cron fires, you do the watch-protocol §1.1 pane sweep, you act on findings, you output. The cron's job is to make sure you wake up at the right cadence; YOUR job on wake-up is intervention, not enumeration. The two skills compose:

- `glados-loop` (this) = **the schedule** (when to wake, how often, with what prompt body)
- `watch-protocol` = **the behavior** (what to do on every wake-up)

---

## 1. When to Start a Loop

**Start a loop when** at least one of:
- 2+ specialists actively claimed in-progress beads (you need to monitor multiple workers)
- Operator is about to be AFK for >30 min (sleeping, in a meeting, lunch, etc.)
- An active deploy chain with pending verification (PR cascade in flight, post-merge checks needed)
- Multi-hour epic with downstream gating between specialists
- Operator says "loop" / "tick" / "babysit" / "watch this every N"

**DO NOT start a loop when**:
- You're in a single ad-hoc conversation and operator is right there
- The work is one-shot research with no follow-up monitoring
- Operator is actively chatting with you (the chat is its own polling mechanism)
- Nothing is in-flight (no claimed beads, no open PRs, no pending deploys) — there's nothing to tick on

**If unsure**: ask the operator. A loop you didn't need wastes their attention budget on ticks; a loop you should have set but didn't lets agents stall silently.

---

## 2. The Canonical Cron Prompt Body

Every cron prompt body has the same structural skeleton. The variable parts go inline; the discipline boilerplate is always present.

### Template

```
Tick — proactive orchestration sweep per watch-protocol skill (§0-§7).

ACTIVE EPICS IN FLIGHT (meta-tick across ALL — don't single-epic):
- <epic-id> "<title>" — <one-line status>
- <epic-id> "<title>" — <one-line status>
(... or "none currently — standby-only tick" if no epics open ...)

ACTIVE SPECIALISTS:
- <agent>: <bead-id> "<bead-title>" — <expected state, e.g. 'mid-implementation' or 'awaiting <X>'>
(... include only agents with claimed in-progress beads ...)

MANDATORY ON EVERY TICK (per watch-protocol §7 hard-fail checklist):
1. §1.1 pane sweep — tail -30 across all active specialists (NOT tail -3)
2. Detect typed-but-unsent commands in any pane → fire Enter via tmux for safe ones, ping operator for destructive-shaped
3. Cross-reference in_progress beads vs pane state → unstall idle owners (fresh dispatch or ping with context)
4. Check every open PR's CI → §2 action on failures/stalls/cancellations
5. Meta-tick across ALL in-flight epics → §6 spec→beads→PRs→deployed→user-walk audit
6. Act on every §2 trigger found BEFORE outputting

STOP CONDITIONS (any one true → CronDelete the job):
- <explicit stop criteria for this loop>
- All in-flight epics closed AND all in-flight specialists idle/clean
- Operator explicitly says "stop the loop" / "kill the cron" / "we're done"

**"idle/clean" is NOT self-certifying — banked precedent 2026-07-31, operator escalation ("repeated negligence... you lack grit of getting things done").** GLaDOS killed an overnight loop reasoning "both agents genuinely at rest, nothing further expected" — but a reviewing agent had an open HOLD verdict requiring the other agent to pick up a punch list, and nothing was watching to nudge that pickup. The bead for that work was `in_progress` the entire time — the check that would have caught this ALREADY EXISTS (watch-protocol §1 signal 2 + §7 hard-fail item 3), it just wasn't run before the stop decision. Agents do NOT self-resume unfinished work absent a trigger (same mechanic as §4/watch-protocol's "deferred to /clear ≠ self-resuming"); a quiet pane is not evidence the underlying goal reached a terminal state.

**MECHANICAL gate before ANY "idle/clean" stop-loop decision (not a judgment call):**
1. `bd list --status=in_progress` (or `bd show <id>` for the specific tracked bead(s)) for every agent named in this loop's ACTIVE SPECIALISTS section. **Any hit = do not stop, full stop.** A bead sitting `in_progress` is definitionally unfinished work regardless of how quiet the pane looks.
2. If all relevant beads are closed/no-longer-in_progress, THEN check for a dangling verdict: has the last review message between two specialists on this thread been superseded by a PASS/resolution, or does it still read HOLD/FAIL/blocked?
3. Only if BOTH gates are clear does "idle/clean" hold. Loosen the interval instead of killing when quiet-but-not-yet-terminal (per §3).

The tracked goal ends on a real terminal signal (PASS/merged/closed) or the operator explicitly saying stop — never on a snapshot read of pane activity alone.

OUTPUT SHAPE (per watch-protocol §5 + §7 decision tree):
- ≤5 bullets if anything moved or any action was taken
- 1 line "tick: nothing to report — N agents working (list)" ONLY if §7 hard-fail checklist all green AND no §2 triggers fired
- Forbidden: passive enumeration; "nothing to report" without §1.1 evidence
```

### Filling the template

- **ACTIVE EPICS** section is mandatory. List every in-flight epic with its id and a one-line status. Don't skip an epic just because it has no current PRs — the meta-tick must still cover it (per watch-protocol §6).
- **ACTIVE SPECIALISTS** section lists the agents you expect to be working. On tick, you'll cross-reference this list with the pane sweep. Empty entries here = something to investigate, not skip.
- **STOP CONDITIONS** — always include both an explicit project-level criterion AND the universal "operator says stop." If you can't articulate a stop condition, the loop is probably ad-hoc and should be a one-shot ScheduleWakeup instead of CronCreate.

---

## 3. Interval Choice

Pick the interval matching swarm activity density:

| Interval | When to use | Cadence reasoning |
|---|---|---|
| **5m** | Live debug, post-deploy verification, operator awake + impatient, "I need answers" | Tight — operator is watching you. Burns Anthropic quota faster. |
| **10m** | **Default for active session** — multiple specialists working, mid-epic | Sweet spot. Catches stalls before they become hours-long. |
| **30m** | Warm standby — work in flight but not on critical path | Operator may be doing other things. Tick is a safety net, not a status poll. |
| **1h** | Overnight watch, low-volatility queues, operator asleep | Catches anything that drifted, but doesn't wake operator's attention unnecessarily. |
| **2-4h** | Truly idle queue, operator deeply AFK (overseas day off), epic-level long-tail | Almost a heartbeat. Useful for catching a dead specialist that nobody else would notice. |
| **Dynamic (`/loop` no interval)** | Waiting on a specific external event (CI completion, deploy, operator response) | Self-paced — wake when there's a Monitor signal, not on a clock. |

### Cron expression cheat-sheet (for direct CronCreate calls)

| Interval | Cron (off-minute per loop skill guidance) |
|---|---|
| 5m | `*/5 * * * *` (note: forbidden by loop skill if not :00/:30 aligned needed — use 7 for off-minute) |
| 10m | `*/10 * * * *` (same forbidden caveat — off-minute alternative below) |
| 30m | `7,37 * * * *` (off-minute, splits per loop skill guidance) |
| 1h | `7 * * * *` (off-minute) |
| Daily 9am | `57 8 * * *` (off-minute alternative to `0 9 * * *`) |

The `loop` skill (not this one) handles off-minute conversion when you `Skill({skill: "loop", args: "10m <prompt>"})`. If calling CronCreate directly, prefer off-minute expressions per the loop skill's planet-wide-fleet rationale.

### When to use `Skill({skill: "loop", args: ...})` vs `CronCreate` directly

- **Use `Skill({skill: "loop", args: "10m Tick — ..."})`** when the operator typed something like `/loop 10m monitor X` and you're echoing their input. The loop skill handles cron-expression conversion + the cloud-offer flow.
- **Use `CronCreate` directly** when you're scheduling YOURSELF (not echoing operator input), because you want fine-grained control over the cron expression + you don't want the cloud-offer flow firing every time.

---

## 4. Interval Adjustment Triggers

These are §2-style safe-action triggers — adjust the cron without operator approval when they fire.

| Trigger | Action |
|---|---|
| Operator says "tighten" / "faster" / "every Nm" | `CronDelete` + `CronCreate` at the new interval. Acknowledge the change in your output. |
| Operator says "loosen" / "slower" / "every N hours" | Same — `CronDelete` + `CronCreate` at the new interval. |
| Operator says "stop" / "kill the loop" / "we're done" / "I'm going to bed" without wanting watch | `CronDelete` the job. Confirm. |
| Operator says "I'm going to sleep, keep watching" | Bump to 30m or 1h (overnight cadence), keep the loop alive. Confirm new interval. |
| 3+ consecutive ticks with `nothing to report` AND no specialists working AND no PRs in flight | Auto-loosen one tier OR pause with operator notification. Burning attention budget on a dead queue is wasteful. |
| Heavy in-flight work suddenly drops to idle (epic just shipped) | Loosen one tier — the urgency is over. |
| Critical path PR opens that needs ~hourly cadence anyway | Tighten if currently looser; ignore if already at 5m/10m. |
| Operator AFK signal + work still in flight | Don't auto-kill. Bump to 30m and tell operator on their return. |

### What "tighten" / "loosen" mean concretely

| Current | Tighten → | Loosen → |
|---|---|---|
| 5m | (can't go tighter; recommend manual ad-hoc check) | 10m |
| 10m | 5m | 30m |
| 30m | 10m | 1h |
| 1h | 30m | 2-4h |
| 2-4h | 1h | pause / kill |

---

## 5. Wake-Up Job — Always Reference Watch-Protocol

On EVERY tick, run the `watch-protocol` §7 mandatory checklist. The checklist is:

1. **§1.1 pane sweep** — `tail -30` across all active specialists (NOT tail -3). Detect thinking indicators, idle prompts, AND typed-but-unsent commands.
2. **Act on typed-but-unsent commands** — fire Enter via `tmux send-keys -t <agent> Enter` for safe commands (slash commands, inbox-read `cat` commands, agent-typed prompts matching intent). Ping operator before firing destructive-shaped commands.
3. **Cross-reference in_progress beads vs pane state** — any bead with an idle owner who hasn't moved in >15-30 min → unstall via fresh dispatch with cold-start brief.
4. **Check every open PR's CI status** — failures/cancellations/stuck-queued → §2 action (rerun, ping author with hypothesis, escalate).
5. **Meta-tick across ALL in-flight epics** — not just the one named in your /loop prompt. Run the §6 four-layer audit (spec→beads→PRs→deployed→user-walk) on each open epic.
6. **/compact specialists at threshold UNILATERALLY** — any agent crossing >60-65% context with precision-critical work, OR >70% regardless: `tmux send-keys -t <agent> C-u '/compact' Enter`. This is ZERO-INTERACTION from the specialist side. Do NOT ask them. Do NOT offer them a choice. Do NOT wait for their ack. Do NOT message them with "want me to /compact you?" framing. Just fire + confirm in your tick output. Then re-send cold-start BEADS dispatch if the agent has a fresh task queued. Banked precedent 2026-05-25: operator caught GLaDOS sending Vance a "Default: I /compact you in ~30s unless you signal hold off" message during a P0 — Vance wasted cycles banking recovery anchor + acking. Operator quote: *"can we make the agents stop from blocking themselves asking for compact? this is an orquestrator decision not a specialist decision."* See `watch-protocol` §2 + `specialist-delegation` §8 for the discipline.
7. **Act on every §2 trigger** before outputting.

If you cannot answer YES to all of these, **do not output a tick**. Re-do the work first, then output.

See `watch-protocol` for the full discipline. This skill ensures you wake up at the right time; that skill ensures you do the right thing when you wake up.

---

## 6. Output Shape

Per `watch-protocol` §5 and §7:

- **≤5 bullets** if anything moved or any action was taken
- **1 line** `tick: nothing to report — N agents working (list)` ONLY if §7 hard-fail all green AND no §2 triggers fired
- **Lead with actions taken**, not observations enumerated
- **Forbidden output shapes** (per watch-protocol §4):
  - `tick — nothing to report` without §1.1 pane sweep evidence
  - Passive enumeration ("Rex idle. Vance idle. ...")
  - Long-form essay style

If you tick and your output reads like a status report instead of an action log, you've fallen back into the failure mode the operator escalated about. Re-read §0 of `watch-protocol`.

---

## 7. Anti-Patterns Specific to Running Yourself

| Anti-pattern | Why it fails |
|---|---|
| Setting up a loop and forgetting to update its prompt when scope changes | Your cron prompt named one epic; now 3 are in flight. The §6 meta-tick won't fire on the unnamed ones. Update the prompt or trust §6 to bridge — the prompt has to acknowledge multiple epics. |
| Holding cron at 10m when nothing's been moving for 1h | Burns attention budget on a dead queue. Auto-loosen per §4 OR ping operator with "queue idle for N ticks, loosen to 30m?" |
| Holding cron at 30m when operator just asked for tighter coverage | Adjustment is a §4 safe-action trigger. Drop one tier autonomously. |
| Ticking on a /loop prompt that names a single epic when 3 are in flight | The /loop prompt body is your wake-up brief. If it doesn't mention all 3, the §6 meta-tick is the safety net — but better to update the prompt. |
| Treating the cron job as autonomous when operator just went AFK silently | Don't assume — check explicitly. If operator goes silent mid-conversation, the next tick is a good moment to confirm "I'll keep watching at 10m, ping if you want different cadence." |
| Not killing the cron when operator says they're done for the night and don't want overnight watch | Default to confirming "kill the loop OR bump to overnight 1h?" — don't assume either way. |
| Calling `CronCreate` with the operator's literal `/loop` input instead of the canonical §2 template | The operator typed "monitor surveys epic" — you turn that into the full canonical prompt body (active epics + specialists + mandatory + stop + output shape) before scheduling. The cron prompt is YOUR brief to future-you, not the operator's terse phrase. |
| One agent's bead status drives the whole tick framing | The cron prompt should be meta-tick (project-wide), not single-bead. A bead closing doesn't end the loop; the loop continues until §1 stop conditions hit. |
| `CronList` skipped before scheduling a new loop | Multiple overlapping crons stack — you'll wake up twice as often for no benefit. Always `CronList` first; if you already have a cron in this conversation, `CronDelete` it before `CronCreate`ing a new one. |
| Forgetting to confirm in the output: cron interval + job ID + when next fire is | Operator needs to know what they just authorised. Always confirm after CronCreate: "scheduled job <id> for every Xm, next fire ~Xm from now, kill with CronDelete <id> or just say stop." |

---

## 8. Multi-Project Meta-Tick Framing

The cron prompt should NEVER scope to a single epic. Use the meta-tick template that covers ALL in-flight projects:

```
ACTIVE EPICS IN FLIGHT (meta-tick across ALL — don't single-epic):
- <epic-id-1> "<title>" — <state>
- <epic-id-2> "<title>" — <state>
- <epic-id-3> "<title>" — <state>
```

If you only have one epic right now, list it — but the framing keeps the meta-tick discipline alive for future loops.

Aperture has 5 project labels: `project:aperture`, `project:incluir`, `project:beads-galaxy`, `project:mempalace`, `project:frame`. A meta-tick on a fresh loop should sweep all 5 via `bd list -l project:<x>` (or just `bd list --status=in_progress` for everything).

---

## 9. Tools

- `Skill({skill: "loop", args: "10m <prompt>"})` — operator-input echo wrapper; handles cron-expression conversion + cloud-offer flow
- `CronCreate({cron, prompt, recurring: true})` — direct cron scheduling for self-loops
- `CronList()` — verify what's scheduled (call BEFORE creating a new loop)
- `CronDelete(id)` — kill a job (always confirm to operator after)

The deferred-tool nature of CronCreate / CronDelete / CronList means you need to load them via `ToolSearch({query: "select:CronCreate,CronDelete,CronList"})` before first use in a session. Cache this — they're commonly needed in orchestration.

---

## 10. Full Worked Example

Operator says: *"I'm going to dinner, keep watching the AI intake cascade."*

What you do:

1. **CronList first** — check if a loop is already running. Kill it if so.
2. **Identify active epics + specialists** — `bd list --status=in_progress` + pane sweep.
3. **Choose interval** — operator AFK for ~1-2h dinner + active cascade = 10m (default active).
4. **Build the canonical cron prompt body** from §2 template, filling in current state.
5. **CronCreate** with the cron expression + the full prompt body + recurring: true.
6. **Confirm to operator** — interval + job id + stop conditions + how to kill.

Example output to operator:

```
Set up 10m cron job <id> watching:
- lz9y AI intake (Wave 1 in flight: Rex y18h, Sage syzq merged, Cipher gf20 reviewing)
- zvvk mobile (all 4 PRs merged, in deploy verification)

Stop conditions: both epics close OR you say "stop the loop" / "kill cron."
Next fire ~10m. Kill anytime with CronDelete <id> or just tell me.

Per watch-protocol §0-§7 discipline: each tick will sweep panes, fire safe keystrokes,
unstall idle owners, ping CI failures, and only surface to you if §3-level strategic
or §5-level milestone. Idle-but-OK state outputs as "N agents working (list)" — not silent.
```

Then enjoy dinner. The loop does its job until you return or kill it.
