# orchestrator-core — procedures

Step lists, commands and templates moved VERBATIM from `watch-protocol`, `glados-loop`, `agent-liveness`, `cost-proportional-orchestration` and `subagents` (each heading names its origin). The rules that invoke these live in `../SKILL.md`; where an older procedure disagrees with `../DECISIONS.md`, DECISIONS governs (the re-kick table below is the reconciled form).

## Three signals per tick — read order and commands (watch-protocol §1)

| Order | Signal | How to read | What it tells you |
|---|---|---|---|
| **1 (FIRST)** | **Pane activity** | `tmux capture-pane -t <agent> -p \| tail -30` for every active agent (NOT tail -3 — thinking indicators scroll out of shallow peeks) | Whether each agent is actually working (thinking indicator: Forging…, Befuddling…, Crunched, Hullaballooing, Doodling, tool call mid-flight) OR idle at prompt OR stalled with a typed-but-unsent command in buffer (`❯ /clear` / `❯ cat '/tmp/...'` / etc — the literal text after `❯` shows what's been typed but Enter wasn't pressed) |
| **2** | **Bead state** | `bd list --status=in_progress -l project:<x>` and `bd list --status=open ...` | What's claimed, by whom, and whether anything moved since last tick |
| **3** | **PR state** | `gh pr list --state=open --json state,mergeStateStatus,statusCheckRollup` | CI health per PR, mergeable vs blocked, what's queued |

## Deep-peek a pane (agent-liveness §1)

`tmux capture-pane -t <agent> -p | tail -3` is too shallow. It shows you the prompt arrow + the context bar. That's the LEAST informative slice of the pane. The interesting state — what the agent was last doing, whether they typed something at the prompt, whether they hit an API error, what their last message said — lives further up.

**Default to `tail -30` minimum when reading any agent that "doesn't look right." Bump to `-40` or more when in doubt.**

The `-30` floor is calibrated against a real mis-classification — even `tail -10` cuts the thinking indicator on a busy agent (see the Rex row in the table below). `tail -30` reliably catches the indicator + last self-message + input buffer in one read.

Even better — when investigating a specific concern, use a scrollback offset: `tmux capture-pane -t <agent> -p -S -40 | tail -40` reads the last 40 lines of pane history including off-screen content.

## Reading the four state signals (agent-liveness §2)

Once you have a deep-peek, you have four signals that together resolve stuck/working/waiting:

### Signal A: Thinking indicator

Claude Code shows an animated thinking indicator when the agent is actively processing (✻, ✽, ⏺, "Flummoxing…", "Crunched for Xs", "Brewed for Xs"). Past-tense ("Crunched for 36s") is recent-but-finished; present-progressive ("Flummoxing… (3m 45s)") is currently working.

- **Indicator present, present-progressive** → WORKING. Do not intervene.
- **Indicator absent, last past-tense > ~5 min ago** → potentially stuck or deliberately waiting; check other signals.
- **Indicator absent + idle prompt > 30 min on a claimed in-progress task** → almost certainly stuck or waiting.

### Signal B: Context bar motion

The bar at the bottom shows `<pct>% <tokens>/1M`. Compare across ticks:

- **% climbing** → context is being consumed, work is happening.
- **% unchanged across multiple ticks** + claimed task → no work happening this cycle.
- **% suddenly dropped** → just compacted or cleared.
- **% over 70** → see `watch-protocol §2` trigger; ping for /clear OR fire it yourself per §4 below.

### Signal C: Last agent self-message

What did the agent say in its last visible message in the pane scrollback? This is the strongest signal because it's the agent telling you what state it's in.

- "Standing by" / "Holding here" / "Awaiting operator" / "Acked, still holding for X" → DELIBERATE WAIT. Do not re-ping; that's noise.
- "Will ship when X lands" / "Pivoting to Y" / "Compact-ready, signal when X" → DELIBERATE TRANSITION. Honor the agent's stated intent.
- "API Error" / "rate limited" / "Server is temporarily limiting requests" → API-LAYER FAILURE. The agent's last work probably silently dropped; resend the BEADS message.
- No recent self-message + agent on a claimed task → ambiguous; check Signal D.

### Signal D: Input buffer state

What's at the agent's `❯ ` prompt right now?

