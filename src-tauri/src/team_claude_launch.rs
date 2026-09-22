//! Managed Claude launch data, not a legacy boot or an authority-bearing DTO.
//! The lifecycle adapter owns tmux/gate publication and must not expose this plan
//! until the native owner candidate is durable. No harness is launched here.
use crate::state::{ExecutionTuple, Harness};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub(crate) const MODEL: &str = "claude-sonnet-5";
pub(crate) const WINDOW_MS: i64 = 85_000;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClaudeError {
    Invalid,
    Unsafe,
    Owner,
    Process,
    Revoked,
    Missing,
    Closed,
    PostInput,
    Model,
    Io,
}
impl ClaudeError {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::Invalid => "E_CLAUDE_OBSERVATION_INVALID",
            Self::Unsafe | Self::Io => "E_CLAUDE_RUNTIME_IO",
            Self::Owner => "E_GENERATION_MISMATCH",
            Self::Process => "E_PROCESS_IDENTITY",
            Self::Revoked => "E_CLAUDE_REVOKED",
            Self::Missing => "E_CLAUDE_OBSERVATION_MISSING",
            Self::Closed => "E_CLAUDE_OBSERVATION_CLOSED",
            Self::PostInput => "E_CLAUDE_PREINPUT_UNVERIFIED",
            Self::Model => "E_MODEL_UNVERIFIED",
        }
    }
}

pub(crate) fn exact_tuple(tuple: &ExecutionTuple) -> Result<(), ClaudeError> {
    if tuple.harness != Harness::Claude || tuple.model != MODEL || tuple.reasoning.is_some() {
        return Err(ClaudeError::Model);
    }
    Ok(())
}
pub(crate) fn valid_selector(team: &str, seat: &str, generation: u64) -> Result<(), ClaudeError> {
    if !crate::agent_loader::is_valid_seat_name(team)
        || team.len() > 16
        || !crate::agent_loader::is_valid_seat_name(seat)
        || generation == 0
    {
        return Err(ClaudeError::Invalid);
    }
    Ok(())
}
pub(crate) fn canonical_uuid(value: &str) -> bool {
    uuid::Uuid::parse_str(value)
        .map(|v| v.get_version_num() == 4 && v.to_string() == value)
        .unwrap_or(false)
}
fn shell_atom(value: &str) -> Result<String, ClaudeError> {
    if value.is_empty() || value.chars().any(char::is_control) {
        return Err(ClaudeError::Invalid);
    }
    Ok(format!("'{}'", value.replace('\'', "'\\''")))
}

/// This plan is never Deserialize: no worker path/session/model authority.
/// The fixed helper is supplied by the native installation binding, not a DTO.
pub(crate) struct ClaudeLaunchPlan {
    pub(crate) session_id: String,
    pub(crate) settings_path: PathBuf,
    pub(crate) mcp_path: PathBuf,
    pub(crate) argv: Vec<String>,
    pub(crate) settings: serde_json::Value,
}
impl ClaudeLaunchPlan {
    pub(crate) fn new(
        home: &Path,
        team: &str,
        seat: &str,
        generation: u64,
        tuple: &ExecutionTuple,
        installed_helper: &Path,
    ) -> Result<Self, ClaudeError> {
        exact_tuple(tuple)?;
        valid_selector(team, seat, generation)?;
        if !home.is_absolute() || !installed_helper.is_absolute() {
            return Err(ClaudeError::Unsafe);
        }
        let session_id = uuid::Uuid::new_v4().to_string();
        let base = home
            .join(".aperture/run/managed")
            .join(seat)
            .join(format!("g{generation}"));
        let settings_path = base.join("claude-settings.json");
        let mcp_path = base.join("claude-mcp.json");
        let helper = installed_helper.to_str().ok_or(ClaudeError::Unsafe)?;
        let command = format!(
            "{} --managed-claude-observe --team {} --seat {}",
            shell_atom(helper)?,
            shell_atom(team)?,
            shell_atom(seat)?
        );
        let settings = serde_json::json!({"statusLine":{"type":"command","command":command}});
        // No positional prompt, resume, continue, fork or fallback. Isolated
        // settings and strict MCP list are native generated files, not caller paths.
        let argv = vec![
            "--model".into(),
            MODEL.into(),
            "--session-id".into(),
            session_id.clone(),
            "--settings".into(),
            settings_path.to_str().ok_or(ClaudeError::Unsafe)?.into(),
            "--strict-mcp-config".into(),
            "--mcp-config".into(),
            mcp_path.to_str().ok_or(ClaudeError::Unsafe)?.into(),
        ];
        Ok(Self {
            session_id,
            settings_path,
            mcp_path,
            argv,
            settings,
        })
    }
}

/// Only native code may construct/publish this attempt after the exact candidate
/// has been committed. Raw bearer and reservation nonce are never serialized.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ClaudeAttempt {
    pub(crate) schema_version: u32,
    pub(crate) team: String,
    pub(crate) seat: String,
    pub(crate) generation: u64,
    pub(crate) reservation_nonce_sha256: String,
    pub(crate) snapshot_sha256: String,
    pub(crate) team_generation: u64,
    pub(crate) token_id: String,
    pub(crate) root_pid: u32,
    pub(crate) root_start_time_us: u64,
    pub(crate) session_id: String,
    pub(crate) requested_model: String,
    pub(crate) created_at_ms: i64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::ReasoningEffort;
    fn tuple() -> ExecutionTuple {
        ExecutionTuple {
            harness: Harness::Claude,
            model: MODEL.into(),
            reasoning: None,
        }
    }
    #[test]
    fn fresh_exact_model_plan_is_preinput_and_has_no_fallback() {
        let a = ClaudeLaunchPlan::new(
            Path::new("/fixture"),
            "t1",
            "t1-lead",
            1,
            &tuple(),
            Path::new("/installed/aperture-boot"),
        )
        .unwrap();
        let b = ClaudeLaunchPlan::new(
            Path::new("/fixture"),
            "t1",
            "t1-lead",
            1,
            &tuple(),
            Path::new("/installed/aperture-boot"),
        )
        .unwrap();
        assert_ne!(a.session_id, b.session_id);
        assert!(canonical_uuid(&a.session_id));
        assert_eq!(a.argv.len(), 9);
        assert_eq!(a.argv[1], MODEL);
        for forbidden in [
            "--resume",
            "--continue",
            "--fork-session",
            "--fallback-model",
        ] {
            assert!(!a.argv.iter().any(|v| v == forbidden));
        }
        assert!(a.settings["statusLine"].get("refreshInterval").is_none());
        assert!(a.settings["statusLine"]["command"]
            .as_str()
            .unwrap()
            .ends_with("--team 't1' --seat 't1-lead'"));
        assert_eq!(
            a.settings_path,
            Path::new("/fixture/.aperture/run/managed/t1-lead/g1/claude-settings.json")
        );
        assert!(a.mcp_path.ends_with("claude-mcp.json"));
    }
    #[test]
    fn alias_codex_reasoning_and_bad_selectors_are_not_authority() {
        for model in ["sonnet", "opus", "claude-fable-5", "unknown"] {
            let mut t = tuple();
            t.model = model.into();
            assert!(exact_tuple(&t).is_err());
        }
        let mut t = tuple();
        t.reasoning = Some(ReasoningEffort::High);
        assert!(exact_tuple(&t).is_err());
        t.reasoning = None;
        t.harness = Harness::Codex;
        assert!(exact_tuple(&t).is_err());
        for seat in ["../lead", "a/b", ""] {
            assert!(valid_selector("t1", seat, 1).is_err());
        }
        assert!(valid_selector("t1", "t1-lead", 0).is_err());
    }
    #[test]
    fn statusline_command_quotes_native_paths_not_shell_source() {
        let p = ClaudeLaunchPlan::new(
            Path::new("/fixture"),
            "t1",
            "t1-lead",
            1,
            &tuple(),
            Path::new("/installed/with'quote/aperture-boot"),
        )
        .unwrap();
        assert!(p.settings["statusLine"]["command"]
            .as_str()
            .unwrap()
            .starts_with("'/installed/with'\\''quote/aperture-boot'"));
        assert!(shell_atom("a\nb").is_err());
    }
}

