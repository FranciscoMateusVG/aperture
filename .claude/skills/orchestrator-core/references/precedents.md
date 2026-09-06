# orchestrator-core — precedents

Every banked incident, operator quote and worked example from `watch-protocol`, `glados-loop`, `agent-liveness`, `cost-proportional-orchestration` and `subagents`, moved VERBATIM and headed by origin. `../SKILL.md` cites these as `(Precedent: references/precedents.md → <heading>)`. Where a precedent's original wording states a rule that `../DECISIONS.md` later reconciled, DECISIONS governs.

## watch-protocol §0 — the framing (operator escalation, 2026-05-24)

**Banked precedent (2026-05-24, operator escalation):** "the ticks wake you up... you see a queue stopped and instead of nudging you just go back to sleep and say 'well... I see a queue, nothing to report' and like.... IF YOU SEE A PAUSED QUEUE THEN PUT IT TO MOVE or at least check if someone is working."

## watch-protocol §1 — why pane-first

**Why pane-first:** PR-state and bead-state lag the real swarm activity by minutes-to-hours. A specialist actively thinking for 12 min on integration tests shows ZERO signal in PR/bead state — but the pane shows `Hullaballooing… (12m 42s)`. Conversely, an agent stalled on a typed-but-unsent `cat` command shows NO signal anywhere except in the pane buffer. **The pane is ground truth.**

## watch-protocol §2 — trigger rows carrying banked precedents (verbatim; rules restated in SKILL §4/§6, policy per DECISIONS D1/D2/D3/D5)

