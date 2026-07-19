//! Loads agent definitions from the runtime folder tree at
//! `~/.claude/aperture/<agent>/`. Each agent dir contains:
//!   - `manifest.json` — metadata (name, emoji, model, window, role, kind, enabled)
//!   - `prompt.md`     — the system prompt (typically a symlink into the repo)
//!   - `skills/`       — directory of skill subdirs (typically symlinks into shared/)
//!
//! This module replaces the old hardcoded `default_agents()` table in `config.rs`
//! and the `~/.claude/agents/<agent>/skills.txt` manifest file. Agents are pure
//! data now — adding/disabling one requires no Rust recompile.
//!
//! The repo holds canonical sources at `agents/<name>/{manifest.json,skills.txt}`
//! and `prompts/<name>.md` and `.claude/skills/<skill>/`. `just setup` rebuilds
//! the runtime tree from those sources via symlinks.

use crate::state::AgentDef;
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;

/// Per-agent metadata loaded from `~/.claude/aperture/<agent>/manifest.json`.
///
/// The fields tagged `#[allow(dead_code)]` are not yet read by the runtime but
/// are validated at parse time — serde will reject a manifest that's missing
/// `model`, `window`, or `role`. They're kept on the struct so adding UI
/// features (per-agent emoji, alternate tmux window names, explicit codex
/// kind switching) doesn't require a schema change.
#[derive(Debug, Deserialize)]
pub struct AgentManifest {
    /// Display name (e.g. "GLaDOS"). The directory name is the canonical key.
    #[allow(dead_code)]
    pub name: String,
    #[serde(default)]
    #[allow(dead_code)]
    pub emoji: String,
    pub model: String,
    #[allow(dead_code)]
    pub window: String,
    pub role: String,
    #[serde(default = "default_kind")]
    #[allow(dead_code)]
    pub kind: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_kind() -> String {
    "claude-code".into()
}
fn default_enabled() -> bool {
    true
}

fn aperture_root() -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    format!("{}/.claude/aperture", home)
}

/// Scan `~/.claude/aperture/` for agent directories and parse each manifest.
/// Skips `shared/` and any directory missing manifest.json or prompt.md, with
/// a warning to stderr. Disabled agents (`"enabled": false`) are excluded.
pub fn load_agents_from_disk() -> HashMap<String, AgentDef> {
    let root = aperture_root();
    let mut agents = HashMap::new();

    let entries = match fs::read_dir(&root) {
        Ok(e) => e,
        Err(e) => {
            eprintln!(
                "[aperture] could not read {}: {} — did you run `just setup`?",
                root, e
            );
            return agents;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let dir_name = entry.file_name().to_string_lossy().to_string();
        // Reserved names: shared/ holds skill symlinks, _* are scratch dirs.
        if dir_name == "shared" || dir_name.starts_with('_') {
            continue;
        }

        let manifest_path = path.join("manifest.json");
        let prompt_path = path.join("prompt.md");

        if !manifest_path.exists() {
            eprintln!(
                "[aperture] skipping '{}': missing manifest.json",
                dir_name
            );
            continue;
        }
        if !prompt_path.exists() {
            eprintln!("[aperture] skipping '{}': missing prompt.md", dir_name);
            continue;
        }

        let manifest_text = match fs::read_to_string(&manifest_path) {
            Ok(t) => t,
            Err(e) => {
                eprintln!(
                    "[aperture] could not read {}: {}",
                    manifest_path.display(),
                    e
                );
                continue;
            }
        };
        let manifest: AgentManifest = match serde_json::from_str(&manifest_text) {
            Ok(m) => m,
            Err(e) => {
                eprintln!(
                    "[aperture] invalid manifest at {}: {}",
                    manifest_path.display(),
                    e
                );
                continue;
            }
        };

        if !manifest.enabled {
            continue;
        }

        // The directory name is the canonical lowercase key used everywhere
        // (tmux window targeting, BEADS, message routing). The display name
        // in manifest.json is currently informational; the launcher renders
        // it via the frontend if/when it wants pretty labels.
        let key = dir_name.to_lowercase();
        agents.insert(
            key.clone(),
            AgentDef {
                name: key,
                model: manifest.model,
                role: manifest.role,
                prompt_file: prompt_path.to_string_lossy().to_string(),
                tmux_window_id: None,
                status: "stopped".into(),
                attention: false,
                current_task_id: None,
                current_task_title: None,
                current_task_extra_count: None,
                dot_state: None,
                dot_state_since: None,
                kickoff_fired_at: None,
            },
        );
    }

    agents
}

/// Return (skill_name, skill_content) pairs for an agent, in deterministic
/// alphabetical order. Each entry under `<agent>/skills/` is expected to be
/// a directory containing a `SKILL.md` (or `skill.md`) file — typically a
/// symlink into `shared/`.
pub fn load_agent_skills(agent_name: &str) -> Vec<(String, String)> {
    let skills_dir = format!("{}/{}/skills", aperture_root(), agent_name);

    let mut skills: Vec<(String, String)> = Vec::new();
    let entries = match fs::read_dir(&skills_dir) {
        Ok(e) => e,
        Err(_) => return skills, // no skills dir is fine
    };

    for entry in entries.flatten() {
        let path = entry.path();
        // Resolve symlink targets implicitly via fs::metadata (follows links).
        let is_dir = fs::metadata(&path).map(|m| m.is_dir()).unwrap_or(false);
        if !is_dir {
            continue;
        }
        let skill_md = ["SKILL.md", "skill.md"]
            .iter()
            .map(|n| path.join(n))
            .find(|p| p.exists());
        let Some(skill_md) = skill_md else { continue };
        let skill_name = entry.file_name().to_string_lossy().to_string();
        match fs::read_to_string(&skill_md) {
            Ok(content) => skills.push((skill_name, content)),
            Err(e) => eprintln!(
                "[aperture] could not read skill {}: {}",
                skill_md.display(),
                e
            ),
        }
    }

    skills.sort_by(|a, b| a.0.cmp(&b.0));
    skills
}