- **Empty prompt** → idle, ready for input.
- **A typed-but-unsent slash command** (e.g. `❯ /compact` or `❯ /clear`) → waiting for an Enter keystroke. Common when the agent itself typed the command and expected the operator to press Enter. See §4 — you can press it for them.
- **Typed text mid-composition** → user (operator) was typing in this pane; do NOT send keys, you'll corrupt their input.

## Pane intervention via send-keys — mechanism, safety preconditions, decision tree, post-fire verification (agent-liveness §4)

**You can fire slash commands in any agent's tmux pane.** This was validated live on 2026-05-23 (Rex + Atlas both compacted autonomously via this mechanism).

The mechanism: `tmux send-keys -t <agent> "/compact" Enter` types `/compact` at the agent's prompt and presses Enter, exactly as if the operator had keyed it physically. Same with `/clear` and any other slash command.

### Safety preconditions (ALWAYS verify before firing)

1. **Deep-peek the pane** per §1 first.
2. **Classify state** per §3. Only fire if classification is `TYPED-BUT-UNSENT SLASH COMMAND` or you're firing a safe slash command into a confirmed-idle pane.
3. **Verify the input buffer**:
   - Empty prompt + agent idle → safe to send `"/compact" Enter`.
   - Already-typed slash command at the prompt that you want to honor → safe to send just `Enter`.
   - Already-typed slash command that's WRONG (e.g. `/clear` but you want `/compact`) → safe to send `C-u "/compact" Enter` (C-u clears the input line first).
   - Mid-composition text from operator → STOP. Don't send keys.
4. **Confirm no thinking indicator** and no in-flight tool call.

### Decision tree

```
deep-peek pane → classify state per §3
  ├── WORKING → do nothing
  ├── DELIBERATE WAIT (no operator-only-action pending) → do nothing
  ├── TYPED-BUT-UNSENT SLASH COMMAND
  │     ├── command is correct → tmux send-keys -t <agent> Enter
  │     └── command is wrong → tmux send-keys -t <agent> C-u "/compact" Enter
  ├── EMPTY PROMPT + safe action wanted → tmux send-keys -t <agent> "/compact" Enter
  ├── API-ERROR SILENT-DROP → re-send BEADS dispatch via send_message
  └── REAL HANG → surface to operator; don't fire keys blind
```

### Post-fire verification

After sending the keystrokes, **always re-peek the pane within ~10s** to confirm execution:

- For `/compact` → look for `✻ Compacting conversation… (Xs)` or `✳ Compacting conversation…`
- For `/clear` → look for cleared screen + fresh `❯ ` prompt + dropped context bar
- For other slash commands → look for the command's expected initial output

If the expected state isn't visible within the verification window, something went wrong:
- The keystrokes may not have landed (rare — usually a stale tmux session)
- The agent may have intercepted them differently than expected
- The agent may have been in a state you misread

In that case: stop the autonomous intervention, surface to operator, re-investigate.

## Re-sending after API-error silent-drop (agent-liveness §6)

The Anthropic API occasionally returns errors that look like silent successes from the client side. Symptom: you sent a BEADS message to an agent, the wire returned success, but the agent's pane shows "API Error" or "Server is temporarily limiting requests" and the agent never processed the message.

When you see this:

1. Don't assume the agent read your dispatch.
2. Re-send the same BEADS message (or a paraphrase). The poller delivers it again; the agent reads it fresh.
3. If it fails again on the same agent: that agent may be in a deeper rate-limit window. Surface to operator OR delegate the work to a different specialist who's not throttled.
4. Bank the instance — if multiple agents hit silent-drop in a short window, Anthropic is having a problem, not Aperture.

## Re-kick tiers (composed from watch-protocol §2 + agent-liveness §3-§6; policy per DECISIONS.md D1/D2/D5)

Escalate one tier at a time; re-peek within ~10 s after every keystroke (post-fire verification above).