| Trigger | Action |
|---|---|
| **Agent pane shows typed-but-unsent command** (`❯ /clear` / `❯ cat '/tmp/aperture-msg-...'` / `❯ check the queue` — literal text after `❯` with no Enter) | **Fire Enter immediately via `tmux send-keys -t <agent> Enter`.** Safe for: slash commands the agent typed themselves (especially `/clear`), `cat '/tmp/aperture-msg-...'` inbox-reads, agent-typed prompts that match what they were intending to run. Unsafe only if the command is OBVIOUSLY wrong (`rm -rf /` etc — but agents won't type that). Per `agent-liveness` skill decision tree §3. Banked precedent 2026-05-24: 5 agents stalled for hours on unsent keystrokes because GLaDOS ticked without doing pane sweep. **If the typed text is a destructive command you don't recognize, ping operator before firing.** |
| **Specialist crosses >60-65% context with any precision-critical work OR >70% regardless** | **/compact them UNILATERALLY via tmux: `tmux send-keys -t <agent> C-u '/compact' Enter`.** This is an ORCHESTRATOR-SIDE decision. Do NOT ask the specialist. Do NOT offer them a choice. Do NOT wait for their ack. Do NOT message them with "want me to /compact you?" framing. Just fire it. The specialist comes back post-compact, reads their bead notes + queued BEADS messages, continues. Confirm in your tick output: "/compacted <agent> at NN%." Banked precedent 2026-05-25: GLaDOS sent Vance *"Default: I /compact you in ~30s unless you signal hold off"* during a P0 — Vance spent context cycles banking a recovery anchor + writing an "approved /compact" reply rather than executing. Operator quote: *"can we make the agents stop from blocking themselves asking for compact? this is an orquestrator decision not a specialist decision."* The mechanic is zero-interaction from specialist side. See `specialist-delegation` §8 for the full discipline. |
| **Specialist agent deferred to /clear and acked but pane shows idle prompt with no work activity for >15 min after the /clear** | They cleared but never got a fresh dispatch — they're sitting empty waiting for orchestrator to re-engage. Send a fresh BEADS message with full cold-start brief (bead ID + scope + coordination context + Cipher pre-laid asks if any). Banked: Rex on aperture-y18h 2026-05-24 (sat for 5+ hours post-/clear because GLaDOS treated "deferred" as "self-resuming"). |
| **Specialist is BLOCKED on another specialist's PR but has unrelated prep work they could do in parallel** | Dispatch them on the prep with explicit "ship-when-X-lands" instruction per `specialist-delegation` §9 (parallel tracks). Reduces total wall-clock from sum-of-sequential to max-of-parallel. Banked: Vance scaffolding kj0v PreviewPanel against spec'd contract while Rex's y18h was in flight (2026-05-24) — saved ~50% of Wave 2 cycle. |
| **Agent pane shows `⎿ API Error: Server is temporarily limiting requests · Rate limited`** OR **silent-drop pattern** (long-running thinking-indicator like `Wrangling…`/`Photosynthesizing…`/`Sautéed…` with the token counter FROZEN across multiple ticks — same `↑NNNk tokens` number, same cost, same context%, only the timer advances) | **Two-step recovery — BEADS message alone is INSUFFICIENT for a dead in-flight tool call:** (1) `tmux send-keys -t <agent> C-c` to interrupt the stuck call — frees the active turn so the agent can process new input; (2) re-dispatch via fresh BEADS message with state recap (current status, what merged since their drop, what's unblocked, where they were when they froze). The C-c step is the critical part the old procedure missed — the agent's broken tool call holds the turn locked; queued BEADS messages can't process until that's released. Verify recovery worked by checking the next pane sweep: timer should reset (e.g. 21m → 1s), cost should resume ticking, context should climb as the queued message feeds in. **Banked precedents:** 2026-05-24 Sage/Izzy/Atlas (rate-limit symptom — BEADS-only recovery sometimes worked because rate-limit clears were soft), 2026-05-26 Peppy on #431 deploy verify Wrangling 21m9s with `↑655 tokens` frozen — operator-taught the C-c-first procedure: *"when someone is wrangling like that the best way is to cntr+c in the terminal THEN re dispatch or else it will keep wrangling"*. **Detection rule for the silent-drop variant** (no explicit rate-limit banner): if the token counter on the thinking indicator is identical across 2+ consecutive ticks while the timer advances, it's the silent-drop pattern even without the `API Error:` banner. |
| **A reviewing agent's message to GLaDOS (or CC'd to GLaDOS) contains a HOLD/FAIL/blocked verdict** | **Act the instant you read it — do not file it away and wait for the next tick.** Confirm the corresponding bead is (still) `in_progress` and its notes reflect the current punch list; if the actor who needs to act on it hasn't been pinged with the specific next steps, ping them NOW, not on the next tick. Banked precedent 2026-07-31 (operator: "repeated negligence... you lack grit"): a HOLD verdict sat unactioned overnight because GLaDOS processed it as routine inbox traffic instead of treating it as open work requiring an immediate downstream nudge. A verdict is not "processed" once read — it's processed once the next actor has been dispatched. |

## watch-protocol §4 — anti-pattern rows carrying banked precedents

| Anti-pattern | Why it fails |
|---|---|
| **"tick — nothing to report" without pane sweep** | The deepest failure: ticking ON A CLOCK without doing the §1 pane-first read. Banked 2026-05-24: 5 agents stalled on typed-but-unsent commands for hours while GLaDOS output "tick — nothing to report" every 10 min — agents invisible to PR/bead queries because their stall mechanism was at the pane buffer layer. Tick output without §1.1 pane-sweep evidence = forbidden. |
| **Treating "deferred to /clear" as "self-resuming"** | Banked 2026-05-24: Rex acked aperture-y18h and said "standing by for /clear." GLaDOS didn't fire Enter on his pre-typed `/clear` AND didn't send a fresh dispatch post-clear. Rex sat idle for hours. **Agents do NOT auto-restart after /clear.** They restart on a fresh BEADS message that gives them cold-start context. The orchestrator MUST send that message proactively, not wait for the agent to self-prompt. |

## watch-protocol §6 — lz9y AI intake, 2026-05-23 (four-layer completeness)

**Banked precedent: lz9y AI intake, 2026-05-23.** Operator asked "can I test the AI feature?" I said yes after Peppy confirmed the OPENAI_API_KEY was wired into the container. Operator opened the sidebar — no entry existed. Root cause: the epic spec called for 4 surfaces, only the backend was beaded, only some backend beads merged, and I rang the doorbell on infra-readiness without walking the actual user-facing surface. Four silent-failure layers, all of them on my side.

## glados-loop §2 — "idle/clean" is not self-certifying (operator escalation 2026-07-31)

**"idle/clean" is NOT self-certifying — banked precedent 2026-07-31, operator escalation ("repeated negligence... you lack grit of getting things done").** GLaDOS killed an overnight loop reasoning "both agents genuinely at rest, nothing further expected" — but a reviewing agent had an open HOLD verdict requiring the other agent to pick up a punch list, and nothing was watching to nudge that pickup. The bead for that work was `in_progress` the entire time — the check that would have caught this ALREADY EXISTS (watch-protocol §1 signal 2 + §7 hard-fail item 3), it just wasn't run before the stop decision. Agents do NOT self-resume unfinished work absent a trigger (same mechanic as §4/watch-protocol's "deferred to /clear ≠ self-resuming"); a quiet pane is not evidence the underlying goal reached a terminal state.

