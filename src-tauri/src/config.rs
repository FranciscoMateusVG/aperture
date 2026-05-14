//! Boot-time agent state assembly.
//!
//! Agents are no longer hardcoded — they're loaded from `~/.claude/aperture/`
//! by `agent_loader::load_agents_from_disk()`. This file is now thin: it just
//! threads model overrides on top of whatever the disk says.

use crate::state::AppState;
use std::collections::HashMap;

pub fn load_agent_overrides(home: &str) -> HashMap<String, String> {
    let path = format!("{}/.aperture/agent-config.json", home);
    match std::fs::read_to_string(&path) {
        Ok(data) => serde_json::from_str::<HashMap<String, String>>(&data).unwrap_or_default(),
        Err(_) => HashMap::new(),
    }
}

pub fn save_agent_override(home: &str, name: &str, model: &str) {
    let path = format!("{}/.aperture/agent-config.json", home);
    let mut overrides = load_agent_overrides(home);
    overrides.insert(name.to_string(), model.to_string());
    if let Ok(json) = serde_json::to_string_pretty(&overrides) {
        let _ = std::fs::create_dir_all(format!("{}/.aperture", home));
        let _ = std::fs::write(&path, json);
    }
}

pub fn default_state() -> AppState {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    // CARGO_MANIFEST_DIR is src-tauri/, so parent is the project root.
    let project_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| format!("{}/projects/aperture", home));

    let mut agents = crate::agent_loader::load_agents_from_disk();
    if agents.is_empty() {
        eprintln!(
            "[aperture] WARNING: no agents loaded from ~/.claude/aperture/ — \
             run `just setup` from the project root to build the runtime tree."
        );
    }

    // Apply persisted model overrides (set via the launcher's model picker;
    // these survive across rebuilds and live at ~/.aperture/agent-config.json).
    let overrides = load_agent_overrides(&home);
    for (name, model) in &overrides {
        if let Some(agent) = agents.get_mut(name) {
            agent.model = model.clone();
        }
    }

    AppState {
        tmux_session: "aperture".into(),
        agents,
        mcp_server_path: format!("{}/mcp-server/dist/index.js", project_dir),
        mcp_sentry_server_path: format!("{}/mcp-server-sentry/dist/index.js", project_dir),
        db_path: format!("{}/.aperture/messages.db", home),
        project_dir,
    }
}
