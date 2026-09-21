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
use std::os::unix::fs::MetadataExt;
use std::path::Path;

/// Per-agent metadata loaded from `~/.claude/aperture/<agent>/manifest.json`.
///
/// The fields tagged `#[allow(dead_code)]` are not yet read by the runtime but
/// are validated at parse time — serde will reject a manifest that's missing
/// `model`, `window`, or `role`. They're kept on the struct so adding UI
/// features (alternate tmux window names, explicit codex kind switching)
/// doesn't require a schema change. `emoji` is passed through to the
/// launcher card (aperture-84bby).
#[derive(Debug, Deserialize)]
pub struct AgentManifest {
    /// Display name (e.g. "GLaDOS"). The directory name is the canonical key.
    #[allow(dead_code)]
    pub name: String,
    #[serde(default)]
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

fn is_reserved_seat_principal(name: &str) -> bool {
    matches!(name, "operator" | "watchdog")
}

fn is_coordination_trio(name: &str) -> bool {
    matches!(name, "glados" | "wheatley" | "peppy")
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

fn teams_root() -> String {
    if let Ok(dir) = std::env::var("APERTURE_TEAMS_DIR") {
        if !dir.is_empty() {
            return dir;
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    format!("{}/.aperture/teams", home)
}

/// Exact parity with the TS/hub/UI rule in §4.1. ASCII is intentional: these
/// ids become filenames, tmux windows and unix-socket stems.
pub fn is_valid_seat_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    if bytes.is_empty() || bytes.len() > 31 {
        return false;
    }
    bytes.iter().enumerate().all(|(index, byte)| {
        byte.is_ascii_lowercase()
            || byte.is_ascii_digit()
            || (index > 0 && (*byte == b'_' || *byte == b'-'))
    })
}

fn is_valid_project_label(project: &str) -> bool {
    let Some(name) = project.strip_prefix("project:") else {
        return false;
    };
    let bytes = name.as_bytes();
    !bytes.is_empty()
        && bytes.iter().enumerate().all(|(index, byte)| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || (index > 0 && (*byte == b'_' || *byte == b'-'))
        })
}

#[derive(Debug, Deserialize)]
struct TeamState {
    state: String,
    generation: u64,
}

#[derive(Debug, Deserialize)]
struct TeamSeat {
    name: String,
    role: String,
}

#[derive(Debug, Deserialize)]
struct TeamGrant {
    from: String,
    to: String,
    scope: String,
    by: String,
    at: String,
}

#[derive(Debug, Deserialize)]
struct TeamSnapshot {
    team: String,
    project: String,
    repo: String,
    lead: String,
    seats: Vec<TeamSeat>,
    #[serde(default)]
    grants: Vec<TeamGrant>,
}

fn is_real_file(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|meta| meta.is_file() && !meta.file_type().is_symlink())
        .unwrap_or(false)
}

fn is_real_dir(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|meta| meta.is_dir() && !meta.file_type().is_symlink())
        .unwrap_or(false)
}

fn path_entry_exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

fn archive_journal_blocks(teams_root: &Path, team: &str) -> bool {
    let Some(aperture_root) = teams_root.parent() else {
        return true;
    };
    let journal_root = aperture_root.join("run/team-journals");
    match fs::symlink_metadata(&journal_root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Ok(metadata)
            if metadata.is_dir()
                && !metadata.file_type().is_symlink()
                && metadata.uid() == unsafe { libc::geteuid() }
                && metadata.mode() & 0o077 == 0 =>
        {
            match fs::symlink_metadata(journal_root.join(format!("{team}.archive.json"))) {
                Ok(_) => true,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
                Err(_) => true,
            }
        }
        _ => true,
    }
}

fn path_inside(root: &Path, path: &Path) -> bool {
    match (fs::canonicalize(root), fs::canonicalize(path)) {
        (Ok(root), Ok(path)) => path.starts_with(root),
        _ => false,
    }
}

