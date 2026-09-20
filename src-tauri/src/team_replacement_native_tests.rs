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
        "schema_version":1,"team":"t1","project":"project:aperture","mission":"fixture","acceptance":"fixture",
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
