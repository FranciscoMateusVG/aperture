import type { AgentDef } from "../types";

// Presence-dot state derivation (aperture-8gypy).
//
// ARCHITECTURE (amended 2026-07-19, per Wheatley's wul6m watchdog-spec
// review): the hub subscriber connection AND the 60s silence deadline live
// in a Rust-side watchdog actor, not here. That actor is the single owner
// of "has it been 60s since kickoff with no hub join" — the frontend does
// NOT open its own WebSocket to the hub and does NOT carry its own copy of
// the deadline constant. Two independent deadline computations (frontend +
// backend) is exactly how a bare-numeric drift like "45 vs 60" happens
// silently; owning it in exactly one place structurally prevents that class
// of bug rather than relying on both sides remembering to agree.
//
// Contract (see docs/presence-dots-spec.md): the watchdog exposes
// `dot_state` + `dot_state_since` as two more fields on the AgentDef already
// returned by the existing `list_agents` poll (AgentList.ts refreshes every
// 3s). No new event-bridge/listen() mechanism — a push-based event stream
// has the exact failure mode this feature exists to kill (a missed/dropped
// event on cold start leaves a dot frozen on stale state, silence reading as
// success again, just at a different layer). Polling is self-healing: every
// 3s the frontend re-reads ground truth, same as status/model already do.
//
// Until a backend ships dot_state (pre-wul6m), this module degrades to a
// LOCAL, DEADLINE-FREE read of kickoff_fired_at: "booting" is just "kickoff
// fired, unconfirmed" with no cutoff judgment attached, so it's safe to
// compute here. "online" and "stuck" are never guessed client-side — they
// only ever come from a backend-populated dot_state.

export type DotState = "spawned" | "booting" | "online" | "stuck";

/** The full state per docs/presence-dots-spec.md. Prefers the backend's
 *  authoritative dot_state; falls back to the local deadline-free read only
 *  when the field is absent (backend hasn't shipped the watchdog yet). */
export function deriveDotState(agent: AgentDef): DotState {
  if (agent.dot_state) return agent.dot_state;
  return agent.kickoff_fired_at ? "booting" : "spawned";
}

const DOT_TOOLTIPS: Record<DotState, string> = {
  spawned: "Starting — waiting for first turn",
  booting: "Booting — kickoff sent, waiting to connect to the hub",
  online: "Online — connected to the message hub",
  stuck: "Stuck — kickoff sent {N}, still not connected. Check the tmux pane.",
};

/** Whole seconds elapsed between an ISO timestamp and `now` (ms epoch).
 *  null when the timestamp is absent or unparseable — callers must NOT
 *  substitute 0, that is exactly the bogus "~0s" this exists to kill. */
export function secondsSince(iso: string | null | undefined, now: number): number | null {
  if (!iso) return null;
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return null;
  return Math.max(0, Math.round((now - t) / 1000));
}

/** When the current dot_state began. Prefers dot_state_since (backend-
 *  stamped with the TRANSITION time since aperture-ull4y); falls back to
 *  kickoff_fired_at for the pre-watchdog local "booting" read. */
function stateSince(agent: AgentDef): string | null | undefined {
  return agent.dot_state_since ?? agent.kickoff_fired_at;
}

/** {N} is cosmetic display math only (a live-updating "how long has this
 *  been true" counter) — not a deadline decision, so client-side clock
 *  arithmetic is fine here even though the state itself isn't. When no
 *  usable timestamp exists the elapsed clause is dropped entirely rather
 *  than rendered as "0s ago". `now` is injectable for tests. */
export function dotTooltip(state: DotState, agent: AgentDef, now: number = Date.now()): string {
  if (state !== "stuck") return DOT_TOOLTIPS[state];
  const seconds = secondsSince(stateSince(agent), now);
  return DOT_TOOLTIPS.stuck.replace("{N}", seconds == null ? "earlier" : `${seconds}s ago`);
}

