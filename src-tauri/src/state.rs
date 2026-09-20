use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use crate::team_replacement::native::NativePreparedReplacement;

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
    /// Opaque, launcher-local human replacement permits. The permit itself is
    /// deliberately non-serializable and is removed before Start invokes the
    /// consuming native seam. Keeping this behind its own mutex avoids holding
    /// the global AppState lock across the bounded lifecycle operation.
    pub team_preparations: Arc<Mutex<RuntimePermitStore>>,
}

const MAX_RUNTIME_PERMITS: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct PermitKey {
    team: String,
    seat: String,
    generation: u64,
}

/// Process-local storage only. A lost map never reconstructs authority from
/// disk; callers must run a fresh native Prepare, which revalidates the durable
/// Ready evidence before issuing a replacement opaque id.
pub struct RuntimePermitStore {
    permits: HashMap<String, NativePreparedReplacement>,
    reservations: HashSet<PermitKey>,
}

impl RuntimePermitStore {
    pub fn new() -> Self {
        Self { permits: HashMap::new(), reservations: HashSet::new() }
    }

    pub(crate) fn reserve(&mut self, team: &str, seat: &str, generation: u64) -> Result<(), String> {
        let key = PermitKey { team: team.into(), seat: seat.into(), generation };
        if self.reservations.contains(&key) {
            return Err("E_STATE_CONFLICT: replacement preparation is already running".into());
        }
        let replacing_existing = self.permits.values().any(|permit| {
            permit.team() == team && permit.seat() == seat && permit.generation() == generation
        });
        if !replacing_existing && self.permits.len() + self.reservations.len() >= MAX_RUNTIME_PERMITS {
            return Err("E_RUNTIME_UNAVAILABLE: replacement permit capacity is exhausted".into());
        }
        self.reservations.insert(key);
        Ok(())
    }

    pub(crate) fn cancel_reservation(&mut self, team: &str, seat: &str, generation: u64) {
        self.reservations.remove(&PermitKey { team: team.into(), seat: seat.into(), generation });
    }

    pub(crate) fn publish(
        &mut self,
        id: String,
        permit: NativePreparedReplacement,
    ) -> Result<(), String> {
        let key = PermitKey {
            team: permit.team().into(),
            seat: permit.seat().into(),
            generation: permit.generation(),
        };
        if !self.reservations.remove(&key) || self.permits.contains_key(&id) {
            return Err("E_STATE_CONFLICT: replacement permit reservation changed".into());
        }
        self.permits.retain(|_, existing| {
            existing.team() != key.team
                || existing.seat() != key.seat
                || existing.generation() != key.generation
        });
        self.permits.insert(id, permit);
        Ok(())
    }

    /// Validate all selectors before consuming. A mismatched request cannot
    /// destroy the legitimate opaque permit.
    pub(crate) fn take(
        &mut self,
        id: &str,
        team: &str,
        seat: &str,
        generation: u64,
    ) -> Result<NativePreparedReplacement, String> {
        let Some(permit) = self.permits.get(id) else {
            return Err("E_PREPARATION_EXPIRED: replacement preparation is unavailable".into());
        };
        if permit.team() != team || permit.seat() != seat || permit.generation() != generation {
            return Err("E_PREPARATION_EXPIRED: replacement preparation selectors changed".into());
        }
        self.permits.remove(id).ok_or_else(|| {
            "E_PREPARATION_EXPIRED: replacement preparation is unavailable".into()
        })
    }
}

// Aperture V4 shared execution/ownership DTOs. These are serialized to the UI,
// but authoritative owner records (pid/token/thread/nonce) remain private to
// owner.rs and are projected through OwnerSummary only.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Harness {
    Claude,
    Codex,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningEffort {
    Low,
    Medium,
    High,
    Xhigh,
    Max,
    Ultra,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExecutionTuple {
    pub harness: Harness,
    pub model: String,
    pub reasoning: Option<ReasoningEffort>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OwnerState {
    Starting,
    Active,
    Stale,
    Quarantined,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OwnerSummary {
    pub generation: u64,
    pub state: OwnerState,
    pub since: String,
    pub configured: ExecutionTuple,
    pub actual: Option<ExecutionTuple>,
    pub process_count: u32,
    pub thread_bound: bool,
}

#[cfg(test)]
mod runtime_permit_store_tests {
    use super::*;

    #[test]
    fn reservations_are_bounded_and_same_target_is_single_flight() {
        let mut store = RuntimePermitStore::new();
        store.reserve("t1", "t1-backend", 1).unwrap();
        assert!(store.reserve("t1", "t1-backend", 1).unwrap_err().starts_with("E_STATE_CONFLICT"));
        store.cancel_reservation("t1", "t1-backend", 1);
        store.reserve("t1", "t1-backend", 1).unwrap();
        store.cancel_reservation("t1", "t1-backend", 1);

        for generation in 1..=MAX_RUNTIME_PERMITS as u64 {
            store.reserve("t2", &format!("t2-seat-{generation}"), generation).unwrap();
        }
        assert!(store.reserve("t3", "t3-backend", 1).unwrap_err().starts_with("E_RUNTIME_UNAVAILABLE"));
    }
}
