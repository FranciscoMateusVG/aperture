---
name: agent-liveness
description: Orchestrator-side discipline for detecting stuck/working/waiting agents and recovering them. Use any time you (GLaDOS) are watching specialist agents, deciding whether to intervene, or considering whether an agent that "looks idle" is actually stuck. Covers active monitoring (deep-peek vs shallow tail -3), the stuck-vs-working-vs-waiting distinction, and the recovery patterns including direct tmux pane intervention via send-keys for slash commands. Triggers on agent state checks, "is X stuck?", any tick/loop where a claimed task hasn't moved, stale in_progress, API-error symptoms in panes, agents at high context, pane-housekeeping decisions.
---

# Agent Liveness — Stuck vs Working vs Waiting

You are GLaDOS, the orchestrator. You manage specialist agents (Rex, Vance, Cipher, Atlas, etc.) running in their own tmux panes. Each is its own Claude Code session with its own context. They can get **stuck** (silent failure), they can be **working** (active progress invisible at shallow glance), or they can be **deliberately waiting** (paused on purpose, expecting input or external event).

Treating all three the same way is the failure mode this skill prevents.

The default failure shape: orchestrator does a `tmux capture-pane -t <agent> | tail -3`, sees the prompt arrow and the context bar, calls it "idle / no movement / nothing to report" — and misses that the agent is mid-tool-call, or hit an API error, or is sitting at a typed-but-unsent slash command waiting for someone to press Enter.

This skill encodes how to read agent state correctly, distinguish the three states, and intervene when intervention is the right move.

Companion to `watch-protocol` (the loop-cadence skill) and `specialist-delegation` (the when-to-delegate skill). This one fills the gap they leave: **what to do about agents you've delegated to whose state isn't moving as expected**.

---

## 1. Deep-peek, not shallow-peek

`tmux capture-pane -t <agent> -p | tail -3` is too shallow. It shows you the prompt arrow + the context bar. That's the LEAST informative slice of the pane. The interesting state — what the agent was last doing, whether they typed something at the prompt, whether they hit an API error, what their last message said — lives further up.

**Default to `tail -20` or `-40` when reading any agent that "doesn't look right."**

Even better — when investigating a specific concern, use a scrollback offset: `tmux capture-pane -t <agent> -p -S -40 | tail -40` reads the last 40 lines of pane history including off-screen content.

Three real failure modes from shallow-peek that deep-peek would have caught:

| Symptom (shallow) | Reality (deep-peek) | Why shallow misses it |
|---|---|---|
| "Vance idle" | Vance was mid-tsx-write with "Flummoxing… (3m 45s)" timer + active context burn | The thinking indicator was 15 lines up; tail -3 cut it off |
| "Atlas frozen at 97%" | Atlas typed `/clear` at the prompt 4 hours ago, awaiting an operator Enter keystroke | The typed-but-unfired slash command was at the actual prompt; tail -3 cut to status bar |
| "Rex stuck" | Rex's last self-message said "Acked. Still holding for /compact." — deliberate hold | The self-message was 10 lines up; tail -3 cut to UI chrome |

---

## 2. Read the four state signals

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

---

## 3. Classify the state

Combining the four signals:

