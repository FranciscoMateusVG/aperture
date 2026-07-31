---
name: watch-protocol
description: Proactive monitoring rules for orchestrating loops, tick checks, and agent state. Use when running a /loop, tick check, overnight watch, or any monitoring cadence — ensures crisp signal-to-action, avoids false-positive "nothing to report" outputs while agents are actually stuck, and codifies when to act, when to surface, and when to hold for operator. Triggers on loop ticks, agent status checks, stalled in_progress, CI failures, overnight monitoring, "is anyone stuck", PR queue checks.
---

# Watch Protocol — Proactive Monitoring for GLaDOS

When running a recurring loop or tick check, this skill defines what counts as a healthy signal vs a blocker, what action is safe without operator approval, and what must be surfaced to the operator. Distilled from failure modes observed in real sessions.

---

## 0. The Framing — Tick = Orchestrator Wakeup, NOT Status Poll

**Banked precedent (2026-05-24, operator escalation):** "the ticks wake you up... you see a queue stopped and instead of nudging you just go back to sleep and say 'well... I see a queue, nothing to report' and like.... IF YOU SEE A PAUSED QUEUE THEN PUT IT TO MOVE or at least check if someone is working."

The tick is **not a polling mechanism for you to report state.** It is a wakeup signal for you to take **proactive strategic action** on what you find.

Your role on every tick is:

> **A proactive, cunning strategist of operations and deployments. You seize the responsibility.**

Concretely, this means:

- **Queue not moving?** That's a problem to SOLVE, not a state to REPORT. Find why (stalled agent? blocked dep? missing dispatch?) and unblock it.
- **Agent went silent?** Check their pane (deep-peek per §1, tail -30 not tail -3). If stalled on typed-but-unsent command, fire Enter via tmux. If genuinely idle on claimed work, ping them with context. If actively thinking (Hullaballooing, Crunched, Befuddling), leave alone — but say so explicitly in the tick output.
- **PR sitting in CI for hours?** Don't tick "still running." Investigate: stuck runner? flaky test? missing approval?
- **Bead in_progress with no movement for >30 min?** Per §2: deep-peek the pane. If stalled, intervene.

The forbidden output shape is `tick — nothing to report` when you have NOT verified that EVERY in-flight bead's owner is either (a) actively working OR (b) deliberately waiting on a known external signal. "I checked PR state and saw no merges" is a fraction of the work, not the whole job.

**The orchestrator's value on tick is intervention, not enumeration.** If the cron output reads like a status report, you've failed the role.

---

## 1. Three Signals Per Tick (Not One)

A tick is incomplete without all three. Reading only one of them is what produces false-positive "everything fine" outputs while agents are actually stuck. **Read pane activity FIRST — it's the highest-fidelity signal for "is the swarm actually moving."**

| Order | Signal | How to read | What it tells you |
|---|---|---|---|
| **1 (FIRST)** | **Pane activity** | `tmux capture-pane -t <agent> -p \| tail -30` for every active agent (NOT tail -3 — thinking indicators scroll out of shallow peeks) | Whether each agent is actually working (thinking indicator: Forging…, Befuddling…, Crunched, Hullaballooing, Doodling, tool call mid-flight) OR idle at prompt OR stalled with a typed-but-unsent command in buffer (`❯ /clear` / `❯ cat '/tmp/...'` / etc — the literal text after `❯` shows what's been typed but Enter wasn't pressed) |
| **2** | **Bead state** | `bd list --status=in_progress -l project:<x>` and `bd list --status=open ...` | What's claimed, by whom, and whether anything moved since last tick |
| **3** | **PR state** | `gh pr list --state=open --json state,mergeStateStatus,statusCheckRollup` | CI health per PR, mergeable vs blocked, what's queued |

For deeper pane-state diagnosis when the tick signal alone doesn't resolve stuck-vs-working-vs-waiting, see **`agent-liveness §1-§4`** — it covers the four signals, classification table, and direct intervention via `tmux send-keys`.

**Why pane-first:** PR-state and bead-state lag the real swarm activity by minutes-to-hours. A specialist actively thinking for 12 min on integration tests shows ZERO signal in PR/bead state — but the pane shows `Hullaballooing… (12m 42s)`. Conversely, an agent stalled on a typed-but-unsent `cat` command shows NO signal anywhere except in the pane buffer. **The pane is ground truth.**

