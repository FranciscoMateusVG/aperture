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
    /// Notification badge — set when the agent calls
    /// `send_message(to: "operator", ...)`. The operator clears it by clicking
    /// the agent in the launcher. There is no chat panel; the agent's actual
    /// message body lives in their tmux scrollback.
    #[serde(default)]
    pub attention: bool,
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
    /// `default_state`. Was used by an older message DB; today the message
    /// log is JSONL at `~/.aperture/message-log.jsonl` and BEADS owns the
    /// real durable store.
    #[allow(dead_code)]
    pub db_path: String,
    pub project_dir: String,
}