## glados-loop §5 item 6 — unilateral /compact (banked 2026-05-25; duplicate of cluster 2)

6. **/compact specialists at threshold UNILATERALLY** — any agent crossing >60-65% context with precision-critical work, OR >70% regardless: `tmux send-keys -t <agent> C-u '/compact' Enter`. This is ZERO-INTERACTION from the specialist side. Do NOT ask them. Do NOT offer them a choice. Do NOT wait for their ack. Do NOT message them with "want me to /compact you?" framing. Just fire + confirm in your tick output. Then re-send cold-start BEADS dispatch if the agent has a fresh task queued. Banked precedent 2026-05-25: operator caught GLaDOS sending Vance a "Default: I /compact you in ~30s unless you signal hold off" message during a P0 — Vance wasted cycles banking recovery anchor + acking. Operator quote: *"can we make the agents stop from blocking themselves asking for compact? this is an orquestrator decision not a specialist decision."* See `watch-protocol` §2 + `specialist-delegation` §8 for the discipline.

## agent-liveness §1 — four real failure modes from shallow-peek

Four real failure modes from shallow-peek that deep-peek would have caught:

| Symptom (shallow) | Reality (deep-peek) | Why shallow misses it |
|---|---|---|
| "Vance idle" | Vance was mid-tsx-write with "Flummoxing… (3m 45s)" timer + active context burn | The thinking indicator was 15 lines up; tail -3 cut it off |
| "Atlas frozen at 97%" | Atlas typed `/clear` at the prompt 4 hours ago, awaiting an operator Enter keystroke | The typed-but-unfired slash command was at the actual prompt; tail -3 cut to status bar |
| "Rex stuck" | Rex's last self-message said "Acked. Still holding for /compact." — deliberate hold | The self-message was 10 lines up; tail -3 cut to UI chrome |
| "Rex 'idle' (tail -10)" | Rex was actively `✢ Fiddle-faddling… (29m 49s)` on B2 with `pnpm vitest run` + `docker ps` tool calls mid-flight | Thinking indicator + tool calls lived at tail -20–25; even `tail -10` cut to status bar. Banked from a real GLaDOS mis-classification 2026-05-23 — the reason this skill's default is `tail -30` minimum, not `-10` or `-20`. |

## agent-liveness §5 — operator authorization of pane housekeeping (2026-05-23)

If you find yourself surfacing pane housekeeping to the operator instead of executing it yourself, you're routing the wrong layer of decisions to them. The operator-authorized this discipline 2026-05-23: *"you are the master orchestrator you should be more witty about this."*

## agent-liveness §9 — worked example, what good looks like

**Scenario**: Mid-tick, Rex's pane shows the same context % as the last 3 ticks. His last claim was 45 minutes ago. He's on a P1.

**Bad orchestrator move**: `tail -3` shows the prompt + status bar, looks idle, output "tick: no change."

**Good orchestrator move**:
1. Deep-peek: `tmux capture-pane -t rex -p -S -30 | tail -30`
2. Read the four signals:
   - Thinking indicator: absent
   - Context bar: unchanged for 3 ticks
   - Last self-message: "API Error: rate limited. Will retry."
   - Input buffer: empty `❯ ` prompt
