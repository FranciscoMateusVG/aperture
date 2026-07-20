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
use std::path::Path;

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
    // APERTURE_AGENTS_DIR (aperture-syepg) overrides the registry root. The
    // boot-verification harness (aperture-xt16e) points this at a stub registry
    // for isolation; the node codex-bridge already honors the same var with the
    // same default. Ignored when unset/empty → real ~/.claude/aperture.
    if let Ok(dir) = std::env::var("APERTURE_AGENTS_DIR") {
        if !dir.is_empty() {
            return dir;
        }
    }
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

/// Link an agent's manifest-selected Aperture skills into its isolated Codex
/// home. Codex reads `$CODEX_HOME/skills`, while Claude Code reads the runtime
/// registry directly; keeping the same selected links in both places prevents
/// the injected prompt from being the only source of an agent's skills.
///
/// Only skills already linked through the registry's `shared/` directory are
/// accepted. That prevents a malformed per-agent runtime folder from causing
/// Codex to load an arbitrary directory as a skill.
pub fn populate_codex_skill_home(agent_name: &str, codex_home: &str) -> Result<usize, String> {
    let root = aperture_root();
    link_codex_skills(
        &Path::new(&root).join(agent_name).join("skills"),
        &Path::new(&root).join("shared"),
        &Path::new(codex_home).join("skills"),
    )
}

fn link_codex_skills(
    agent_skills_dir: &Path,
    shared_skills_dir: &Path,
    codex_skills_dir: &Path,
) -> Result<usize, String> {
    if !agent_skills_dir.exists() {
        return Ok(0);
    }

    fs::create_dir_all(codex_skills_dir).map_err(|e| {
        format!(
            "could not create Codex skill directory {}: {}",
            codex_skills_dir.display(),
            e
        )
    })?;

    let mut linked = 0;
    for entry in fs::read_dir(agent_skills_dir).map_err(|e| {
        format!(
            "could not read agent skill directory {}: {}",
            agent_skills_dir.display(),
            e
        )
    })? {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }

        let source = entry.path();
        if !fs::metadata(&source).map(|m| m.is_dir()).unwrap_or(false) {
            continue;
        }

        let shared_source = shared_skills_dir.join(&name);
        let resolved_source = fs::canonicalize(&source).map_err(|e| e.to_string())?;
        let resolved_shared = fs::canonicalize(&shared_source).map_err(|e| {
            format!(
                "agent skill '{}' is not present in shared registry {}: {}",
                name.to_string_lossy(),
                shared_skills_dir.display(),
                e
            )
        })?;
        // Equality with the canonical shared entry is the registry boundary:
        // runtime links may resolve onward to the repo's canonical skill body.
        if resolved_source != resolved_shared {
            return Err(format!(
                "refusing Codex skill '{}' outside the shared Aperture registry",
                name.to_string_lossy()
            ));
        }

        let destination = codex_skills_dir.join(&name);
        match fs::symlink_metadata(&destination) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                fs::remove_file(&destination).map_err(|e| e.to_string())?;
            }
            Ok(_) => {
                // Preserve Codex's built-in directories (for example `.system`)
                // and avoid replacing any non-Aperture content on a name clash.
                continue;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.to_string()),
        }

        #[cfg(unix)]
        std::os::unix::fs::symlink(&resolved_shared, &destination).map_err(|e| {
            format!(
                "could not link Codex skill {}: {}",
                destination.display(),
                e
            )
        })?;
        #[cfg(not(unix))]
        return Err("Codex skill linking requires a Unix filesystem".into());

        linked += 1;
    }

    Ok(linked)
}

#[cfg(test)]
mod tests {
    use super::link_codex_skills;
    use std::fs;
    use std::path::PathBuf;

    fn temp_dir(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "aperture-agent-loader-{}-{}",
            name,
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    #[cfg(unix)]
    fn codex_skill_home_links_only_registry_selected_skills() {
        let root = temp_dir("codex-skills");
        let shared = root.join("shared");
        let selected = root.join("rex/skills");
        let codex_skills = root.join("codex/skills");
        let skill = shared.join("beads");
        fs::create_dir_all(&skill).unwrap();
        fs::write(skill.join("SKILL.md"), "beads skill").unwrap();
        fs::create_dir_all(&selected).unwrap();
        std::os::unix::fs::symlink("../../shared/beads", selected.join("beads")).unwrap();

        // A built-in is not an Aperture skill and must survive the assembly.
        fs::create_dir_all(codex_skills.join(".system")).unwrap();

        assert_eq!(
            link_codex_skills(&selected, &shared, &codex_skills).unwrap(),
            1
        );
        assert!(codex_skills.join("beads").is_symlink());
        assert_eq!(
            fs::read_to_string(codex_skills.join("beads/SKILL.md")).unwrap(),
            "beads skill"
        );
        assert!(codex_skills.join(".system").is_dir());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    #[cfg(unix)]
    fn codex_skill_home_rejects_agent_skill_outside_shared_registry() {
        let root = temp_dir("codex-skills-reject");
        let shared = root.join("shared");
        let selected = root.join("rex/skills");
        let codex_skills = root.join("codex/skills");
        let untrusted = root.join("untrusted");
        fs::create_dir_all(&shared).unwrap();
        fs::create_dir_all(&selected).unwrap();
        fs::create_dir_all(&untrusted).unwrap();
        fs::write(untrusted.join("SKILL.md"), "not an Aperture skill").unwrap();
        std::os::unix::fs::symlink("../../untrusted", selected.join("malicious")).unwrap();

        let error = link_codex_skills(&selected, &shared, &codex_skills).unwrap_err();
        assert!(error.contains("not present in shared registry"));
        assert!(!codex_skills.join("malicious").exists());

        let _ = fs::remove_dir_all(root);
    }
}
