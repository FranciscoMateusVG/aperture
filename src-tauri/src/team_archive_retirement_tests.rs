use super::*;
use crate::journal::{ensure_private_dir, write_private_bytes_atomic};
use crate::owner::{Incarnation, ProcessIdentity as OwnedProcess};
use crate::state::{Harness, ReasoningEffort};
use std::fs;
struct Fixture {
    home: PathBuf,
    snapshot: TeamSnapshot,
    state: TeamStateFile,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.home);
    }
}
fn write<T: Serialize>(p: &Path, v: &T) {
    ensure_private_dir(p.parent().unwrap()).unwrap();
    write_private_json_atomic(p, v, true).unwrap();
}
impl Fixture {
    fn new() -> Self {
        let home =
            std::env::temp_dir().join(format!("aperture-retirement-{}", uuid::Uuid::new_v4()));
        ensure_private_dir(&home).unwrap();
        let snapshot:TeamSnapshot=serde_json::from_value(serde_json::json!({"schema_version":1,"team":"t1","project":"project:aperture","repo":"aperture","mission":"unfinished","acceptance":"not passed","preset":{"id":null,"sha256":null},"lead":"t1-worker","seats":[{"name":"t1-worker","role":"qa","harness":"codex","model":"gpt-5.6-sol","reasoning":"high"}],"fallbacks":[],"grants":[],"created_at":"2026-09-20T00:00:00Z","creation_request_id":uuid::Uuid::new_v4().to_string(),"staging_uuid":uuid::Uuid::new_v4().to_string()})).unwrap();
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
            harness: Harness::Codex,
            model: "gpt-5.6-sol".into(),
            reasoning: Some(ReasoningEffort::High),
        };
        let mut owner =
            OwnerStore::initial_record(&AuthenticatedActor::launcher(), "t1-worker", tuple)
                .unwrap();
        owner.state = OwnerState::Active;
        owner.generation = 1;
        let mut child = std::process::Command::new("/usr/bin/true").spawn().unwrap();
        let pid = child.id();
        child.wait().unwrap();
        owner.incarnation = Some(Incarnation {
            pid,
            start_time: 1,
            thread_id: uuid::Uuid::new_v4().to_string(),
            token_id: "a".repeat(64),
            harness: Harness::Codex,
            model: "gpt-5.6-sol".into(),
            reasoning: Some(ReasoningEffort::High),
            observed: true,
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
        write(
            &home.join(".aperture/run/revocations/t1-worker.json"),
            &serde_json::json!({"schema_version":1,"seat":"t1-worker","revoked_through_generation":1,"revoked_token_ids":["a".repeat(64)]}),
        );
        ensure_private_dir(&home.join(".aperture/run/hub-tokens")).unwrap();
        write(
            &home.join(".claude/aperture/t1-worker/manifest.json"),
            &serde_json::json!({"name":"t1-worker","model":"gpt-5.6-sol","window":"t1-worker","role":"qa","enabled":true}),
        );
        write_private_bytes_atomic(
            &home.join(".claude/aperture/t1-worker/.complete"),
            b"complete\n",
            false,
        )
        .unwrap();
        write_private_bytes_atomic(
            &home.join(".aperture/teams/t1/history.txt"),
            b"unfinished work preserved\n",
            false,
        )
        .unwrap();
        Self {
            home,
            snapshot,
            state,
        }
    }
    fn inspect(&self) -> Result<ArchiveJournalApproval, String> {
        inspect(&self.home, &self.snapshot, &self.state)
    }
    fn record(&self, actor: &AuthenticatedActor) {
        record_stopped(
            &self.home,
            actor,
            "t1",
            "t1-worker",
            1,
            CheckpointRecovery::None,
            true,
        )
        .unwrap();
    }
    fn mutate(&self, path: &str, f: impl FnOnce(&mut serde_json::Value)) {
        let p = self.home.join(path);
        let mut v: serde_json::Value = read_private_json(&p).unwrap();
        f(&mut v);
        write(&p, &v);
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
fn retirement_requires_real_authority_no_loss_without_valid_and_append_only_fact() {
    let _lock = crate::team_auth::tests::ENV_LOCK.lock().unwrap();
    let f = Fixture::new();
    for actor in [
        AuthenticatedActor::operator_ui(),
        AuthenticatedActor::launcher(),
    ] {
        assert!(record_stopped(
            &f.home,
            &actor,
            "t1",
            "t1-worker",
            1,
            CheckpointRecovery::None,
            true
        )
        .is_err());
    }
    let (_env, actor) = auth(&f);
    assert!(record_stopped(
        &f.home,
        &actor,
        "t1",
        "t1-worker",
        1,
        CheckpointRecovery::None,
        false
    )
    .is_err());
    assert!(!has_facts(&f.home, &f.snapshot).unwrap());
    f.record(&actor);
    assert!(record_stopped(
        &f.home,
        &actor,
        "t1",
        "t1-worker",
        1,
        CheckpointRecovery::Valid,
        false
    )
    .is_err());
    assert!(has_facts(&f.home, &f.snapshot).unwrap());
    assert_eq!(f.inspect().unwrap().category, ArchiveCategory::Retirement);
    assert!(!f.home.join(".beads").exists());
}
#[test]
fn retirement_binding_process_and_revocation_negatives_never_approve() {
    let _lock = crate::team_auth::tests::ENV_LOCK.lock().unwrap();
    for mode in 0..12 {
        let f = Fixture::new();
        let (_env, actor) = auth(&f);
        f.record(&actor);
        match mode {
            0 => f.mutate(".aperture/run/managed/t1-worker/g1/retired.json", |v| {
                v["token_id"] = "b".repeat(64).into()
            }),
            1 => f.mutate(".aperture/run/managed/t1-worker/g1/retired.json", |v| {
                v["accepted_checkpoint_loss"] = false.into()
            }),
            2 => f.mutate(".aperture/run/owner/t1-worker.json", |v| {
                v["state"] = "stale".into()
            }),
            3 => f.mutate(".aperture/run/owner/t1-worker.json", |v| {
                v["state"] = "quarantined".into()
            }),
            4 => f.mutate(".aperture/run/owner/t1-worker.json", |v| {
                v["generation"] = 2.into()
            }),
            5 => f.mutate(".aperture/run/revocations/t1-worker.json", |v| {
                v["revoked_through_generation"] = 2.into()
            }),
            6 => write_private_bytes_atomic(
                &f.home.join(".aperture/run/hub-tokens/t1-worker.token"),
                b"fixture-not-secret",
                false,
            )
            .unwrap(),
            7 => {
                fs::remove_file(fact_path(&f.home, "t1-worker", 1)).unwrap();
            }
            8 => {
                let p = fact_path(&f.home, "t1-worker", 1);
                fs::remove_file(&p).unwrap();
                std::os::unix::fs::symlink("missing", p).unwrap();
            }
            9 => f.mutate(".aperture/run/managed/t1-worker/g1/retired.json", |v| {
                v["writer"] = "operator".into()
            }),
            10 => f.mutate(".aperture/run/managed/t1-worker/g1/retired.json", |v| {
                v["team_generation"] = 2.into()
            }),
            _ => f.mutate(".aperture/teams/t1/team.json", |v| {
                v["mission"] = "changed".into()
            }),
        };
        assert!(f.inspect().is_err(), "mode {mode}");
    }
    let f = Fixture::new();
    let (_env, actor) = auth(&f);
    f.record(&actor);
    for state in [
        ProcessState::Same,
        ProcessState::Recycled,
        ProcessState::Unreadable,
    ] {
        assert!(inspect_with(&f.home, &f.snapshot, &f.state, 1, |_| state).is_err());
    }
}
#[test]
fn retirement_finalizer_preserves_history_without_mission_pass_and_rolls_back() {
    let _lock = crate::team_auth::tests::ENV_LOCK.lock().unwrap();
    let f = Fixture::new();
    let (_env, actor) = auth(&f);
    f.record(&actor);
    let owner_path = f.home.join(".aperture/run/owner/t1-worker.json");
    let owner_before = fs::read(&owner_path).unwrap();
    let fact_before = fs::read(fact_path(&f.home, "t1-worker", 1)).unwrap();
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
    assert_eq!(
        read_private_json::<OwnerRecord>(&owner_path).unwrap().state,
        OwnerState::Stale
    );
    assert_eq!(
        read_private_json::<TeamStateFile>(&f.home.join(".aperture/teams/archive/t1/state.json"))
            .unwrap()
            .state,
        TeamLifecycle::Archived
    );
    assert_eq!(
        fs::read(f.home.join(".aperture/teams/archive/t1/history.txt")).unwrap(),
        b"unfinished work preserved\n"
    );
    assert_eq!(
        fs::read(fact_path(&f.home, "t1-worker", 1)).unwrap(),
        fact_before
    );
    let plan =
        crate::journal::read_journal(&f.home.join(".aperture/run/archive-manifests/t1.json"))
            .unwrap();
    assert_eq!(
        plan.archive_approval.as_ref().unwrap().category,
        ArchiveCategory::Retirement
    );
    crate::journal::write_journal(
        &f.home.join(".aperture/run/team-journals/t1.archive.json"),
        &plan,
    )
    .unwrap();
    crate::team_archive_finalize::finalize(&f.home, &actor, "t1", 1, None).unwrap();
    crate::team_archive_finalize::rollback(&f.home, &actor, "t1", 1).unwrap();
    // Rollback restores registry/history, never resurrects a revoked worker.
    let old: OwnerRecord = serde_json::from_slice(&owner_before).unwrap();
    let current: OwnerRecord = read_private_json(&owner_path).unwrap();
    assert_eq!(current.state, OwnerState::Stale);
    assert_eq!(current.generation, old.generation);
    assert_eq!(current.incarnation, old.incarnation);
    assert_eq!(current.requested, old.requested);
    assert_eq!(
        read_private_json::<TeamStateFile>(&f.home.join(".aperture/teams/t1/state.json")).unwrap(),
        f.state
    );
    assert!(!f.home.join(".beads").exists());
}
#[test]
fn retirement_prejournal_recheck_denies_changed_facts_without_journal() {
    let _lock = crate::team_auth::tests::ENV_LOCK.lock().unwrap();
    let f = Fixture::new();
    let (_env, actor) = auth(&f);
    f.record(&actor);
    let a = f.inspect().unwrap();
    f.mutate(".aperture/run/managed/t1-worker/g1/retired.json", |v| {
        v["stopped_at_ms"] = 0.into()
    });
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
    assert!(!f
        .home
        .join(".aperture/run/team-journals/t1.archive.json")
        .exists());
}
