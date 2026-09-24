use super::*;
use crate::journal::{ensure_private_dir, write_private_json_atomic};
use crate::owner::{Incarnation, ProcessIdentity as OwnedProcess};
use crate::team_auth::AuthenticatedActor;
use std::fs;
use uuid::Uuid;

pub(crate) struct Fixture {
    pub home: PathBuf,
    pub snapshot: TeamSnapshot,
    pub state: TeamStateFile,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.home);
    }
}
fn write<T: Serialize>(path: &Path, v: &T) {
    ensure_private_dir(path.parent().unwrap()).unwrap();
    write_private_json_atomic(path, v, true).unwrap();
}
impl Fixture {
    fn new(recovered: bool, observed: bool) -> Self {
        let home =
            std::env::temp_dir().join(format!("aperture-diagnostic-archive-{}", Uuid::new_v4()));
        ensure_private_dir(&home).unwrap();
        let snapshot: TeamSnapshot = serde_json::from_value(serde_json::json!({
            "schema_version":1,"team":"t1","project":"project:aperture","repo":"aperture","mission":"not a category","acceptance":"not evidence","preset":{"id":null,"sha256":null},
            "lead":"t1-worker","seats":[{"name":"t1-worker","role":"qa","harness":"claude","model":MODEL,"reasoning":null}],"fallbacks":[],"grants":[],
            "created_at":"2026-09-20T00:00:00Z","creation_request_id":Uuid::new_v4().to_string(),"staging_uuid":Uuid::new_v4().to_string()
        })).unwrap();
        let state = TeamStateFile {
            schema_version: 1,
            state: TeamLifecycle::Active,
            generation: 1,
            epic_id: Some("aperture-ab123".into()),
            failure: None,
            updated_at: "2026-09-20T00:00:00Z".into(),
        };
        write(&home.join(".aperture/teams/t1/team.json"), &snapshot);
        write(&home.join(".aperture/teams/t1/state.json"), &state);
        let tuple = ExecutionTuple {
            harness: Harness::Claude,
            model: MODEL.into(),
            reasoning: None,
        };
        let mut owner =
            OwnerStore::initial_record(&AuthenticatedActor::launcher(), "t1-worker", tuple.clone())
                .unwrap();
        owner.state = OwnerState::Quarantined;
        owner.generation = 1;
        let session = Uuid::new_v4().to_string();
        // A real inert fixture child is reaped before any collection; no assumed
        // "large PID means gone", no provider/managed process or signal.
        let mut child = std::process::Command::new("/usr/bin/true").spawn().unwrap();
        let pid = child.id();
        child.wait().unwrap();
        owner.incarnation = Some(Incarnation {
            pid,
            start_time: 1,
            thread_id: if observed {
                session.clone()
            } else {
                String::new()
            },
            token_id: "a".repeat(64),
            harness: Harness::Claude,
            model: MODEL.into(),
            reasoning: None,
            observed,
            processes: vec![OwnedProcess {
                pid,
                start_time: 1,
                ppid: 1,
                pgid: pid,
                cmdline_sha256: "b".repeat(64),
                cwd: home.to_string_lossy().into(),
            }],
        });
        write(&home.join(".aperture/run/owner/t1-worker.json"), &owner);
        let a = ClaudeAttempt {
            schema_version: 1,
            team: "t1".into(),
            seat: "t1-worker".into(),
            generation: 1,
            reservation_nonce_sha256: "c".repeat(64),
            snapshot_sha256: hash(&snapshot).unwrap(),
            team_generation: 1,
            token_id: "a".repeat(64),
            root_pid: pid,
            root_start_time_us: 1,
            session_id: session.clone(),
            requested_model: MODEL.into(),
            created_at_ms: 1001,
            mode: ClaudeLaunchMode::DiagnosticPreinput,
        };
        let base = home.join(".aperture/run/managed/t1-worker/g1");
        let mut plan = ClaudeLaunchPlan::new(
            &home,
            "t1",
            "t1-worker",
            1,
            &tuple,
            &home.join(".aperture/bin/aperture-boot"),
        )
        .unwrap();
        plan.argv[3] = session.clone();
        plan.argv.extend([
            "--append-system-prompt-file".into(),
            base.join("prompt.md").to_string_lossy().into(),
        ]);
        let launch = Launch {
            schema_version: 1,
            team: "t1".into(),
            seat: "t1-worker".into(),
            generation: 1,
            session_id: session,
            nonce_sha256: "c".repeat(64),
            token_id: "a".repeat(64),
            snapshot_sha256: hash(&snapshot).unwrap(),
            executable: home.join("claude"),
            helper: home.join(".aperture/bin/aperture-boot"),
            node: home.join("node"),
            tmux: home.join("tmux"),
            cwd: home.join("projects/aperture"),
            cwd_identity: (1, 2),
            worktree: None,
            args: plan.argv,
            pins: BTreeMap::new(),
            private_pins: BTreeMap::new(),
            mode: ClaudeLaunchMode::DiagnosticPreinput,
        };
        let release = Release {
            schema_version: 1,
            launch_sha256: hash(&launch).unwrap(),
            attempt_sha256: hash(&a).unwrap(),
            root_pid: pid,
            root_start_time_us: 1,
        };
        write(
            &home.join(".aperture/run/t1-worker.g1.claude-attempt.json"),
            &a,
        );
        write(&base.join("claude-launch.json"), &launch);
        write(&base.join("claude-release.json"), &release);
        let dir = home.join(".aperture/teams/t1/runtime-attempts/t1-worker/g0");
        let admission = Admission {
            schema_version: 1,
            attempt_id: Uuid::new_v4().to_string(),
            team: "t1".into(),
            seat: "t1-worker".into(),
            old_generation: 0,
            admitted_at_ms: 1000,
            native_budget_ms: 170000,
            cleanup_reserve_ms: 40000,
        };
        write(&dir.join("admitted.json"), &admission);
        write(
            &dir.join("effects.json"),
            &Fact {
                schema_version: 1,
                attempt_id: admission.attempt_id.clone(),
                kind: "effects_may_have_occurred".into(),
            },
        );
        write(
            &dir.join(if recovered {
                "reconciled.json"
            } else {
                "terminal.json"
            }),
            &Fact {
                schema_version: 1,
                attempt_id: admission.attempt_id,
                kind: if recovered {
                    "stopped_reconciled"
                } else {
                    "smoke_cleaned"
                }
                .into(),
            },
        );
        write(
            &home.join(".aperture/run/revocations/t1-worker.json"),
            &serde_json::json!({"schema_version":1,"seat":"t1-worker","revoked_through_generation":1,"revoked_token_ids":["a".repeat(64)]}),
        );
        ensure_private_dir(&home.join(".aperture/run/hub-tokens")).unwrap();
        write(
            &home.join(".claude/aperture/t1-worker/manifest.json"),
            &serde_json::json!({"name":"t1-worker","model":MODEL,"window":"t1-worker","role":"qa","enabled":true}),
        );
        crate::journal::write_private_bytes_atomic(
            &home.join(".claude/aperture/t1-worker/.complete"),
            b"complete\n",
            false,
        )
        .unwrap();
        Fixture {
            home,
            snapshot,
            state,
        }
    }
    fn inspect(&self) -> Result<ArchiveJournalApproval, String> {
        inspect(&self.home, &self.snapshot, &self.state)
    }
    fn mutate(&self, path: &str, f: impl FnOnce(&mut serde_json::Value)) {
        let path = self.home.join(path);
        let mut v: serde_json::Value = read(&path).unwrap();
        f(&mut v);
        write(&path, &v);
    }
    fn bind_release(&self) {
        let path = self.home.join(".aperture/run/managed/t1-worker/g1");
        let l: Launch = read(&path.join("claude-launch.json")).unwrap();
        let a: ClaudeAttempt = read(
            &self
                .home
                .join(".aperture/run/t1-worker.g1.claude-attempt.json"),
        )
        .unwrap();
        let mut r: Release = read(&path.join("claude-release.json")).unwrap();
        r.launch_sha256 = hash(&l).unwrap();
        r.attempt_sha256 = hash(&a).unwrap();
        write(&path.join("claude-release.json"), &r);
    }
}

