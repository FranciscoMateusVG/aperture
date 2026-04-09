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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpiderlingDef {
    pub name: String,
    pub task_id: String,
    pub tmux_window_id: Option<String>,
    pub worktree_path: String,
    pub worktree_branch: String,
    #[serde(default)]
    pub source_repo: Option<String>,
    pub requested_by: String,
    pub status: String,
    pub spawned_at: String,
}

pub struct AppState {
    pub tmux_session: String,
    pub agents: HashMap<String, AgentDef>,
    pub spiderlings: HashMap<String, SpiderlingDef>,
    pub mcp_server_path: String,
    pub db_path: String,
    pub project_dir: String,
}
