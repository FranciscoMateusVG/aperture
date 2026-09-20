use super::*;
use crate::team_checkpoint::{CheckpointPayload, CheckpointValidation, CheckpointWriter};
use std::os::unix::fs::{symlink, PermissionsExt};
struct Fixture {
    home: PathBuf,
    root: PathBuf,
    wt: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let home =
            std::env::temp_dir().join(format!("aperture-repo-fixture-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&home).unwrap();
        std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o700)).unwrap();
        let home = home.canonicalize().unwrap();
        let root = home.join("projects/aperture");
        std::fs::create_dir_all(&root).unwrap();
        let wt = home.join("projects/aperture-worktrees/task");
        std::fs::create_dir_all(wt.parent().unwrap()).unwrap();
        let f = Self { home, root, wt };
        f.git(&["init", "-q", "--initial-branch=main"]);
        f.git(&[
            "-c",
            "user.email=fixture@example.invalid",
            "-c",
            "user.name=Fixture",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "fixture",
        ]);
        f.git(&[
            "worktree",
            "add",
            "-q",
            "-b",
            "fixture-task",
            f.wt.to_str().unwrap(),
        ]);
        f.git(&[
            "remote",
            "add",
            "origin",
            "https://github.com/example/fixture.git",
        ]);
        f
    }
    fn git(&self, args: &[&str]) {
        let result = Command::new("/usr/bin/git")
            .current_dir(&self.root)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .args(args)
            .output()
            .unwrap();
        assert!(result.status.success(), "fixture Git setup failed");
    }
    fn binding(&self) -> BoundRepository {
        BoundRepository::from_catalog_root(&self.home, "aperture", self.root.clone()).unwrap()
    }
    fn native(&self) -> NativeCommands {
        NativeCommands {
            deadline: Instant::now() + Duration::from_secs(10),
        }
    }
    fn entry(&self) -> CheckpointEntry {
        let head = line(self.native().git(&self.wt, &["rev-parse", "HEAD"]).unwrap()).unwrap();
        CheckpointEntry {
            schema_version: 1,
            checkpoint_id: "t1-worker/1/1".into(),
            team: "t1".into(),
            seat: "t1-worker".into(),
            generation: 1,
            seq: 1,
            written_by: CheckpointWriter::Explicit,
            written_at: 1,
            content_hash: "a".repeat(64),
            validation: CheckpointValidation::Pending,
            payload: CheckpointPayload {
                task_id: "aperture-fixture".into(),
                worktree: "task".into(),
                branch: "fixture-task".into(),
                head_sha: head,
                dirty_files: vec![],
                open_pr: None,
                running_procs: vec![],
                decisions: vec![],
                next_step: "continue".into(),
                remote_effects: vec![],
            },
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.home);
    }
}
struct FakePr {
    native: NativeCommands,
    response: Vec<u8>,
    calls: usize,
    change_after_pr: Option<PathBuf>,
}
impl ReadCommands for FakePr {
    fn git(&mut self, cwd: &Path, args: &[&str]) -> Result<Vec<u8>, RepositoryError> {
        self.native.git(cwd, args)
    }
    fn prs(&mut self, _cwd: &Path, repo: &str, branch: &str) -> Result<Vec<u8>, RepositoryError> {
        assert_eq!(repo, "example/fixture");
        assert_eq!(branch, "fixture-task");
        self.calls += 1;
        if let Some(path) = self.change_after_pr.take() {
            std::fs::write(path, "fixture mutation").unwrap();
        }
        Ok(self.response.clone())
    }
}
fn runner(f: &Fixture) -> FakePr {
    FakePr {
        native: f.native(),
        response: b"[]".to_vec(),
        calls: 0,
        change_after_pr: None,
    }
}
#[test]
fn actual_git_registered_worktree_and_bootstrap_without_mutating_real_repo() {
    let f = Fixture::new();
    let b = f.binding();
    assert_eq!(b.bootstrap_cwd().unwrap(), f.root);
    assert_eq!(
        registered_worktree(&b, "task", &mut f.native()).unwrap(),
        f.wt
    );
    assert_eq!(std::fs::metadata(&f.root).unwrap().mode() & 0o777, 0o755);
}
#[test]
fn missing_unsafe_and_unregistered_paths_never_get_created_or_admitted() {
    let f = Fixture::new();
    let b = f.binding();
    for rel in ["missing", "../aperture", "/tmp", "task/..", "task//nested"] {
        assert!(registered_worktree(&b, rel, &mut f.native()).is_err());
    }
    assert!(!b.worktrees.join("missing").exists());
    let orphan = b.worktrees.join("orphan");
    std::fs::create_dir(&orphan).unwrap();
    std::fs::copy(f.wt.join(".git"), orphan.join(".git")).unwrap();
    assert!(registered_worktree(&b, "orphan", &mut f.native()).is_err());
}
#[test]
fn worktree_or_gitdir_symlink_and_commondir_escape_are_rejected() {
    let f = Fixture::new();
    let b = f.binding();
    symlink(&f.wt, b.worktrees.join("alias")).unwrap();
    assert!(registered_worktree(&b, "alias", &mut f.native()).is_err());
    let bytes = std::fs::read_to_string(f.wt.join(".git")).unwrap();
    let gitdir = PathBuf::from(bytes.trim().strip_prefix("gitdir: ").unwrap());
    std::fs::write(gitdir.join("commondir"), "../../../elsewhere\n").unwrap();
    assert_eq!(
        registered_worktree(&b, "task", &mut f.native()).unwrap_err(),
        RepositoryError::Unbound
    );
}
#[test]
fn root_inode_change_is_not_same_binding() {
    let f = Fixture::new();
    let b = f.binding();
    std::fs::rename(&f.root, f.root.with_file_name("moved")).unwrap();
    std::fs::create_dir(&f.root).unwrap();
    assert_eq!(b.bootstrap_cwd().unwrap_err(), RepositoryError::Unsafe);
}
#[test]
fn nul_porcelain_membership_rejects_ambiguous_prunable_bare_and_cap() {
    let p = Path::new("/fixture/worktree");
    let row = b"worktree /fixture/worktree\0HEAD abc\0branch refs/heads/main\0\0";
    membership(row, p).unwrap();
    assert_eq!(
        membership(&[row.as_slice(), row.as_slice()].concat(), p).unwrap_err(),
        RepositoryError::Ambiguous
    );
    for extra in [b"prunable unavailable".as_slice(), b"bare".as_slice()] {
        let mut bytes = b"worktree /fixture/worktree\0".to_vec();
        bytes.extend_from_slice(extra);
        bytes.extend_from_slice(b"\0\0");
        assert_eq!(membership(&bytes, p).unwrap_err(), RepositoryError::Unbound);
    }
    assert!(membership(b"worktree /other\0\0", p).is_err());
    assert!(membership(b"worktree /fixture/worktree", p).is_err());
    assert!(membership(&vec![0; OUTPUT_CAP + 1], p).is_err());
}
#[test]
fn collector_reads_native_git_and_only_mocked_gh_then_rechecks() {
    let f = Fixture::new();
    std::fs::write(f.wt.join("note.txt"), "fixture").unwrap();
    let entry = f.entry();
    let mut r = runner(&f);
    let result = collect(&f.binding(), &entry, &mut r).unwrap();
    assert_eq!(result.head_sha, entry.payload.head_sha);
    assert_eq!(result.dirty_files, vec!["note.txt"]);
    assert_eq!(result.open_pr, None);
    assert_eq!(r.calls, 1);
    r = runner(&f);
    r.response =
        serde_json::to_vec(&serde_json::json!([{"number":1,"headRefOid":entry.payload.head_sha}]))
            .unwrap();
    assert_eq!(
        collect(&f.binding(), &entry, &mut r)
            .unwrap()
            .open_pr
            .unwrap()
            .number,
        1
    );
}
#[test]
fn branch_mismatch_and_remote_capability_are_not_provider_queries() {
    let f = Fixture::new();
    let mut entry = f.entry();
    entry.payload.branch = "other".into();
    let mut r = runner(&f);
    assert_eq!(
        collect(&f.binding(), &entry, &mut r).unwrap_err(),
        RepositoryError::Unbound
    );
    assert_eq!(r.calls, 0);
    f.git(&[
        "remote",
        "set-url",
        "origin",
        "https://synthetic:sentinel@github.com/example/fixture",
    ]);
    assert_eq!(
        collect(&f.binding(), &f.entry(), &mut r).unwrap_err(),
        RepositoryError::PullRequest
    );
    assert_eq!(r.calls, 0);
}
#[test]
fn ambiguous_pr_and_changed_checkout_cannot_publish_mixed_observation() {
    let f = Fixture::new();
    let entry = f.entry();
    let mut r = runner(&f);
    r.response=serde_json::to_vec(&serde_json::json!([{"number":1,"headRefOid":entry.payload.head_sha},{"number":2,"headRefOid":entry.payload.head_sha}])).unwrap();
    assert_eq!(
        collect(&f.binding(), &entry, &mut r).unwrap_err(),
        RepositoryError::Ambiguous
    );
    r = runner(&f);
    r.change_after_pr = Some(f.wt.join("changed.txt"));
    assert_eq!(
        collect(&f.binding(), &entry, &mut r).unwrap_err(),
        RepositoryError::Unbound
    );
}
#[test]
fn dirty_parser_has_exact_rename_paths_and_no_truncation() {
    assert_eq!(
        dirty(b"R  new.txt\0old.txt\0?? untracked\0").unwrap(),
        vec!["new.txt", "old.txt", "untracked"]
    );
    for raw in [
        b"?? ../escape\0".as_slice(),
        b"?? line\nvalue\0",
        b"R  no-source\0",
        b"?? missing-nul",
    ] {
        assert!(dirty(raw).is_err());
    }
    let bytes = (0..201)
        .map(|i| format!("?? file-{i}\0"))
        .collect::<String>();
    assert_eq!(dirty(bytes.as_bytes()).unwrap_err(), RepositoryError::Limit);
}
#[test]
fn gh_repo_parser_never_accepts_userinfo_query_host_override_or_path_traversal() {
    assert_eq!(
        github_repo("git@github.com:example/fixture.git").unwrap(),
        "example/fixture"
    );
    for value in [
        "https://other.test/example/fixture",
        "https://github.com/../fixture",
        "https://github.com/example/fixture?token=sentinel",
        "https://github.com/example/fixture#x",
        "https://user@github.com/example/fixture",
        "/tmp/repo",
    ] {
        assert!(github_repo(value).is_err());
    }
}
#[test]
fn bounded_child_has_fixed_error_and_timeout_without_output_values() {
    let mut command = Command::new("/bin/sh");
    command.args(["-c", "printf synthetic-sentinel; sleep 2"]);
    assert_eq!(
        bounded_command(command, Instant::now() + Duration::from_millis(30)).unwrap_err(),
        RepositoryError::Timeout
    );
    let mut command = Command::new("/bin/sh");
    command.args(["-c", "printf synthetic-sentinel >&2; exit 1"]);
    assert_eq!(
        bounded_command(command, Instant::now() + Duration::from_secs(2)).unwrap_err(),
        RepositoryError::Git
    );
    assert_eq!(RepositoryError::Timeout.code(), "E_ARTIFACT_TIMEOUT");
}
fn snapshot_file(f: &Fixture) -> PathBuf {
    let dir = f.home.join(".aperture/teams/t1");
    crate::journal::ensure_private_dir(&dir).unwrap();
    let path = dir.join("team.json");
    crate::journal::write_private_json_atomic(&path,&serde_json::json!({
        "schema_version":1,"team":"t1","project":"project:aperture","repo":"aperture","mission":"fixture","acceptance":"fixture",
        "preset":{"id":null,"sha256":null},"lead":"t1-worker",
        "seats":[{"name":"t1-worker","role":"backend","harness":"codex","model":"gpt-6-astra","reasoning":"high"}],
        "fallbacks":[],"grants":[],"created_at":"2026-09-20T00:00:00Z","creation_request_id":"fixture","staging_uuid":"fixture"}),false).unwrap();
    path
}
#[test]
fn snapshot_catalog_root_and_registered_launch_cwd_are_native() {
    let f = Fixture::new();
    snapshot_file(&f);
    let b = resolve_native(&f.home, "t1", Instant::now() + Duration::from_secs(5)).unwrap();
    assert_eq!(b.bootstrap_cwd().unwrap(), f.root);
    assert_eq!(
        replacement_cwd(&b, "task", Instant::now() + Duration::from_secs(5)).unwrap(),
        f.wt
    );
}
#[test]
fn immutable_snapshot_change_or_missing_repo_never_infers_from_project() {
    let f = Fixture::new();
    let path = snapshot_file(&f);
    let b = resolve_native(&f.home, "t1", Instant::now() + Duration::from_secs(5)).unwrap();
    let mut value: serde_json::Value = crate::journal::read_private_json(&path).unwrap();
    value["repo"] = serde_json::json!("monorepo-incluir");
    crate::journal::write_private_json_atomic(&path, &value, true).unwrap();
    assert_eq!(b.bootstrap_cwd().unwrap_err(), RepositoryError::Binding);
    assert!(resolve_native(&f.home, "t1", Instant::now() + Duration::from_secs(5)).is_err());
    value.as_object_mut().unwrap().remove("repo");
    crate::journal::write_private_json_atomic(&path, &value, true).unwrap();
    assert!(resolve_native(&f.home, "t1", Instant::now() + Duration::from_secs(5)).is_err());
}
#[test]
fn catalog_directory_called_git_alone_is_not_real_repository() {
    let f = Fixture::new();
    snapshot_file(&f);
    std::fs::rename(f.root.join(".git"), f.root.join("git-saved")).unwrap();
    std::fs::create_dir(f.root.join(".git")).unwrap();
    assert!(resolve_native(&f.home, "t1", Instant::now() + Duration::from_secs(5)).is_err());
}
#[test]
fn one_registered_worktree_without_mission_binding_cannot_enable_launch() {
    let f = Fixture::new();
    snapshot_file(&f);
    let actor = crate::team_auth::AuthenticatedActor::operator_ui();
    let result = crate::team_replacement::native::replace_authorized(
        &f.home,
        crate::team_replacement::native::ReplacementAuthority::Operator(&actor),
        crate::team_replacement::remote::RemoteTarget {
            team: "t1".into(),
            seat: "t1-worker".into(),
            expected_generation: 1,
        },
        &crate::team_replacement::StartSelection {
            harness: "codex".into(),
            model: "gpt-6-astra".into(),
            reasoning: Some("high".into()),
        },
        &[],
    );
    assert_eq!(
        result.unwrap_err(),
        crate::team_replacement::ReplacementError::WorktreeUnbound
    );
    assert!(!f.home.join(".aperture/run").exists());
}
