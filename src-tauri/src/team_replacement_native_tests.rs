//! Private synthetic metadata only: no process collector, signal, hub, harness,
//! token provisioner, provider or real registry in these adapter tests.
use super::*;
use crate::journal::{ensure_private_dir, write_private_bytes_atomic, write_private_json_atomic};
use crate::state::{Harness, ReasoningEffort};
use std::os::unix::fs::symlink;
struct Fixture(std::path::PathBuf);
impl Fixture {
    fn new() -> Self {
        let home =
            std::env::temp_dir().join(format!("aperture-native-adapter-{}", uuid::Uuid::new_v4()));
        ensure_private_dir(&home).unwrap();
        Self(home)
    }
    fn write(&self, relative: &str, value: &serde_json::Value) {
        let path = self.0.join(relative);
        ensure_private_dir(path.parent().unwrap()).unwrap();
        write_private_json_atomic(&path, value, true).unwrap();
    }
    fn revocation(&self) {
        ensure_private_dir(&self.0.join(".aperture/run/hub-tokens")).unwrap();
        self.write(".aperture/run/owner/t1-worker.json", &serde_json::json!({
            "schema_version":1,"seat":"t1-worker","generation":1,"state":"active",
            "reservation_nonce_sha256":null,"provisional_token_id":null,
            "requested":{"harness":"codex","model":"gpt-6-astra","reasoning":"high"},
            "incarnation":{"pid":900001,"start_time":42,"thread_id":"old-thread","token_id":"a".repeat(64),
                "harness":"codex","model":"gpt-6-astra","reasoning":"high","observed":true,"processes":[]},
            "since":"2026-09-20T00:00:00Z","writer":"launcher"}));
        self.write(".aperture/run/revocations/t1-worker.json", &revoked_value());
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn target() -> remote::RemoteTarget {
    remote::RemoteTarget {
        team: "t1".into(),
        seat: "t1-worker".into(),
        expected_generation: 1,
    }
}
fn selection() -> StartSelection {
    StartSelection {
        harness: "codex".into(),
        model: "gpt-6-astra".into(),
        reasoning: Some("high".into()),
    }
}
fn revoked_value() -> serde_json::Value {
    serde_json::json!({"schema_version":1,"seat":"t1-worker","revoked_through_generation":1,"revoked_token_ids":["a".repeat(64)]})
}
fn snapshot() -> OwnershipSnapshot {
    OwnershipSnapshot {
        seat: "t1-worker".into(),
        generation: 1,
        thread_id: "old-thread".into(),
        complete: true,
        unowned_matches: vec![],
        processes: vec![OwnedProcess {
            identity: ProcessIdentity {
                pid: 900001,
                start_time: "42".into(),
            },
            parent_pid: 1,
            process_group: 900001,
            depth: 0,
            cmdline_sha256: "a".repeat(64),
            cwd: "/fixture".into(),
        }],
    }
}
fn team() -> TeamSnapshot {
    serde_json::from_value(serde_json::json!({
        "schema_version":1,"team":"t1","project":"project:aperture","repo":"aperture","mission":"fixture","acceptance":"fixture",
        "preset":{"id":null,"sha256":null},"lead":"t1-lead",
        "seats":[{"name":"t1-worker","role":"backend","harness":"codex","model":"gpt-6-astra","reasoning":"high"}],
        "fallbacks":[{"harness":"codex","model":"gpt-6-astra","reasoning":"medium"}],"grants":[],
        "created_at":"2026-09-20T00:00:00Z","creation_request_id":"fixture","staging_uuid":"fixture"})).unwrap()
}
#[test]
fn missing_binding_before_even_filesystem_or_native_collection() {
    let f = Fixture::new();
    let home = f.0.join("nonexistent-home");
    let actor = AuthenticatedActor::operator_ui();
    let result = replace_authorized(
        &home,
        ReplacementAuthority::Operator(&actor),
        target(),
        &selection(),
        &[],
    );
    assert_eq!(
        result.unwrap_err(),
        ReplacementError::RepoBindingUnavailable
    );
    assert!(!home.exists());
    assert_eq!(std::fs::read_dir(&f.0).unwrap().count(), 0);
}
#[test]
fn project_cwd_or_checkpoint_does_not_grant_repository_authority() {
    let f = Fixture::new();
    f.write(
        ".aperture/teams/t1/team.json",
        &serde_json::to_value(team()).unwrap(),
    );
    f.write(
        ".aperture/teams/t1/checkpoints/t1-worker/fake.json",
        &serde_json::json!({"worktree":"/tmp","project":"aperture"}),
    );
    let before = std::fs::read(f.0.join(".aperture/teams/t1/team.json")).unwrap();
    let actor = AuthenticatedActor::operator_ui();
    assert_eq!(
        replace_authorized(
            &f.0,
            ReplacementAuthority::Operator(&actor),
            target(),
            &selection(),
            &[]
        )
        .unwrap_err(),
        ReplacementError::RepoBindingUnavailable
    );
    assert_eq!(
        before,
        std::fs::read(f.0.join(".aperture/teams/t1/team.json")).unwrap()
    );
    assert!(!f.0.join(".aperture/run").exists());
}
#[test]
fn selectors_reject_paths_and_zero_generation_before_io() {
    let f = Fixture::new();
    let actor = AuthenticatedActor::operator_ui();
    for t in [
        remote::RemoteTarget {
            team: "../t1".into(),
            ..target()
        },
        remote::RemoteTarget {
            seat: "../worker".into(),
            ..target()
        },
        remote::RemoteTarget {
            expected_generation: 0,
            ..target()
        },
    ] {
        assert_eq!(
            replace_authorized(
                &f.0,
                ReplacementAuthority::Operator(&actor),
                t,
                &selection(),
                &[]
            )
            .unwrap_err(),
            ReplacementError::GenerationMismatch
        );
    }
    assert_eq!(std::fs::read_dir(&f.0).unwrap().count(), 0);
}
#[test]
fn launcher_is_not_operator_authorization() {
    assert_eq!(
        ReplacementAuthority::Operator(&AuthenticatedActor::launcher())
            .revalidate(&target())
            .unwrap_err(),
        ReplacementError::AuthorizationRequired
    );
}
#[test]
fn immutable_tuple_exact_no_substrings_alias_or_reasoning_widening() {
    let t = team();
    let mut s = tuple(&selection()).unwrap();
    selected_in_snapshot(&t, "t1-worker", &s).unwrap();
    s.reasoning = Some(ReasoningEffort::Medium);
    selected_in_snapshot(&t, "t1-worker", &s).unwrap();
    s.reasoning = Some(ReasoningEffort::Ultra);
    assert!(selected_in_snapshot(&t, "t1-worker", &s).is_err());
    s.reasoning = Some(ReasoningEffort::High);
    s.model.push_str("-latest");
    assert!(selected_in_snapshot(&t, "t1-worker", &s).is_err());
    s.model = "gpt-6-astra".into();
    s.harness = Harness::Claude;
    assert!(selected_in_snapshot(&t, "t1-worker", &s).is_err());
    let mut unknown = selection();
    unknown.harness = "codex-alias".into();
    assert!(tuple(&unknown).is_err());
    unknown = selection();
    unknown.reasoning = Some("default".into());
    assert!(tuple(&unknown).is_err());
}
#[test]
fn duplicate_or_missing_seat_cannot_inherit_fallback() {
    let mut t = team();
    let selected = tuple(&selection()).unwrap();
    assert!(selected_in_snapshot(&t, "absent", &selected).is_err());
    t.seats.push(t.seats[0].clone());
    assert!(selected_in_snapshot(&t, "t1-worker", &selected).is_err());
}
#[test]
fn late_descendant_recycled_pid_or_generation_never_matches_frozen_set() {
    let old = snapshot();
    assert!(same_identities(&old, &old));
    let mut fresh = old.clone();
    fresh.processes[0].identity.start_time = "43".into();
    assert!(!same_identities(&old, &fresh));
    fresh = old.clone();
    fresh.processes.push(OwnedProcess {
        identity: ProcessIdentity {
            pid: 900002,
            start_time: "45".into(),
        },
        ..fresh.processes[0].clone()
    });
    assert!(!same_identities(&old, &fresh));
    fresh = old.clone();
    fresh.generation = 2;
    assert!(!same_identities(&old, &fresh));
    fresh = old.clone();
    fresh.thread_id = "other".into();
    assert!(!same_identities(&old, &fresh));
    fresh = old.clone();
    fresh.complete = false;
    assert!(!same_identities(&old, &fresh));
}
#[test]
fn native_revocation_readback_requires_exact_floor_digest_and_absence() {
    let f = Fixture::new();
    f.revocation();
    assert!(revoked_metadata(&f.0, &snapshot()).unwrap());
    for mutation in [
        serde_json::json!({"revoked_through_generation":0}),
        serde_json::json!({"revoked_through_generation":2}),
        serde_json::json!({"seat":"other"}),
        serde_json::json!({"revoked_token_ids":["b".repeat(64)]}),
        serde_json::json!({"revoked_token_ids":["a".repeat(64),"a".repeat(64)]}),
        serde_json::json!({"unexpected":true}),
    ] {
        let mut value = revoked_value();
        for (k, v) in mutation.as_object().unwrap() {
            value[k] = v.clone();
        }
        f.write(".aperture/run/revocations/t1-worker.json", &value);
        assert!(revoked_metadata(&f.0, &snapshot()).is_err());
    }
    f.write(".aperture/run/revocations/t1-worker.json", &revoked_value());
    write_private_bytes_atomic(
        &f.0.join(".aperture/run/hub-tokens/t1-worker.token"),
        b"not-a-real-token",
        false,
    )
    .unwrap();
    assert!(revoked_metadata(&f.0, &snapshot()).is_err());
}
#[test]
fn revoked_readback_rejects_corrupt_symlink_and_changed_owner() {
    let f = Fixture::new();
    f.revocation();
    let path = f.0.join(".aperture/run/revocations/t1-worker.json");
    write_private_bytes_atomic(&path, b"invalid-json", true).unwrap();
    assert!(revoked_metadata(&f.0, &snapshot()).is_err());
    std::fs::remove_file(&path).unwrap();
    symlink("missing", &path).unwrap();
    assert!(revoked_metadata(&f.0, &snapshot()).is_err());
    std::fs::remove_file(&path).unwrap();
    f.write(".aperture/run/revocations/t1-worker.json", &revoked_value());
    let owner_path = f.0.join(".aperture/run/owner/t1-worker.json");
    let mut owner: serde_json::Value = read_private_json(&owner_path).unwrap();
    owner["state"] = serde_json::json!("starting");
    f.write(".aperture/run/owner/t1-worker.json", &owner);
    assert!(revoked_metadata(&f.0, &snapshot()).is_err());
}
#[test]
fn stable_error_codes_do_not_expose_paths_or_guessed_state() {
    assert_eq!(
        ReplacementError::RepoBindingUnavailable.code(),
        "E_REPO_BINDING_UNAVAILABLE"
    );
    assert_eq!(
        ReplacementError::CheckpointUnavailable.code(),
        "E_CHECKPOINT_UNAVAILABLE"
    );
}

#[test]
fn controls_change_requires_real_grant_not_just_fallback_membership() {
    let current = tuple(&selection()).unwrap();
    let mut selected = current.clone();
    selected.model = "another-model".into();
    unchanged_execution_controls(&current, &selected).unwrap();
    selected.reasoning = Some(ReasoningEffort::Medium);
    assert_eq!(
        unchanged_execution_controls(&current, &selected).unwrap_err(),
        ReplacementError::AuthorizationRequired
    );
    selected = current.clone();
    selected.harness = Harness::Claude;
    assert_eq!(
        unchanged_execution_controls(&current, &selected).unwrap_err(),
        ReplacementError::AuthorizationRequired
    );
}

#[test]
fn bootstrap_is_operator_g0_only_and_never_creates_state_for_bad_selector() {
    let f = Fixture::new();
    let home = f.0.join("absent");
    assert_eq!(
        bootstrap_authorized(&home, &AuthenticatedActor::launcher(), "t1", "t1-worker", 0)
            .unwrap_err(),
        ReplacementError::AuthorizationRequired
    );
    assert_eq!(
        bootstrap_authorized(
            &home,
            &AuthenticatedActor::operator_ui(),
            "t1",
            "t1-worker",
            1
        )
        .unwrap_err(),
        ReplacementError::GenerationMismatch
    );
    assert!(!home.exists());
}

fn claude_candidate_fixture(f: &Fixture) -> (OwnerStore, StartReservation, Incarnation) {
    let store = OwnerStore::new(f.0.join(".aperture/run/owner"));
    let actor = AuthenticatedActor::launcher();
    let requested = ExecutionTuple {
        harness: Harness::Claude,
        model: crate::team_claude_launch::MODEL.into(),
        reasoning: None,
    };
    store
        .initialize_owner(&actor, "t1-worker", requested.clone())
        .unwrap();
    let res = store
        .reserve_start(&actor, "t1-worker", 0, requested)
        .unwrap();
    store
        .bind_and_publish_token(&actor, &res, "a".repeat(64), || Ok(()))
        .unwrap();
    let candidate = Incarnation {
        pid: 900001,
        start_time: 42,
        thread_id: String::new(),
        token_id: "a".repeat(64),
        harness: Harness::Claude,
        model: crate::team_claude_launch::MODEL.into(),
        reasoning: None,
        observed: false,
        processes: vec![crate::owner::ProcessIdentity {
            pid: 900001,
            start_time: 42,
            ppid: 1,
            pgid: 900001,
            cmdline_sha256: "b".repeat(64),
            cwd: "/fixture".into(),
        }],
    };
    (store, res, candidate)
}
#[test]
fn claude_candidate_attempt_release_order_has_real_durable_owner_at_each_boundary() {
    let f = Fixture::new();
    let (store, res, candidate) = claude_candidate_fixture(&f);
    let phase = std::cell::Cell::new(0);
    candidate_then_claude_release(
        &store,
        &AuthenticatedActor::launcher(),
        &res,
        candidate.clone(),
        || {
            let current = store.read_owner("t1-worker").unwrap();
            assert_eq!(current.incarnation.as_ref(), Some(&candidate));
            assert_eq!(current.state, OwnerState::Starting);
            assert!(store
                .commit_start(&AuthenticatedActor::launcher(), &res)
                .is_err());
            phase.set(1);
            Ok(())
        },
        || {
            assert_eq!(phase.get(), 1);
            assert_eq!(
                store.read_owner("t1-worker").unwrap().incarnation.as_ref(),
                Some(&candidate)
            );
            phase.set(2);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(phase.get(), 2);
    assert_eq!(
        store.read_owner("t1-worker").unwrap().state,
        OwnerState::Starting
    );
    // Releasing the gate is never model observation or an Active commit.
    assert!(store
        .commit_start(&AuthenticatedActor::launcher(), &res)
        .is_err());
}
#[test]
fn claude_candidate_attempt_release_failures_never_advance_past_failed_boundary() {
    for boundary in 0..3 {
        let f = Fixture::new();
        let (store, res, mut candidate) = claude_candidate_fixture(&f);
        let before = std::fs::read(f.0.join(".aperture/run/owner/t1-worker.json")).unwrap();
        if boundary == 0 {
            candidate.token_id = "c".repeat(64);
        }
        let phase = std::cell::Cell::new(0);
        let result = candidate_then_claude_release(
            &store,
            &AuthenticatedActor::launcher(),
            &res,
            candidate.clone(),
            || {
                phase.set(1);
                if boundary == 1 {
                    Err(ReplacementError::ModelUnverified)
                } else {
                    Ok(())
                }
            },
            || {
                phase.set(2);
                Err(ReplacementError::OutcomeUnknown)
            },
        );
        assert!(result.is_err());
        assert_eq!(phase.get(), boundary);
        let owner = store.read_owner("t1-worker").unwrap();
        assert_eq!(owner.state, OwnerState::Starting);
        if boundary == 0 {
            assert_eq!(
                before,
                std::fs::read(f.0.join(".aperture/run/owner/t1-worker.json")).unwrap()
            );
            assert!(owner.incarnation.is_none());
        } else {
            assert_eq!(owner.incarnation.as_ref(), Some(&candidate));
            assert!(!owner.incarnation.unwrap().observed);
        }
        assert!(store
            .commit_start(&AuthenticatedActor::launcher(), &res)
            .is_err());
    }
}
#[test]
fn claude_observation_exact_none_is_not_reasoning_wildcard_and_cleanup_identity_is_preserved() {
    for drift in ["none", "model", "reasoning", "birth"] {
        let f = Fixture::new();
        let (store, res, candidate) = claude_candidate_fixture(&f);
        let actor = AuthenticatedActor::launcher();
        candidate_then_claude_release(
            &store,
            &actor,
            &res,
            candidate.clone(),
            || Ok(()),
            || Ok(()),
        )
        .unwrap();
        let mut actual = ExecutionTuple {
            harness: Harness::Claude,
            model: candidate.model.clone(),
            reasoning: None,
        };
        if drift == "model" {
            actual.model = "sonnet".into();
        }
        if drift == "reasoning" {
            actual.reasoning = Some(ReasoningEffort::High);
        }
        let observation = crate::owner::RuntimeObservation {
            pid: candidate.pid,
            start_time: if drift == "birth" { 43 } else { 42 },
            token_id: candidate.token_id.clone(),
            thread_id: uuid::Uuid::new_v4().to_string(),
            actual,
        };
        let observed = store.record_runtime_observation(&actor, &res, observation);
        if drift == "none" {
            observed.unwrap();
            let owner = store.commit_start(&actor, &res).unwrap();
            assert_eq!(owner.state, OwnerState::Active);
            assert_eq!(owner.incarnation.unwrap().reasoning, None);
        } else {
            assert!(observed.is_err() || store.commit_start(&actor, &res).is_err());
            let owner = store.read_owner("t1-worker").unwrap();
            assert_eq!(owner.state, OwnerState::Starting);
            let inc = owner.incarnation.unwrap();
            assert_eq!(inc.processes, candidate.processes);
            assert_eq!(
                (inc.pid, inc.start_time, inc.token_id.as_str()),
                (
                    candidate.pid,
                    candidate.start_time,
                    candidate.token_id.as_str()
                )
            );
        }
    }
}
#[test]
fn claude_no_socket_or_receipt_does_not_prove_observation_or_cleanup() {
    let f = Fixture::new();
    let (store, res, candidate) = claude_candidate_fixture(&f);
    candidate_then_claude_release(
        &store,
        &AuthenticatedActor::launcher(),
        &res,
        candidate,
        || Ok(()),
        || Ok(()),
    )
    .unwrap();
    assert!(!f.0.join(".aperture/run/t1-worker.sock").exists());
    assert!(!matches!(
        runtime_observation(&f.0, "t1", &res, &Harness::Claude),
        Ok(Some(_))
    ));
    let before = std::fs::read(f.0.join(".aperture/run/owner/t1-worker.json")).unwrap();
    assert_eq!(
        cleanup_native(&f.0, "t1", &res, None, Instant::now()).unwrap_err(),
        ReplacementError::StartCleanupUnverified
    );
    assert_eq!(
        before,
        std::fs::read(f.0.join(".aperture/run/owner/t1-worker.json")).unwrap()
    );
}

#[test]
fn claude_stopped_requires_all_exact_identities_gone_not_socket_absence() {
    let original = snapshot();
    claude_stopped(&original, |_| ProcessState::Gone).unwrap();
    for state in [
        ProcessState::Same,
        ProcessState::Recycled,
        ProcessState::Unreadable,
    ] {
        assert_eq!(
            claude_stopped(&original, |_| state),
            Err(ReplacementError::StopUnverified)
        );
    }
    let mut bad = original.clone();
    bad.processes.clear();
    assert!(claude_stopped(&bad, |_| ProcessState::Gone).is_err());
    bad = original.clone();
    bad.complete = false;
    assert!(claude_stopped(&bad, |_| ProcessState::Gone).is_err());
    bad = original;
    bad.unowned_matches.push(bad.processes[0].identity.clone());
    assert!(claude_stopped(&bad, |_| ProcessState::Gone).is_err());
}

#[test]
fn disabled_claude_native_plan_denies_before_runtime_io_without_affecting_codex_policy() {
    let f = Fixture::new();
    let home = f.0.canonicalize().unwrap();
    let root = home.join("projects/aperture");
    ensure_private_dir(&root).unwrap();
    assert!(std::process::Command::new("/usr/bin/git")
        .args(["init", "-q"])
        .arg(&root)
        .status()
        .unwrap()
        .success());
    f.write(
        ".aperture/teams/t1/team.json",
        &serde_json::to_value(team()).unwrap(),
    );
    let repo =
        repository::resolve_native(&home, "t1", Instant::now() + Duration::from_secs(3)).unwrap();
    let before = std::fs::read(home.join(".aperture/teams/t1/team.json")).unwrap();
    let requested = ExecutionTuple {
        harness: Harness::Claude,
        model: crate::team_claude_launch::MODEL.into(),
        reasoning: None,
    };
    // Deliberately missing HOME: policy must reject before examining any native
    // runtime, executable, token, attempt or process, even with a bound repo.
    assert!(matches!(
        NativePlan::preflight_selected(
            &home.join("missing-home"),
            "t1",
            "t1-worker",
            &requested,
            &repo,
            None,
            &deadline::Deadline::new()
        ),
        Err(ReplacementError::LaunchUnavailable)
    ));
    let mut claude_team = team();
    claude_team.seats[0].harness = Harness::Claude;
    claude_team.seats[0].model = requested.model.clone();
    claude_team.seats[0].reasoning = None;
    f.write(
        ".aperture/teams/t1/team.json",
        &serde_json::to_value(claude_team).unwrap(),
    );
    let actor = AuthenticatedActor::operator_ui();
    assert_eq!(
        bootstrap_authorized(&home, &actor, "t1", "t1-worker", 0).unwrap_err(),
        ReplacementError::LaunchUnavailable
    );
    let selected = StartSelection {
        harness: "claude".into(),
        model: requested.model.clone(),
        reasoning: None,
    };
    assert_eq!(
        replace_authorized(
            &home,
            ReplacementAuthority::Operator(&actor),
            target(),
            &selected,
            &[]
        )
        .unwrap_err(),
        ReplacementError::LaunchUnavailable
    );
    // Restore fixture snapshot only, to assert no incidental state mutation.
    f.write(
        ".aperture/teams/t1/team.json",
        &serde_json::from_slice(&before).unwrap(),
    );
    assert!(!crate::teams::managed_launch_enabled(&Harness::Claude));
    assert!(crate::teams::managed_launch_enabled(&Harness::Codex));
    assert!(!home.join("missing-home").exists());
    assert!(!home.join(".aperture/run").exists());
    assert_eq!(
        before,
        std::fs::read(home.join(".aperture/teams/t1/team.json")).unwrap()
    );
}

#[test]
fn archival_authority_cannot_enter_replacement_or_impersonate_operator() {
    let f = Fixture::new();
    for actor in [AuthenticatedActor::operator_ui(), AuthenticatedActor::launcher()] {
        let authority = ReplacementAuthority::GladosArchive(&actor);
        assert_eq!(authority.revalidate(&target()), Err(ReplacementError::AuthorizationRequired));
        assert!(matches!(authority.inspect(&f.0, &target(), &[]), Err(remote::RemoteError::Authority)));
        assert_eq!(replace_authorized(&f.0, authority, target(), &selection(), &[]).unwrap_err(),
            ReplacementError::AuthorizationRequired);
        assert_eq!(stop_for_archive(&f.0, &actor, "t1", "t1-worker", 1, &[]).unwrap_err(),
            ReplacementError::AuthorizationRequired);
    }
    assert_eq!(std::fs::read_dir(&f.0).unwrap().count(), 0);
}

// Startup-smoke tests use a fake private HOME, real capability validation and
// synthetic owner/floor records. No harness, hub, signal, prompt or provider.
struct SmokeEnv(Vec<(&'static str, Option<std::ffi::OsString>)>);
impl SmokeEnv {
    fn set(f: &Fixture) -> Self {
        let values = [("HOME", f.0.clone()), ("APERTURE_AGENTS_DIR", f.0.join(".claude/aperture")),
            ("APERTURE_TEAMS_DIR", f.0.join(".aperture/teams"))];
        let mut old = vec![];
        for (key, value) in values { old.push((key, std::env::var_os(key))); std::env::set_var(key, value); }
        Self(old)
    }
}
impl Drop for SmokeEnv {
    fn drop(&mut self) { for (k,v) in self.0.drain(..) { match v { Some(v) => std::env::set_var(k,v), None => std::env::remove_var(k) } } }
}
fn smoke_fixture(f: &Fixture) -> AuthenticatedActor {
    let mut snapshot = team(); snapshot.lead = "t1-worker".into();
    snapshot.seats[0].role = "lead".into(); snapshot.seats[0].harness = Harness::Claude;
    snapshot.seats[0].model = "claude-sonnet-5".into(); snapshot.seats[0].reasoning = None;
    snapshot.fallbacks.clear();
    snapshot.creation_request_id=uuid::Uuid::new_v4().to_string();
    snapshot.staging_uuid=uuid::Uuid::new_v4().to_string();
    f.write(".aperture/teams/t1/team.json", &serde_json::to_value(snapshot).unwrap());
    f.write(".aperture/teams/t1/state.json", &serde_json::json!({"schema_version":1,"state":"active","generation":1,"epic_id":"aperture-fixture","failure":null,"updated_at":"2026-09-22T00:00:00Z"}));
    f.write(".claude/aperture/glados/manifest.json", &serde_json::json!({"name":"GLaDOS","model":"sonnet","window":"glados","role":"orchestrator","enabled":true}));
    for (rel, bytes) in [(".claude/aperture/glados/prompt.md", b"fixture".as_slice()),
        (".claude/aperture/t1-worker/TEAM", b"".as_slice()), (".claude/aperture/t1-worker/.complete", b"".as_slice()),
        (".aperture/run/hub-tokens/glados.token", b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".as_slice())] {
        let p=f.0.join(rel);ensure_private_dir(p.parent().unwrap()).unwrap();write_private_bytes_atomic(&p,bytes,false).unwrap();
    }
    OwnerStore::new(f.0.join(".aperture/run/owner")).initialize_owner(&AuthenticatedActor::launcher(),"t1-worker",
        ExecutionTuple { harness:Harness::Claude, model:"claude-sonnet-5".into(), reasoning:None }).unwrap();
    crate::team_auth::authenticate_glados_control().unwrap()
}
fn synthetic_cleaned_smoke(f: &Fixture, actor: &AuthenticatedActor) -> (SmokeAdmission, NativeStarted, deadline::RuntimeAttempt) {
    let admission=SmokeAdmission::issue(&f.0,actor,"t1","t1-worker",0).unwrap();
    let mut attempt=deadline::RuntimeAttempt::begin_bootstrap(&f.0,&AuthenticatedActor::launcher(),"t1","t1-worker",deadline::Deadline::new()).unwrap();
    attempt.admit_effects().unwrap();
    let store=OwnerStore::new(f.0.join(".aperture/run/owner"));let launcher=AuthenticatedActor::launcher();
    let selected=ExecutionTuple { harness:Harness::Claude,model:"claude-sonnet-5".into(),reasoning:None };
    let res=store.reserve_start(&launcher,"t1-worker",0,selected.clone()).unwrap();
    store.bind_and_publish_token(&launcher,&res,"a".repeat(64),||Ok(())).unwrap();
    let pid=900001;let birth=42;let thread=uuid::Uuid::new_v4().to_string();
    store.record_start_candidate(&launcher,&res,Incarnation { pid,start_time:birth,thread_id:String::new(),token_id:"a".repeat(64),
        harness:Harness::Claude,model:selected.model.clone(),reasoning:None,observed:false,
        processes:vec![crate::owner::ProcessIdentity {pid,start_time:birth,ppid:1,pgid:pid,cmdline_sha256:"b".repeat(64),cwd:"/fixture".into()}] }).unwrap();
    store.record_runtime_observation(&launcher,&res,crate::owner::RuntimeObservation {pid,start_time:birth,token_id:"a".repeat(64),thread_id:thread.clone(),actual:selected}).unwrap();
    store.commit_start(&launcher,&res).unwrap();
    store.quarantine_failed_start(&launcher,&res,&crate::owner::FailedStartIdentity {pid,start_time:birth,token_id:"a".repeat(64),thread_id:thread.clone()}).unwrap();
    f.write(".aperture/run/revocations/t1-worker.json", &revoked_value());
    let started=NativeStarted { reservation:res,harness:Harness::Claude,child:None,candidate:StartedCandidate {
        observed:StartedReplacement {generation:1,thread_id:thread,requested_model:"claude-sonnet-5".into(),actual_model:Some("claude-sonnet-5".into()),model_verified:true},
        actual_harness:Some("claude".into()),actual_reasoning:None,process:team_process::identity_from_owner(pid,birth).unwrap(),token_id:"a".repeat(64) } };
    (admission,started,attempt)
}
#[test]
fn smoke_authority_is_glados_only_before_any_runtime_io() {
    let f=Fixture::new();
    for actor in [AuthenticatedActor::operator_ui(),AuthenticatedActor::launcher()] {
        assert!(matches!(bootstrap_claude_smoke_authorized(&f.0,&actor,"t1","t1-worker",0,&[]),Err(ReplacementError::AuthorizationRequired)));
    }
    assert!(!f.0.join(".aperture").exists());
    assert!(!crate::teams::managed_launch_enabled(&Harness::Claude));
}
#[test]
fn smoke_admission_pins_g0_exact_tuple_snapshot_and_current_capability() {
    let _guard=crate::team_auth::tests::ENV_LOCK.lock().unwrap();let f=Fixture::new();let _env=SmokeEnv::set(&f);let actor=smoke_fixture(&f);
    assert!(SmokeAdmission::issue(&f.0,&actor,"t1","t1-worker",1).is_err());
    let a=SmokeAdmission::issue(&f.0,&actor,"t1","t1-worker",0).unwrap();
    assert!(a.matches_target(&f.0,"t2","t1-worker",0).is_err());
    assert!(a.matches_target(&f.0,"t1","t1-worker",1).is_err());
    let path=f.0.join(".aperture/teams/t1/team.json");let mut snapshot:TeamSnapshot=read_private_json(&path).unwrap();
    snapshot.mission="changed".into();f.write(".aperture/teams/t1/team.json",&serde_json::to_value(snapshot.clone()).unwrap());
    assert!(a.revalidate().is_err());
    for (model,reasoning) in [("sonnet",None),("claude-sonnet-5",Some(ReasoningEffort::High))] {
        snapshot.seats[0].model=model.into();snapshot.seats[0].reasoning=reasoning;
        f.write(".aperture/teams/t1/team.json",&serde_json::to_value(&snapshot).unwrap());
        assert!(SmokeAdmission::issue(&f.0,&actor,"t1","t1-worker",0).is_err());
    }
    write_private_bytes_atomic(&f.0.join(".aperture/run/hub-tokens/glados.token"),b"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",true).unwrap();
    assert!(matches!(SmokeAdmission::issue(&f.0,&actor,"t1","t1-worker",0),Err(ReplacementError::AuthorizationRequired)));
    assert!(!f.0.join(".aperture/teams/t1/runtime-attempts").exists());
}
#[test]
fn smoke_terminal_is_cleaned_not_active_and_never_allows_retry() {
    let _guard=crate::team_auth::tests::ENV_LOCK.lock().unwrap();let f=Fixture::new();let _env=SmokeEnv::set(&f);let actor=smoke_fixture(&f);
    let (a,started,mut attempt)=synthetic_cleaned_smoke(&f,&actor);
    let proof=SmokeCleanupProof::capture(&a,&started,attempt.id(),attempt.budget(),&actor).unwrap();
    assert_eq!(proof.actual.reasoning,None);
    attempt.finish_smoke_cleaned(proof,&actor).unwrap();
    let dir=f.0.join(".aperture/teams/t1/runtime-attempts/t1-worker/g0");
    let fact:serde_json::Value=read_private_json(&dir.join("terminal.json")).unwrap();
    assert_eq!(fact["kind"],"smoke_cleaned");
    assert_eq!(OwnerStore::new(f.0.join(".aperture/run/owner")).read_owner("t1-worker").unwrap().state,OwnerState::Quarantined);
    assert!(SmokeAdmission::issue(&f.0,&actor,"t1","t1-worker",0).is_err());
    assert!(deadline::RuntimeAttempt::begin_bootstrap(&f.0,&AuthenticatedActor::launcher(),"t1","t1-worker",deadline::Deadline::new()).is_err());
}
#[test]
fn smoke_terminal_rechecks_every_bound_fact_before_publication() {
    let _guard=crate::team_auth::tests::ENV_LOCK.lock().unwrap();
    for drift in ["owner", "snapshot", "floor", "token", "actor", "admission", "effects", "process", "terminal", "state"] {
        let f=Fixture::new();let _env=SmokeEnv::set(&f);let actor=smoke_fixture(&f);
        let (a,started,mut attempt)=synthetic_cleaned_smoke(&f,&actor);
        let proof=SmokeCleanupProof::capture(&a,&started,attempt.id(),attempt.budget(),&actor).unwrap();
        let dir=".aperture/teams/t1/runtime-attempts/t1-worker/g0";
        match drift {
            "owner"=>{let mut owner=OwnerStore::new(f.0.join(".aperture/run/owner")).read_owner("t1-worker").unwrap();owner.generation=2;f.write(".aperture/run/owner/t1-worker.json",&serde_json::to_value(owner).unwrap());},
            "snapshot"=>{let mut snapshot:TeamSnapshot=read_private_json(&f.0.join(".aperture/teams/t1/team.json")).unwrap();snapshot.mission="drift".into();f.write(".aperture/teams/t1/team.json",&serde_json::to_value(snapshot).unwrap());},
            "state"=>f.write(".aperture/teams/t1/state.json",&serde_json::json!({"schema_version":1,"state":"archived","generation":1,"epic_id":"aperture-fixture","failure":null,"updated_at":"2026-09-22T00:00:00Z"})),
            "floor"=>f.write(".aperture/run/revocations/t1-worker.json",&serde_json::json!({"broken":true})),
            "token"=>write_private_bytes_atomic(&f.0.join(".aperture/run/hub-tokens/t1-worker.token"),b"unexpected",false).unwrap(),
            "actor"=>write_private_bytes_atomic(&f.0.join(".aperture/run/hub-tokens/glados.token"),b"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",true).unwrap(),
            "admission"=>f.write(&format!("{dir}/admitted.json"),&serde_json::json!({"broken":true})),
            "effects"=>f.write(&format!("{dir}/effects.json"),&serde_json::json!({"broken":true})),
            "terminal"=>f.write(&format!("{dir}/terminal.json"),&serde_json::json!({"existing":"retained"})),
            "process"=>{let mut owner=OwnerStore::new(f.0.join(".aperture/run/owner")).read_owner("t1-worker").unwrap();owner.incarnation.as_mut().unwrap().processes[0].pid=std::process::id();f.write(".aperture/run/owner/t1-worker.json",&serde_json::to_value(owner).unwrap());},
            _=>unreachable!(),
        }
        assert!(attempt.finish_smoke_cleaned(proof,&actor).is_err(),"{drift}");
        if drift!="terminal" {assert!(!f.0.join(format!("{dir}/terminal.json")).exists(),"{drift}");}
        else {assert_eq!(read_private_json::<serde_json::Value>(&f.0.join(format!("{dir}/terminal.json"))).unwrap()["existing"],"retained");}
    }
}
#[test]
fn smoke_cleanup_proof_does_not_renew_cleanup_deadline_or_cross_attempt() {
    let _guard=crate::team_auth::tests::ENV_LOCK.lock().unwrap();let f=Fixture::new();let _env=SmokeEnv::set(&f);let actor=smoke_fixture(&f);
    let (a,started,attempt)=synthetic_cleaned_smoke(&f,&actor);
    let proof=SmokeCleanupProof::capture(&a,&started,attempt.id(),attempt.budget(),&actor).unwrap();
    let published=std::cell::Cell::new(false);
    assert_eq!(proof.with_revalidated(&f.0,"t1","t1-worker",0,attempt.id(),
        &deadline::Deadline::fixture_elapsed(Duration::from_secs(171)),&actor,||{published.set(true);Ok(())}),Err(ReplacementError::Deadline));
    assert!(!published.get());
    let proof=SmokeCleanupProof::capture(&a,&started,attempt.id(),attempt.budget(),&actor).unwrap();
    assert!(proof.with_revalidated(&f.0,"t1","t1-worker",0,"different-attempt",attempt.budget(),&actor,||{published.set(true);Ok(())}).is_err());
    assert!(!published.get());
    let held=crate::owner::try_lock(&f.0.join(".aperture/run/team-locks"),"t1").unwrap();
    assert!(SmokeCleanupProof::capture(&a,&started,attempt.id(),attempt.budget(),&actor).is_err());
    drop(held);
}
#[test]
fn smoke_finally_runs_once_on_success_auth_drift_and_observation_failure() {
    for observation in [Ok(()),Err(ReplacementError::AuthorizationRequired),Err(ReplacementError::ModelUnverified),Err(ReplacementError::Deadline)] {
        for cleanup in [Ok(()),Err(ReplacementError::StartCleanupUnverified)] {
            let count=std::cell::Cell::new(0);
            let result=smoke_finally(observation.clone(),||{count.set(count.get()+1);cleanup.clone()});
            assert_eq!(count.get(),1);
            assert_eq!(result,if cleanup.is_err(){cleanup}else{observation.clone()});
        }
    }
}

// Recovery fixtures never launch a harness or call the hub. The PID is beyond
// the native allocation range; native absence is nevertheless checked.
fn stopped_recovery_fixture(f: &Fixture) -> (AuthenticatedActor, StoppedSmokeProof) {
    let actor = smoke_fixture(f);
    let store = OwnerStore::new(f.0.join(".aperture/run/owner"));
    let launcher = AuthenticatedActor::launcher();
    let tuple = ExecutionTuple {harness:Harness::Claude,model:"claude-sonnet-5".into(),reasoning:None};
    let res = store.reserve_start(&launcher,"t1-worker",0,tuple.clone()).unwrap();
    store.bind_and_publish_token(&launcher,&res,"a".repeat(64),||Ok(())).unwrap();
    let pid=2_000_000_001; let birth=1_790_000_000_000_001;
    store.record_start_candidate(&launcher,&res,Incarnation {pid,start_time:birth,
        thread_id:String::new(),token_id:"a".repeat(64),harness:Harness::Claude,
        model:tuple.model,reasoning:None,observed:false,
        processes:vec![crate::owner::ProcessIdentity {pid,start_time:birth,ppid:1,pgid:pid,
            cmdline_sha256:"b".repeat(64),cwd:"/fixture".into()}]}).unwrap();
    let owner=store.read_owner("t1-worker").unwrap();
    let snapshot_hash={use sha2::{Digest,Sha256};format!("{:x}",Sha256::digest(std::fs::read(f.0.join(".aperture/teams/t1/team.json")).unwrap()))};
    f.write(".aperture/run/t1-worker.g1.claude-attempt.json",&serde_json::json!({
        "schema_version":1,"team":"t1","seat":"t1-worker","generation":1,
        "reservation_nonce_sha256":owner.reservation_nonce_sha256,"snapshot_sha256":snapshot_hash,
        "team_generation":1,"token_id":"a".repeat(64),"root_pid":pid,"root_start_time_us":birth,
        "session_id":uuid::Uuid::new_v4().to_string(),"requested_model":"claude-sonnet-5","created_at_ms":1}));
    let id=uuid::Uuid::new_v4().to_string();
    f.write(".aperture/teams/t1/runtime-attempts/t1-worker/g0/admitted.json",&serde_json::json!({
        "schema_version":1,"attempt_id":id,"team":"t1","seat":"t1-worker","old_generation":0,
        "admitted_at_ms":chrono::Utc::now().timestamp_millis()-200_000,"native_budget_ms":170_000,"cleanup_reserve_ms":40_000}));
    f.write(".aperture/teams/t1/runtime-attempts/t1-worker/g0/effects.json",&serde_json::json!({
        "schema_version":1,"attempt_id":id,"kind":"effects_may_have_occurred"}));
    let attempt=deadline::UnfinishedBootstrap::read_locked(&f.0,"t1","t1-worker").unwrap();
    let mut s=snapshot();s.thread_id.clear();s.processes[0].identity=team_process::identity_from_owner(pid,birth).unwrap();
    s.processes[0].process_group=pid;s.processes[0].cmdline_sha256="b".repeat(64);
    let guard=team_process::persist_for_stop(&f.0,"t1",&launcher,s).unwrap();
    let expected=store.read_owner_locked("t1-worker").unwrap();
    f.write(".aperture/run/revocations/t1-worker.json",&revoked_value());
    (actor,StoppedSmokeProof {home:f.0.clone(),team:"t1".into(),expected,attempt,guard})
}
#[test]
fn recovery_quarantines_exact_stopped_owner_without_observation_reset_or_new_start() {
    let _lock=crate::team_auth::tests::ENV_LOCK.lock().unwrap();let f=Fixture::new();let _env=SmokeEnv::set(&f);
    let (actor,p)=stopped_recovery_fixture(&f);
    let store=OwnerStore::new(f.0.join(".aperture/run/owner"));
    let original=p.expected.clone();
    let result=store.quarantine_reconciled_smoke(&actor,&p).unwrap();
    assert_eq!(result.state,OwnerState::Quarantined);assert_eq!(result.generation,1);
    assert_eq!(result.incarnation,original.incarnation);assert!(!result.incarnation.unwrap().observed);
    assert!(result.provisional_token_id.is_none());assert!(result.reservation_nonce_sha256.is_none());
    p.attempt.record_locked(&f.0).unwrap();
    let dir=f.0.join(".aperture/teams/t1/runtime-attempts/t1-worker/g0");
    assert!(!dir.join("terminal.json").exists());
    let fact:serde_json::Value=read_private_json(&dir.join("reconciled.json")).unwrap();
    assert_eq!(fact["kind"],"stopped_reconciled");
    assert!(store.quarantine_reconciled_smoke(&actor,&p).is_err());
    assert!(!f.0.join(".aperture/run/managed").exists());
}
#[test]
fn recovery_proof_rejects_drift_and_missing_revocation_without_owner_write() {
    let _lock=crate::team_auth::tests::ENV_LOCK.lock().unwrap();
    for kind in ["actor","owner","attempt","floor","token","snapshot","active"] {
        let f=Fixture::new();let _env=SmokeEnv::set(&f);let (actor,p)=stopped_recovery_fixture(&f);
        match kind {
            "actor"=>write_private_bytes_atomic(&f.0.join(".aperture/run/hub-tokens/glados.token"),b"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",true).unwrap(),
            "owner"|"active"=>{let mut o=p.expected.clone(); if kind=="owner" {o.generation=2;} else {o.state=OwnerState::Active;}
                f.write(".aperture/run/owner/t1-worker.json",&serde_json::to_value(o).unwrap());},
            "attempt"=>f.write(".aperture/run/t1-worker.g1.claude-attempt.json",&serde_json::json!({})),
            "floor"=>std::fs::remove_file(f.0.join(".aperture/run/revocations/t1-worker.json")).unwrap(),
            "token"=>write_private_bytes_atomic(&f.0.join(".aperture/run/hub-tokens/t1-worker.token"),b"not-revoked",false).unwrap(),
            "snapshot"=>{let mut s:TeamSnapshot=read_private_json(&f.0.join(".aperture/teams/t1/team.json")).unwrap();s.mission="changed".into();f.write(".aperture/teams/t1/team.json",&serde_json::to_value(s).unwrap());},
            _=>unreachable!(),
        }
        let path=f.0.join(".aperture/run/owner/t1-worker.json");let before=std::fs::read(&path).unwrap();
        assert!(OwnerStore::new(f.0.join(".aperture/run/owner")).quarantine_reconciled_smoke(&actor,&p).is_err(),"{kind}");
        assert_eq!(std::fs::read(path).unwrap(),before);
    }
}
#[test]
fn recovery_entry_rejects_non_glados_and_non_diagnostic_selectors_before_effects() {
    let f=Fixture::new();
    for actor in [AuthenticatedActor::operator_ui(),AuthenticatedActor::launcher()] {
        assert_eq!(reconcile_stopped_claude_smoke(&f.0,&actor,"t1","t1-worker",1),Err(ReplacementError::AuthorizationRequired));
    }
    assert_eq!(std::fs::read_dir(&f.0).unwrap().count(),0);
}