#[test]
fn cleaned_observed_and_reconciled_unobserved_are_factual_retirement_not_mission() {
    for (recovered, observed) in [(true, false), (false, true), (true, true)] {
        let f = Fixture::new(recovered, observed);
        let a = f.inspect().unwrap();
        assert_eq!(a.category, ArchiveCategory::DiagnosticRetirement);
        assert_eq!(
            a.owner_states,
            vec![("t1-worker".into(), "quarantined".into())]
        );
        assert!(!f.home.join(".beads").exists());
        let raw = serde_json::to_value(&a).unwrap();
        assert_eq!(raw["category"], "diagnostic_retirement");
        let mut mission = a.clone();
        mission.category = ArchiveCategory::Mission;
        let legacy = serde_json::to_value(&mission).unwrap();
        assert!(legacy.get("category").is_none());
        assert_eq!(
            serde_json::from_value::<ArchiveJournalApproval>(legacy)
                .unwrap()
                .category,
            ArchiveCategory::Mission
        );
    }
}

#[test]
fn normal_or_forged_default_mode_positional_launch_is_denied_even_with_rebound_hash() {
    for mode in [true, false] {
        let f = Fixture::new(false, true);
        f.mutate(
            ".aperture/run/managed/t1-worker/g1/claude-launch.json",
            |v| {
                if mode {
                    v["mode"] = "normal_positional".into();
                } else {
                    v["args"].as_array_mut().unwrap().push("mission".into());
                }
            },
        );
        f.bind_release();
        assert!(f.inspect().is_err());
    }
}