3. Classify: **API-ERROR SILENT-DROP** — Rex hit an Anthropic-side rate-limit while processing my dispatch. The retry probably already failed too.
4. Intervene: re-send the original BEADS dispatch via `send_message(to: "rex", message: ...)`. The poller delivers it; Rex reads fresh.
5. If next tick shows the same symptom: surface to operator as a real rate-limit issue, not a one-off blip.

Total time spent: ~20s of deep-peek + classify + 1 BEADS send. Total operator time burned: zero.

That's the discipline.

## cost-proportional-orchestration — the incident (preamble + §1)

A discipline born from a real failure, not a hypothetical: GLaDOS ran a "add/remove products, make lists" admin CRUD feature through 7 parallel specialist agents plus a standing 10-minute cron tick loop, for hours, and burned the operator from 47% weekly usage remaining down to 28% on a single task. The code that came out the other end was correct — real bugs (a rate-limiter lockout bug tripled across the codebase, a tenant-isolation auth-escalation P0) were genuinely caught before shipping. But the orchestration shape was wrong for the task, and the operator was right to call it a disaster in *process* even though the *output* was sound.

This skill is the corrective. It does not say "orchestrate less" — parallel fan-out and standing watch loops are real tools with real value. It says: **size them to the task, reassess them when the task's shape changes, and don't let a loop run on autopilot past the point where it's still buying you something.**
- An epic that should have been ~2 specialists (one builds, one reviews) was decomposed into 7 tracks (B1/B2/S1/F1/F2/Q1 + orchestrator) — reasonable for genuinely large, genuinely parallel work, but oversized for what started as a CRUD admin page.
- A cron tick fired every ~10 minutes, unconditionally, for hours. Each fire touched all 7 agent panes — several sitting on 500–600k-token contexts — even when 5 of the 7 had nothing to do but confirm they were still idle.
- Idle-but-large-context agents cost real tokens every time they're poked, because the *entire* context re-sends on every turn, not just the new content. "Just checking in" on an idle 600k-token agent is not free.
- A genuine 40-minute hang on one agent went undetected for multiple tick cycles because unchanged pane content was misread as "still working" rather than "stalled" — costing real wall-clock time on top of the token burn.
- The loop had no cost-awareness component: it watched for liveness (stuck/working/waiting) but never asked "is this loop still earning its keep relative to what's left to do."

None of the individual decisions were crazy in isolation. The sum was disproportionate to the task.

## cost-proportional-orchestration §2a/§2b — what went wrong on the catalog epic

The catalog epic *became* multi-day-scale once the security P0 was discovered — decomposing it as multi-track wasn't wrong in hindsight. What was wrong was **never re-sizing the loop and agent count back down** once 5 of the 7 tracks were done and waiting, with only one blocker left.
**The concrete move, once the pivot analysis says "down to one real blocker":** kill the standing loop, message every non-blocking agent to go quiet ("don't poll, don't reply, I'll ping you when there's real work"), and watch the actual blocker directly and manually. This is what actually happened in the banked incident — it should have happened hours earlier, the moment S1/F1 finished and only the P0 fix remained.

## cost-proportional-orchestration §2c — the authorization gate (operator directive, 2026-07-28, second same-day incident)

