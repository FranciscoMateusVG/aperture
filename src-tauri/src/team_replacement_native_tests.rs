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