#[test]
fn owner_generation_identity_tuple_and_mixed_state_are_denied() {
    for (key, value) in [
        ("generation", serde_json::json!(2)),
        ("state", serde_json::json!("active")),
        ("provisional_token_id", serde_json::json!("a".repeat(64))),
        (
            "reservation_nonce_sha256",
            serde_json::json!("c".repeat(64)),
        ),
    ] {
        let f = Fixture::new(false, true);
        f.mutate(".aperture/run/owner/t1-worker.json", |v| v[key] = value);
        assert!(f.inspect().is_err());
    }
    for key in ["token_id", "thread_id", "model"] {
        let f = Fixture::new(false, true);
        f.mutate(".aperture/run/owner/t1-worker.json", |v| {
            v["incarnation"][key] = "changed".into()
        });
        assert!(f.inspect().is_err());
    }
    let mut f = Fixture::new(false, true);
    let mut extra = f.snapshot.seats[0].clone();
    extra.name = "t1-other".into();
    f.snapshot.seats.push(extra);
    write(&f.home.join(".aperture/teams/t1/team.json"), &f.snapshot);
    assert!(f.inspect().is_err());
}

#[test]
fn terminal_corruption_missing_release_and_floor_or_token_fail_closed() {
    for path in [
        ".aperture/teams/t1/runtime-attempts/t1-worker/g0/terminal.json",
        ".aperture/run/managed/t1-worker/g1/claude-release.json",
        ".aperture/run/revocations/t1-worker.json",
    ] {
        let f = Fixture::new(false, true);
        fs::remove_file(f.home.join(path)).unwrap();
        assert!(f.inspect().is_err());
    }
    for kind in ["ready", "active", "failed", "unknown"] {
        let f = Fixture::new(false, true);
        f.mutate(
            ".aperture/teams/t1/runtime-attempts/t1-worker/g0/terminal.json",
            |v| v["kind"] = kind.into(),
        );
        assert!(f.inspect().is_err());
    }
    let f = Fixture::new(false, true);
    f.mutate(".aperture/run/revocations/t1-worker.json", |v| {
        v["revoked_through_generation"] = 2.into()
    });
    assert!(f.inspect().is_err());
    let f = Fixture::new(false, true);
    write(
        &f.home.join(".aperture/run/hub-tokens/t1-worker.token"),
        &"not-a-bearer-fixture",
    );
    assert!(f.inspect().is_err());
    let f = Fixture::new(false, true);
    std::os::unix::fs::symlink(
        "missing",
        f.home.join(".aperture/run/hub-tokens/t1-worker.token"),
    )
    .unwrap();
    assert!(f.inspect().is_err());
    let f = Fixture::new(false, true);
    fs::write(
        f.home
            .join(".aperture/run/managed/t1-worker/g1/claude-release.json"),
        b"{",
    )
    .unwrap();
    assert!(f.inspect().is_err());
}