// ── Turn-state chip (aperture-ull4y) ──
//
// One-word label rendered next to the model string. Composes the process
// status, the presence dot state, and the hub turn_state into a single
// operator-facing word; the counter in booting/stuck is recomputed on every
// render off dot_state_since (the list rebuilds on the 3s poll — no
// per-second timer, on purpose).
//
//   not running          → null (no chip; stopped is already the card's look)
//   online + busy        → "busy"   (pulses — the agent is mid-turn)
//   online + idle        → "idle"
//   online + unknown     → "online" (subscriber down / no frame yet)
//   booting              → "booting {N}s"
//   stuck                → "stuck {N}s"
//   spawned              → "spawned"

export type StateChipKind = "busy" | "idle" | "online" | "booting" | "stuck" | "spawned";

export interface StateChip {
  label: string;
  kind: StateChipKind;
  tooltip: string;
}

const CHIP_TOOLTIPS: Record<StateChipKind, string> = {
  busy: "Mid-turn — the hub reports this agent is generating",
  idle: "Between turns — online and waiting for input",
  online: "Online — connected to the hub, turn state not reported",
  booting: "Booting — kickoff sent, waiting to connect to the hub",
  stuck: "Stuck — kickoff sent, still not connected. Check the tmux pane.",
  spawned: "Spawned — process exists, kickoff not fired yet",
};

/** Pure: (agent, now-ms) → chip or null. Never throws on a partial AgentDef
 *  (older backend without turn_state / dot_state_since). */
export function deriveStateChip(agent: AgentDef, now: number): StateChip | null {
  if (agent.status !== "running") return null;
  const dot = deriveDotState(agent);
  const withCounter = (kind: "booting" | "stuck"): StateChip => {
    const seconds = secondsSince(stateSince(agent), now);
    return {
      kind,
      label: seconds == null ? kind : `${kind} ${seconds}s`,
      tooltip: CHIP_TOOLTIPS[kind],
    };
  };
  switch (dot) {
    case "online": {
      const kind: StateChipKind =
        agent.turn_state === "busy" ? "busy" :
        agent.turn_state === "idle" ? "idle" : "online";
      return { kind, label: kind, tooltip: CHIP_TOOLTIPS[kind] };
    }
    case "booting": return withCounter("booting");
    case "stuck": return withCounter("stuck");
    case "spawned": return { kind: "spawned", label: "spawned", tooltip: CHIP_TOOLTIPS.spawned };
  }
}

// ── Current-work summary line (aperture-nr65b) ──
//
// Composes with the dot state above: booting/stuck describe why the agent
// isn't reachable right now, and that's strictly more useful to the operator
// than whatever bead title happened to be claimed before the agent went
// quiet — so those two states override the work line entirely, same
// principle as the dot itself (don't show a claim you can't back up).
//
// Backend contract (agents.rs::resolve_current_tasks + AgentDef in
// state.rs): current_task_id is a three-state field —
//   - undefined/null → no data (agent stopped, or the bd query failed this
//     cycle) → render nothing, exactly like before this feature shipped.
//   - "" (empty-string sentinel) → query succeeded, nothing claimed → idle.
//   - non-empty → the claimed bead's id; current_task_title carries the
//     title, current_task_extra_count the count of other in_progress beads
//     beyond this one.

export interface WorkSummary {
  text: string;
  tooltip: string;
}

export function deriveWorkSummary(agent: AgentDef, dotState: DotState): WorkSummary | null {
  if (dotState === "booting") {
    return { text: "Booting…", tooltip: "Kickoff sent, waiting to connect to the hub" };
  }
  if (dotState === "stuck") {
    return { text: "Stuck", tooltip: "Not responding — check the tmux pane" };
  }
  if (agent.current_task_id == null) {
    return null; // no data — same rendering as before this feature shipped
  }
  if (agent.current_task_id === "") {
    return { text: "Idle", tooltip: "Online, no claimed task" };
  }
  const extra = agent.current_task_extra_count ?? 0;
  const title = agent.current_task_title ?? "(untitled)";
  return {
    text: extra > 0 ? `${title} (+${extra} more)` : title,
    tooltip: extra > 0
      ? `${agent.current_task_id} — ${title}\n+${extra} more in_progress`
      : `${agent.current_task_id} — ${title}`,
  };
}
