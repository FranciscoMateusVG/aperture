export interface AgentDef {
  name: string;
  model: string;
  role: string;
  prompt_file: string;
  tmux_window_id: string | null;
  status: string; // "stopped" | "running" | "error"
  /** Notification badge — set by the backend when this agent calls
   *  `send_message(to: "operator", ...)`. Cleared when the operator clicks
   *  the agent's row in the launcher. There is no chat panel; the
   *  agent's actual message body lives in their tmux scrollback. */
  attention?: boolean;
  /** Why the badge is lit (aperture-ull4y). "message" = operator doorbell
   *  (poller mailbox sweep); "crash" = watchdog 3-strike latch. null/absent
   *  whenever attention is false. Rendered as distinct badges. */
  attention_reason?: "message" | "crash" | null;
  /** Hub turn-state (aperture-ull4y): "busy" mid-turn, "idle" between turns,
   *  null when unknown (offline, subscriber down, no frame since join).
   *  From the ws-hub busy/idle broadcasts via the Rust watchdog, on the
   *  existing 3s poll — no frontend WS (aperture-1iqpn). */
  turn_state?: "busy" | "idle" | null;
  /** FRONTEND-LOCAL, never sent by the backend: the lifecycle op currently
   *  in flight for this agent, so the card can lock its buttons and show a
   *  spinner across polls. Set by AgentList when it fires start/stop/restart,
   *  cleared when the invoke settles. */
  op_pending?: "starting" | "stopping" | "restarting" | null;
  /** ISO 8601 timestamp set by agents.rs the moment the post-launch kickoff
   *  turn is fired (aperture-syepg). Absent/null before kickoff fires (or
   *  on backends that don't populate it yet — the presence-dot derivation
   *  in hub-presence.ts treats that as "spawned," not as an error).
   *  See docs/presence-dots-spec.md for the full state contract. */
  kickoff_fired_at?: string | null;
  /** Backend-computed presence-dot state (aperture-8gypy / aperture-wul6m).
   *  The watchdog actor in Rust owns the hub subscriber connection AND the
   *  60s silence deadline — this field is the authoritative answer, refreshed
   *  every list_agents() poll (3s). Absent on a backend that hasn't shipped
   *  the watchdog yet; the frontend degrades to a local, deadline-free
   *  spawned/booting-only read of kickoff_fired_at in that case (see
   *  hub-presence.ts::deriveDotState). "online"/"stuck" NEVER come from a
   *  client-side guess — only from this field. */
  dot_state?: "spawned" | "booting" | "online" | "stuck" | null;
  /** ISO 8601 timestamp of when the current dot_state BEGAN — the state
   *  transition time, NOT the poll tick (fixed in aperture-ull4y; it used to
   *  be stamped with the tick time so "{N}s ago" always read ~0). Kickoff
   *  time for booting/stuck, stable-online time for online. Cosmetic
   *  display math only, safe to compute the counter client-side. */
  dot_state_since?: string | null;
  /** Current-work summary (aperture-nr65b). Three-state field, see
   *  hub-presence.ts::deriveWorkSummary for the full contract:
   *  undefined/null = no data (stopped, or the backend's bd query failed);
   *  "" = query succeeded, nothing claimed (idle); non-empty = the claimed
   *  bead's id, paired with current_task_title. */
  current_task_id?: string | null;
  current_task_title?: string | null;
  /** Count of other in_progress beads beyond current_task_id, when the
   *  latter is non-empty. */
  current_task_extra_count?: number | null;
}