#[test]
fn living_recycled_and_unreadable_are_never_gone() {
    let f = Fixture::new(false, true);
    let lock = try_lock(&f.home.join(".aperture/run/team-locks"), "t1").unwrap();
    let seats = vec![OwnerStore::new(f.home.join(".aperture/run/owner"))
        .lock("t1-worker")
        .unwrap()];
    for state in [
        ProcessState::Same,
        ProcessState::Recycled,
        ProcessState::Unreadable,
    ] {
        assert!(
            inspect_with(&f.home, &f.snapshot, &f.state, &lock, &seats, |_| state
                .clone())
            .is_err()
        );
    }
}

#[test]
fn symlink_hardlink_and_unsafe_private_fact_are_denied() {
    use std::os::unix::fs::PermissionsExt;
    for mode in 0..3 {
        let f = Fixture::new(false, true);
        let path = f
            .home
            .join(".aperture/run/managed/t1-worker/g1/claude-release.json");
        if mode == 0 {
            let other = path.with_extension("saved");
            fs::rename(&path, &other).unwrap();
            std::os::unix::fs::symlink(&other, &path).unwrap();
        } else if mode == 1 {
            fs::hard_link(&path, path.with_extension("linked")).unwrap();
        } else {
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        }
        assert!(f.inspect().is_err());
    }
}

struct Env {
    home: Option<std::ffi::OsString>,
    agents: Option<std::ffi::OsString>,
}
impl Drop for Env {
    fn drop(&mut self) {
        for (k, v) in [("HOME", &self.home), ("APERTURE_AGENTS_DIR", &self.agents)] {
            match v {
                Some(v) => std::env::set_var(k, v),
                None => std::env::remove_var(k),
            }
        }
    }
}
fn auth(f: &Fixture) -> (Env, AuthenticatedActor) {
    let env = Env {
        home: std::env::var_os("HOME"),
        agents: std::env::var_os("APERTURE_AGENTS_DIR"),
    };
    write(
        &f.home.join(".claude/aperture/glados/manifest.json"),
        &serde_json::json!({"name":"GLaDOS","model":"sonnet","window":"glados","role":"orchestrator","enabled":true}),
    );
    fs::write(f.home.join(".claude/aperture/glados/prompt.md"), "fixture").unwrap();
    crate::journal::write_private_bytes_atomic(
        &f.home.join(".aperture/run/hub-tokens/glados.token"),
        b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        false,
    )
    .unwrap();
    std::env::set_var("HOME", &f.home);
    std::env::set_var("APERTURE_AGENTS_DIR", f.home.join(".claude/aperture"));
    (
        env,
        crate::team_auth::authenticate_glados_control().unwrap(),
    )
}