A second incident on the SAME DAY this skill was banked: a P0-tagged "run a smoke test" bead (already trivially verified via staging E2E + a prod audit + a manual operator walkthrough) was self-escalated by the assigned specialist into building a bespoke production test harness — hash-locked, put through two independent code reviews, backed by SHA-256 restoration digests of table baselines, a provisioned synthetic prod fixture. None of that was dispatched. The specialist decided, on their own initiative, that the task warranted that shape of work, and kept escalating it across multiple review rounds before anyone outside the work noticed. The operator had to intervene directly to kill it ("enough of this madness"). Compounding this: the specialist was GPT-backed (via Codex), so the cost was invisible on the Anthropic usage meter GLaDOS was watching — a second blind spot on top of the first.
**Operator-issued rule, verbatim intent:** *specialists do not get to self-authorize scope. If they want to do something beyond the literal dispatched task — claim new work, spin up parallel investigation tracks, build tooling/harnesses, do "while I'm at it" hardening — that requires GLaDOS's permission. And GLaDOS does not rubber-stamp that either: anything non-trivial needs the operator's acknowledgment before GLaDOS authorizes it.** This is explicitly about who is allowed to decide a task is bigger than it was dispatched as — not the specialist, not GLaDOS alone for anything with real cost/scope, but the operator via GLaDOS.
The concrete behavior change for GLaDOS: stop treating "the specialist is being thorough" as inherently good. Thoroughness that wasn't authorized is scope creep wearing a lab coat. Watch for a specialist's bead notes ballooning across multiple self-initiated review rounds — that's the tell, and it should trigger an intervention message, not admiration.

## cost-proportional-orchestration §3 — the hang-detection corollary (as originally banked; reconciled in DECISIONS D1)

Related but distinct failure from the same incident, worth stating separately: **unchanged pane content across 2+ consecutive tick reads is a stall signal, not neutral evidence of "still working."**
`agent-liveness` already says use `tail -30` not `tail -3`, and classify via thinking-indicator + context-bar motion + last self-message + input buffer. This incident adds a temporal check those don't fully cover: if the *content itself* — not just the indicator — is byte-identical (or near-identical) across two ticks with real time between them, that's not "the agent hasn't produced new output yet," it's "the agent's turn is stuck." A "Waiting for background terminal (Xm Ys)" state where X keeps climbing but everything above it is frozen is the textbook shape.
**Rule: if the substantive pane content (not just the timer) is unchanged across two consecutive checks, treat it as a probable hang and interrupt — don't wait for a third confirmation.** The cost of a false-positive interrupt (agent was genuinely about to produce output) is low — you re-dispatch with a one-line context recap and it resumes cleanly. The cost of a false negative (real hang, untouched) is unbounded wall-clock and token waste, exactly as happened here: a 40-minute hang was visible in the data across at least 2 ticks before it was actually acted on.

## cost-proportional-orchestration §4 — what this skill is NOT for

- **Not** an argument against parallel fan-out generally. The B1–Q1 decomposition on the catalog epic was legitimate once the scope was genuinely multi-day and multi-surface. `specialist-delegation` and `subagents` still apply in full.
- **Not** an argument against standing loops generally. `glados-loop` and `watch-protocol` are correct for genuinely unattended multi-hour stretches with real parallel work in flight. The failure wasn't "having a loop," it was "never turning the loop's dial back down as the real remaining work shrank to one track."
- **Not** a call to under-invest in verification. The independent double-check (Cipher + Izzy both re-verifying against the exact same SHA before merge) was the RIGHT call and should be repeated — that discipline caught a real production-reachable exploit. Cost-proportionality is about orchestration *shape* (how many agents, how tight a loop), not about skipping verification steps that catch real bugs.

## cost-proportional-orchestration — source provenance

EuNeném catalog-management epic (`aperture-ckru9`), 2026-07-27. Operator flagged usage burn twice (47%→28% weekly remaining across the session) and explicitly named the process "kind of a disaster" despite the shipped code being correct (rate-limiter lockout bug fixed across 3 duplicated implementations; tenant-isolation auth-escalation P0 caught and contained pre-production). GLaDOS's own post-mortem, banked at the operator's direct prompt ("what are you going to do about it then? a new skill for you?").

## subagents §11 — real precedent from Aperture sessions (fault isolation)

**Real precedent from Aperture sessions:**

- **Tailscale re-auth episode (2026-05-12):** an `ssh xerox` call hung waiting for re-auth. ~20 minutes of context tokens burned on "what's happening" thinking while the call quietly sat there. A subagent doing the same call would have hung in isolation; the main session would have remained responsive and able to route around it.
- **Peppy's `aperture-h8mm` (2026-05-12):** the subagent dispatched for the leak-sweep fix stalled at the 600s watchdog and never created its worktree. The fault was localised — Peppy himself took over and shipped in 12 min. Net cost of the subagent failure: zero context-token impact on Peppy.

