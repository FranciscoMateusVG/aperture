use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDef {
    pub name: String,
    pub model: String,
    pub role: String,
    pub prompt_file: String,
    pub tmux_window_id: Option<String>,
    pub status: String,
    /// Manifest `emoji` (aperture-84bby). `None` when the manifest omits it
    /// or leaves it empty; the launcher card then falls back to its own map.
    #[serde(default)]
    pub emoji: Option<String>,
    /// Notification badge — set when the agent calls
    /// `send_message(to: "operator", ...)`. The operator clears it by clicking
    /// the agent in the launcher. There is no chat panel; the agent's actual
    /// message body lives in their tmux scrollback.
    #[serde(default)]
    pub attention: bool,
    /// Why the attention badge is lit (aperture-ull4y). `"message"` — the
    /// agent rang the operator doorbell (poller.rs mailbox sweep).
    /// `"crash"` — the watchdog latched red after exhausting re-kicks
    /// (watchdog.rs 3-strike latch). `None` whenever `attention` is false.
    /// The frontend renders a different badge per reason; `clear_attention`
    /// clears both fields together.
    #[serde(default)]
    pub attention_reason: Option<String>,
    /// Hub turn-state (aperture-ull4y): `"busy"` while the agent is mid-turn,
    /// `"idle"` between turns, `None` when unknown (agent offline, hub
    /// subscriber down, or no busy/idle frame seen since the last join).
    /// Sourced from the ws-hub `busy`/`idle` presence broadcasts via the
    /// watchdog subscriber — previously received and discarded. Carried on
    /// the existing 3s `list_agents` poll; there is deliberately NO frontend
    /// WebSocket (docs/presence-dots-spec.md, aperture-1iqpn).
    #[serde(default)]
    pub turn_state: Option<String>,
    /// Current-work summary line (aperture-nr65b). Resolved from BEADS on
    /// each `list_agents` poll — the top `in_progress` bead assigned to this
    /// agent, most-recently-claimed first. Three distinct states, all
    /// load-bearing for the frontend (see docs/presence-dots-spec.md):
    ///
    /// - `None` — no data available: agent is stopped, or the `bd` query
    ///   itself failed this cycle. Frontend renders nothing extra, exactly
    ///   as it did before this feature shipped.
    /// - `Some("")` (empty string sentinel) — query succeeded, agent has no
    ///   in_progress bead claimed. Frontend renders "idle."
    /// - `Some(id)` (non-empty) — the claimed bead's id; current_task_title
    ///   carries its title, current_task_extra_count the count of other
    ///   in_progress beads beyond this one.
    ///
    /// See agents.rs::resolve_current_tasks for how the distinction is made.
    #[serde(default)]
    pub current_task_id: Option<String>,
    #[serde(default)]
    pub current_task_title: Option<String>,
    /// Count of OTHER in_progress beads beyond the one shown (0 = just this
    /// one). `None` alongside `current_task_id: None` means "no data,"
    /// never "definitely zero."
    #[serde(default)]
    pub current_task_extra_count: Option<u32>,
    /// Presence-dot state (aperture-8gypy / aperture-wul6m). Backend-computed
    /// by the wul6m watchdog actor once it ships; absent today (safe no-op —
    /// frontend derives spawned/booting locally from kickoff_fired_at).
    #[serde(default)]
    pub dot_state: Option<String>,
    #[serde(default)]
    pub dot_state_since: Option<String>,
    #[serde(default)]
    pub kickoff_fired_at: Option<String>,
}

pub struct AppState {
    pub tmux_session: String,
    pub agents: HashMap<String, AgentDef>,
    pub mcp_server_path: String,
    /// Path to the Sentry MCP wrap server's compiled entrypoint
    /// (`mcp-server-sentry/dist/index.js`). Wired into each agent's MCP
    /// config alongside `aperture-bus` so agents see `mcp__sentry__*`
    /// tools. The wrap layer enforces Cipher's 9 constraints from
    /// aperture-ttzz (allowlist, audit emission, operator approval).
    pub mcp_sentry_server_path: String,
    /// Vestigial — kept so we don't have to thread a removal through
    /// `default_state`. Was used by an older message DB; today BEADS owns
    /// the durable message store (delivery via the aperture-bus WS hub).
    #[allow(dead_code)]
    pub db_path: String,
    pub project_dir: String,
}
