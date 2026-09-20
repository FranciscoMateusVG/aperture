//! Read-only native repository/worktree evidence. Catalog authority is supplied
//! only by Rex's immutable team binding; no worker/preset/cwd path authority.
use crate::team_checkpoint::{ArtifactObservation, CheckpointEntry, PullRequestRef};
use std::collections::HashSet;
use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::CommandExt;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const OUTPUT_CAP: usize = 2 * 1024 * 1024;
const WORKTREE_CAP: usize = 4096;
const FILE_CAP: usize = 200;
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RepositoryError {
    Binding,
    Unsafe,
    Unbound,
    Ambiguous,
    Limit,
    Timeout,
    Io,
    Git,
    PullRequest,
}
impl RepositoryError {
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::Binding => "E_REPO_BINDING_UNAVAILABLE",
            Self::Unbound | Self::Ambiguous => "E_WORKTREE_UNBOUND",
            Self::Unsafe => "E_REPO_PATH_UNSAFE",
            Self::Limit => "E_ARTIFACT_LIMIT",
            Self::Timeout => "E_ARTIFACT_TIMEOUT",
            Self::PullRequest => "E_PR_UNVERIFIED",
            Self::Io | Self::Git => "E_ARTIFACT_UNVERIFIED",
        }
    }
}
#[derive(Clone, PartialEq, Eq)]
struct DirectoryIdentity {
    dev: u64,
    ino: u64,
}
fn directory(path: &Path) -> Result<DirectoryIdentity, RepositoryError> {
    let m = std::fs::symlink_metadata(path).map_err(|_| RepositoryError::Unsafe)?;
    if !m.is_dir()
        || m.file_type().is_symlink()
        || m.uid() != unsafe { libc::geteuid() }
        || m.mode() & 0o022 != 0
    {
        return Err(RepositoryError::Unsafe);
    }
    Ok(DirectoryIdentity {
        dev: m.dev(),
        ino: m.ino(),
    })
}
fn relative(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 4096
        && !value.chars().any(char::is_control)
        && !value.starts_with('/')
        && value
            .split('/')
            .all(|p| !p.is_empty() && p != "." && p != "..")
        && Path::new(value)
            .components()
            .all(|p| matches!(p, Component::Normal(_)))
}
fn descend(root: &Path, rel: &str) -> Result<PathBuf, RepositoryError> {
    if !relative(rel) {
        return Err(RepositoryError::Unsafe);
    }
    directory(root)?;
    let mut path = root.to_path_buf();
    for part in Path::new(rel).components() {
        path.push(part);
        directory(&path)?;
    }
    Ok(path)
}
// Not Serialize/Deserialize. Constructor remains private and must be called
// only by the catalog-binding entrypoint after exact immutable-key resolution.
pub(crate) struct BoundRepository {
    root: PathBuf,
    worktrees: PathBuf,
    root_id: DirectoryIdentity,
    projects: PathBuf,
    repo_key: String,
    snapshot: Option<(PathBuf, crate::teams::TeamSnapshot)>,
}
impl BoundRepository {
    fn from_catalog_root(
        home: &Path,
        repo_key: &str,
        root: PathBuf,
    ) -> Result<Self, RepositoryError> {
        if repo_key.is_empty()
            || repo_key.len() > 64
            || !repo_key
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
            || !repo_key.as_bytes()[0].is_ascii_alphanumeric()
        {
            return Err(RepositoryError::Binding);
        }
        directory(home)?;
        let projects = descend(home, "projects")?;
        let expected = descend(&projects, repo_key)?;
        if root != expected {
            return Err(RepositoryError::Binding);
        }
        directory(&root.join(".git"))?;
        let root_id = directory(&root)?;
        Ok(Self {
            root,
            worktrees: projects.join(format!("{repo_key}-worktrees")),
            root_id,
            projects,
            repo_key: repo_key.into(),
            snapshot: None,
        })
    }
    fn revalidate(&self) -> Result<(), RepositoryError> {
        if let Some((path, before)) = &self.snapshot {
            let current: crate::teams::TeamSnapshot =
                crate::journal::read_private_json(path).map_err(|_| RepositoryError::Binding)?;
            if current != *before {
                return Err(RepositoryError::Binding);
            }
        }
        directory(self.projects.parent().ok_or(RepositoryError::Unsafe)?)?;
        directory(&self.projects)?;
        if descend(&self.projects, &self.repo_key)? != self.root
            || directory(&self.root)? != self.root_id
        {
            return Err(RepositoryError::Unsafe);
        }
        directory(&self.root.join(".git"))?;
        Ok(())
    }
    /// Bootstrap only: selected repo root is not permission to edit main.
    pub(crate) fn bootstrap_cwd(&self) -> Result<PathBuf, RepositoryError> {
        self.revalidate()?;
        Ok(self.root.clone())
    }
}
/// Read fixed private snapshot, then the ONE native catalog. This performs no
/// launch or grant; the caller still derives/revalidates its action authority.
pub(crate) fn resolve_native(
    home: &Path,
    team: &str,
    deadline: Instant,
) -> Result<BoundRepository, RepositoryError> {
    if team.len() > 16 || !crate::agent_loader::is_valid_seat_name(team) {
        return Err(RepositoryError::Binding);
    }
    let path = home.join(".aperture/teams").join(team).join("team.json");
    let snapshot: crate::teams::TeamSnapshot =
        crate::journal::read_private_json(&path).map_err(|_| RepositoryError::Binding)?;
    if snapshot.schema_version != 1 || snapshot.team != team {
        return Err(RepositoryError::Binding);
    }
    let root = crate::teams::resolve_repository(home, &snapshot.project, &snapshot.repo)
        .map_err(|_| RepositoryError::Binding)?;
    let mut bound = BoundRepository::from_catalog_root(home, &snapshot.repo, root)?;
    let mut commands = NativeCommands { deadline };
    // A directory merely called .git is not proof of a real non-bare repo.
    if line(commands.git(&bound.root, &["rev-parse", "--is-bare-repository"])?)? != "false"
        || Path::new(&line(
            commands.git(&bound.root, &["rev-parse", "--show-toplevel"])?,
        )?) != bound.root
    {
        return Err(RepositoryError::Binding);
    }
    bound.snapshot = Some((path, snapshot));
    bound.revalidate()?;
    Ok(bound)
}
/// Private path result is consumed only by native LaunchSpec. It is not emitted
/// in the UI/BEADS receipt and does not authorize arbitrary repo edits.
pub(crate) fn replacement_cwd(
    repo: &BoundRepository,
    relative: &str,
    deadline: Instant,
) -> Result<PathBuf, RepositoryError> {
    registered_worktree(repo, relative, &mut NativeCommands { deadline })
}
pub(crate) fn collect_native(
    repo: &BoundRepository,
    entry: &CheckpointEntry,
    deadline: Instant,
) -> Result<ArtifactObservation, RepositoryError> {
    collect(repo, entry, &mut NativeCommands { deadline })
}