#[test]
fn production_finalizer_archives_without_epic_then_rolls_back_exact_history_and_owner() {
    let _envlock = crate::team_auth::tests::ENV_LOCK.lock().unwrap();
    for recovered in [true, false] {
        let f = Fixture::new(recovered, !recovered);
        let (_env, actor) = auth(&f);
        let owner_path = f.home.join(".aperture/run/owner/t1-worker.json");
        let owner_before = fs::read(&owner_path).unwrap();
        let admission_path = "runtime-attempts/t1-worker/g0/admitted.json";
        let admission_before =
            fs::read(f.home.join(".aperture/teams/t1").join(admission_path)).unwrap();
        let a = f.inspect().unwrap();
        crate::team_archive_finalize::finalize(
            &f.home,
            &actor,
            "t1",
            1,
            Some(crate::team_archive_finalize::FreshArchive {
                snapshot: &f.snapshot,
                state: &f.state,
                approval: &a,
            }),
        )
        .unwrap();
        assert_eq!(fs::read(&owner_path).unwrap(), owner_before);
        let archived: TeamStateFile =
            read(&f.home.join(".aperture/teams/archive/t1/state.json")).unwrap();
        assert_eq!(archived.state, TeamLifecycle::Archived);
        let manifest: serde_json::Value = read(
            &f.home
                .join(".claude/aperture/_archived/t1/t1-worker/manifest.json"),
        )
        .unwrap();
        assert_eq!(manifest["enabled"], false);
        assert_eq!(
            fs::read(
                f.home
                    .join(".aperture/teams/archive/t1")
                    .join(admission_path)
            )
            .unwrap(),
            admission_before
        );
        let plan =
            crate::journal::read_journal(&f.home.join(".aperture/run/archive-manifests/t1.json"))
                .unwrap();
        assert_eq!(
            plan.archive_approval.as_ref().unwrap().category,
            ArchiveCategory::DiagnosticRetirement
        );
        // Crash after physical moves/readback but before journal unlink: exact
        // quarantined postimage supports idempotent forward recovery as well.
        crate::journal::write_journal(
            &f.home.join(".aperture/run/team-journals/t1.archive.json"),
            &plan,
        )
        .unwrap();
        crate::team_archive_finalize::finalize(&f.home, &actor, "t1", 1, None).unwrap();
        crate::team_archive_finalize::rollback(&f.home, &actor, "t1", 1).unwrap();
        assert_eq!(fs::read(&owner_path).unwrap(), owner_before);
        assert_eq!(
            read::<TeamStateFile>(&f.home.join(".aperture/teams/t1/state.json")).unwrap(),
            f.state
        );
        assert_eq!(
            fs::read(f.home.join(".aperture/teams/t1").join(admission_path)).unwrap(),
            admission_before
        );
    }
}

#[test]
fn fresh_finalizer_rechecks_facts_and_auth_before_journal() {
    let _envlock = crate::team_auth::tests::ENV_LOCK.lock().unwrap();
    for drift in 0..3 {
        let f = Fixture::new(false, true);
        let (_env, actor) = auth(&f);
        let a = f.inspect().unwrap();
        if drift == 1 {
            crate::journal::write_private_bytes_atomic(
                &f.home.join(".aperture/run/hub-tokens/glados.token"),
                b"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                true,
            )
            .unwrap();
        } else if drift == 2 {
            f.mutate(".aperture/run/revocations/t1-worker.json", |v| {
                v["revoked_token_ids"] = serde_json::json!(["a".repeat(64), "b".repeat(64)])
            });
            assert!(f.inspect().is_ok());
        } else {
            f.mutate(
                ".aperture/teams/t1/runtime-attempts/t1-worker/g0/terminal.json",
                |v| v["kind"] = "unknown".into(),
            );
        }
        assert!(crate::team_archive_finalize::finalize(
            &f.home,
            &actor,
            "t1",
            1,
            Some(crate::team_archive_finalize::FreshArchive {
                snapshot: &f.snapshot,
                state: &f.state,
                approval: &a
            })
        )
        .is_err());
        assert!(!crate::team_archive_finalize::has_journal(&f.home, "t1"));
        assert!(f.home.join(".aperture/teams/t1").is_dir());
    }
}