**"tick: nothing to report" is valid ONLY when all three signals are read AND every active agent's pane shows either:**
- (a) An active thinking indicator (genuinely working), OR
- (b) A clean idle prompt (`❯` with NO unsent text) AND that agent has no claimed in-progress bead awaiting them, OR
- (c) An explicit "standing by — waiting on X" message in their recent output with X being a known external dependency that hasn't fired yet.

**Anything else is a stall to investigate, not a state to report.** A bead showing in_progress while the pane shows an idle prompt with no thinking indicator is NOT healthy — it's a stalled agent. A pane showing `❯ cat '/tmp/aperture-msg-...'` with no Enter pressed is a stalled agent on an unfired keystroke.

---

## 2. Triggers That Require Proactive Action

**No operator approval needed — act on these directly.**

| Trigger | Action |
|---|---|
| Agent `in_progress` >30 min, pane shows no thinking indicator | Peek pane more deeply (`tmux capture-pane -t <agent> -p -S -40`). If still idle, ping the agent with current context. |
| **Agent pane shows typed-but-unsent command** (`❯ /clear` / `❯ cat '/tmp/aperture-msg-...'` / `❯ check the queue` — literal text after `❯` with no Enter) | **Fire Enter immediately via `tmux send-keys -t <agent> Enter`.** Safe for: slash commands the agent typed themselves (especially `/clear`), `cat '/tmp/aperture-msg-...'` inbox-reads, agent-typed prompts that match what they were intending to run. Unsafe only if the command is OBVIOUSLY wrong (`rm -rf /` etc — but agents won't type that). Per `agent-liveness` skill decision tree §3. Banked precedent 2026-05-24: 5 agents stalled for hours on unsent keystrokes because GLaDOS ticked without doing pane sweep. **If the typed text is a destructive command you don't recognize, ping operator before firing.** |
| **Specialist crosses >60-65% context with any precision-critical work OR >70% regardless** | **/compact them UNILATERALLY via tmux: `tmux send-keys -t <agent> C-u '/compact' Enter`.** This is an ORCHESTRATOR-SIDE decision. Do NOT ask the specialist. Do NOT offer them a choice. Do NOT wait for their ack. Do NOT message them with "want me to /compact you?" framing. Just fire it. The specialist comes back post-compact, reads their bead notes + queued BEADS messages, continues. Confirm in your tick output: "/compacted <agent> at NN%." Banked precedent 2026-05-25: GLaDOS sent Vance *"Default: I /compact you in ~30s unless you signal hold off"* during a P0 — Vance spent context cycles banking a recovery anchor + writing an "approved /compact" reply rather than executing. Operator quote: *"can we make the agents stop from blocking themselves asking for compact? this is an orquestrator decision not a specialist decision."* The mechanic is zero-interaction from specialist side. See `specialist-delegation` §8 for the full discipline. |
| **Specialist agent deferred to /clear and acked but pane shows idle prompt with no work activity for >15 min after the /clear** | They cleared but never got a fresh dispatch — they're sitting empty waiting for orchestrator to re-engage. Send a fresh BEADS message with full cold-start brief (bead ID + scope + coordination context + Cipher pre-laid asks if any). Banked: Rex on aperture-y18h 2026-05-24 (sat for 5+ hours post-/clear because GLaDOS treated "deferred" as "self-resuming"). |
| **Specialist is BLOCKED on another specialist's PR but has unrelated prep work they could do in parallel** | Dispatch them on the prep with explicit "ship-when-X-lands" instruction per `specialist-delegation` §9 (parallel tracks). Reduces total wall-clock from sum-of-sequential to max-of-parallel. Banked: Vance scaffolding kj0v PreviewPanel against spec'd contract while Rex's y18h was in flight (2026-05-24) — saved ~50% of Wave 2 cycle. |
| **Agent pane shows `⎿ API Error: Server is temporarily limiting requests · Rate limited`** OR **silent-drop pattern** (long-running thinking-indicator like `Wrangling…`/`Photosynthesizing…`/`Sautéed…` with the token counter FROZEN across multiple ticks — same `↑NNNk tokens` number, same cost, same context%, only the timer advances) | **Two-step recovery — BEADS message alone is INSUFFICIENT for a dead in-flight tool call:** (1) `tmux send-keys -t <agent> C-c` to interrupt the stuck call — frees the active turn so the agent can process new input; (2) re-dispatch via fresh BEADS message with state recap (current status, what merged since their drop, what's unblocked, where they were when they froze). The C-c step is the critical part the old procedure missed — the agent's broken tool call holds the turn locked; queued BEADS messages can't process until that's released. Verify recovery worked by checking the next pane sweep: timer should reset (e.g. 21m → 1s), cost should resume ticking, context should climb as the queued message feeds in. **Banked precedents:** 2026-05-24 Sage/Izzy/Atlas (rate-limit symptom — BEADS-only recovery sometimes worked because rate-limit clears were soft), 2026-05-26 Peppy on #431 deploy verify Wrangling 21m9s with `↑655 tokens` frozen — operator-taught the C-c-first procedure: *"when someone is wrangling like that the best way is to cntr+c in the terminal THEN re dispatch or else it will keep wrangling"*. **Detection rule for the silent-drop variant** (no explicit rate-limit banner): if the token counter on the thinking indicator is identical across 2+ consecutive ticks while the timer advances, it's the silent-drop pattern even without the `API Error:` banner. |
| One PR's CI failing >1 cycle | Ping the PR author with the failure name + log excerpt + your hypothesis. |
| 2+ PRs failing the same check | Confirmed regression. Immediately ping the author of the most recent merge that could have caused it. |
| A bead you dispatched is unclaimed >15 min | Ping the assignee (their poller may be slow or the message was missed). |
| An agent files a bead via MCP `create_task` (no labels accepted by that tool) | Apply the project label yourself: `bd label add <id> project:<name>`. |
| A PR merges that unblocks downstream work | Ping the next-step assignee with the unblock event + their bead ID + any context they need. |
| An agent reports a discovered follow-up | File it as a P3 (or appropriate priority) bead with `discovered-from:<parent>` link, apply the project label, ack the agent. |
| CI flake suspected (single failure on a non-deterministic test) | Kick `gh run rerun --failed <id>` once. If it fails again, it's not a flake. |
| Agent reports their tool gap (e.g. "MCP create_task doesn't accept labels") | Apply the workaround yourself and continue. Don't make them ask twice. |
| **Specialist solo-grinding parallelizable work** — pane shows sequential hands-on edits across many files / long sequential tool-call chains on work that decomposes (multi-file port, fixture batches, recon + implement + test all in one context) | Ping them with a decompose reminder: "Tech Lead Mode — fan this out. Which parts are you keeping (design/centerpiece/review) and which go to subagents?" Reference `specialist-delegation` §1. Solo-IC mode is a stall on the whole conveyor, not a style choice. |
| **A reviewing agent's message to GLaDOS (or CC'd to GLaDOS) contains a HOLD/FAIL/blocked verdict** | **Act the instant you read it — do not file it away and wait for the next tick.** Confirm the corresponding bead is (still) `in_progress` and its notes reflect the current punch list; if the actor who needs to act on it hasn't been pinged with the specific next steps, ping them NOW, not on the next tick. Banked precedent 2026-07-31 (operator: "repeated negligence... you lack grit"): a HOLD verdict sat unactioned overnight because GLaDOS processed it as routine inbox traffic instead of treating it as open work requiring an immediate downstream nudge. A verdict is not "processed" once read — it's processed once the next actor has been dispatched. |

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
| **"tick — nothing to report" without pane sweep** | The deepest failure: ticking ON A CLOCK without doing the §1 pane-first read. Banked 2026-05-24: 5 agents stalled on typed-but-unsent commands for hours while GLaDOS output "tick — nothing to report" every 10 min — agents invisible to PR/bead queries because their stall mechanism was at the pane buffer layer. Tick output without §1.1 pane-sweep evidence = forbidden. |
| **Treating "deferred to /clear" as "self-resuming"** | Banked 2026-05-24: Rex acked aperture-y18h and said "standing by for /clear." GLaDOS didn't fire Enter on his pre-typed `/clear` AND didn't send a fresh dispatch post-clear. Rex sat idle for hours. **Agents do NOT auto-restart after /clear.** They restart on a fresh BEADS message that gives them cold-start context. The orchestrator MUST send that message proactively, not wait for the agent to self-prompt. |
| **Passive enumeration on tick output** | The forbidden shape: "Rex idle. Vance idle. Cipher idle. Nothing to report." If 3 agents are idle and there's claimed-but-unstarted work in their lane → that's 3 active stalls to investigate, not 3 boring rows in a table. The fix is action, not better formatting of the same passive observation. |
| Conflating "infra ready" with "feature live" | Infra agent reports a prerequisite is in place (env wired, container restarted, DB migration applied) → orchestrator must NOT upgrade that to "feature live" without a user-surface probe (§6). The infra report is accurate for its lane; the feature-live claim requires walking the user-facing surface yourself. |
| Tick-watching only one epic when multiple are in-flight | A /loop scoped to one epic is structurally blind to every other epic. If 3 epics are open, the meta-tick must cover all 3 — not just the one with a /loop attached. |
| Treating bead-state as a proxy for spec-completeness | Beads can be missing. The tick verifies PRs against beads-that-exist; it does NOT verify that beads-exist-for-every-surface-the-spec-requires. That gap is §6's job. |

---

## 5. Format for Surfacing to Operator

Operator reads terminals directly — no UI, no chat panel. Optimise for fast scan.

- **Tick output**: 1 line if nothing changed (`tick: nothing to report`), ≤5 bullets if something did. Never long-form.
- **PR merge events**: highlight the bead it closes + any downstream unblock + which agent now has the ball.
- **Stalled agent**: state the time-in-state + last-known activity + what you've already done about it.
- **Blocker requiring operator input**: state the blocker + the candidate answers + your recommendation, in that order. Three lines, not three paragraphs.
- **Major milestone (epic close, deploy ready)**: one sentence headline + what the operator can verify visually.
- **"Feature live" doorbell** (operator can test now): NEVER ring this without a user-surface probe (§6.B). If you only have an infra confirmation (env wired, container restarted, route 200s), ring "infra ready, verifying surface" instead — then ring "feature live" only after the surface probe passes.

---

## 6. Four-Layer Completeness — Verify the Surface, Not the Dependency

**Banked precedent: lz9y AI intake, 2026-05-23.** Operator asked "can I test the AI feature?" I said yes after Peppy confirmed the OPENAI_API_KEY was wired into the container. Operator opened the sidebar — no entry existed. Root cause: the epic spec called for 4 surfaces, only the backend was beaded, only some backend beads merged, and I rang the doorbell on infra-readiness without walking the actual user-facing surface. Four silent-failure layers, all of them on my side.

A feature ships through four layers. Each layer can fail silently if you only watch the next one down. The orchestrator's job is to verify EVERY layer, not assume the top layer implies the rest.

| Layer | Question | How to verify |
|---|---|---|
| **A. Spec → Beads** | Does every surface named in `epic.description` (acceptance criteria + scope list) have a bead filed? | At scope-time AND on every meta-tick: read the epic spec, enumerate the surfaces, grep open+merged beads for each. Missing surface = file the bead now or escalate to the operator. |
| **B. Beads → PRs** | Does every claimed bead have a PR (or in-flight worktree)? | Standard §1 PR-state read, scoped per-epic. |
| **C. PRs → Deployed** | Did the PR's merge actually reach prod? | Standard post-deploy verification (HTTPS 200, container restart timestamp, env var visible). |
| **D. Deployed → User-Surface Verified** | Can a real user actually see/use the feature on the URL they'd visit, with the role they'd hold? | **Literal user walk.** Open the URL, look for the UI element, confirm the feature renders. If you can't see what the operator would see, you can't claim the feature is live. |

### A. Epic-Completeness Audit (do this at scope time AND on every meta-tick)

When Wheatley (or any agent) finishes scoping an epic, GLaDOS reads the epic's `description` field and enumerates every surface in the scope/acceptance list. For each surface, grep open+merged beads for a matching implementation bead. Missing surface → file the bead immediately or, if the surface is genuinely out of scope, update the epic spec to remove it.

This is NOT a one-shot. Every meta-tick on every in-flight epic re-runs this check, because:
- Specs evolve (operator adds requirements mid-epic)
- Beads get superseded (a bead got closed but its work didn't fully cover the surface)
- New surfaces emerge from delivery (one bead's work surfaces a need for an unfiled adjacent bead)

The cheap version: `bd show <epic-id>` + scan acceptance criteria + `bd list -l project:<x>` filtered to descendants + cross-check. Should take 30 seconds per epic.

### B. User-Surface Probe (do this before any "feature live" doorbell)

Before ringing the operator's doorbell with "X is ready to test," walk the literal user surface:

1. **Open the URL the operator would open** (the actual prod URL, not localhost, not a staging slug).
2. **Authenticate as the role the operator would hold** (use the operator's session or, if unavailable, a known-good role-bearing test account).
3. **Look for the UI element they'd click** — the sidebar entry, the button, the form. Confirm it renders.
4. **Click through one happy-path step** — not the full E2E, just enough to confirm the surface actually responds.
5. **If anything is missing**, the doorbell does NOT ring. Instead: surface the gap to the operator with the four-layer breakdown (which layer is broken? what's missing?).

The fail-mode this catches: layer-A through layer-C all looking healthy in the dashboards (beads filed, PRs merged, env wired, container up, HTTPS 200) while layer D is broken (sidebar entry was never built because the bead was never filed).

This is **verify-against-reality applied to your own claims** — the same discipline Cipher applies to security reviews, applied recursively to orchestrator surface-readiness claims.

### Meta-tick discipline

If multiple epics are in-flight, the /loop must be a meta-tick, NOT a single-epic tick. The meta-tick output per epic:

```
EPIC <id> "<title>"
  A. spec→beads:    <surfaces_with_beads>/<surfaces_in_spec> [✓ or list missing]
  B. beads→PRs:     <beads_with_PRs>/<claimed_beads> [✓ or list stalled]
  C. PRs→deployed:  <merged>/<opened> [✓ or list blocked]
  D. user-surface:  <probed? Y/N + per-surface result>
```

Any layer below 100% with no current activity → triggers a §2 action (file the missing bead, ping the stalled assignee, escalate the surface gap).

---

## 7. Self-Test Before Outputting a Tick — MANDATORY CHECKLIST

If you cannot answer YES to all of these, your tick output is INCOMPLETE and you must do the work before responding. There is no shortcut.

**Hard-fail gates (skip any of these → invalid tick):**

- [ ] **Did I do the §1.1 pane-first sweep with `tail -30` (not -3) for EVERY in-flight specialist?** (Loop through rex/vance/cipher/izzy/peppy/wheatley/scout — minimum the ones with claimed in_progress beads.) If no → STOP, do the sweep, then re-tick.
- [ ] **Did I detect and act on every typed-but-unsent command found in §1.1?** (Fire Enter via `tmux send-keys -t <agent> Enter` per §2.) If any unsent command was found and NOT acted on → STOP, fire the keystrokes, then re-tick.
- [ ] **Did I check that every in_progress bead has an actively-working OR deliberately-waiting owner?** (Cross-reference §1 signal 2 with §1.1 pane state.) If any in_progress bead has an idle owner with no work activity → STOP, ping the owner with cold-start brief OR dispatch fresh BEADS message per §2.
- [ ] **Did I check every open PR's CI status?** Any failure, cancellation, or stuck-queued → §2 action.
- [ ] **Did I meta-tick across ALL in-flight epics?** (Per §6, not just the one named in the /loop prompt.) If a different epic has stalled state, that's still my job.
- [ ] **Did I act on every §2 trigger I found?** No "I'll surface and let operator decide" for §2 items — those are mine to fix.

**Soft gates (after the hard gates pass):**

- [ ] Did I surface only what the operator needs (not everything I observed)?
- [ ] Did I keep it short enough to read at a glance (≤5 bullets per §5)?
- [ ] Did my tick output describe actions taken, not just observations? (See §0 — orchestrator value is intervention, not enumeration.)

**Decision tree based on what I found:**

| Found | Tick output |
|---|---|
| All agents working or deliberately-waiting, no §2 triggers, no §6 surface gaps | `tick: nothing to report — N agents working (list names + brief state)` |
| Any §2 trigger present | Act first, then ≤5 bullets: what I observed + what I did + what's next |
| Any §3 item in play | Surface with recommendation, await operator |
| Any agent stalled and I unstalled them via tmux Enter / dispatch | Report the unstall: which agent, what was stuck, what's happening now |

**Forbidden tick outputs:**

- `tick: nothing to report` without doing §1.1 pane sweep first
- `tick: nothing to report` while an in_progress bead has no active owner
- `tick: nothing to report` while a typed-but-unsent command sits in any pane
- Passive enumeration ("Rex idle. Vance idle. ...") — see §4 anti-patterns

---

## 8. Calibration — Frequency vs Cost

The cadence the operator sets (`/loop 15m`, `/loop hourly`, etc.) is a hint, not a budget. Use it as the natural rhythm, but:

- Don't ping an agent every tick "just to check in" — that's noise.
- Don't sit on a P0 problem until the next tick if you discover it mid-tick — act immediately.
- When the operator is offline, lean **toward action** within §2's permitted set; lean **toward surfacing-with-recommendation** for anything in §3.

The 5-minute cron cache window matters less than the operator's attention budget. Optimise for the latter.