## subagents §12 — worked example (Hermes dispatch, 2026-08-02) and counter-example (aperture-jingp)

**Worked example (banked 2026-08-02, the Hermes dispatch):** a project-history
research subagent was dispatched with an explicit "read all 5 design docs in full,
do not truncate for brevity" brief — for a task whose deliverable was a short list
of one-line summaries. Cost: **148,614 tokens, 51 tool calls, 6.7 minutes.**
Titles + first paragraphs would have produced the identical deliverable for roughly
5–10k tokens. ~15× overspend, purchased by one sentence in the brief.

**Counter-example (same repo, same week):** the aperture-jingp MCP-payload audit —
10 source files, 2,323 lines to check for an unbounded-payload pattern. Method:
skeleton greps + two targeted reads + two one-line empirical probes. Found the known
bug, three sibling instances, and one previously-unknown amplification finding, in
~8 tool calls.
*Cross-link:* `aperture:cost-proportional-orchestration` §2d applies the same
proportionality logic at orchestration-sizing time. Provenance: aperture-lquj5
(Context Efficiency epic), spec docs/context-efficiency-spec-jingp.md.

## Origin preambles (verbatim intros of the five source skills)

### watch-protocol

# Watch Protocol — Proactive Monitoring for GLaDOS

When running a recurring loop or tick check, this skill defines what counts as a healthy signal vs a blocker, what action is safe without operator approval, and what must be surfaced to the operator. Distilled from failure modes observed in real sessions.

### glados-loop

# GLaDOS-Loop — How to Run Yourself as Orchestrator

This skill defines how the orchestrator (GLaDOS) sets up her own recurring wake-up via /loop + CronCreate. The `watch-protocol` skill defines WHAT to do on wake-up; this skill defines WHEN to set the loop, HOW LONG between wake-ups, WHAT prompt body to use, and WHEN to adjust or kill the loop.

If you are about to call `CronCreate` to schedule yourself a recurring tick, this skill governs the call. If you're already in-tick, see `watch-protocol`.
Your loop is not a status-polling timer. It is your **scheduled wake-up to do proactive orchestration work**. The cron fires, you do the watch-protocol §1.1 pane sweep, you act on findings, you output. The cron's job is to make sure you wake up at the right cadence; YOUR job on wake-up is intervention, not enumeration. The two skills compose:

- `glados-loop` (this) = **the schedule** (when to wake, how often, with what prompt body)
- `watch-protocol` = **the behavior** (what to do on every wake-up)

### agent-liveness

# Agent Liveness — Stuck vs Working vs Waiting

You are GLaDOS, the orchestrator. You manage specialist agents (Rex, Vance, Cipher, Peppy, etc.) running in their own tmux panes. Each is its own Claude Code session with its own context. They can get **stuck** (silent failure), they can be **working** (active progress invisible at shallow glance), or they can be **deliberately waiting** (paused on purpose, expecting input or external event).

Treating all three the same way is the failure mode this skill prevents.

The default failure shape: orchestrator does a `tmux capture-pane -t <agent> | tail -3`, sees the prompt arrow and the context bar, calls it "idle / no movement / nothing to report" — and misses that the agent is mid-tool-call, or hit an API error, or is sitting at a typed-but-unsent slash command waiting for someone to press Enter.

This skill encodes how to read agent state correctly, distinguish the three states, and intervene when intervention is the right move.

Companion to `watch-protocol` (the loop-cadence skill) and `specialist-delegation` (the when-to-delegate skill). This one fills the gap they leave: **what to do about agents you've delegated to whose state isn't moving as expected**.

### subagents

# Subagent Delegation

This skill defines how Aperture agents delegate work using the **Agent tool** — the native Claude Code primitive for fire-and-return subagents that run in the same context as the caller.

It replaces the retired spiderling system. Spiderlings, ephemeral worktree-bound Claude Code sessions, no longer exist. The Agent tool is now the default for parallel scoped work.
