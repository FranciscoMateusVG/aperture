//! Native managed Codex configuration. Fixed runtime/installed-tool paths only;
//! mission repository/worktree authority is supplied by BoundRepository.
//! Never the legacy supervisor (reuse/respawn), legacy model override or shell
//! launcher. No credentials/configuration values implement Debug or Serialize.
use super::{deadline::Deadline, launch_gate::LaunchSpec, ReplacementError};
use crate::journal::{
    ensure_private_dir, open_private_file_nofollow, read_private_json, validate_component_path,
    write_private_bytes_atomic,
};
use crate::owner::{OwnerRecord, OwnerStore, StartReservation};
use crate::state::{ExecutionTuple, Harness, OwnerState};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

const BYTE_CAP: usize = 2 * 1024 * 1024;
const INSTALLED_FILE_CAP: u64 = 128 * 1024 * 1024;
// Native Codex distributions exceed 128 MiB; scripts retain the smaller bound.
const CODEX_EXECUTABLE_CAP: u64 = 512 * 1024 * 1024;
fn error() -> ReplacementError {
    ReplacementError::LaunchUnavailable
}
fn hash(b: &[u8]) -> String {
    format!("{:x}", Sha256::digest(b))
}
struct PrivateBytes(Vec<u8>);
impl Drop for PrivateBytes {
    fn drop(&mut self) {
        self.0.fill(0);
    }
}
fn bytes(path: &Path, cap: usize) -> Result<PrivateBytes, ReplacementError> {
    let f = open_private_file_nofollow(path).map_err(|_| error())?;
    let mut out = PrivateBytes(vec![]);
    f.take(cap as u64 + 1)
        .read_to_end(&mut out.0)
        .map_err(|_| error())?;
    if out.0.len() > cap {
        return Err(error());
    }
    Ok(out)
}
fn private_dir(path: &Path) -> Result<(), ReplacementError> {
    let m = std::fs::symlink_metadata(path).map_err(|_| error())?;
    if !m.is_dir()
        || m.file_type().is_symlink()
        || m.uid() != unsafe { libc::geteuid() }
        || m.mode() & 0o077 != 0
    {
        return Err(error());
    }
    Ok(())
}
pub(crate) fn installed_file(path: &Path) -> Result<String, ReplacementError> {
    installed_file_with_cap(path, INSTALLED_FILE_CAP)
}
fn installed_pin(path: &Path, executable: &Path) -> Result<String, ReplacementError> {
    if path == executable {
        installed_file_with_cap(path, CODEX_EXECUTABLE_CAP)
    } else {
        installed_file(path)
    }
}
pub(crate) fn installed_file_with_cap(path: &Path, cap: u64) -> Result<String, ReplacementError> {
    let m = std::fs::symlink_metadata(path).map_err(|_| error())?;
    if !m.is_file()
        || m.file_type().is_symlink()
        || m.nlink() != 1
        || m.mode() & 0o022 != 0
        || (m.uid() != unsafe { libc::geteuid() } && m.uid() != 0)
    {
        return Err(error());
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| error())?;
    let now = file.metadata().map_err(|_| error())?;
    if now.ino() != m.ino() || now.dev() != m.dev() || now.len() > cap {
        return Err(error());
    }
    let mut h = Sha256::new();
    let mut f = file.take(cap + 1);
    let mut total = 0u64;
    let mut buffer = [0u8; 16384];
    loop {
        let n = f.read(&mut buffer).map_err(|_| error())?;
        if n == 0 {
            break;
        }
        total += n as u64;
        if total > cap {
            return Err(error());
        }
        h.update(&buffer[..n]);
    }
    Ok(format!("{:x}", h.finalize()))
}
fn binary(home: &Path) -> Result<PathBuf, ReplacementError> {
    for candidate in [
        home.join(".npm-global/bin/codex"),
        PathBuf::from("/opt/homebrew/bin/codex"),
        PathBuf::from("/usr/local/bin/codex"),
    ] {
        if std::fs::symlink_metadata(&candidate).is_err() {
            continue;
        }
        // Installed package-manager symlinks are not caller-selected paths.
        let resolved = std::fs::canonicalize(candidate).map_err(|_| error())?;
        installed_pin(&resolved, &resolved)?;
        if std::fs::metadata(&resolved).map_err(|_| error())?.mode() & 0o111 == 0 {
            return Err(error());
        }
        return Ok(resolved);
    }
    Err(error())
}
// Resolve the actual Node executable, not the Volta shim. No shell rc, caller
// path/env override or project-selected runtime. The existing bounded native
// subprocess primitive bounds output/time and reaps its process group.
pub(crate) fn node_binary(home: &Path, budget: &Deadline) -> Result<PathBuf, ReplacementError> {
    node_from_candidates(
        home,
        &[
            home.join(".volta/bin/node"),
            home.join(".npm-global/bin/node"),
            home.join(".local/bin/node"),
            PathBuf::from("/opt/homebrew/bin/node"),
            PathBuf::from("/usr/local/bin/node"),
            PathBuf::from("/usr/bin/node"),
        ],
        budget,
    )
}
fn node_from_candidates(
    home: &Path,
    candidates: &[PathBuf],
    budget: &Deadline,
) -> Result<PathBuf, ReplacementError> {
    for candidate in candidates {
        match std::fs::symlink_metadata(candidate) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return Err(error()),
            Ok(_) => {}
        }
        let executable = std::fs::canonicalize(candidate).map_err(|_| error())?;
        let before = installed_file(&executable)?;
        if std::fs::metadata(&executable).map_err(|_| error())?.mode() & 0o111 == 0 {
            return Err(error());
        }
        let mut command = std::process::Command::new(&executable);
        command
            .args(["-p", "process.execPath"])
            .current_dir(home)
            .env_clear()
            .env("HOME", home)
            .env("PATH", "/usr/bin:/bin");
        let output = super::repository::bounded_command(
            command,
            budget.forward_until(Duration::from_secs(3))?,
        )
        .map_err(|_| error())?;
        if output.is_empty() || output.len() > 4096 || installed_file(&executable)? != before {
            return Err(error());
        }
        let text = std::str::from_utf8(&output)
            .map_err(|_| error())?
            .trim_end_matches(['\r', '\n']);
        if text.chars().any(char::is_control) {
            return Err(error());
        }
        let path = PathBuf::from(text);
        if !path.is_absolute() {
            return Err(error());
        }
        let real = std::fs::canonicalize(&path).map_err(|_| error())?;
        // A shim returned as 'Node' is not proof of the actual child executable.
        if real.starts_with(home.join(".volta/bin")) {
            return Err(error());
        }
        installed_file(&real)?;
        if std::fs::metadata(&real).map_err(|_| error())?.mode() & 0o111 == 0 {
            return Err(error());
        }
        return Ok(real);
    }
    Err(error())
}
pub(crate) fn inventory(root: &Path, budget: &Deadline) -> Result<BTreeMap<String, String>, ReplacementError> {
    let mut out = BTreeMap::new();
    let mut pending = vec![(root.to_path_buf(), 0usize)];
    let mut count = 0;
    let mut total = 0;
    while let Some((dir, depth)) = pending.pop() {
        budget.forward(Duration::ZERO)?;
        private_dir(&dir)?;
        if depth > 8 {
            return Err(error());
        }
        for e in std::fs::read_dir(&dir).map_err(|_| error())? {
            let e = e.map_err(|_| error())?;
            count += 1;
            if count > 512 {
                return Err(error());
            }
            let p = e.path();
            let m = std::fs::symlink_metadata(&p).map_err(|_| error())?;
            if m.file_type().is_symlink() {
                return Err(error());
            }
            if m.is_dir() {
                pending.push((p, depth + 1));
            } else {
                let b = bytes(&p, BYTE_CAP)?;
                total += b.0.len();
                if total > BYTE_CAP {
                    return Err(error());
                }
                let rel = p
                    .strip_prefix(root)
                    .map_err(|_| error())?
                    .to_str()
                    .ok_or_else(error)?
                    .to_string();
                if rel.chars().any(char::is_control) {
                    return Err(error());
                }
                out.insert(rel, hash(&b.0));
            }
        }
    }
    if out.is_empty() {
        return Err(error());
    }
    Ok(out)
}
fn quote(s: &str) -> String {
    serde_json::to_string(s).expect("string encoding")
}
fn config(
    home: &Path,
    infra: &Path,
    node: &Path,
    seat: &str,
    role: &str,
    tuple: &ExecutionTuple,
    g: u64,
    cwd: &Path,
    codex_home: &Path,
    token: &Path,
    password: &str,
) -> Result<PrivateBytes, ReplacementError> {
    if tuple.harness != Harness::Codex {
        return Err(error());
    }
    let reasoning =
        serde_json::to_value(tuple.reasoning.as_ref().ok_or_else(error)?).map_err(|_| error())?;
    let r = reasoning.as_str().ok_or_else(error)?;
    let p = |p: &Path| p.to_str().ok_or_else(error).map(quote);
    let env = [
        ("AGENT_NAME", seat.to_string()),
        ("AGENT_ROLE", role.into()),
        ("AGENT_MODEL", format!("codex/{}", tuple.model)),
        ("APERTURE_TEAM_GENERATION", g.to_string()),
        (
            "APERTURE_HUB_TOKEN_FILE",
            token.to_string_lossy().into_owned(),
        ),
        ("HOME", home.to_string_lossy().into_owned()),
        (
            "BEADS_DIR",
            home.join(".aperture/.beads").to_string_lossy().into_owned(),
        ),
        ("BD_ACTOR", seat.into()),
        ("BEADS_DOLT_PASSWORD", password.into()),
        (
            "APERTURE_MAILBOX",
            home.join(".aperture/mailbox")
                .to_string_lossy()
                .into_owned(),
        ),
    ]
    .iter()
    .map(|(k, v)| format!("{k} = {}", quote(v)))
    .collect::<Vec<_>>()
    .join(", ");
    Ok(PrivateBytes(format!("model = {}\nmodel_reasoning_effort = {}\nmodel_instructions_file = {}\napproval_policy = \"never\"\nsandbox_mode = \"danger-full-access\"\n\n[projects.{}]\ntrust_level = \"trusted\"\n\n[mcp_servers.aperture-bus]\ncommand = {}\nargs = [{}]\nenv = {{ {} }}\n\n[mcp_servers.sentry]\ncommand = {}\nargs = [{}]\nenv = {{ {} }}\n",
        quote(&tuple.model),quote(r),p(&codex_home.join("prompt.md"))?,p(cwd)?,p(node)?,p(&infra.join("mcp-server/dist/index.js"))?,env,
        p(node)?,p(&infra.join("mcp-server-sentry/dist/index.js"))?,env).into_bytes()))
}
pub(crate) struct NativeLaunchBinding {
    home: PathBuf,
    infra: PathBuf,
    seat: String,
    team: String,
    tuple: ExecutionTuple,
    role: String,
    runtime: PathBuf,
    cwd: PathBuf,
    executable: PathBuf,
    node: PathBuf,
    snapshot: crate::teams::TeamSnapshot,
    pins: BTreeMap<PathBuf, String>,
    skills: BTreeMap<String, String>,
    worktree: Option<String>,
    cwd_identity: (u64, u64),
    recovery: Option<crate::team_checkpoint::CheckpointPayload>,
}
impl NativeLaunchBinding {
    pub(crate) fn preflight(
        home: &Path,
        team: &str,
        seat: &str,
        tuple: &ExecutionTuple,
        repo: &super::repository::BoundRepository,
        checkpoint: Option<&crate::team_checkpoint::CheckpointEntry>,
        budget: &Deadline,
    ) -> Result<Self, ReplacementError> {
        Self::preflight_selected(
            home,
            team,
            seat,
            tuple,
            repo,
            checkpoint.map(|e| e.payload.worktree.as_str()),
            budget,
        )
    }
    pub(crate) fn preflight_selected(
        home: &Path,
        team: &str,
        seat: &str,
        tuple: &ExecutionTuple,
        repo: &super::repository::BoundRepository,
        worktree: Option<&str>,
        budget: &Deadline,
    ) -> Result<Self, ReplacementError> {
        let worktree = worktree.map(str::to_string);
        let cwd = match &worktree {
            Some(w) => super::repository::replacement_cwd(
                repo,
                w,
                budget.forward_until(Duration::from_secs(10))?,
            )
            .map_err(|_| error())?,
            None => repo.bootstrap_cwd().map_err(|_| error())?,
        };
        let infra = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .ok_or_else(error)?
            .to_path_buf();
        let executable = binary(home)?;
        let mut bound =
            Self::preflight_at(home, team, seat, tuple, cwd, budget, infra, executable)?;
        bound.worktree = worktree;
        Ok(bound)
    }
    fn preflight_at(
        home: &Path,
        team: &str,
        seat: &str,
        tuple: &ExecutionTuple,
        cwd: PathBuf,
        budget: &Deadline,
        infra: PathBuf,
        executable: PathBuf,
    ) -> Result<Self, ReplacementError> {
        budget.forward(Duration::ZERO)?;
        if tuple.harness != Harness::Codex
            || !cwd.is_absolute()
            || !crate::agent_loader::is_valid_seat_name(seat)
            || !crate::agent_loader::is_valid_seat_name(team)
        {
            return Err(error());
        }
        let runtime = validate_component_path(&home.join(".claude/aperture"), seat, false)
            .map_err(|_| error())?;
        private_dir(&runtime)?;
        let snapshot: crate::teams::TeamSnapshot =
            read_private_json(&home.join(".aperture/teams").join(team).join("team.json"))
                .map_err(|_| error())?;
        let seats: Vec<_> = snapshot.seats.iter().filter(|s| s.name == seat).collect();
        if snapshot.team != team || seats.len() != 1 || seats[0].harness != Harness::Codex {
            return Err(error());
        }
        let s = seats[0];
        let configured = ExecutionTuple {
            harness: s.harness.clone(),
            model: s.model.clone(),
            reasoning: s.reasoning.clone(),
        };
        if *tuple != configured && !snapshot.fallbacks.contains(tuple) {
            return Err(error());
        }
        let node = node_binary(home, budget)?;
        let mut pins = BTreeMap::new();
        let manifest: serde_json::Value =
            read_private_json(&runtime.join("manifest.json")).map_err(|_| error())?;
        let marker: serde_json::Value =
            read_private_json(&runtime.join("TEAM")).map_err(|_| error())?;
        if manifest.get("name").and_then(|v| v.as_str()) != Some(seat)
            || manifest.get("role").and_then(|v| v.as_str()) != Some(s.role.as_str())
            || manifest.get("model").and_then(|v| v.as_str())
                != Some(format!("codex/{}", s.model).as_str())
            || manifest.get("enabled").and_then(|v| v.as_bool()) != Some(true)
            || marker.get("team").and_then(|v| v.as_str()) != Some(team)
            || marker.get("role").and_then(|v| v.as_str()) != Some(s.role.as_str())
            || marker.get("schema_version").and_then(|v| v.as_u64()) != Some(1)
        {
            return Err(error());
        }
        for name in [
            "prompt.md",
            "manifest.json",
            "TEAM",
            ".complete",
            "resident.txt",
        ] {
            let p = runtime.join(name);
            pins.insert(p.clone(), hash(&bytes(&p, BYTE_CAP)?.0));
        }
        let skills = inventory(&runtime.join("skills"), budget)?;
        let auth = home.join(".codex/auth.json");
        // Preflight opens metadata only; the authorized native consumer seeds
        // private auth at publication, never returns it in a DTO/log/argv.
        open_private_file_nofollow(&auth).map_err(|_| error())?;
        for p in [
            executable.clone(),
            node.clone(),
            infra.join("mcp-server/dist/index.js"),
            infra.join("mcp-server-sentry/dist/index.js"),
        ] {
            pins.insert(p.clone(), installed_pin(&p, &executable)?);
        }
        let meta = std::fs::symlink_metadata(&cwd).map_err(|_| error())?;
        if !meta.is_dir() || meta.file_type().is_symlink() {
            return Err(error());
        }
        let cwd_identity = (meta.dev(), meta.ino());
        Ok(Self {
            home: home.into(),
            infra,
            seat: seat.into(),
            team: team.into(),
            tuple: tuple.clone(),
            role: s.role.clone(),
            runtime,
            cwd,
            executable,
            node,
            snapshot: snapshot.clone(),
            pins,
            skills,
            worktree: None,
            cwd_identity,
            recovery: None,
        })
    }
    pub(crate) fn bind_recovery(
        &mut self,
        entry: Option<&crate::team_checkpoint::CheckpointEntry>,
    ) -> Result<(), ReplacementError> {
        if let Some(e) = entry {
            if e.team != self.team
                || e.seat != self.seat
                || self.worktree.as_deref() != Some(e.payload.worktree.as_str())
                || e.validation != crate::team_checkpoint::CheckpointValidation::Ok
            {
                return Err(error());
            }
            crate::team_checkpoint::validate_payload(&e.payload, &[]).map_err(|_| error())?;
            self.recovery = Some(e.payload.clone());
        }
        Ok(())
    }
    pub(crate) fn revalidate(&self, budget: &Deadline) -> Result<(), ReplacementError> {
        budget.forward(Duration::ZERO)?;
        let snapshot: crate::teams::TeamSnapshot = read_private_json(
            &self
                .home
                .join(".aperture/teams")
                .join(&self.team)
                .join("team.json"),
        )
        .map_err(|_| error())?;
        if snapshot != self.snapshot {
            return Err(error());
        }
        let repo = super::repository::resolve_native(
            &self.home,
            &self.team,
            budget.forward_until(Duration::from_secs(10))?,
        )
        .map_err(|_| error())?;
        let cwd = match &self.worktree {
            Some(w) => super::repository::replacement_cwd(
                &repo,
                w,
                budget.forward_until(Duration::from_secs(10))?,
            )
            .map_err(|_| error())?,
            None => repo.bootstrap_cwd().map_err(|_| error())?,
        };
        let meta = std::fs::symlink_metadata(&cwd).map_err(|_| error())?;
        if cwd != self.cwd || (meta.dev(), meta.ino()) != self.cwd_identity {
            return Err(error());
        }
        if std::fs::metadata(&self.node).map_err(|_| error())?.mode() & 0o111 == 0 {
            return Err(error());
        }
        for (p, before) in &self.pins {
            let now = if p.starts_with(&self.runtime) {
                hash(&bytes(p, BYTE_CAP)?.0)
            } else {
                installed_pin(p, &self.executable)?
            };
            if now != *before {
                return Err(error());
            }
        }
        if inventory(&self.runtime.join("skills"), budget)? != self.skills {
            return Err(error());
        }
        Ok(())
    }
    /// No overwrite/reuse of generation homes. A failed publication stays as
    /// evidence and is not silently cleaned up or retried by this operation.
    pub(crate) fn publish(
        &self,
        res: &StartReservation,
        token: &crate::hub_auth::managed::ManagedToken,
        budget: &Deadline,
    ) -> Result<LaunchSpec, ReplacementError> {
        let password = std::env::var("BEADS_DOLT_PASSWORD").unwrap_or_default();
        self.publish_with_password(res, token, budget, &password)
    }
    fn publish_with_password(
        &self,
        res: &StartReservation,
        token: &crate::hub_auth::managed::ManagedToken,
        budget: &Deadline,
        password: &str,
    ) -> Result<LaunchSpec, ReplacementError> {
        self.revalidate(budget)?;
        if res.seat != self.seat || token.generation() != res.generation {
            return Err(error());
        }
        let _team = crate::owner::try_lock(&self.home.join(".aperture/run/team-locks"), &self.team)
            .map_err(|_| error())?;
        let store = OwnerStore::new(self.home.join(".aperture/run/owner"));
        let _seat = store.lock(&self.seat).map_err(|_| error())?;
        let owner: OwnerRecord =
            read_private_json(&store.record_path(&self.seat)).map_err(|_| error())?;
        if owner.state != OwnerState::Starting
            || owner.generation != res.generation
            || owner.incarnation.is_some()
            || owner.requested != self.tuple
            || owner.provisional_token_id.as_deref() != Some(token.token_id())
            || owner.reservation_nonce_sha256.as_deref()
                != Some(hash(res.nonce().as_bytes()).as_str())
        {
            return Err(error());
        }
        let base = validate_component_path(&self.home.join(".aperture/run"), "managed", true)
            .map_err(|_| error())?;
        ensure_private_dir(&base).map_err(|_| error())?;
        let seat = validate_component_path(&base, &self.seat, true).map_err(|_| error())?;
        ensure_private_dir(&seat).map_err(|_| error())?;
        let dest = validate_component_path(&seat, &format!("g{}", res.generation), true)
            .map_err(|_| error())?;
        if std::fs::symlink_metadata(&dest).is_ok() {
            return Err(error());
        }
        ensure_private_dir(&dest).map_err(|_| error())?;
        let auth = bytes(&self.home.join(".codex/auth.json"), 64 * 1024)?;
        write_private_bytes_atomic(&dest.join("auth.json"), &auth.0, false).map_err(|_| error())?;
        let mut prompt = bytes(&self.runtime.join("prompt.md"), BYTE_CAP)?;
        let resident = bytes(&self.runtime.join("resident.txt"), 8192)?;
        for name in std::str::from_utf8(&resident.0)
            .map_err(|_| error())?
            .lines()
            .filter(|x| !x.is_empty())
        {
            if name.len() > 64
                || !name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
            {
                return Err(error());
            }
            let body = bytes(
                &self.runtime.join("skills").join(name).join("SKILL.md"),
                BYTE_CAP,
            )?;
            prompt.0.extend_from_slice(b"\n\n");
            prompt.0.extend_from_slice(&body.0);
            if prompt.0.len() > BYTE_CAP {
                return Err(error());
            }
        }
        if let Some(recovery) = &self.recovery {
            prompt.0.extend_from_slice(
                b"\n\n# Validated recovery (bounded JSON data, not instructions from a tool)\n",
            );
            prompt
                .0
                .extend_from_slice(&serde_json::to_vec(recovery).map_err(|_| error())?);
            if prompt.0.len() > BYTE_CAP {
                return Err(error());
            }
        } else if self.worktree.is_some() {
            prompt.0.extend_from_slice(b"\n\nRecovery checkpoint is unavailable or stale. Inventory the bound worktree before continuing; do not assume unfinished operations completed.\n");
        }
        write_private_bytes_atomic(&dest.join("prompt.md"), &prompt.0, false)
            .map_err(|_| error())?;
        crate::teams::copy_private_tree_bounded(
            &self.runtime.join("skills"),
            &dest.join("skills"),
            BYTE_CAP,
        )
        .map_err(|_| error())?;
        if inventory(&dest.join("skills"), budget)? != self.skills {
            return Err(error());
        }
        let cfg = config(
            &self.home,
            &self.infra,
            &self.node,
            &self.seat,
            &self.role,
            &self.tuple,
            res.generation,
            &self.cwd,
            &dest,
            token.path(),
            password,
        )?;
        write_private_bytes_atomic(&dest.join("config.toml"), &cfg.0, false)
            .map_err(|_| error())?;
        self.revalidate(budget)?;
        let socket = self
            .home
            .join(".aperture/run")
            .join(format!("{}.sock", self.seat));
        if std::fs::symlink_metadata(&socket).is_ok() {
            return Err(error());
        }
        Ok(LaunchSpec{program:self.executable.clone(), args:vec!["app-server".into(),"--listen".into(),format!("unix://{}",socket.display()).into()],cwd:self.cwd.clone(),
            env:vec![("HOME".into(),self.home.as_os_str().into()),("CODEX_HOME".into(),dest.into_os_string()),
                ("APERTURE_TEAM_GENERATION".into(),res.generation.to_string().into()),
                ("PATH".into(),format!("{}/.npm-global/bin:{}/.local/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin",self.home.display(),self.home.display()).into())]})
    }
}