fn read_metadata_file(path: &Path) -> Result<Vec<u8>, RepositoryError> {
    use std::os::unix::fs::OpenOptionsExt;
    let f = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| RepositoryError::Unsafe)?;
    let m = f.metadata().map_err(|_| RepositoryError::Unsafe)?;
    if !m.is_file()
        || m.uid() != unsafe { libc::geteuid() }
        || m.nlink() != 1
        || m.mode() & 0o022 != 0
    {
        return Err(RepositoryError::Unsafe);
    }
    let mut bytes = vec![];
    f.take(8193)
        .read_to_end(&mut bytes)
        .map_err(|_| RepositoryError::Io)?;
    if bytes.len() > 8192 {
        return Err(RepositoryError::Limit);
    }
    Ok(bytes)
}
fn gitdir_binding(repo: &BoundRepository, path: &Path) -> Result<(), RepositoryError> {
    let bytes = read_metadata_file(&path.join(".git"))?;
    let text = std::str::from_utf8(&bytes).map_err(|_| RepositoryError::Unsafe)?;
    let raw = text
        .strip_suffix('\n')
        .unwrap_or(text)
        .strip_prefix("gitdir: ")
        .ok_or(RepositoryError::Unsafe)?;
    let native = repo.root.join(".git/worktrees");
    let rel = Path::new(raw)
        .strip_prefix(&native)
        .map_err(|_| RepositoryError::Unbound)?
        .to_str()
        .ok_or(RepositoryError::Unsafe)?;
    let gitdir = descend(&native, rel)?;
    if gitdir != Path::new(raw) {
        return Err(RepositoryError::Unsafe);
    }
    // Native linked-worktree layout only; refuse a commondir redirect outside
    // the selected repository before invoking Git in the worktree.
    if read_metadata_file(&gitdir.join("commondir"))? != b"../..\n" {
        return Err(RepositoryError::Unbound);
    }
    let back = read_metadata_file(&gitdir.join("gitdir"))?;
    let back = std::str::from_utf8(&back)
        .map_err(|_| RepositoryError::Unsafe)?
        .trim_end_matches('\n');
    if Path::new(back) != path.join(".git") {
        return Err(RepositoryError::Unbound);
    }
    Ok(())
}
fn membership(bytes: &[u8], wanted: &Path) -> Result<(), RepositoryError> {
    if bytes.len() > OUTPUT_CAP || !bytes.ends_with(&[0]) {
        return Err(RepositoryError::Limit);
    }
    let mut count = 0;
    let mut matches = 0;
    for record in bytes
        .split(|b| *b == 0)
        .collect::<Vec<_>>()
        .split(|field| field.is_empty())
    {
        if record.is_empty() {
            continue;
        }
        count += 1;
        if count > WORKTREE_CAP {
            return Err(RepositoryError::Limit);
        }
        let first = std::str::from_utf8(record[0]).map_err(|_| RepositoryError::Unsafe)?;
        let path = first
            .strip_prefix("worktree ")
            .ok_or(RepositoryError::Git)?;
        if record.iter().skip(1).any(|f| f.starts_with(b"worktree ")) {
            return Err(RepositoryError::Ambiguous);
        }
        if Path::new(path) == wanted {
            if record
                .iter()
                .any(|f| f.starts_with(b"prunable") || *f == b"bare")
            {
                return Err(RepositoryError::Unbound);
            }
            matches += 1;
        }
    }
    match matches {
        1 => Ok(()),
        0 => Err(RepositoryError::Unbound),
        _ => Err(RepositoryError::Ambiguous),
    }
}
// Private command seam. Production chooses fixed executables/arguments below;
// only isolated tests may supply canned command output.
trait ReadCommands {
    fn git(&mut self, cwd: &Path, args: &[&str]) -> Result<Vec<u8>, RepositoryError>;
    fn prs(&mut self, cwd: &Path, repo: &str, branch: &str) -> Result<Vec<u8>, RepositoryError>;
}
struct NativeCommands {
    deadline: Instant,
}
impl ReadCommands for NativeCommands {
    fn git(&mut self, cwd: &Path, args: &[&str]) -> Result<Vec<u8>, RepositoryError> {
        let mut command = Command::new("/usr/bin/git");
        command
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("LC_ALL", "C")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("GIT_TERMINAL_PROMPT", "0")
            .args([
                "-c",
                "core.fsmonitor=false",
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "core.untrackedCache=false",
            ])
            .args(args)
            .current_dir(cwd);
        bounded_command(command, self.deadline)
    }
    fn prs(&mut self, cwd: &Path, repo: &str, branch: &str) -> Result<Vec<u8>, RepositoryError> {
        let mut command = Command::new("/opt/homebrew/bin/gh");
        command
            .current_dir(cwd)
            .env("GH_HOST", "github.com")
            .env("GH_PROMPT_DISABLED", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .args([
                "pr",
                "list",
                "--repo",
                repo,
                "--head",
                branch,
                "--state",
                "open",
                "--limit",
                "2",
                "--json",
                "number,headRefOid",
            ]);
        bounded_command(command, self.deadline)
    }
}
pub(crate) fn bounded_command(
    mut command: Command,
    deadline: Instant,
) -> Result<Vec<u8>, RepositoryError> {
    if Instant::now() >= deadline {
        return Err(RepositoryError::Timeout);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    // No interpolation. Child has a private group; timeout is not a retry.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
    let mut child = command.spawn().map_err(|_| RepositoryError::Io)?;
    let mut stdout = child.stdout.take().ok_or(RepositoryError::Io)?;
    let fd = stdout.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        let _ = child.kill();
        let _ = child.wait();
        return Err(RepositoryError::Io);
    }
    let mut result = vec![];
    let mut exited = None;
    let mut eof = false;
    let failure = loop {
        let mut block = [0u8; 8192];
        match stdout.read(&mut block) {
            Ok(0) => eof = true,
            Ok(n) => {
                result.extend_from_slice(&block[..n]);
                if result.len() > OUTPUT_CAP {
                    break RepositoryError::Limit;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(_) => break RepositoryError::Io,
        }
        if exited.is_none() {
            match child.try_wait() {
                Ok(v) => exited = v,
                Err(_) => break RepositoryError::Io,
            }
        }
        if eof {
            if let Some(status) = exited {
                return if status.success() {
                    Ok(result)
                } else {
                    Err(RepositoryError::Git)
                };
            }
        }
        if Instant::now() >= deadline {
            break RepositoryError::Timeout;
        }
        std::thread::sleep(Duration::from_millis(2));
    };
    // Signal only while this owned, unreaped child still anchors its group.
    // If a completed child left an inherited pipe open, return uncertainty;
    // never signal a potentially reused group id based on that stale PID.
    if exited.is_none() {
        unsafe {
            libc::kill(-(child.id() as i32), libc::SIGKILL);
        }
        let _ = child.wait();
    }
    Err(failure)
}
fn registered_worktree<C: ReadCommands>(
    repo: &BoundRepository,
    relative: &str,
    commands: &mut C,
) -> Result<PathBuf, RepositoryError> {
    repo.revalidate()?;
    let path = descend(&repo.worktrees, relative)?;
    let id = directory(&path)?;
    gitdir_binding(repo, &path)?;
    let bytes = commands.git(&repo.root, &["worktree", "list", "--porcelain", "-z"])?;
    membership(&bytes, &path)?;
    repo.revalidate()?;
    gitdir_binding(repo, &path)?;
    if directory(&path)? != id {
        return Err(RepositoryError::Unsafe);
    }
    Ok(path)
}
fn line(bytes: Vec<u8>) -> Result<String, RepositoryError> {
    let s = String::from_utf8(bytes).map_err(|_| RepositoryError::Git)?;
    let value = s.strip_suffix('\n').unwrap_or(&s);
    if value.is_empty() || value.len() > 4096 || value.chars().any(char::is_control) {
        return Err(RepositoryError::Git);
    }
    Ok(value.into())
}
fn sha(s: &str) -> bool {
    s.len() == 40
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn github_repo(raw: &str) -> Result<String, RepositoryError> {
    let name = raw
        .strip_prefix("git@github.com:")
        .or_else(|| raw.strip_prefix("https://github.com/"))
        .ok_or(RepositoryError::PullRequest)?;
    let name = name.strip_suffix(".git").unwrap_or(name);
    let parts: Vec<_> = name.split('/').collect();
    if parts.len() != 2
        || parts.iter().any(|p| {
            p.is_empty()
                || p.len() > 100
                || *p == "."
                || *p == ".."
                || !p
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        })
    {
        return Err(RepositoryError::PullRequest);
    }
    Ok(name.into())
}
fn dirty(bytes: &[u8]) -> Result<Vec<String>, RepositoryError> {
    if !bytes.is_empty() && !bytes.ends_with(&[0]) {
        return Err(RepositoryError::Git);
    }
    let mut fields = bytes.split(|b| *b == 0).peekable();
    let mut paths = HashSet::new();
    while let Some(field) = fields.next() {
        if field.is_empty() {
            if fields.peek().is_none() {
                break;
            }
            return Err(RepositoryError::Git);
        }
        if field.len() < 4
            || field[2] != b' '
            || !field[..2].iter().all(|b| b" MADRCU?!".contains(b))
        {
            return Err(RepositoryError::Git);
        }
        let name = std::str::from_utf8(&field[3..]).map_err(|_| RepositoryError::Unsafe)?;
        if !relative(name) || name.len() > 512 {
            return Err(RepositoryError::Unsafe);
        }
        paths.insert(name.to_owned());
        if field[..2].contains(&b'R') || field[..2].contains(&b'C') {
            let old = std::str::from_utf8(fields.next().ok_or(RepositoryError::Git)?)
                .map_err(|_| RepositoryError::Unsafe)?;
            if !relative(old) || old.len() > 512 {
                return Err(RepositoryError::Unsafe);
            }
            paths.insert(old.into());
        }
        if paths.len() > FILE_CAP {
            return Err(RepositoryError::Limit);
        }
    }
    let mut paths: Vec<_> = paths.into_iter().collect();
    paths.sort();
    Ok(paths)
}
fn collect<C: ReadCommands>(
    repo: &BoundRepository,
    entry: &CheckpointEntry,
    commands: &mut C,
) -> Result<ArtifactObservation, RepositoryError> {
    let path = registered_worktree(repo, &entry.payload.worktree, commands)?;
    let head = line(commands.git(&path, &["rev-parse", "--verify", "HEAD"])?)?;
    if !sha(&head) {
        return Err(RepositoryError::Git);
    }
    let branch = line(commands.git(&path, &["symbolic-ref", "--quiet", "--short", "HEAD"])?)?;
    if branch != entry.payload.branch {
        return Err(RepositoryError::Unbound);
    }
    let status = commands.git(
        &path,
        &[
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--ignore-submodules=all",
        ],
    )?;
    let dirty_files = dirty(&status)?;
    let remote = github_repo(&line(
        commands.git(&repo.root, &["config", "--get", "remote.origin.url"])?,
    )?)?;
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Pr {
        number: u64,
        #[serde(rename = "headRefOid")]
        head: String,
    }
    let prs: Vec<Pr> = serde_json::from_slice(&commands.prs(&path, &remote, &branch)?)
        .map_err(|_| RepositoryError::PullRequest)?;
    if prs.len() > 1 {
        return Err(RepositoryError::Ambiguous);
    }
    let open_pr = match prs.into_iter().next() {
        None => None,
        Some(pr) => {
            if pr.number == 0 || !sha(&pr.head) {
                return Err(RepositoryError::PullRequest);
            }
            Some(PullRequestRef {
                repository: remote,
                number: pr.number,
                head_sha: pr.head,
            })
        }
    };
    // Recheck native membership, HEAD and dirty projection after the external
    // read; a changing checkout cannot become a validation fact by mixing times.
    if registered_worktree(repo, &entry.payload.worktree, commands)? != path
        || line(commands.git(&path, &["rev-parse", "--verify", "HEAD"])?)? != head
        || commands.git(
            &path,
            &[
                "status",
                "--porcelain=v1",
                "-z",
                "--untracked-files=all",
                "--ignore-submodules=all",
            ],
        )? != status
    {
        return Err(RepositoryError::Unbound);
    }
    if line(commands.git(&path, &["symbolic-ref", "--quiet", "--short", "HEAD"])?)? != branch {
        return Err(RepositoryError::Unbound);
    }
    Ok(ArtifactObservation {
        head_sha: head,
        dirty_files,
        open_pr,
    })
}

#[cfg(test)]
#[path = "team_repository_native_tests.rs"]
mod tests;
