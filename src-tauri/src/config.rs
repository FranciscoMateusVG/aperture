use crate::state::{AgentDef, AppState, SpiderlingDef};
use std::collections::HashMap;

pub fn default_agents(project_dir: &str) -> HashMap<String, AgentDef> {
    let mut agents = HashMap::new();
    agents.insert(
        "glados".into(),
        AgentDef {
            name: "glados".into(),
            model: "opus".into(),
            role: "orchestrator".into(),
            prompt_file: format!("{}/prompts/glados.md", project_dir),
            tmux_window_id: None,
            status: "stopped".into(),
        },
    );
    agents.insert(
        "wheatley".into(),
        AgentDef {
            name: "wheatley".into(),
            model: "sonnet".into(),
            role: "worker".into(),
            prompt_file: format!("{}/prompts/wheatley.md", project_dir),
            tmux_window_id: None,
            status: "stopped".into(),
        },
    );
    agents.insert(
        "peppy".into(),
        AgentDef {
            name: "peppy".into(),
            model: "opus".into(),
            role: "infra".into(),
            prompt_file: format!("{}/prompts/peppy.md", project_dir),
            tmux_window_id: None,
            status: "stopped".into(),
        },
    );
    agents.insert(
        "izzy".into(),
        AgentDef {
            name: "izzy".into(),
            model: "opus".into(),
            role: "testing".into(),
            prompt_file: format!("{}/prompts/izzy.md", project_dir),
            tmux_window_id: None,
            status: "stopped".into(),
        },
    );
    agents.insert(
        "vance".into(),
        AgentDef {
            name: "vance".into(),
            model: "opus".into(),
            role: "design".into(),
            prompt_file: format!("{}/prompts/vance.md", project_dir),
            tmux_window_id: None,
            status: "stopped".into(),
        },
    );
    agents.insert(
        "rex".into(),
        AgentDef {
            name: "rex".into(),
            model: "opus".into(),
            role: "backend".into(),
            prompt_file: format!("{}/prompts/rex.md", project_dir),
            tmux_window_id: None,
            status: "stopped".into(),
        },
    );
    agents.insert(
        "scout".into(),
        AgentDef {
            name: "scout".into(),
            model: "opus".into(),
            role: "mobile".into(),
            prompt_file: format!("{}/prompts/scout.md", project_dir),
            tmux_window_id: None,
            status: "stopped".into(),
        },
    );
    agents.insert(
        "cipher".into(),
        AgentDef {
            name: "cipher".into(),
            model: "opus".into(),
            role: "security".into(),
            prompt_file: format!("{}/prompts/cipher.md", project_dir),
            tmux_window_id: None,
            status: "stopped".into(),
        },
    );
    agents.insert(
        "sage".into(),
        AgentDef {
            name: "sage".into(),
            model: "opus".into(),
            role: "growth".into(),
            prompt_file: format!("{}/prompts/sage.md", project_dir),
            tmux_window_id: None,
            status: "stopped".into(),
        },
    );
    agents.insert(
        "atlas".into(),
        AgentDef {
            name: "atlas".into(),
            model: "opus".into(),
            role: "documentation".into(),
            prompt_file: format!("{}/prompts/atlas.md", project_dir),
            tmux_window_id: None,
            status: "stopped".into(),
        },
    );
    agents.insert(
        "sentinel".into(),
        AgentDef {
            name: "sentinel".into(),
            model: "opus".into(),
            role: "overseer".into(),
            prompt_file: format!("{}/prompts/sentinel.md", project_dir),
            tmux_window_id: None,
            status: "stopped".into(),
        },
    );
    agents.insert(
        "sterling".into(),
        AgentDef {
            name: "sterling".into(),
            model: "opus".into(),
            role: "quality".into(),
            prompt_file: format!("{}/prompts/sterling.md", project_dir),
            tmux_window_id: None,
            status: "stopped".into(),
        },
    );
    agents.insert(
        "planner".into(),
        AgentDef {
            name: "planner".into(),
            model: "opus".into(),
            role: "director".into(),
            prompt_file: format!("{}/prompts/planner.md", project_dir),
            tmux_window_id: None,
            status: "stopped".into(),
        },
    );
    agents
}

fn load_spiderlings(home: &str) -> HashMap<String, SpiderlingDef> {
    let path = format!("{}/.aperture/active-spiderlings.json", home);
    match std::fs::read_to_string(&path) {
        Ok(data) => {
            let list: Vec<SpiderlingDef> = serde_json::from_str(&data).unwrap_or_default();
            list.into_iter().map(|s| (s.name.clone(), s)).collect()
        }
        Err(_) => HashMap::new(),
    }
}

pub fn default_state() -> AppState {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let project_dir = format!("{}/projects/aperture", home);
    AppState {
        tmux_session: "aperture".into(),
        agents: default_agents(&project_dir),
        spiderlings: load_spiderlings(&home),
        mcp_server_path: format!("{}/mcp-server/dist/index.js", project_dir),
        db_path: format!("{}/.aperture/messages.db", home),
        project_dir,
    }
}