#[test]
fn real_control_receipt_marks_mission_unknown_and_installed_list_layout_is_unchanged() {
    let _envlock = crate::team_auth::tests::ENV_LOCK.lock().unwrap();
    let f = Fixture::new(false, true);
    let (_env, _actor) = auth(&f);
    let result = crate::teams::team_control_headless(
        r#"{"action":"archive","input":{"team":"t1","expected_generation":1}}"#,
    )
    .unwrap();
    let response = serde_json::to_value(result).unwrap();
    assert_eq!(response["result"]["state"], "archived");
    for key in [
        "reconciliation",
        "reviews",
        "metrics",
        "remote_effects",
        "worktrees",
    ] {
        assert_eq!(response["result"]["checks"][key], "unknown");
    }
    for key in ["process_stop", "revocation"] {
        assert_eq!(response["result"]["checks"][key], "verified");
    }
    let result = crate::teams::team_control_headless(r#"{"action":"list_teams"}"#).unwrap();
    assert_eq!(
        serde_json::to_value(result).unwrap()["result"],
        serde_json::json!([])
    );
    assert!(!f.home.join(".beads").exists());
}

#[test]
fn diagnostic_journal_partial_inverse_and_owner_drift_are_bound() {
    use crate::journal::{read_journal, write_journal, JournalMove, JournalOperation};
    let _envlock = crate::team_auth::tests::ENV_LOCK.lock().unwrap();
    let f = Fixture::new(true, false);
    let (_env, actor) = auth(&f);
    let a = f.inspect().unwrap();
    let before = fs::read(f.home.join(".aperture/teams/t1/team.json")).unwrap();
    crate::team_archive_finalize::finalize(
        &f.home,
        &actor,
        "t1",
        1,
        Some(crate::team_archive_finalize::FreshArchive {
            snapshot: &f.snapshot,
            state: &f.state,
            approval: &a,
        }),
    )
    .unwrap();
    let planpath = f.home.join(".aperture/run/archive-manifests/t1.json");
    let source = read_journal(&planpath).unwrap();
    // Old consumers ignore unknown category but reject quarantined owner states;
    // never let category loss silently reinterpret a diagnostic as a mission.
    let mut invalid = source.clone();
    invalid.archive_approval.as_mut().unwrap().category = ArchiveCategory::Mission;
    assert!(write_journal(
        &f.home
            .join(".aperture/run/team-journals/invalid.archive.json"),
        &invalid
    )
    .is_err());
    let ownerpath = f.home.join(".aperture/run/owner/t1-worker.json");
    let owner: OwnerRecord = read(&ownerpath).unwrap();
    for case in 0..3 {
        let mut changed = owner.clone();
        match case {
            0 => changed.generation += 1,
            1 => changed.requested.model = "other".into(),
            _ => changed.incarnation.as_mut().unwrap().pid += 1,
        }
        write(&ownerpath, &changed);
        assert!(crate::team_archive_finalize::rollback(&f.home, &actor, "t1", 1).is_err());
        assert!(f.home.join(".aperture/teams/archive/t1").exists());
        write(&ownerpath, &owner);
    }
    // Recover an inverse interrupted after exactly one no-replace move.
    let mut inverse = source.clone();
    inverse.operation = JournalOperation::RollbackArchive;
    inverse.uuid = Uuid::new_v4().to_string();
    inverse.step = 0;
    inverse.moves = source
        .moves
        .iter()
        .rev()
        .map(|mv| JournalMove {
            from_root: mv.to_root.clone(),
            from_rel: mv.to_rel.clone(),
            to_root: mv.from_root.clone(),
            to_rel: mv.from_rel.clone(),
            kind: mv.kind.clone(),
        })
        .collect();
    write_journal(
        &f.home.join(".aperture/run/team-journals/t1.archive.json"),
        &inverse,
    )
    .unwrap();
    crate::journal::rename_no_replace(
        &f.home.join(".claude/aperture/_archived/t1/t1-worker"),
        &f.home.join(".claude/aperture/t1-worker"),
    )
    .unwrap();
    crate::team_archive_finalize::rollback(&f.home, &actor, "t1", 1).unwrap();
    assert_eq!(
        fs::read(f.home.join(".aperture/teams/t1/team.json")).unwrap(),
        before
    );
    assert_eq!(
        hash(&read::<OwnerRecord>(&ownerpath).unwrap()).unwrap(),
        hash(&owner).unwrap()
    );
}