fn read_active_team(root: &Path, name: &str) -> Option<TeamSnapshot> {
    if !is_valid_seat_name(name) {
        return None;
    }
    let dir = root.join(name);
    if !is_real_dir(&dir) || !path_inside(root, &dir) {
        return None;
    }
    let state_path = dir.join("state.json");
    let team_path = dir.join("team.json");
    let journal_path = dir.join("journal.json");
    if !is_real_file(&state_path)
        || !is_real_file(&team_path)
        || path_entry_exists(&journal_path)
        || archive_journal_blocks(root, name)
    {
        return None;
    }
    if !path_inside(&dir, &state_path) || !path_inside(&dir, &team_path) {
        return None;
    }
    let state_before = fs::read_to_string(&state_path).ok()?;
    let team_before = fs::read_to_string(&team_path).ok()?;
    if path_entry_exists(&journal_path) || archive_journal_blocks(root, name) {
        return None;
    }
    let team_after = fs::read_to_string(&team_path).ok()?;
    let state_after = fs::read_to_string(&state_path).ok()?;
    if path_entry_exists(&journal_path)
        || archive_journal_blocks(root, name)
        || state_before != state_after
        || team_before != team_after
    {
        return None;
    }
    let state: TeamState = serde_json::from_str(&state_before).ok()?;
    let snapshot: TeamSnapshot = serde_json::from_str(&team_before).ok()?;
    let _generation = state.generation;
    if state.state != "active"
        || snapshot.team != name
        || !is_valid_project_label(&snapshot.project)
        || !crate::teams::repository_binding_is_wellformed(&snapshot.project, &snapshot.repo)
        || !is_valid_seat_name(&snapshot.lead)
    {
        return None;
    }
    let mut names = HashSet::new();
    for seat in &snapshot.seats {
        if !is_valid_seat_name(&seat.name)
            || is_reserved_seat_principal(&seat.name)
            || is_coordination_trio(&seat.name)
            || seat.role.trim().is_empty()
            || !names.insert(seat.name.clone())
        {
            return None;
        }
    }
    if !names.contains(&snapshot.lead) {
        return None;
    }
    for grant in &snapshot.grants {
        if grant.scope != "message"
            || grant.by != "glados"
            || (grant.from != snapshot.team && !names.contains(&grant.from))
            || !is_valid_seat_name(&grant.to)
            || grant.at.trim().is_empty()
        {
            return None;
        }
    }
    Some(snapshot)
}

fn active_team_memberships(root: &Path) -> HashMap<String, Vec<(String, String)>> {
    let mut memberships: HashMap<String, Vec<(String, String)>> = HashMap::new();
    if !is_real_dir(root) {
        return memberships;
    }
    let Ok(entries) = fs::read_dir(root) else {
        return memberships;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || name.starts_with('_') || name == "archive" || name == "presets" {
            continue;
        }
        let Some(team) = read_active_team(root, &name) else {
            continue;
        };
        for seat in team.seats {
            memberships
                .entry(seat.name)
                .or_default()
                .push((team.team.clone(), seat.role));
        }
    }
    memberships
}

/// Scan `~/.claude/aperture/` for agent directories and parse each manifest.
/// Skips `shared/` and any directory missing manifest.json or prompt.md, with
/// a warning to stderr. Disabled agents (`"enabled": false`) are excluded.
pub fn load_agents_from_disk() -> HashMap<String, AgentDef> {
    load_agents_from_roots(Path::new(&aperture_root()), Path::new(&teams_root()))
}

