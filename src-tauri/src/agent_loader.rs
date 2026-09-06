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
use std::collections::{HashMap, HashSet};
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
                attention_reason: None,
                turn_state: None,
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

/// Read the optional resident-skill list at
/// `~/.claude/aperture/<agent>/resident.txt` (aperture-i7bg0). One skill name
/// per line; `#` comments and blank lines are stripped — the same line
/// convention `just setup` uses for the repo's `skills.txt`. The runtime copy
/// is a symlink created by `just setup` from `agents/<name>/resident.txt`.
///
/// Returns `None` when the file does not exist. Callers treat that as "no
/// resident/lazy split configured" and keep injecting every skill body, so
/// the rollout is opt-in per agent with zero behavior change for agents
/// without the file. Note `Some(vec![])` (a file of only comments/blanks) is
/// distinct: it means "inject no skill bodies at all".
pub fn load_agent_resident_list(agent_name: &str) -> Option<Vec<String>> {
    let path = format!("{}/{}/resident.txt", aperture_root(), agent_name);
    read_resident_list(Path::new(&path))
}

fn read_resident_list(path: &Path) -> Option<Vec<String>> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            // Present-but-unreadable degrades to "no split configured": a
            // permissions hiccup must widen injection back to all skills,
            // never silently strip an agent down to zero.
            eprintln!(
                "[aperture] warning: could not read {}: {} — injecting all skills",
                path.display(),
                e
            );
            return None;
        }
    };
    Some(parse_skill_lines(&text))
}

/// Shared line convention for `skills.txt` / `resident.txt`: strip `#`
/// comments (inline or full-line), trim whitespace, drop blank lines.
fn parse_skill_lines(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let line = line.split('#').next().unwrap_or("").trim();
            (!line.is_empty()).then(|| line.to_string())
        })
        .collect()
}