use crate::journal::{
    ensure_private_dir, open_private_file_nofollow, read_private_json, validate_component_path,
    write_private_bytes_atomic, write_private_json_atomic,
};
use crate::owner::{try_lock, OwnerRecord, OwnerStore, StartReservation};
use crate::state::OwnerState;
use crate::team_replacement::{deadline::Deadline, launch as common, repository};
use crate::teams::{classify_managed_seat, ManagedSeatState, TeamSnapshot};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::Read;
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::CommandExt;
use std::process::Command;
use std::time::{Duration, Instant};
const PRIVATE_CAP: usize = 2 * 1024 * 1024;
const EXECUTABLE_CAP: u64 = 512 * 1024 * 1024;
const GATE_WAIT: Duration = Duration::from_secs(10);
fn digest(v: &[u8]) -> String {
    format!("{:x}", Sha256::digest(v))
}
struct PrivateBytes(Vec<u8>);
impl Drop for PrivateBytes {
    fn drop(&mut self) {
        self.0.fill(0)
    }
}
fn private_bytes(path: &Path, cap: usize) -> Result<PrivateBytes, ClaudeError> {
    let f = open_private_file_nofollow(path).map_err(|_| ClaudeError::Unsafe)?;
    let mut out = PrivateBytes(Vec::new());
    f.take(cap as u64 + 1)
        .read_to_end(&mut out.0)
        .map_err(|_| ClaudeError::Io)?;
    if out.0.len() > cap {
        return Err(ClaudeError::Invalid);
    }
    Ok(out)
}
fn installed(path: &Path, executable: bool) -> Result<String, ClaudeError> {
    let h = if executable {
        common::installed_file_with_cap(path, EXECUTABLE_CAP)
    } else {
        common::installed_file(path)
    }
    .map_err(|_| ClaudeError::Unsafe)?;
    if executable
        && std::fs::metadata(path)
            .map_err(|_| ClaudeError::Unsafe)?
            .mode()
            & 0o111
            == 0
    {
        return Err(ClaudeError::Unsafe);
    }
    Ok(h)
}
fn select_binary(candidates: &[PathBuf]) -> Result<PathBuf, ClaudeError> {
    for p in candidates {
        match std::fs::symlink_metadata(p) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return Err(ClaudeError::Unsafe),
            Ok(_) => {}
        }
        let resolved = std::fs::canonicalize(p).map_err(|_| ClaudeError::Unsafe)?;
        installed(&resolved, true)?;
        return Ok(resolved);
    }
    Err(ClaudeError::Missing)
}
fn generation_dir(home: &Path, seat: &str, generation: u64) -> PathBuf {
    home.join(".aperture/run/managed")
        .join(seat)
        .join(format!("g{generation}"))
}
fn active_team(home: &Path, team: &str, seat: &str) -> Result<TeamSnapshot, ClaudeError> {
    match classify_managed_seat(home, seat).map_err(|_| ClaudeError::Owner)? {
        Some(ManagedSeatState::Active { team: t, .. }) if t == team => {}
        _ => return Err(ClaudeError::Owner),
    }
    read_private_json(&home.join(".aperture/teams").join(team).join("team.json"))
        .map_err(|_| ClaudeError::Unsafe)
}
/// Fixed-path native material, never a command DTO and never Debug. The caller
/// supplies only a BoundRepository already resolved by immutable team policy.
pub(crate) struct ClaudeBinding {
    home: PathBuf,
    team: String,
    seat: String,
    tuple: ExecutionTuple,
    role: String,
    runtime: PathBuf,
    infra: PathBuf,
    cwd: PathBuf,
    cwd_identity: (u64, u64),
    worktree: Option<String>,
    executable: PathBuf,
    node: PathBuf,
    helper: PathBuf,
    tmux: PathBuf,
    snapshot: TeamSnapshot,
    pins: BTreeMap<PathBuf, String>,
    skills: BTreeMap<String, String>,
}
impl ClaudeBinding {
    pub(crate) fn preflight(
        home: &Path,
        team: &str,
        seat: &str,
        tuple: &ExecutionTuple,
        repo: &repository::BoundRepository,
        worktree: Option<&str>,
        budget: &Deadline,
    ) -> Result<Self, ClaudeError> {
        exact_tuple(tuple)?;
        valid_selector(team, seat, 1)?;
        let cwd = match worktree {
            Some(w) => repository::replacement_cwd(
                repo,
                w,
                budget
                    .forward_until(Duration::from_secs(10))
                    .map_err(|_| ClaudeError::Closed)?,
            ),
            None => repo.bootstrap_cwd(),
        }
        .map_err(|_| ClaudeError::Unsafe)?;
        let infra = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .ok_or(ClaudeError::Unsafe)?
            .to_path_buf();
        let executable = select_binary(&[
            home.join(".local/bin/claude"),
            home.join(".npm-global/bin/claude"),
            PathBuf::from("/opt/homebrew/bin/claude"),
            PathBuf::from("/usr/local/bin/claude"),
        ])?;
        let tmux = select_binary(&[
            PathBuf::from("/opt/homebrew/bin/tmux"),
            PathBuf::from("/usr/local/bin/tmux"),
        ])?;
        Self::preflight_at(
            home, team, seat, tuple, cwd, worktree, budget, infra, executable, tmux,
        )
    }
    fn preflight_at(
        home: &Path,
        team: &str,
        seat: &str,
        tuple: &ExecutionTuple,
        cwd: PathBuf,
        worktree: Option<&str>,
        budget: &Deadline,
        infra: PathBuf,
        executable: PathBuf,
        tmux: PathBuf,
    ) -> Result<Self, ClaudeError> {
        exact_tuple(tuple)?;
        valid_selector(team, seat, 1)?;
        let helper = home.join(".aperture/bin/aperture-boot");
        installed(&helper, true)?;
        let node = common::node_binary(home, budget).map_err(|_| ClaudeError::Unsafe)?;
        let runtime = validate_component_path(&home.join(".claude/aperture"), seat, false)
            .map_err(|_| ClaudeError::Unsafe)?;
        let snapshot = active_team(home, team, seat)?;
        let seats: Vec<_> = snapshot.seats.iter().filter(|s| s.name == seat).collect();
        if seats.len() != 1 {
            return Err(ClaudeError::Owner);
        }
        let s = seats[0];
        let configured = ExecutionTuple {
            harness: s.harness.clone(),
            model: s.model.clone(),
            reasoning: s.reasoning.clone(),
        };
        if configured != *tuple && !snapshot.fallbacks.contains(tuple) {
            return Err(ClaudeError::Model);
        }
        let manifest: serde_json::Value =
            read_private_json(&runtime.join("manifest.json")).map_err(|_| ClaudeError::Unsafe)?;
        let marker: serde_json::Value =
            read_private_json(&runtime.join("TEAM")).map_err(|_| ClaudeError::Unsafe)?;
        if manifest["name"].as_str() != Some(seat)
            || manifest["role"].as_str() != Some(&s.role)
            || manifest["model"].as_str() != Some(&s.model)
            || manifest["enabled"].as_bool() != Some(true)
            || marker["team"].as_str() != Some(team)
            || marker["role"].as_str() != Some(&s.role)
            || marker["schema_version"].as_u64() != Some(1)
        {
            return Err(ClaudeError::Owner);
        }
        let mut pins = BTreeMap::new();
        for name in [
            "prompt.md",
            "manifest.json",
            "TEAM",
            ".complete",
            "resident.txt",
        ] {
            let p = runtime.join(name);
            pins.insert(p.clone(), digest(&private_bytes(&p, PRIVATE_CAP)?.0));
        }
        for p in [&executable, &node, &helper, &tmux] {
            pins.insert(p.clone(), installed(p, true)?);
        }
        for p in [
            infra.join("mcp-server/dist/index.js"),
            infra.join("mcp-server-sentry/dist/index.js"),
        ] {
            pins.insert(p.clone(), installed(&p, false)?);
        }
        let skills =
            common::inventory(&runtime.join("skills"), budget).map_err(|_| ClaudeError::Unsafe)?;
        let m = std::fs::symlink_metadata(&cwd).map_err(|_| ClaudeError::Unsafe)?;
        if !m.is_dir() || m.file_type().is_symlink() {
            return Err(ClaudeError::Unsafe);
        }
        Ok(Self {
            home: home.into(),
            team: team.into(),
            seat: seat.into(),
            tuple: tuple.clone(),
            role: s.role.clone(),
            runtime,
            infra,
            cwd,
            cwd_identity: (m.dev(), m.ino()),
            worktree: worktree.map(str::to_owned),
            executable,
            node,
            helper,
            tmux,
            snapshot,
            pins,
            skills,
        })
    }
    pub(crate) fn revalidate(&self, budget: &Deadline) -> Result<(), ClaudeError> {
        if active_team(&self.home, &self.team, &self.seat)? != self.snapshot {
            return Err(ClaudeError::Owner);
        }
        let repo = repository::resolve_native(
            &self.home,
            &self.team,
            budget
                .forward_until(Duration::from_secs(10))
                .map_err(|_| ClaudeError::Closed)?,
        )
        .map_err(|_| ClaudeError::Unsafe)?;
        let cwd = match &self.worktree {
            Some(w) => repository::replacement_cwd(
                &repo,
                w,
                budget
                    .forward_until(Duration::from_secs(10))
                    .map_err(|_| ClaudeError::Closed)?,
            ),
            None => repo.bootstrap_cwd(),
        }
        .map_err(|_| ClaudeError::Unsafe)?;
        let m = std::fs::symlink_metadata(&cwd).map_err(|_| ClaudeError::Unsafe)?;
        if cwd != self.cwd || (m.dev(), m.ino()) != self.cwd_identity {
            return Err(ClaudeError::Unsafe);
        }
        for (p, before) in &self.pins {
            let now = if p.starts_with(&self.runtime) {
                digest(&private_bytes(p, PRIVATE_CAP)?.0)
            } else {
                installed(
                    p,
                    p == &self.executable
                        || p == &self.node
                        || p == &self.helper
                        || p == &self.tmux,
                )?
            };
            if now != *before {
                return Err(ClaudeError::Unsafe);
            }
        }
        if common::inventory(&self.runtime.join("skills"), budget)
            .map_err(|_| ClaudeError::Unsafe)?
            != self.skills
        {
            return Err(ClaudeError::Unsafe);
        }
        Ok(())
    }
    pub(crate) fn publish(
        &self,
        res: &StartReservation,
        token: &crate::hub_auth::managed::ManagedToken,
        budget: &Deadline,
    ) -> Result<PublishedClaude, ClaudeError> {
        let password = std::env::var("BEADS_DOLT_PASSWORD").unwrap_or_default();
        self.publish_with_password(res, token, budget, &password)
    }
    fn publish_with_password(
        &self,
        res: &StartReservation,
        token: &crate::hub_auth::managed::ManagedToken,
        budget: &Deadline,
        password: &str,
    ) -> Result<PublishedClaude, ClaudeError> {
        self.revalidate(budget)?;
        let _team = try_lock(&self.home.join(".aperture/run/team-locks"), &self.team)
            .map_err(|_| ClaudeError::Owner)?;
        let store = OwnerStore::new(self.home.join(".aperture/run/owner"));
        let _seat = store.lock(&self.seat).map_err(|_| ClaudeError::Owner)?;
        let owner: OwnerRecord =
            read_private_json(&store.record_path(&self.seat)).map_err(|_| ClaudeError::Owner)?;
        if res.seat != self.seat
            || token.generation() != res.generation
            || owner.state != OwnerState::Starting
            || owner.generation != res.generation
            || owner.requested != self.tuple
            || owner.incarnation.is_some()
            || owner.provisional_token_id.as_deref() != Some(token.token_id())
            || owner.reservation_nonce_sha256.as_deref()
                != Some(digest(res.nonce().as_bytes()).as_str())
        {
            return Err(ClaudeError::Owner);
        }
        let base = validate_component_path(&self.home.join(".aperture/run"), "managed", true)
            .map_err(|_| ClaudeError::Unsafe)?;
        ensure_private_dir(&base).map_err(|_| ClaudeError::Unsafe)?;
        let dir =
            validate_component_path(&base, &self.seat, true).map_err(|_| ClaudeError::Unsafe)?;
        ensure_private_dir(&dir).map_err(|_| ClaudeError::Unsafe)?;
        let dest = validate_component_path(&dir, &format!("g{}", res.generation), true)
            .map_err(|_| ClaudeError::Unsafe)?;
        match std::fs::symlink_metadata(&dest) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            _ => return Err(ClaudeError::Closed),
        }
        ensure_private_dir(&dest).map_err(|_| ClaudeError::Unsafe)?;
        let plan = ClaudeLaunchPlan::new(
            &self.home,
            &self.team,
            &self.seat,
            res.generation,
            &self.tuple,
            &self.helper,
        )?;
        let mut prompt = private_bytes(&self.runtime.join("prompt.md"), PRIVATE_CAP)?;
        let resident = private_bytes(&self.runtime.join("resident.txt"), 8192)?;
        for name in std::str::from_utf8(&resident.0)
            .map_err(|_| ClaudeError::Invalid)?
            .lines()
            .filter(|s| !s.is_empty())
        {
            if name.len() > 64
                || !name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
            {
                return Err(ClaudeError::Unsafe);
            }
            let content = private_bytes(
                &self.runtime.join("skills").join(name).join("SKILL.md"),
                PRIVATE_CAP,
            )?;
            prompt.0.extend_from_slice(b"\n\n");
            prompt.0.extend_from_slice(&content.0);
            if prompt.0.len() > PRIVATE_CAP {
                return Err(ClaudeError::Invalid);
            }
        }
        if self.worktree.is_some() {
            prompt.0.extend_from_slice(b"\n\nInventory the bound worktree before continuing. Do not assume pending effects completed.\n");
        }
        let prompt_path = dest.join("prompt.md");
        write_private_bytes_atomic(&prompt_path, &prompt.0, false)
            .map_err(|_| ClaudeError::Unsafe)?;
        crate::teams::copy_private_tree_bounded(
            &self.runtime.join("skills"),
            &dest.join("skills"),
            PRIVATE_CAP,
        )
        .map_err(|_| ClaudeError::Unsafe)?;
        if common::inventory(&dest.join("skills"), budget).map_err(|_| ClaudeError::Unsafe)?
            != self.skills
        {
            return Err(ClaudeError::Unsafe);
        }
        let mut env = serde_json::json!({"HOME":self.home,"AGENT_NAME":self.seat,"AGENT_ROLE":self.role,"AGENT_MODEL":MODEL,"APERTURE_TEAM_GENERATION":res.generation.to_string(),"APERTURE_HUB_TOKEN_FILE":token.path(),"BEADS_DIR":self.home.join(".aperture/.beads"),"BD_ACTOR":self.seat,"BEADS_DOLT_PASSWORD":password,"APERTURE_MAILBOX":self.home.join(".aperture/mailbox")});
        let mut mcp = serde_json::json!({"mcpServers":{"aperture-bus":{"command":self.node,"args":[self.infra.join("mcp-server/dist/index.js")],"env":env},"sentry":{"command":self.node,"args":[self.infra.join("mcp-server-sentry/dist/index.js")],"env":env}}});
        let config = PrivateBytes(serde_json::to_vec(&mcp).map_err(|_| ClaudeError::Invalid)?);
        if config.0.len() > PRIVATE_CAP {
            return Err(ClaudeError::Invalid);
        }
        write_private_bytes_atomic(&plan.mcp_path, &config.0, false)
            .map_err(|_| ClaudeError::Unsafe)?;
        // No credential values are returned, logged or placed in tmux argv.
        env.take();
        mcp.take();
        write_private_json_atomic(&plan.settings_path, &plan.settings, false)
            .map_err(|_| ClaudeError::Unsafe)?;
        let mut args = plan.argv.clone();
        args.extend([
            "--append-system-prompt-file".into(),
            prompt_path.to_string_lossy().into_owned(),
        ]);
        let mut record = LaunchRecord {
            schema_version: 1,
            team: self.team.clone(),
            seat: self.seat.clone(),
            generation: res.generation,
            session_id: plan.session_id.clone(),
            nonce_sha256: digest(res.nonce().as_bytes()),
            token_id: token.token_id().into(),
            snapshot_sha256: digest(
                &serde_json::to_vec(&self.snapshot).map_err(|_| ClaudeError::Invalid)?,
            ),
            executable: self.executable.clone(),
            helper: self.helper.clone(),
            node: self.node.clone(),
            tmux: self.tmux.clone(),
            cwd: self.cwd.clone(),
            cwd_identity: self.cwd_identity,
            worktree: self.worktree.clone(),
            args,
            pins: self.pins.clone(),
            private_pins: BTreeMap::new(),
        };
        for p in [&plan.settings_path, &plan.mcp_path, &prompt_path] {
            record
                .private_pins
                .insert(p.clone(), digest(&private_bytes(p, PRIVATE_CAP)?.0));
        }
        write_private_json_atomic(&dest.join("claude-launch.json"), &record, false)
            .map_err(|_| ClaudeError::Unsafe)?;
        Ok(PublishedClaude {
            home: self.home.clone(),
            record,
        })
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LaunchRecord {
    schema_version: u32,
    team: String,
    seat: String,
    generation: u64,
    session_id: String,
    nonce_sha256: String,
    token_id: String,
    snapshot_sha256: String,
    executable: PathBuf,
    helper: PathBuf,
    node: PathBuf,
    tmux: PathBuf,
    cwd: PathBuf,
    cwd_identity: (u64, u64),
    worktree: Option<String>,
    args: Vec<String>,
    pins: BTreeMap<PathBuf, String>,
    private_pins: BTreeMap<PathBuf, String>,
}
pub(crate) struct PublishedClaude {
    home: PathBuf,
    record: LaunchRecord,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GateRelease {
    schema_version: u32,
    launch_sha256: String,
    attempt_sha256: String,
    root_pid: u32,
    root_start_time_us: u64,
}
/// Metadata only, held by the lifecycle caller; no Deserialize and no signal
/// methods. Cleanup is the existing exact OwnerStore/process/revocation path.
pub(crate) struct PendingClaude {
    published: PublishedClaude,
    pub(crate) process: crate::owner::ProcessIdentity,
    pub(crate) window_id: String,
    pub(crate) pane_id: String,
}
fn exact_id(s: &str, prefix: char) -> bool {
    s.starts_with(prefix)
        && s.len() > 1
        && s.len() < 22
        && s[1..].bytes().all(|b| b.is_ascii_digit())
}
fn pane_result(bytes: &[u8]) -> Result<(String, String, u32), ClaudeError> {
    if bytes.len() > 128 {
        return Err(ClaudeError::Invalid);
    }
    let text = std::str::from_utf8(bytes)
        .map_err(|_| ClaudeError::Invalid)?
        .trim_end_matches('\n');
    let parts: Vec<_> = text.split('|').collect();
    if parts.len() != 3
        || !exact_id(parts[0], '@')
        || !exact_id(parts[1], '%')
        || !parts[2].bytes().all(|b| b.is_ascii_digit())
    {
        return Err(ClaudeError::Invalid);
    }
    let pid = parts[2].parse::<u32>().map_err(|_| ClaudeError::Invalid)?;
    if pid <= 1 || pid > i32::MAX as u32 {
        return Err(ClaudeError::Invalid);
    }
    Ok((parts[0].into(), parts[1].into(), pid))
}
fn record_path(home: &Path, record: &LaunchRecord) -> PathBuf {
    generation_dir(home, &record.seat, record.generation).join("claude-launch.json")
}
fn record_sha(record: &LaunchRecord) -> Result<String, ClaudeError> {
    Ok(digest(
        &serde_json::to_vec(record).map_err(|_| ClaudeError::Invalid)?,
    ))
}
fn validate_record(home: &Path, r: &LaunchRecord, budget: &Deadline) -> Result<(), ClaudeError> {
    valid_selector(&r.team, &r.seat, r.generation)?;
    if r.schema_version != 1
        || [&r.executable, &r.helper, &r.node, &r.tmux, &r.cwd]
            .iter()
            .any(|p| !p.is_absolute())
        || !canonical_uuid(&r.session_id)
        || r.pins.len() > 32
        || r.private_pins.len() != 3
        || r.helper != home.join(".aperture/bin/aperture-boot")
    {
        return Err(ClaudeError::Invalid);
    }
    let snapshot = active_team(home, &r.team, &r.seat)?;
    if digest(&serde_json::to_vec(&snapshot).map_err(|_| ClaudeError::Invalid)?)
        != r.snapshot_sha256
    {
        return Err(ClaudeError::Owner);
    }
    let base = generation_dir(home, &r.seat, r.generation);
    let mut plan = ClaudeLaunchPlan::new(
        home,
        &r.team,
        &r.seat,
        r.generation,
        &ExecutionTuple {
            harness: Harness::Claude,
            model: MODEL.into(),
            reasoning: None,
        },
        &r.helper,
    )?;
    // Only the native attempt's preallocated UUID is allowed. The rest of argv
    // is rederived; a private malformed record cannot add flags/initial input.
    plan.argv[3] = r.session_id.clone();
    plan.argv.extend([
        "--append-system-prompt-file".into(),
        base.join("prompt.md").to_string_lossy().into_owned(),
    ]);
    if plan.argv != r.args {
        return Err(ClaudeError::Invalid);
    }
    let expected_private = [
        base.join("claude-settings.json"),
        base.join("claude-mcp.json"),
        base.join("prompt.md"),
    ];
    if r.private_pins.keys().any(|p| !expected_private.contains(p)) {
        return Err(ClaudeError::Unsafe);
    }
    let expected_settings =
        serde_json::to_vec_pretty(&plan.settings).map_err(|_| ClaudeError::Invalid)?;
    let actual_settings: serde_json::Value =
        read_private_json(&plan.settings_path).map_err(|_| ClaudeError::Unsafe)?;
    if actual_settings != plan.settings || expected_settings.len() > PRIVATE_CAP {
        return Err(ClaudeError::Unsafe);
    }
    let runtime = home.join(".claude/aperture").join(&r.seat);
    let infra = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .ok_or(ClaudeError::Unsafe)?;
    let allowed_installed = [
        r.executable.clone(),
        r.node.clone(),
        r.helper.clone(),
        r.tmux.clone(),
        infra.join("mcp-server/dist/index.js"),
        infra.join("mcp-server-sentry/dist/index.js"),
    ];
    for p in &allowed_installed {
        if !r.pins.contains_key(p) {
            return Err(ClaudeError::Unsafe);
        }
    }
    for (p, h) in &r.pins {
        let actual = if p.starts_with(&runtime) {
            if ![
                "prompt.md",
                "manifest.json",
                "TEAM",
                ".complete",
                "resident.txt",
            ]
            .iter()
            .any(|n| p == &runtime.join(n))
            {
                return Err(ClaudeError::Unsafe);
            }
            digest(&private_bytes(p, PRIVATE_CAP)?.0)
        } else {
            if !allowed_installed.contains(p) {
                return Err(ClaudeError::Unsafe);
            }
            installed(p, [&r.executable, &r.node, &r.helper, &r.tmux].contains(&p))?
        };
        if actual != *h {
            return Err(ClaudeError::Unsafe);
        }
    }
    for (p, h) in &r.private_pins {
        if digest(&private_bytes(p, PRIVATE_CAP)?.0) != *h {
            return Err(ClaudeError::Unsafe);
        }
    }
    let repo = repository::resolve_native(
        home,
        &r.team,
        budget
            .forward_until(Duration::from_secs(3))
            .map_err(|_| ClaudeError::Closed)?,
    )
    .map_err(|_| ClaudeError::Unsafe)?;
    let cwd = match &r.worktree {
        Some(w) => repository::replacement_cwd(
            &repo,
            w,
            budget
                .forward_until(Duration::from_secs(3))
                .map_err(|_| ClaudeError::Closed)?,
        ),
        None => repo.bootstrap_cwd(),
    }
    .map_err(|_| ClaudeError::Unsafe)?;
    let m = std::fs::symlink_metadata(&cwd).map_err(|_| ClaudeError::Unsafe)?;
    if cwd != r.cwd || (m.dev(), m.ino()) != r.cwd_identity {
        return Err(ClaudeError::Unsafe);
    }
    Ok(())
}
impl PublishedClaude {
    pub(crate) fn session_id(&self) -> &str {
        &self.record.session_id
    }
    pub(crate) fn spawn(self, budget: &Deadline) -> Result<PendingClaude, ClaudeError> {
        validate_record(&self.home, &self.record, budget)?;
        let r = &self.record;
        let dir = generation_dir(&self.home, &r.seat, r.generation);
        // Durable admission before the native tmux call; a lost response does
        // not grant permission to create another window. Gate times out itself.
        write_private_json_atomic(
            &dir.join("claude-spawn.json"),
            &serde_json::json!({"schema_version":1,"launch_sha256":record_sha(r)?}),
            false,
        )
        .map_err(|_| ClaudeError::Closed)?;
        let mut cmd = Command::new(&r.tmux);
        cmd.env_clear()
            .env("HOME", &self.home)
            .env("PATH", "/usr/bin:/bin")
            .args([
                "new-window",
                "-d",
                "-t",
                "aperture",
                "-n",
                &format!("{}-g{}", r.seat, r.generation),
                "-P",
                "-F",
                "#{window_id}|#{pane_id}|#{pane_pid}",
                "-c",
            ])
            .arg(&r.cwd)
            .arg("/usr/bin/env")
            .arg("-i")
            .arg(format!("HOME={}", self.home.display()))
            .arg("PATH=/usr/bin:/bin")
            .arg("TERM=xterm-256color")
            .arg(&r.helper)
            .args([
                "--managed-claude-gate",
                "--team",
                &r.team,
                "--seat",
                &r.seat,
                "--generation",
                &r.generation.to_string(),
            ]);
        let output = repository::bounded_command(
            cmd,
            budget
                .forward_until(Duration::from_secs(3))
                .map_err(|_| ClaudeError::Closed)?,
        )
        .map_err(|_| ClaudeError::Io)?;
        let (window_id, pane_id, pid) = pane_result(&output)?;
        let p = crate::team_process::observe(pid)
            .map_err(|_| ClaudeError::Process)?
            .ok_or(ClaudeError::Process)?;
        let process = crate::team_process::native::capture_gated_identity(&p.identity)
            .map_err(|_| ClaudeError::Process)?;
        Ok(PendingClaude {
            published: self,
            process,
            window_id,
            pane_id,
        })
    }
}
impl PendingClaude {
    pub(crate) fn session_id(&self) -> &str {
        self.published.session_id()
    }
    /// Lifecycle caller has already durably bound this exact candidate and
    /// written its UUID attempt. No send-keys/prompt or harness input here.
    pub(crate) fn release(
        &self,
        res: &StartReservation,
        budget: &Deadline,
    ) -> Result<(), ClaudeError> {
        let home = &self.published.home;
        let r = &self.published.record;
        validate_record(home, r, budget)?;
        if res.seat != r.seat
            || res.generation != r.generation
            || digest(res.nonce().as_bytes()) != r.nonce_sha256
        {
            return Err(ClaudeError::Owner);
        }
        crate::team_claude_observation::with_gated_attempt(
            home,
            &r.team,
            &r.seat,
            r.generation,
            |a| {
                if a.session_id != r.session_id
                    || a.token_id != r.token_id
                    || a.root_pid != self.process.pid
                    || a.root_start_time_us != self.process.start_time
                {
                    return Err(ClaudeError::Owner);
                }
                let release = GateRelease {
                    schema_version: 1,
                    launch_sha256: record_sha(r)?,
                    attempt_sha256: digest(
                        &serde_json::to_vec(a).map_err(|_| ClaudeError::Invalid)?,
                    ),
                    root_pid: a.root_pid,
                    root_start_time_us: a.root_start_time_us,
                };
                write_private_json_atomic(
                    &generation_dir(home, &r.seat, r.generation).join("claude-release.json"),
                    &release,
                    false,
                )
                .map_err(|_| ClaudeError::Closed)
            },
        )
    }
}
/// Thin aperture-boot entrypoint. No fallback, thread discovery, legacy boot or
/// supervisor. A missing release expires before Claude can execute. A stale or
/// ambiguous release is terminal; never retried. Success replaces this process
/// in-place so the exact pane PID/birth remains the owned harness root.
pub(crate) fn gate_native(
    home: &Path,
    team: &str,
    seat: &str,
    generation: u64,
) -> Result<(), ClaudeError> {
    valid_selector(team, seat, generation)?;
    let until = Instant::now() + GATE_WAIT;
    let dir = generation_dir(home, seat, generation);
    let path = dir.join("claude-release.json");
    loop {
        match std::fs::symlink_metadata(&path) {
            Ok(_) => break,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(ClaudeError::Unsafe),
        }
        if Instant::now() >= until {
            return Err(ClaudeError::Closed);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let r: LaunchRecord =
        read_private_json(&dir.join("claude-launch.json")).map_err(|_| ClaudeError::Unsafe)?;
    if r.team != team || r.seat != seat || r.generation != generation {
        return Err(ClaudeError::Owner);
    }
    let budget = Deadline::new();
    validate_record(home, &r, &budget)?;
    let release: GateRelease = read_private_json(&path).map_err(|_| ClaudeError::Unsafe)?;
    let mut cmd = gate_command(home, &r);
    crate::team_claude_observation::with_gated_attempt(home, team, seat, generation, |a| {
        validate_release(&r, a, &release, std::process::id())?;
        if Instant::now() > until {
            return Err(ClaudeError::Closed);
        }
        // Advisory lock descriptors are close-on-exec. Keep the exact native
        // owner/token check locked through exec, not across a user-space gap.
        let _error = cmd.exec();
        Err(ClaudeError::Io)
    })
}
fn validate_release(
    r: &LaunchRecord,
    a: &ClaudeAttempt,
    release: &GateRelease,
    pid: u32,
) -> Result<(), ClaudeError> {
    if release.schema_version != 1
        || release.launch_sha256 != record_sha(r)?
        || release.attempt_sha256
            != digest(&serde_json::to_vec(a).map_err(|_| ClaudeError::Invalid)?)
        || release.root_pid != pid
        || release.root_pid != a.root_pid
        || release.root_start_time_us != a.root_start_time_us
        || a.team != r.team
        || a.seat != r.seat
        || a.generation != r.generation
        || a.session_id != r.session_id
        || a.token_id != r.token_id
        || a.reservation_nonce_sha256 != r.nonce_sha256
    {
        return Err(ClaudeError::Owner);
    }
    Ok(())
}
fn gate_command(home: &Path, r: &LaunchRecord) -> Command {
    let mut cmd = Command::new(&r.executable);
    cmd.args(&r.args)
        .current_dir(&r.cwd)
        .env_clear()
        .env("HOME", home)
        .env(
            "PATH",
            format!(
                "{}:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin",
                r.node
                    .parent()
                    .unwrap_or(Path::new("/nonexistent"))
                    .display()
            ),
        )
        .env("TERM", "xterm-256color")
        .env("LANG", "en_US.UTF-8")
        .env("AGENT_NAME", &r.seat)
        .env("APERTURE_TEAM_GENERATION", r.generation.to_string());
    cmd
}

#[cfg(test)]
mod gate_tests {
    use super::*;
    fn fixture() -> (LaunchRecord, ClaudeAttempt, GateRelease) {
        let plan = ClaudeLaunchPlan::new(
            Path::new("/fixture"),
            "t1",
            "t1-worker",
            1,
            &ExecutionTuple {
                harness: Harness::Claude,
                model: MODEL.into(),
                reasoning: None,
            },
            Path::new("/fixture/.aperture/bin/aperture-boot"),
        )
        .unwrap();
        let r = LaunchRecord {
            schema_version: 1,
            team: "t1".into(),
            seat: "t1-worker".into(),
            generation: 1,
            session_id: plan.session_id.clone(),
            nonce_sha256: "a".repeat(64),
            token_id: "b".repeat(64),
            snapshot_sha256: "c".repeat(64),
            executable: PathBuf::from("/fixture/bin/claude"),
            helper: PathBuf::from("/fixture/.aperture/bin/aperture-boot"),
            node: PathBuf::from("/fixture/bin/node"),
            tmux: PathBuf::from("/fixture/bin/tmux"),
            cwd: PathBuf::from("/fixture/repository"),
            cwd_identity: (1, 2),
            worktree: None,
            args: plan.argv,
            pins: BTreeMap::new(),
            private_pins: BTreeMap::new(),
        };
        let a = ClaudeAttempt {
            schema_version: 1,
            team: r.team.clone(),
            seat: r.seat.clone(),
            generation: r.generation,
            reservation_nonce_sha256: r.nonce_sha256.clone(),
            snapshot_sha256: r.snapshot_sha256.clone(),
            team_generation: 1,
            token_id: r.token_id.clone(),
            root_pid: 900001,
            root_start_time_us: 1790000000000001,
            session_id: r.session_id.clone(),
            requested_model: MODEL.into(),
            created_at_ms: 1,
        };
        let release = GateRelease {
            schema_version: 1,
            launch_sha256: record_sha(&r).unwrap(),
            attempt_sha256: digest(&serde_json::to_vec(&a).unwrap()),
            root_pid: a.root_pid,
            root_start_time_us: a.root_start_time_us,
        };
        (r, a, release)
    }
    #[test]
    fn gate_release_binds_every_native_identity_and_both_private_artifacts() {
        let (r, a, release) = fixture();
        assert!(validate_release(&r, &a, &release, a.root_pid).is_ok());
        for bad in 0..10 {
            let (mut r, mut a, mut release) = fixture();
            match bad {
                0 => release.schema_version = 2,
                1 => release.root_pid += 1,
                2 => release.root_start_time_us += 1,
                3 => release.launch_sha256 = "d".repeat(64),
                4 => release.attempt_sha256 = "d".repeat(64),
                5 => a.session_id = uuid::Uuid::new_v4().to_string(),
                6 => a.token_id = "d".repeat(64),
                7 => r.generation += 1,
                8 => r.args.push("a caller prompt".into()),
                _ => a.team = "other".into(),
            }
            assert!(validate_release(&r, &a, &release, 900001).is_err());
        }
        assert!(validate_release(&r, &a, &release, 900002).is_err());
    }
    #[test]
    fn gate_command_has_exact_model_session_and_no_inherited_environment() {
        let (r, _, _) = fixture();
        let cmd = gate_command(Path::new("/fixture"), &r);
        assert_eq!(cmd.get_program(), r.executable.as_os_str());
        assert_eq!(cmd.get_current_dir(), Some(r.cwd.as_path()));
        let args: Vec<_> = cmd.get_args().map(|x| x.to_str().unwrap()).collect();
        assert_eq!(args, r.args.iter().map(String::as_str).collect::<Vec<_>>());
        assert!(
            cmd.get_envs()
                .any(|(k, v)| k == "PATH"
                    && v.unwrap().to_str().unwrap().starts_with("/fixture/bin:"))
        );
        let keys: Vec<_> = cmd.get_envs().map(|(k, _)| k.to_str().unwrap()).collect();
        assert_eq!(
            keys,
            vec![
                "AGENT_NAME",
                "APERTURE_TEAM_GENERATION",
                "HOME",
                "LANG",
                "PATH",
                "TERM"
            ]
        );
        for key in [
            "ANTHROPIC_MODEL",
            "ANTHROPIC_DEFAULT_SONNET_MODEL",
            "ANTHROPIC_DEFAULT_OPUS_MODEL",
            "CLAUDE_CODE_EFFORT_LEVEL",
        ] {
            assert!(!keys.contains(&key));
        }
        // Inspect the child environment with an inert executable. No Claude,
        // tmux, provider, owner changes or process-table scan is performed.
        let mut inert = r;
        inert.executable = PathBuf::from("/usr/bin/env");
        inert.cwd = std::env::temp_dir();
        inert.args.clear();
        let output = gate_command(Path::new("/fixture"), &inert)
            .output()
            .unwrap();
        assert!(output.status.success());
        let actual = String::from_utf8(output.stdout).unwrap();
        assert_eq!(actual.lines().count(), 6);
    }
    #[test]
    fn tmux_response_is_exact_id_metadata_not_untrusted_shell_or_multiline() {
        assert_eq!(
            pane_result(b"@23|%17|1234\n").unwrap(),
            ("@23".into(), "%17".into(), 1234)
        );
        for value in [
            "name|%17|1234",
            "@1|pane|1234",
            "@1|%2|1",
            "@1|%2|NaN",
            "@1|%2|123\n@2|%3|124",
            "@1|%2|123|extra",
        ] {
            assert!(pane_result(value.as_bytes()).is_err());
        }
    }
}

#[cfg(test)]
mod publication_tests {
    use super::*;
    use crate::team_auth::AuthenticatedActor;
    use serde_json::json;
    use std::os::unix::fs::{symlink, PermissionsExt};
    struct Fixture {
        home: PathBuf,
        infra: PathBuf,
        cwd: PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("claude-launch-{}", uuid::Uuid::new_v4()));
            ensure_private_dir(&root).unwrap();
            let home = root.canonicalize().unwrap();
            let f = Self {
                cwd: home.join("projects/aperture"),
                infra: home.join("infra"),
                home,
            };
            ensure_private_dir(&f.cwd).unwrap();
            assert!(Command::new("/usr/bin/git")
                .args(["init", "-q"])
                .arg(&f.cwd)
                .status()
                .unwrap()
                .success());
            for p in [
                "tools/claude",
                "tools/node",
                "tools/tmux",
                ".aperture/bin/aperture-boot",
            ] {
                f.write(p, b"synthetic executable never run");
                std::fs::set_permissions(f.home.join(p), std::fs::Permissions::from_mode(0o700))
                    .unwrap();
            }
            f.write(
                ".volta/bin/node",
                format!(
                    "#!/bin/sh\nprintf '%s\\n' '{}'\n",
                    f.home.join("tools/node").display()
                )
                .as_bytes(),
            );
            std::fs::set_permissions(
                f.home.join(".volta/bin/node"),
                std::fs::Permissions::from_mode(0o700),
            )
            .unwrap();
            for p in [
                "infra/mcp-server/dist/index.js",
                "infra/mcp-server-sentry/dist/index.js",
            ] {
                f.write(p, b"fixture JS never executed");
            }
            f.json(".aperture/teams/t1/team.json",json!({"schema_version":1,"team":"t1","project":"project:aperture","repo":"aperture","mission":"fixture","acceptance":"fixture","preset":{"id":null,"sha256":null},"lead":"t1-worker","seats":[{"name":"t1-worker","role":"lead","harness":"claude","model":MODEL,"reasoning":null}],"fallbacks":[],"grants":[],"created_at":"2026-09-20T00:00:00Z","creation_request_id":uuid::Uuid::new_v4().to_string(),"staging_uuid":uuid::Uuid::new_v4().to_string()}));
            f.json(".aperture/teams/t1/state.json",json!({"schema_version":1,"state":"active","generation":1,"epic_id":"aperture-fixture","failure":null,"updated_at":"2026-09-20T00:00:00Z"}));
            f.json(
                ".claude/aperture/t1-worker/manifest.json",
                json!({"name":"t1-worker","role":"lead","model":MODEL,"enabled":true}),
            );
            f.json(
                ".claude/aperture/t1-worker/TEAM",
                json!({"schema_version":1,"team":"t1","role":"lead"}),
            );
            for (p, b) in [
                ("prompt.md", "fixture mission"),
                (".complete", ""),
                ("resident.txt", "constitution\n"),
                ("skills/constitution/SKILL.md", "fixture constitution"),
            ] {
                f.write(&format!(".claude/aperture/t1-worker/{p}"), b.as_bytes());
            }
            f
        }
        fn write(&self, p: &str, b: &[u8]) {
            let p = self.home.join(p);
            ensure_private_dir(p.parent().unwrap()).unwrap();
            write_private_bytes_atomic(&p, b, true).unwrap();
        }
        fn json(&self, p: &str, v: serde_json::Value) {
            let p = self.home.join(p);
            ensure_private_dir(p.parent().unwrap()).unwrap();
            write_private_json_atomic(&p, &v, true).unwrap();
        }
        fn binding(&self) -> Result<ClaudeBinding, ClaudeError> {
            ClaudeBinding::preflight_at(
                &self.home,
                "t1",
                "t1-worker",
                &ExecutionTuple {
                    harness: Harness::Claude,
                    model: MODEL.into(),
                    reasoning: None,
                },
                self.cwd.clone(),
                None,
                &Deadline::new(),
                self.infra.clone(),
                self.home.join("tools/claude"),
                self.home.join("tools/tmux"),
            )
        }
        fn reservation(&self) -> (StartReservation, crate::hub_auth::managed::ManagedToken) {
            let actor = AuthenticatedActor::launcher();
            let store = OwnerStore::new(self.home.join(".aperture/run/owner"));
            let t = ExecutionTuple {
                harness: Harness::Claude,
                model: MODEL.into(),
                reasoning: None,
            };
            store
                .initialize_owner(&actor, "t1-worker", t.clone())
                .unwrap();
            let r = store.reserve_start(&actor, "t1-worker", 0, t).unwrap();
            let token = crate::hub_auth::managed::provision(&self.home, "t1", &actor, &r).unwrap();
            (r, token)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.home).unwrap();
        }
    }
    #[test]
    fn native_publication_is_private_no_replace_and_uses_absolute_node_for_both_mcps() {
        let f = Fixture::new();
        let binding = f.binding().unwrap();
        binding.revalidate(&Deadline::new()).unwrap();
        assert!(!f.home.join(".aperture/run/managed").exists());
        let (r, t) = f.reservation();
        let p = binding
            .publish_with_password(&r, &t, &Deadline::new(), "")
            .unwrap();
        let dir = generation_dir(&f.home, "t1-worker", 1);
        let config: serde_json::Value = read_private_json(&dir.join("claude-mcp.json")).unwrap();
        for server in ["aperture-bus", "sentry"] {
            assert_eq!(
                config["mcpServers"][server]["command"],
                json!(f.home.join("tools/node"))
            );
            assert_eq!(
                config["mcpServers"][server]["env"]["APERTURE_TEAM_GENERATION"],
                "1"
            );
        }
        assert_eq!(
            config["mcpServers"]["aperture-bus"]["args"],
            json!([f.infra.join("mcp-server/dist/index.js")])
        );
        assert_eq!(p.record.args[1], MODEL);
        assert_eq!(p.record.args[3], p.session_id());
        assert_eq!(p.record.args.len(), 11);
        for name in [
            "claude-launch.json",
            "claude-settings.json",
            "claude-mcp.json",
            "prompt.md",
        ] {
            assert_eq!(
                std::fs::metadata(dir.join(name)).unwrap().mode() & 0o777,
                0o600
            );
        }
        assert_eq!(std::fs::metadata(&dir).unwrap().mode() & 0o777, 0o700);
        let before = std::fs::read(dir.join("claude-launch.json")).unwrap();
        assert!(binding
            .publish_with_password(&r, &t, &Deadline::new(), "")
            .is_err());
        assert_eq!(
            before,
            std::fs::read(dir.join("claude-launch.json")).unwrap()
        );
        assert!(!dir.join("claude-spawn.json").exists());
        assert!(!dir.join("claude-release.json").exists());
    }
    #[test]
    fn native_publication_revalidates_owner_snapshot_runtime_node_and_links_before_files() {
        for bad in 0..5 {
            let f = Fixture::new();
            let binding = f.binding().unwrap();
            let (r, t) = f.reservation();
            match bad {
                0 => {
                    let store = OwnerStore::new(f.home.join(".aperture/run/owner"));
                    let mut o = store.read_owner("t1-worker").unwrap();
                    o.provisional_token_id = Some("f".repeat(64));
                    write_private_json_atomic(&store.record_path("t1-worker"), &o, true).unwrap();
                }
                1 => f.write(".claude/aperture/t1-worker/prompt.md", b"changed"),
                2 => f.write("tools/node", b"changed node"),
                3 => {
                    let path = f.home.join("tools/claude");
                    std::fs::remove_file(&path).unwrap();
                    symlink(f.home.join("tools/tmux"), path).unwrap();
                }
                _ => {
                    let path = f.home.join(".aperture/teams/t1/team.json");
                    let mut v: serde_json::Value = read_private_json(&path).unwrap();
                    v["mission"] = json!("changed");
                    write_private_json_atomic(&path, &v, true).unwrap();
                }
            }
            assert!(binding
                .publish_with_password(&r, &t, &Deadline::new(), "")
                .is_err());
            assert!(!generation_dir(&f.home, "t1-worker", 1).exists());
        }
    }
    #[test]
    fn executable_caps_and_private_script_caps_remain_distinct() {
        let f = Fixture::new();
        let script = f.home.join("tools/large");
        let file = std::fs::File::create(&script).unwrap();
        file.set_len(128 * 1024 * 1024 + 1).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(installed(&script, false).is_err());
        assert!(installed(&script, true).is_ok());
        file.set_len(EXECUTABLE_CAP + 1).unwrap();
        assert!(installed(&script, true).is_err());
    }
}