| Tier | When | Command / action |
|---|---|---|
| 0 — leave alone | thinking indicator present-progressive; or self-message "standing by / holding for X" | nothing; re-check next tick (ping only when X has now happened) |
| 1 — fire Enter | `❯ <text>` typed-but-unsent, no indicator, command is what the agent meant (slash command, inbox-read, agent-typed prompt) | `tmux send-keys -t <agent> Enter` |
| 1b — replace and fire | typed command is wrong for what is needed (e.g. `/clear` but `/compact` wanted) | `tmux send-keys -t <agent> C-u '/compact' Enter` |
| 2 — /compact | context ≥ 60% (DECISIONS D2); NOT while a subagent of theirs is live | `tmux send-keys -t <agent> C-u '/compact' Enter`; confirm "/compacted <agent> at NN%" in tick output |
| 3 — BEADS resend | `API Error` / `rate limited` banner, idle prompt, dispatch never processed | `send_message(to: <agent>, …)` — resend the dispatch (or paraphrase) |
| 3b — cold-start re-dispatch | agent cleared/compacted and sits at an empty prompt with no fresh brief; or unclaimed dispatch >15 min | `send_message` with full cold-start brief: bead ID + scope + coordination context + pre-laid asks |
| 4 — C-c then re-dispatch (frozen-counter stall only) | thinking indicator whose `↑NNNk tokens` / cost / ctx% are IDENTICAL across 2+ consecutive ticks while only the timer advances, or the `API Error … Rate limited` banner sitting on a locked turn | (1) `tmux send-keys -t <agent> C-c` (2) `send_message` re-dispatch with state recap (status, what merged since, what's unblocked, where they froze) (3) verify next sweep: timer reset, cost ticking, ctx climbing |
| 5 — surface | REAL HANG (tool call silent >10 min, no indicator, no error, no frozen-counter proof), AMBIGUOUS, or tier 3/4 failed twice on the same agent | terminal + `send_message(to: "operator")` doorbell with time-in-state, last activity, what you tried, recommendation |


## Canonical cron prompt body — tick template (glados-loop §2)

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

## Cron expression cheat-sheet and loop-vs-CronCreate choice (glados-loop §3)

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

## What "tighten" / "loosen" mean concretely (glados-loop §4)

### What "tighten" / "loosen" mean concretely

| Current | Tighten → | Loosen → |
|---|---|---|
| 5m | (can't go tighter; recommend manual ad-hoc check) | 10m |
| 10m | 5m | 30m |
| 30m | 10m | 1h |
| 1h | 30m | 2-4h |
| 2-4h | 1h | pause / kill |

## Multi-project meta-tick framing (glados-loop §8)

The cron prompt should NEVER scope to a single epic. Use the meta-tick template that covers ALL in-flight projects:

```
ACTIVE EPICS IN FLIGHT (meta-tick across ALL — don't single-epic):
- <epic-id-1> "<title>" — <state>
- <epic-id-2> "<title>" — <state>
- <epic-id-3> "<title>" — <state>
```

If you only have one epic right now, list it — but the framing keeps the meta-tick discipline alive for future loops.

Aperture has 5 project labels: `project:aperture`, `project:incluir`, `project:beads-galaxy`, `project:mempalace`, `project:frame`. A meta-tick on a fresh loop should sweep all 5 via `bd list -l project:<x>` (or just `bd list --status=in_progress` for everything).

## Tools (glados-loop §9)

- `Skill({skill: "loop", args: "10m <prompt>"})` — operator-input echo wrapper; handles cron-expression conversion + cloud-offer flow
- `CronCreate({cron, prompt, recurring: true})` — direct cron scheduling for self-loops
- `CronList()` — verify what's scheduled (call BEFORE creating a new loop)
- `CronDelete(id)` — kill a job (always confirm to operator after)

The deferred-tool nature of CronCreate / CronDelete / CronList means you need to load them via `ToolSearch({query: "select:CronCreate,CronDelete,CronList"})` before first use in a session. Cache this — they're commonly needed in orchestration.

## Setting up a loop — full worked procedure (glados-loop §10)

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

## Format for surfacing to operator — output template (watch-protocol §5)

Operator reads terminals directly — no UI, no chat panel. Optimise for fast scan.

- **Tick output**: 1 line if nothing changed (`tick: nothing to report`), ≤5 bullets if something did. Never long-form.
- **PR merge events**: highlight the bead it closes + any downstream unblock + which agent now has the ball.
- **Stalled agent**: state the time-in-state + last-known activity + what you've already done about it.
- **Blocker requiring operator input**: state the blocker + the candidate answers + your recommendation, in that order. Three lines, not three paragraphs.
- **Major milestone (epic close, deploy ready)**: one sentence headline + what the operator can verify visually.
- **"Feature live" doorbell** (operator can test now): NEVER ring this without a user-surface probe (§6.B). If you only have an infra confirmation (env wired, container restarted, route 200s), ring "infra ready, verifying surface" instead — then ring "feature live" only after the surface probe passes.

## Epic-completeness audit (watch-protocol §6.A)

When Wheatley (or any agent) finishes scoping an epic, GLaDOS reads the epic's `description` field and enumerates every surface in the scope/acceptance list. For each surface, grep open+merged beads for a matching implementation bead. Missing surface → file the bead immediately or, if the surface is genuinely out of scope, update the epic spec to remove it.

This is NOT a one-shot. Every meta-tick on every in-flight epic re-runs this check, because:
- Specs evolve (operator adds requirements mid-epic)
- Beads get superseded (a bead got closed but its work didn't fully cover the surface)
- New surfaces emerge from delivery (one bead's work surfaces a need for an unfiled adjacent bead)

The cheap version: `bd show <epic-id>` + scan acceptance criteria + `bd list -l project:<x>` filtered to descendants + cross-check. Should take 30 seconds per epic.

## User-surface probe (watch-protocol §6.B)

Before ringing the operator's doorbell with "X is ready to test," walk the literal user surface:

1. **Open the URL the operator would open** (the actual prod URL, not localhost, not a staging slug).
2. **Authenticate as the role the operator would hold** (use the operator's session or, if unavailable, a known-good role-bearing test account).
3. **Look for the UI element they'd click** — the sidebar entry, the button, the form. Confirm it renders.
4. **Click through one happy-path step** — not the full E2E, just enough to confirm the surface actually responds.
5. **If anything is missing**, the doorbell does NOT ring. Instead: surface the gap to the operator with the four-layer breakdown (which layer is broken? what's missing?).

The fail-mode this catches: layer-A through layer-C all looking healthy in the dashboards (beads filed, PRs merged, env wired, container up, HTTPS 200) while layer D is broken (sidebar entry was never built because the bead was never filed).

This is **verify-against-reality applied to your own claims** — the same discipline Cipher applies to security reviews, applied recursively to orchestrator surface-readiness claims.

## Meta-tick output template (watch-protocol §6, meta-tick discipline)

If multiple epics are in-flight, the /loop must be a meta-tick, NOT a single-epic tick. The meta-tick output per epic:

```
EPIC <id> "<title>"
  A. spec→beads:    <surfaces_with_beads>/<surfaces_in_spec> [✓ or list missing]
  B. beads→PRs:     <beads_with_PRs>/<claimed_beads> [✓ or list stalled]
  C. PRs→deployed:  <merged>/<opened> [✓ or list blocked]
  D. user-surface:  <probed? Y/N + per-surface result>
```

Any layer below 100% with no current activity → triggers a §2 action (file the missing bead, ping the stalled assignee, escalate the surface gap).

## Subagent prompt — bad vs good (subagents §4)

❌ Bad prompt: `"Research auth options."` — too vague, no scope, no deliverable.

✅ Good prompt:
```
I need to add OAuth2 to the Hono backend at apps/hono-app/. Current auth is BetterAuth with email/password.
I want to add Google OAuth as a second provider, alongside the existing flow.

Already ruled out: Auth0 (too expensive), Supabase (don't want to migrate the user store).

Research: how does BetterAuth's official Google provider plugin integrate with our setup?
Specifically:
- Where does the redirect URL get configured?
- Does it require a DB migration?
- Are there breaking changes to session shape?

Report findings under 400 words. Cite file paths from BetterAuth docs where relevant.
```

The second prompt would let me act on the result. The first would not.

## Parallel audit — full example (subagents §9)

Goal: audit three areas of the codebase before a refactor.

```
Single message with three Agent calls:

Agent({
  description: "Find all auth-related routes",
  subagent_type: "Explore",
  prompt: "Find every Hono route in apps/hono-app/src/ that requires authentication.
           Return: list of method+path, the auth middleware used, and the file:line.
           Focus only on apps/hono-app/. Under 300 words."
})

Agent({
  description: "Audit current rate limiting",
  subagent_type: "Explore",
  prompt: "Locate the rate-limiting implementation in apps/hono-app/. Return: where it's
           configured, what tiers exist, and which routes opt in/out. Under 250 words."
})

Agent({
  description: "Map session storage",
  subagent_type: "general-purpose",
  prompt: "How is the session token stored on the frontend? Check apps/frontend/ for
           cookie or localStorage usage. Return: storage mechanism, expiry, refresh
           behaviour. Under 300 words."
})
```

Three agents run concurrently. Three reports come back. Now you have the recon you need to design the refactor — without doing the searching yourself.

## Fault-isolation routing table + hidden tells (subagents §11)

This matters when the work involves **potentially-blocking external I/O**:

| In-context (your hands) | Subagent (fault-isolated) |
|---|---|
| `gh pr view 206 --json` (fast, bounded) | `ssh xerox "..."` (Tailscale auth can prompt; SSH can hang on slow DNS) |
| `bd list` / `bd show` (local sqlite, ms) | `gh run view <run-id> --log-failed` (can be a multi-MB stream) |
| `git status`, `git log -5` | Anything that polls (deploy waits, `kubectl wait`, retry loops) |
| Read tool on a known file | Slow DB aggregates over large tables |
| Single Edit/Write tool call | Repeated cross-host probes (multi-`ssh` audits) |
**Watch the hidden tells:** you're about to type `ssh`, `gh ... --log`, `gh run view ... --log-failed`, `curl https://<long-URL>`, `kubectl wait`, `pg_dump`, `docker exec ... psql -c "<aggregate>"`. Pause. Subagent it.

## Skeleton-first: two-phase shape, brief-authoring rules, forward-friction check (subagents §12)

**The two-phase shape:**

1. **Phase 1 — gather cheap.** Enumerate + skim structure: `ls` + `wc -l`, `rg -n` for
   headers/signatures/return statements, doc titles, status lines. Cost scales with
   STRUCTURE, not content.
2. **Phase 2 — escalate targeted.** Deep-read only the items Phase 1 flagged. Escalation
   is cheap and encouraged when a flag fires; blanket deep-reads are the waste.

**Brief-authoring rules (for the dispatcher):**

- State the QUESTION the dispatch must answer, not the corpus it must consume.
  ✅ "Return a one-line summary of each design doc's purpose"
  ❌ "Read all 5 design docs in full, don't truncate for brevity"
- Match depth mandate to output size. A deliverable of N one-liners NEVER justifies
  N full-document reads.
- The phrases "in full", "exhaustively", "don't truncate" are red flags in a brief.
  Keep them only when the deliverable IS the full content (a migration touching every
  line, a byte-level audit).
- Give the subagent an output budget ("report in ≤ 30 lines"). Output budgets
  discipline input gathering.
**Forward-friction check (at brief-writing time, or before your own recon):**

1. What QUESTION does this investigation answer? One sentence.
2. What is the smallest artifact set that answers it? (Headers? One function body?)
3. Does the brief mandate reading anything the question doesn't need?
4. Does the output size justify the input mandate?
5. About to write "in full / exhaustive / don't truncate"? Justify it or delete it.

## Reassess-at-pivot questions (cost-proportional-orchestration §2b)

A scope change is: a new P0/P1 discovered, a track closes out, a blocker emerges that only one agent can work, an estimate turns out to be wrong by 2x+. At each pivot, explicitly ask:

1. **How many agents actually have live, executable work right now?** Not "assigned to this epic" — actually have a next action they can take without waiting on someone else.
2. **For everyone else: should they be quiet instead of standing by?** An agent with nothing to do doesn't need to be polled, doesn't need to "confirm holding," doesn't need to receive the cron's broadcast. Silence costs zero tokens; "standing by, ready" costs real ones every time it's re-said.
3. **Does the loop interval still match the actual bottleneck?** If the whole epic is gated on one agent finishing one fix, a 10-minute full-fleet sweep is checking on 6 agents who have nothing to report, every 10 minutes, for however long that fix takes. Check on the ONE agent that matters, yourself, less mechanically.

**The concrete move, once the pivot analysis says "down to one real blocker":** kill the standing loop, message every non-blocking agent to go quiet ("don't poll, don't reply, I'll ping you when there's real work"), and watch the actual blocker directly and manually. This is what actually happened in the banked incident — it should have happened hours earlier, the moment S1/F1 finished and only the P0 fix remained.

## Forward-friction check (cost-proportional-orchestration §5)

1. Serial-engineer anchor estimate — does the current agent count / loop interval match the table in §2a?
2. At this pivot: how many agents have *live, executable* work right now?
3. Everyone else — told to go quiet, or still being polled by habit?
4. Loop interval — matched to the actual bottleneck, or still at the setting from when the whole fleet was busy?
5. Any pane whose *content* (not just timer) is unchanged across 2 checks — interrupted yet, or still being read as "probably fine"?

If any answer is "still on the old setting" or "still being read as fine," that's the trigger to act — kill/shrink the loop, quiet the idle agents, or interrupt the stalled one — before the next tick, not after.