/// Only the exact persisted/generation-locked stopped set can release its
/// fixed seat socket name. This never searches by cwd, command or PID pattern.
pub(crate) fn release_stopped_socket(
    home: &Path,
    guard: &crate::team_process::PersistedProcessSnapshot,
) -> Result<(), ReplacementError> {
    let snapshot = guard.snapshot();
    if !snapshot.complete
        || !snapshot.unowned_matches.is_empty()
        || snapshot
            .processes
            .iter()
            .any(|p| crate::team_process::state(&p.identity) != super::ProcessState::Gone)
    {
        return Err(ReplacementError::StopUnverified);
    }
    unlink_socket(home, &snapshot.seat)
}
fn unlink_socket(home: &Path, seat: &str) -> Result<(), ReplacementError> {
    use std::os::unix::fs::FileTypeExt;
    if !crate::agent_loader::is_valid_seat_name(seat) {
        return Err(error());
    }
    let root = home.join(".aperture/run");
    let path =
        validate_component_path(&root, &format!("{seat}.sock"), true).map_err(|_| error())?;
    let meta = match std::fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Ok(m) => m,
        _ => return Err(error()),
    };
    if !meta.file_type().is_socket()
        || meta.uid() != unsafe { libc::geteuid() }
        || meta.nlink() != 1
    {
        return Err(error());
    }
    let checked =
        validate_component_path(&root, &format!("{seat}.sock"), false).map_err(|_| error())?;
    let now = std::fs::symlink_metadata(&checked).map_err(|_| error())?;
    if (now.dev(), now.ino()) != (meta.dev(), meta.ino()) || !now.file_type().is_socket() {
        return Err(error());
    }
    std::fs::remove_file(&checked).map_err(|_| error())?;
    crate::journal::sync_dir(&root).map_err(|_| error())?;
    if !matches!(std::fs::symlink_metadata(&checked),Err(e) if e.kind()==std::io::ErrorKind::NotFound)
    {
        return Err(error());
    }
    Ok(())
}

#[cfg(test)]
#[path = "team_launch_native_tests.rs"]
mod tests;
