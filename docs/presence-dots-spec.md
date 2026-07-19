# Launcher presence dots — card-state spec

Design contract for `aperture-8gypy` (green/red, hub-confirmed) and
`aperture-syepg` (grey/amber, kickoff-fired). Both PRs render into the
*same* dot element on the agent card; this doc is the shared contract so
neither side has to guess the other's shape.

Context: today the card shows a running/stopped binary the moment a tmux
window exists — that's exactly how 7 dead-silent sessions read as "booted."
The dot is an **additive** visual layer answering a narrower, truthful
question: has this agent actually identified itself to the comms hub?
It never replaces `agent.status` and no `status` consumer changes.

**Amended 2026-07-19** (Wheatley's `aperture-wul6m` watchdog-spec review,
relayed by GLaDOS): the hub subscriber connection and the silence-deadline
clock move to a Rust-side watchdog actor. See "Architecture" below — this
replaces an earlier draft of this doc that had the frontend opening its own
`ws://127.0.0.1:4517` connection and computing the deadline client-side.
The deadline constant is **60s**, matching the syepg acceptance SLA and
Izzy's test-harness assertion — one number, one place, not duplicated
between frontend and backend (duplication is exactly how this doc's first
draft shipped with 45s while the SLA said 60s).

## Architecture: backend owns the clock, frontend polls a field

The watchdog actor (Rust, `aperture-wul6m`) owns:

- the hub `role=subscriber` WebSocket connection (auth via whatever
  `aperture-278a4` lands with — a backend-side concern, not the frontend's)
- the 60s silence-deadline computation (kickoff fired, no hub join within
  60s → stuck)
- the re-kick decision (a separate concern, out of scope for this doc)

It exposes the **result** of that computation as two more fields on the
`AgentDef` already returned by the existing `list_agents` Tauri command —
the same struct/poll AgentList.ts already refreshes every 3s:

```rust
dot_state: Option<String>,       // "spawned" | "booting" | "online" | "stuck"
dot_state_since: Option<String>, // ISO 8601, when the current dot_state began
```

**Why polling, not an event bridge:** a push-based `emit()`/`listen()`
event has the exact failure mode this feature exists to kill — a
missed/dropped event on cold start (frontend window not mounted yet when
the first transition fires) or a race at reconnect leaves the dot frozen on
stale state, silence reading as success again, just at a different layer.
Tauri's event system does not replay like the hub's BEADS-backed
unread-replay does. Polling is self-healing: every 3s the frontend re-reads
ground truth, exactly like `status` and `model` already do. If sub-3s
snappiness is wanted later, a push nudge can ride on top of this — but the
polled fields stay canonical regardless, never the event stream alone.

## The four states

| State     | Color (token)              | Animation                  | Meaning                                                              | Computed by |
|-----------|-----------------------------|-----------------------------|-----------------------------------------------------------------------|-------------|
| `spawned` | `--accent-grey` (#5a5a72)   | none                        | tmux window exists, kickoff turn not fired yet                        | syepg (or frontend fallback) |
| `booting` | `--accent-amber` (#e6a23c)  | soft pulse, 1.4s ease       | kickoff turn fired, hub has not confirmed presence yet, <60s elapsed   | wul6m watchdog (or frontend fallback) |
| `online`  | `--accent-green` (existing) | none (steady = healthy)     | hub confirmed presence (`join`/`busy`/`idle`) for this agent name      | wul6m watchdog only |
| `stuck`   | `--accent-red` (existing)   | opacity pulse, 1s ease      | kickoff fired ≥60s ago, still no hub confirmation                     | wul6m watchdog only |

`online` always wins over `booting`/`stuck` regardless of elapsed time —
the moment the hub says join/busy/idle, the dot goes green even if it took
59.9s to get there. Silence past the 60s deadline is the only thing that
paints red; nothing else does. `online` and `stuck` are **never** guessed
client-side — only `dot_state` populated by the watchdog can produce them.

## Data contract (`AgentDef`, `src/types.ts`)

```ts
export interface AgentDef {
  // ...existing fields unchanged...

  /** ISO 8601 timestamp set by agents.rs the moment the kickoff turn is
   *  fired (aperture-syepg). Absent/null before kickoff, or on a backend
   *  that hasn't shipped it yet. */
  kickoff_fired_at?: string | null;

  /** Backend-computed presence-dot state (aperture-wul6m watchdog).
   *  Authoritative — see Architecture above. Absent on a backend that
   *  hasn't shipped the watchdog yet; frontend degrades to a local,
   *  deadline-free spawned/booting-only read of kickoff_fired_at. */
  dot_state?: "spawned" | "booting" | "online" | "stuck" | null;

  /** ISO 8601 timestamp of when the current dot_state began. Cosmetic
   *  display math only (a live "{N}s ago" counter) — not a deadline
   *  decision, safe to read client-side. */
  dot_state_since?: string | null;
}
```

Frontend derivation (`src/services/hub-presence.ts::deriveDotState`):

```ts
export function deriveDotState(agent: AgentDef): DotState {
  if (agent.dot_state) return agent.dot_state;
  return agent.kickoff_fired_at ? "booting" : "spawned";
}
```

Written defensively on purpose: if `dot_state` doesn't exist yet (wul6m
hasn't merged), every running agent reads `spawned`/`booting` off
`kickoff_fired_at` alone — a safe no-op, never a false green, never a
guessed red. Once wul6m ships the field, `online`/`stuck` light up with
zero code change on the frontend. Note there is no 60s constant anywhere
in the frontend — the deadline judgment lives in exactly one place (the
watchdog), by construction, which is what makes the earlier 45-vs-60 drift
structurally impossible to repeat.

## Markup + CSS contract

```html
<span class="agent-mini__icon">
  🎨
  <span class="agent-mini__presence agent-mini__presence--{state}"
        title="{tooltip}"></span>
</span>
```

Rendered **only** when `agent.status === "running"` (no process, no dot —
avoids a fifth phantom state for the stopped case). `.agent-mini__icon`
gets `position: relative` so the dot is an absolutely-positioned badge on
its bottom-right corner (Slack/Discord-style avatar status dot), not a new
column competing for the card's already-tight horizontal space.

CSS class names (both PRs must use exactly these):

```
.agent-mini__presence
.agent-mini__presence--spawned
.agent-mini__presence--booting
.agent-mini__presence--online
.agent-mini__presence--stuck
```

## Tooltip copy

| State     | Tooltip text                                                                 |
|-----------|-------------------------------------------------------------------------------|
| `spawned` | "Starting — waiting for first turn"                                          |
| `booting` | "Booting — kickoff sent, waiting to connect to the hub"                      |
| `online`  | "Online — connected to the message hub"                                     |
| `stuck`   | "Stuck — kickoff sent {N}s ago, still not connected. Check the tmux pane."   |

`{N}` is `Math.round((Date.now() - since) / 1000)` where `since` prefers
`dot_state_since`, falling back to `kickoff_fired_at` pre-watchdog —
recomputed each render, so the operator sees the stuck timer climb, not a
frozen number. This is cosmetic display math, not a deadline decision, so
it's fine to compute client-side even though the state itself never is.

## Non-goals / phantom cases deliberately not modeled

- No separate `busy` vs `idle` color. The hub emits both for Codex bridge
  turn state, but the UI need here is "confirmed present or not" — a fifth
  color for a distinction nothing in this bead's acceptance criteria asks
  for would be modeling a case that doesn't exist yet. `busy`/`idle` both
  map to `online`; the tooltip may mention which if useful, the dot color
  does not change.
- No manual "retry" affordance on the stuck dot in this pass — clicking
  the card already focuses the tmux window, which is where the operator
  fixes a stuck boot today. A dedicated retry action is a plausible
  follow-up, not part of this contract.
- No frontend WebSocket-to-hub connection, no frontend deadline constant,
  no `@tauri-apps/api/event` listener. All three were in this doc's first
  draft and are deliberately removed per the amendment above — the frontend
  is a pure renderer of backend-computed state, full stop.