/// Link an agent's manifest-selected Aperture skills into its isolated Codex
/// home. Codex reads `$CODEX_HOME/skills`, while Claude Code reads the runtime
/// registry directly; keeping the same selected links in both places prevents
/// the injected prompt from being the only source of an agent's skills.
///
/// This directory also feeds Codex's native progressive-disclosure catalog:
/// at session start Codex injects only a `## Skills` index (name +
/// description + SKILL.md path) and reads full bodies lazily on demand. That
/// is what lets `inject_codex_skills` (agents.rs) trim prompt.md to the
/// resident subset from `resident.txt` — every non-resident skill stays
/// reachable through the links made here, ALWAYS the full `skills.txt` set.
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
    if let Err(e) = fs::create_dir_all(codex_skills_dir) {
        eprintln!(
            "[aperture] warning: could not create Codex skill directory {}: {}",
            codex_skills_dir.display(),
            e
        );
        return Ok(0);
    }

    let mut linked = 0;
    let mut active_links = HashSet::new();
    let entries: Vec<_> = match fs::read_dir(agent_skills_dir) {
        Ok(entries) => entries.collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => {
            eprintln!(
                "[aperture] warning: could not read agent skill directory {}: {}",
                agent_skills_dir.display(),
                e
            );
            Vec::new()
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                eprintln!("[aperture] warning: skipping unreadable skill entry: {}", e);
                continue;
            }
        };
        let name = entry.file_name();
        let display_name = name.to_string_lossy();
        if display_name.starts_with('.') {
            continue;
        }

        let source = entry.path();
        match fs::metadata(&source) {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => {
                eprintln!(
                    "[aperture] warning: skipping Codex skill '{}': selected entry is not a directory",
                    display_name
                );
                continue;
            }
            Err(e) => {
                eprintln!(
                    "[aperture] warning: skipping Codex skill '{}': cannot resolve selected entry: {}",
                    display_name, e
                );
                continue;
            }
        }

        let shared_source = shared_skills_dir.join(&name);
        let resolved_source = match fs::canonicalize(&source) {
            Ok(path) => path,
            Err(e) => {
                eprintln!(
                    "[aperture] warning: skipping Codex skill '{}': cannot canonicalize selected entry: {}",
                    display_name, e
                );
                continue;
            }
        };
        let resolved_shared = match fs::canonicalize(&shared_source) {
            Ok(path) => path,
            Err(e) => {
                eprintln!(
                    "[aperture] warning: skipping Codex skill '{}': not present in shared registry {}: {}",
                    display_name,
                    shared_skills_dir.display(),
                    e
                );
                continue;
            }
        };
        // Equality with the canonical shared entry is the registry boundary:
        // runtime links may resolve onward to the repo's canonical skill body.
        if resolved_source != resolved_shared {
            eprintln!(
                "[aperture] warning: skipping Codex skill '{}': selected entry resolves outside the shared Aperture registry",
                display_name
            );
            continue;
        }

        let destination = codex_skills_dir.join(&name);
        match fs::symlink_metadata(&destination) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                if let Err(e) = fs::remove_file(&destination) {
                    eprintln!(
                        "[aperture] warning: skipping Codex skill '{}': cannot replace existing link: {}",
                        display_name, e
                    );
                    continue;
                }
            }
            Ok(_) => {
                // Preserve Codex's built-in directories (for example `.system`)
                // and avoid replacing any non-Aperture content on a name clash.
                eprintln!(
                    "[aperture] warning: skipping Codex skill '{}': destination {} is non-symlink content",
                    display_name,
                    destination.display()
                );
                continue;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                eprintln!(
                    "[aperture] warning: skipping Codex skill '{}': cannot inspect destination: {}",
                    display_name, e
                );
                continue;
            }
        }

        // Link to the registry path (shared/<name>), NOT the canonicalized
        // repo body: (a) re-pointing shared/ after a repo move heals live
        // agents at access time instead of leaving baked-path danglers, and
        // (b) the reconciliation sweep below can identify Aperture-owned
        // links by their textual parent even when the target is dangling.
        #[cfg(unix)]
        if let Err(e) = std::os::unix::fs::symlink(&shared_source, &destination) {
            eprintln!(
                "[aperture] warning: skipping Codex skill '{}': could not create link {}: {}",
                display_name,
                destination.display(),
                e
            );
            continue;
        }
        #[cfg(not(unix))]
        {
            eprintln!(
                "[aperture] warning: skipping Codex skill '{}': native skill linking requires a Unix filesystem",
                display_name
            );
            continue;
        }

        active_links.insert(name);
        linked += 1;
    }

    // Reconcile revocations on every launch. Only remove links that Aperture
    // can prove it owns: their target is inside the shared registry. Codex
    // built-ins and user-installed links pointing elsewhere are preserved.
    let shared_root = fs::canonicalize(shared_skills_dir).ok();
    if let Ok(destinations) = fs::read_dir(codex_skills_dir) {
        for destination in destinations.flatten() {
            let name = destination.file_name();
            if active_links.contains(&name) {
                continue;
            }
            let path = destination.path();
            let Ok(metadata) = fs::symlink_metadata(&path) else {
                continue;
            };
            if !metadata.file_type().is_symlink() {
                continue;
            }
            let owned = fs::read_link(&path)
                .ok()
                .map(|target| {
                    let absolute_target = if target.is_absolute() {
                        target
                    } else {
                        codex_skills_dir.join(target)
                    };
                    // Aperture-created links target shared/<name> verbatim, so
                    // the textual parent match identifies them even when the
                    // target dangles. The canonical-prefix check additionally
                    // catches any link resolving inside the shared registry.
                    absolute_target.parent() == Some(shared_skills_dir)
                        || shared_root
                            .as_ref()
                            .map(|root| absolute_target.starts_with(root))
                            .unwrap_or(false)
                })
                .unwrap_or(false);
            if owned {
                match fs::remove_file(&path) {
                    Ok(()) => eprintln!(
                        "[aperture] removed revoked Codex skill link '{}'",
                        name.to_string_lossy()
                    ),
                    Err(e) => eprintln!(
                        "[aperture] warning: could not remove revoked Codex skill link '{}': {}",
                        name.to_string_lossy(),
                        e
                    ),
                }
            }
        }
    }

    Ok(linked)
}