fn load_agents_from_roots(root: &Path, team_root: &Path) -> HashMap<String, AgentDef> {
    let mut agents = HashMap::new();
    let memberships = active_team_memberships(team_root);

    let entries = match fs::read_dir(root) {
        Ok(e) => e,
        Err(e) => {
            eprintln!(
                "[aperture] could not read {}: {} — did you run `just setup`?",
                root.display(), e
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
        if dir_name == "shared" || dir_name.starts_with('_') || is_reserved_seat_principal(&dir_name) {
            continue;
        }
        if !is_valid_seat_name(&dir_name) || !is_real_dir(&path) || !path_inside(root, &path) {
            eprintln!("[aperture] skipping '{}': invalid or unsafe seat directory", dir_name);
            continue;
        }

        let team_marker = path.join("TEAM");
        let membership_count = memberships.get(&dir_name).map_or(0, Vec::len);
        let is_team_seat = is_real_file(&team_marker);
        if (path_entry_exists(&team_marker) && !is_team_seat)
            || (!is_team_seat && membership_count > 0)
        {
            eprintln!("[aperture] skipping '{}': unsafe TEAM marker", dir_name);
            continue;
        }
        if is_team_seat && is_coordination_trio(&dir_name) {
            eprintln!("[aperture] skipping '{}': fixed coordination seat cannot be a team seat", dir_name);
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
        if is_team_seat
            && (!is_real_file(&manifest_path)
                || !is_real_file(&prompt_path)
                || !is_real_file(&path.join(".complete")))
        {
            eprintln!("[aperture] skipping '{}': incomplete or unsafe team seat", dir_name);
            continue;
        }
        if fs::read(&prompt_path).is_err() {
            eprintln!("[aperture] skipping '{}': unreadable prompt.md", dir_name);
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
        if manifest.name.trim().is_empty()
            || manifest.model.trim().is_empty()
            || manifest.window.trim().is_empty()
            || manifest.role.trim().is_empty()
        {
            eprintln!("[aperture] skipping '{}': blank required manifest field", dir_name);
            continue;
        }

        let role = if is_team_seat {
            let Some(entries) = memberships.get(&dir_name) else {
                eprintln!("[aperture] skipping '{}': team seat has no active snapshot", dir_name);
                continue;
            };
            if entries.len() != 1 {
                eprintln!("[aperture] skipping '{}': ambiguous team membership", dir_name);
                continue;
            }
            entries[0].1.clone()
        } else {
            manifest.role
        };

        // The directory name is the canonical lowercase key used everywhere
        // (tmux window targeting, BEADS, message routing). The display name
        // in manifest.json is currently informational; the launcher renders
        // it via the frontend if/when it wants pretty labels.
        let key = dir_name;
        // Empty string → None so the frontend's `agent.emoji || fallback`
        // and the serialized JSON both read "no emoji" the same way.
        let emoji = Some(manifest.emoji.trim().to_string()).filter(|e| !e.is_empty());
        agents.insert(
            key.clone(),
            AgentDef {
                name: key,
                model: manifest.model,
                role,
                prompt_file: prompt_path.to_string_lossy().to_string(),
                tmux_window_id: None,
                status: "stopped".into(),
                emoji,
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
    use super::{is_valid_seat_name, link_codex_skills, load_agents_from_roots, parse_skill_lines, read_resident_list};
    use serde::Deserialize;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

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

    #[derive(Deserialize)]
    struct NameCases {
        valid: Vec<String>,
        invalid: Vec<String>,
    }

    #[test]
    fn canonical_name_fixture_matches_rust_loader() {
        let cases: NameCases = serde_json::from_str(include_str!(
            "../../tests/fixtures/seat-name-cases.json"
        ))
        .unwrap();
        for name in cases.valid {
            assert!(is_valid_seat_name(&name), "expected valid: {name}");
        }
        for name in cases.invalid {
            assert!(!is_valid_seat_name(&name), "expected invalid: {name}");
        }
    }

    fn write_agent(root: &Path, name: &str, enabled: bool, team: bool) {
        let dir = root.join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("manifest.json"),
            serde_json::json!({
                "name": name,
                "model": "codex/gpt-test",
                "window": name,
                "role": "manifest-role",
                "kind": "codex",
                "enabled": enabled
            })
            .to_string(),
        )
        .unwrap();
        fs::write(dir.join("prompt.md"), "fixture").unwrap();
        if team {
            fs::write(dir.join("TEAM"), "").unwrap();
            fs::write(dir.join(".complete"), "").unwrap();
        }
    }

    fn write_team(root: &Path, team: &str, state: &str, seats: &[(&str, &str)]) {
        let dir = root.join(team);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("state.json"),
            serde_json::json!({"state": state, "generation": 1}).to_string(),
        )
        .unwrap();
        fs::write(
            dir.join("team.json"),
            serde_json::json!({
                "team": team,
                "project": "project:aperture",
                "repo": "aperture",
                "lead": seats[0].0,
                "seats": seats.iter().map(|(name, role)| serde_json::json!({"name": name, "role": role})).collect::<Vec<_>>(),
                "grants": []
            })
            .to_string(),
        )
        .unwrap();
    }

    #[test]
    fn loader_keeps_legacy_and_only_complete_active_unambiguous_team_seats() {
        let root = temp_dir("v4-registry");
        let agents = root.join("agents");
        let teams = root.join("teams");
        fs::create_dir_all(&agents).unwrap();
        fs::create_dir_all(&teams).unwrap();
        write_agent(&agents, "rex", true, false);
        write_agent(&agents, "disabled", false, false);
        write_agent(&agents, "p1-backend", true, true);
        write_agent(&agents, "pending-backend", true, true);
        write_agent(&agents, "missing-marker", true, false);
        write_agent(&agents, "journal-link", true, true);
        write_agent(&agents, "a1234567890123456789012345678901", true, false);
        write_team(&teams, "p1", "active", &[("p1-backend", "backend")]);
        write_team(&teams, "pending", "pending", &[("pending-backend", "backend")]);
        write_team(&teams, "markerless", "active", &[("missing-marker", "backend")]);
        write_team(&teams, "journal-link-team", "active", &[("journal-link", "backend")]);
        #[cfg(unix)]
        std::os::unix::fs::symlink(
            root.join("does-not-exist"),
            teams.join("journal-link-team/journal.json"),
        )
        .unwrap();

        let loaded = load_agents_from_roots(&agents, &teams);
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded.get("rex").unwrap().role, "manifest-role");
        assert_eq!(loaded.get("p1-backend").unwrap().role, "backend");
        assert!(!loaded.contains_key("disabled"));
        assert!(!loaded.contains_key("pending-backend"));
        assert!(!loaded.contains_key("missing-marker"));
        assert!(!loaded.contains_key("journal-link"));

        fs::write(teams.join("p1/journal.json"), "{}").unwrap();
        let during_transition = load_agents_from_roots(&agents, &teams);
        assert!(!during_transition.contains_key("p1-backend"));
        fs::remove_file(teams.join("p1/journal.json")).unwrap();
        fs::create_dir_all(root.join("run/team-journals")).unwrap();
        fs::set_permissions(
            root.join("run/team-journals"),
            fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        fs::write(root.join("run/team-journals/p1.archive.json"), "{}").unwrap();
        let during_archive = load_agents_from_roots(&agents, &teams);
        assert!(!during_archive.contains_key("p1-backend"));
        fs::remove_file(root.join("run/team-journals/p1.archive.json")).unwrap();
        fs::remove_dir(root.join("run/team-journals")).unwrap();
        std::os::unix::fs::symlink(
            root.join("does-not-exist"),
            root.join("run/team-journals"),
        )
        .unwrap();
        let unsafe_archive_root = load_agents_from_roots(&agents, &teams);
        assert!(!unsafe_archive_root.contains_key("p1-backend"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn loader_keeps_admitted_dynamic_repository_independent_of_mutable_offers() {
        let root = temp_dir("dynamic-repository");
        let agents = root.join("agents");
        let teams = root.join("teams");
        fs::create_dir_all(&agents).unwrap();
        fs::create_dir_all(&teams).unwrap();
        write_agent(&agents, "mural-frontend", true, true);
        write_team(&teams, "mural", "active", &[("mural-frontend", "frontend")]);
        let path = teams.join("mural/team.json");
        let mut snapshot: serde_json::Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        snapshot["project"] = serde_json::json!("project:incluir");
        snapshot["repo"] = serde_json::json!("eunenem-engine");
        fs::write(&path, snapshot.to_string()).unwrap();
        assert!(load_agents_from_roots(&agents, &teams).contains_key("mural-frontend"));
        // The private snapshot was admitted earlier. Corruption of offers is
        // not permission to strand its workers or erase it from the roster.
        fs::write(root.join("repositories.json"), b"corrupt").unwrap();
        assert!(load_agents_from_roots(&agents, &teams).contains_key("mural-frontend"));
        for (field, value) in [("repo", "../elsewhere"), ("project", "project:unknown")] {
            let mut invalid = snapshot.clone();
            invalid[field] = serde_json::json!(value);
            fs::write(&path, invalid.to_string()).unwrap();
            assert!(!load_agents_from_roots(&agents, &teams).contains_key("mural-frontend"));
        }
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn loader_rejects_missing_or_malformed_boot_fields_and_prompt() {
        let root = temp_dir("v4-required-fields");
        let agents = root.join("agents");
        let teams = root.join("teams");
        fs::create_dir_all(&agents).unwrap();
        fs::create_dir_all(&teams).unwrap();

        write_agent(&agents, "missing-prompt", true, false);
        fs::remove_file(agents.join("missing-prompt/prompt.md")).unwrap();
        for field in ["name", "model", "window", "role"] {
            let name = format!("bad-{field}");
            write_agent(&agents, &name, true, false);
            let path = agents.join(&name).join("manifest.json");
            let mut value: serde_json::Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
            value.as_object_mut().unwrap().remove(field);
            fs::write(path, value.to_string()).unwrap();
        }
        write_agent(&agents, "bad-malformed", true, false);
        let malformed_path = agents.join("bad-malformed/manifest.json");
        let mut malformed: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&malformed_path).unwrap()).unwrap();
        malformed["window"] = serde_json::json!(42);
        fs::write(malformed_path, malformed.to_string()).unwrap();
        for (name, field, value) in [
            ("bad-enabled", "enabled", serde_json::json!("yes")),
            ("bad-emoji", "emoji", serde_json::json!(42)),
            ("bad-kind", "kind", serde_json::json!(42)),
        ] {
            write_agent(&agents, name, true, false);
            let path = agents.join(name).join("manifest.json");
            let mut manifest: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
            manifest[field] = value;
            fs::write(path, manifest.to_string()).unwrap();
        }

        assert!(load_agents_from_roots(&agents, &teams).is_empty());
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn loader_preserves_readable_repo_owned_legacy_symlinks() {
        let root = temp_dir("v4-legacy-links");
        let agents = root.join("agents");
        let teams = root.join("teams");
        let source = root.join("source");
        let linked = agents.join("legacy-link");
        fs::create_dir_all(&agents).unwrap();
        fs::create_dir_all(&teams).unwrap();
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&linked).unwrap();
        fs::write(
            source.join("manifest.json"),
            serde_json::json!({
                "name": "legacy-link",
                "model": "claude/test",
                "window": "legacy-link",
                "role": "legacy",
                "enabled": true
            })
            .to_string(),
        )
        .unwrap();
        fs::write(source.join("prompt.md"), "legacy prompt").unwrap();
        std::os::unix::fs::symlink(source.join("manifest.json"), linked.join("manifest.json"))
            .unwrap();
        std::os::unix::fs::symlink(source.join("prompt.md"), linked.join("prompt.md")).unwrap();

        let loaded = load_agents_from_roots(&agents, &teams);
        assert!(loaded.contains_key("legacy-link"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn loader_rejects_reserved_team_collisions_but_preserves_fixed_legacy_trio() {
        let root = temp_dir("v4-reserved");
        let agents = root.join("agents");
        let teams = root.join("teams");
        fs::create_dir_all(&agents).unwrap();
        fs::create_dir_all(&teams).unwrap();
        for name in ["operator", "watchdog", "glados", "wheatley", "peppy", "rex"] {
            write_agent(&agents, name, true, name != "rex");
        }
        for name in ["operator", "watchdog", "glados", "wheatley", "peppy"] {
            write_team(&teams, &format!("team-{name}"), "active", &[(name, "lead")]);
        }
        let loaded = load_agents_from_roots(&agents, &teams);
        assert_eq!(loaded.keys().cloned().collect::<Vec<_>>(), vec!["rex".to_string()]);

        for name in ["glados", "wheatley", "peppy"] {
            fs::remove_file(agents.join(name).join("TEAM")).unwrap();
            fs::remove_file(agents.join(name).join(".complete")).unwrap();
        }
        let legacy = load_agents_from_roots(&agents, &teams);
        for name in ["glados", "wheatley", "peppy"] {
            assert!(legacy.contains_key(name), "{name} should remain a fixed legacy principal");
        }
        assert!(!legacy.contains_key("operator"));
        assert!(!legacy.contains_key("watchdog"));
        let _ = fs::remove_dir_all(root);
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