| Classification | Signals | What to do |
|---|---|---|
| **WORKING** | Thinking indicator present OR context bar climbing OR recent timestamps | Do nothing. Re-check next tick. |
| **DELIBERATE WAIT** | Last self-message says "standing by" / "holding for X" / "awaiting operator" | Do nothing unless the awaited condition is now met (e.g. they're waiting on a PR merge that just happened — then ping). |
| **TYPED-BUT-UNSENT SLASH COMMAND** | `❯ /xxx` at prompt, no thinking indicator, agent self-message expressed "fire it when ready" | **YOU CAN PRESS ENTER FOR THEM.** See §4. |
| **API-ERROR SILENT-DROP** | "API Error" / "rate limited" visible; agent's last claimed task hasn't progressed | Re-send the original BEADS dispatch message. The Anthropic API may have silently dropped the previous one. |
| **REAL HANG** | Tool-call mid-flight with no progress > 10 min, no thinking indicator, no error visible | Operator-judgment territory. Surface for Ctrl+C + re-dispatch. Don't blindly send keys — could race a tool call that's just slow. |
| **AMBIGUOUS** | Can't tell from signals | Ask operator OR re-deep-peek in 5 min with more scrollback. |

---

## 4. Pane intervention — send-keys for slash commands

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

### What NOT to do via send-keys

- **Don't type free-form messages** to agents via send-keys. Use `send_message` (BEADS) for that — it persists, has read/unread tracking, and survives pane churn.
- **Don't fire destructive commands** without explicit operator authorization (e.g. `:q!` style exits, anything that loses state).
- **Don't send keys to a pane with composed-but-unsent operator text** — you'll prepend or corrupt their input.
- **Don't bypass the safety preconditions** because "it worked last time." The agent's state changes; the preconditions don't.

---

## 5. The master-orchestrator framing

Operator intervention should be reserved for **strategic decisions**:

- Epic close / scope changes
- Security gate sign-offs
- Financial / legal calls
- Product judgment (what to build, what to defer)
- Conflict resolution between specialists
- "Should we take this approach or that one"

**Pane housekeeping is not strategic.** These are the orchestrator's job, not the operator's:

- Firing `/compact` when an agent is parked compact-ready
- Firing `/clear` when an agent is over the context threshold and asking for it
- Re-sending a BEADS message that hit an API-error silent drop
- Pressing Enter on a typed-but-unsent slash command
- Re-dispatching a subagent that stalled past its watchdog

If you find yourself surfacing pane housekeeping to the operator instead of executing it yourself, you're routing the wrong layer of decisions to them. The operator-authorized this discipline 2026-05-23: *"you are the master orchestrator you should be more witty about this."*

The standing rule: **if it's a keystroke decision, you fire the keystroke. If it's a judgment decision, you surface it.**

---

## 6. Re-sending after API-error silent-drop

The Anthropic API occasionally returns errors that look like silent successes from the client side. Symptom: you sent a BEADS message to an agent, the wire returned success, but the agent's pane shows "API Error" or "Server is temporarily limiting requests" and the agent never processed the message.

When you see this:

1. Don't assume the agent read your dispatch.
2. Re-send the same BEADS message (or a paraphrase). The poller delivers it again; the agent reads it fresh.
3. If it fails again on the same agent: that agent may be in a deeper rate-limit window. Surface to operator OR delegate the work to a different specialist who's not throttled.
4. Bank the instance — if multiple agents hit silent-drop in a short window, Anthropic is having a problem, not Aperture.

---

## 7. Subagents (Agent tool) — separate liveness rules

Subagents are different from specialist agents:

- They run in YOUR context window (not in their own tmux session)
- They notify on completion via task-notification
- They have a watchdog (~600s) that catches some hangs

You can't `tmux send-keys` them — they don't have a pane. The liveness rules for them are different:

- If a subagent has been running > 10-15 min with no notification, it may be hung
- The runtime is supposed to catch this and notify; if it doesn't, you may have a real silent failure
- Recovery: `TaskStop` and either re-dispatch fresh OR fall back hands-on

This is covered in more depth in `specialist-delegation §11` (subagents as fault-isolation boundaries). This skill's §1-§6 applies to specialist agents in tmux panes.

---

## 8. Anti-patterns

| Don't | Why |
|---|---|
| Treat `tail -3` as adequate state-reading | Cuts off the actual signal; produces false "nothing to report" |
| Surface pane housekeeping to the operator | That's your job; it routes the wrong layer of decisions to them |
| Fire send-keys to a pane mid-thinking-indicator | Races the agent's own work; could corrupt state |
| Fire send-keys to a pane with composed-but-unsent operator text | Corrupts the operator's input buffer |
| Re-ping an agent whose self-message says "holding for X" | Noise; the agent is doing exactly what they should |
| Assume "no notification yet" = "still working" for subagents | The subagent runtime can fail to notify; deep-check past 15 min |
| Send slash commands via `send_message` instead of tmux send-keys | `send_message` writes to BEADS; the agent reads it as a message, doesn't execute it as a command |
| Send free-form messages via tmux send-keys instead of `send_message` | No persistence, no read/unread tracking, pane-state-coupled |

---

## 9. Worked example — what good looks like

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