#[cfg(test)]
mod tests {
    use super::{link_codex_skills, parse_skill_lines, read_resident_list};
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
    fn resident_list_absent_file_returns_none() {
        let root = temp_dir("resident-absent");
        assert!(read_resident_list(&root.join("resident.txt")).is_none());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn resident_list_strips_comments_and_blank_lines() {
        let root = temp_dir("resident-parse");
        let path = root.join("resident.txt");
        fs::write(
            &path,
            "# resident core (aperture-i7bg0)\ncommunicate\n\nbeads   # always-active\n   team\n#\n",
        )
        .unwrap();
        assert_eq!(
            read_resident_list(&path).unwrap(),
            vec!["communicate", "beads", "team"]
        );
        let _ = fs::remove_dir_all(root);
    }

    /// A present file that parses to nothing is Some(empty), NOT None —
    /// "inject no skill bodies" is a valid configuration, distinct from
    /// "no split configured".
    #[test]
    fn resident_list_comments_only_file_is_some_empty() {
        let root = temp_dir("resident-empty");
        let path = root.join("resident.txt");
        fs::write(&path, "# nothing resident yet\n\n").unwrap();
        assert_eq!(read_resident_list(&path).unwrap(), Vec::<String>::new());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn skill_line_convention_matches_justfile_skills_txt_parsing() {
        // Mirrors `sed 's/#.*//' | xargs` + skip-empty from the `just setup`
        // recipe: inline comments cut, surrounding whitespace trimmed.
        assert_eq!(
            parse_skill_lines("beads # discipline\n  worktree-discipline  \n\n# all comment\n"),
            vec!["beads", "worktree-discipline"]
        );
        assert_eq!(parse_skill_lines(""), Vec::<String>::new());
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

        assert_eq!(
            link_codex_skills(&selected, &shared, &codex_skills).unwrap(),
            0
        );
        assert!(!codex_skills.join("malicious").exists());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    #[cfg(unix)]
    fn codex_skill_home_skips_real_directory_and_preserves_boot_harness_shape() {
        let root = temp_dir("codex-skills-real-dir");
        let shared = root.join("shared");
        let selected = root.join("rex/skills");
        let codex_skills = root.join("codex/skills");
        fs::create_dir_all(selected.join("smoke")).unwrap();
        fs::write(selected.join("smoke/SKILL.md"), "harness skill").unwrap();

        assert_eq!(
            link_codex_skills(&selected, &shared, &codex_skills).unwrap(),
            0
        );
        assert!(!codex_skills.join("smoke").exists());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    #[cfg(unix)]
    fn codex_skill_home_removes_revoked_owned_links_only() {
        let root = temp_dir("codex-skills-revoke");
        let shared = root.join("shared");
        let selected = root.join("rex/skills");
        let codex_skills = root.join("codex/skills");
        let retained = shared.join("retained");
        let revoked = shared.join("revoked");
        let external = root.join("external");
        for skill in [&retained, &revoked, &external] {
            fs::create_dir_all(skill).unwrap();
            fs::write(skill.join("SKILL.md"), "skill").unwrap();
        }
        fs::create_dir_all(&selected).unwrap();
        fs::create_dir_all(&codex_skills).unwrap();
        std::os::unix::fs::symlink("../../shared/retained", selected.join("retained")).unwrap();
        std::os::unix::fs::symlink(fs::canonicalize(&revoked).unwrap(), codex_skills.join("revoked"))
            .unwrap();
        std::os::unix::fs::symlink(fs::canonicalize(&external).unwrap(), codex_skills.join("external"))
            .unwrap();

        assert_eq!(
            link_codex_skills(&selected, &shared, &codex_skills).unwrap(),
            1
        );
        assert!(codex_skills.join("retained").is_symlink());
        assert!(!codex_skills.join("revoked").exists());
        assert!(codex_skills.join("external").is_symlink());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    #[cfg(unix)]
    fn codex_skill_home_preserves_non_symlink_destination_clash() {
        let root = temp_dir("codex-skills-clash");
        let shared = root.join("shared");
        let selected = root.join("rex/skills");
        let codex_skills = root.join("codex/skills");
        fs::create_dir_all(shared.join("beads")).unwrap();
        fs::write(shared.join("beads/SKILL.md"), "beads skill").unwrap();
        fs::create_dir_all(&selected).unwrap();
        std::os::unix::fs::symlink("../../shared/beads", selected.join("beads")).unwrap();
        fs::create_dir_all(codex_skills.join("beads")).unwrap();
        fs::write(codex_skills.join("beads/local.txt"), "keep").unwrap();

        assert_eq!(
            link_codex_skills(&selected, &shared, &codex_skills).unwrap(),
            0
        );
        assert_eq!(
            fs::read_to_string(codex_skills.join("beads/local.txt")).unwrap(),
            "keep"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    #[cfg(unix)]
    fn codex_skill_home_destination_setup_failure_is_non_fatal() {
        let root = temp_dir("codex-skills-destination-file");
        let shared = root.join("shared");
        let selected = root.join("rex/skills");
        let codex_skills = root.join("codex/skills");
        fs::create_dir_all(shared.join("beads")).unwrap();
        fs::create_dir_all(&selected).unwrap();
        std::os::unix::fs::symlink("../../shared/beads", selected.join("beads")).unwrap();
        fs::create_dir_all(root.join("codex")).unwrap();
        fs::write(&codex_skills, "not a directory").unwrap();

        assert_eq!(
            link_codex_skills(&selected, &shared, &codex_skills).unwrap(),
            0
        );
        assert_eq!(fs::read_to_string(&codex_skills).unwrap(), "not a directory");

        let _ = fs::remove_dir_all(root);
    }

    /// Production-shape regression: in the real runtime tree, shared/<name>
    /// is itself a symlink onward to the repo's skill body. The revocation
    /// sweep must recognize links created through that shape as
    /// Aperture-owned — canonical-prefix alone fails there because the
    /// created link's canonical target lives under the repo, not shared/.
    #[test]
    #[cfg(unix)]
    fn codex_skill_home_revokes_links_it_created_through_symlinked_registry() {
        let root = temp_dir("codex-skills-symlinked-registry");
        let repo_skill = root.join("repo-skills/beads");
        let shared = root.join("shared");
        let selected = root.join("rex/skills");
        let codex_skills = root.join("codex/skills");
        fs::create_dir_all(&repo_skill).unwrap();
        fs::write(repo_skill.join("SKILL.md"), "beads body").unwrap();
        fs::create_dir_all(&shared).unwrap();
        std::os::unix::fs::symlink(&repo_skill, shared.join("beads")).unwrap();
        fs::create_dir_all(&selected).unwrap();
        std::os::unix::fs::symlink("../../shared/beads", selected.join("beads")).unwrap();

        // Launch 1: selected -> linked, readable through the chain.
        assert_eq!(
            link_codex_skills(&selected, &shared, &codex_skills).unwrap(),
            1
        );
        assert!(codex_skills.join("beads").is_symlink());
        assert_eq!(
            fs::read_to_string(codex_skills.join("beads/SKILL.md")).unwrap(),
            "beads body"
        );

        // Revocation: selection removed. Launch 2 must remove the link this
        // function created on launch 1.
        fs::remove_file(selected.join("beads")).unwrap();
        assert_eq!(
            link_codex_skills(&selected, &shared, &codex_skills).unwrap(),
            0
        );
        assert!(!codex_skills.join("beads").exists());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    #[cfg(unix)]
    fn codex_skill_home_removes_all_owned_links_when_selection_disappears() {
        let root = temp_dir("codex-skills-empty-selection");
        let shared = root.join("shared");
        let selected = root.join("rex/skills");
        let codex_skills = root.join("codex/skills");
        let revoked = shared.join("revoked");
        fs::create_dir_all(&revoked).unwrap();
        fs::create_dir_all(&codex_skills).unwrap();
        std::os::unix::fs::symlink(fs::canonicalize(&revoked).unwrap(), codex_skills.join("revoked"))
            .unwrap();

        assert_eq!(
            link_codex_skills(&selected, &shared, &codex_skills).unwrap(),
            0
        );
        assert!(!codex_skills.join("revoked").exists());

        let _ = fs::remove_dir_all(root);
    }
}
