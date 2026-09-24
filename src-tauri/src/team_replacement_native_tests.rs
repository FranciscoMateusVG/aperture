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
    // Selector 1 is the GLaDOS-only recovery entry: the operator UI and the
    // launcher are refused before any native proof; 2 is never a selector.
    for actor in [AuthenticatedActor::operator_ui(), AuthenticatedActor::launcher()] {
        assert_eq!(
            bootstrap_authorized(&home, &actor, "t1", "t1-worker", 1).unwrap_err(),
            ReplacementError::AuthorizationRequired
        );
    }
    assert_eq!(
        bootstrap_authorized(&home, &AuthenticatedActor::operator_ui(), "t1", "t1-worker", 2)
            .unwrap_err(),
        ReplacementError::GenerationMismatch
    );
    assert!(!home.exists());
}

/// Factual shape of the failed QA seat (2026-09-24 readback): quarantined g1,
/// incarnation never observed and thread never bound, the gated root recorded
/// and Gone, provisional token id retained and equal to the incarnation token,
/// no hub token, revocation floor exactly g1 for that digest, normal launch and
/// attempt records, g0 attempt expired with an Unknown terminal.
struct Recovery { f: Fixture, attempt: crate::team_claude_launch::ClaudeAttempt, raw_sha: String, typed_sha: String }
impl Recovery {
    const TEAM: &'static str = "t1";
    const SEAT: &'static str = "t1-qa";
    fn token() -> String { "a".repeat(64) }
    fn new() -> Self {
        use sha2::{Digest, Sha256};
        let f = Fixture::new();
        let home = f.0.clone();
        let agent = home.join(".claude/aperture").join(Self::SEAT);
        f.write(".claude/aperture/t1-qa/manifest.json", &serde_json::json!({"name":Self::SEAT,"role":"qa","model":crate::team_claude_launch::MODEL,"enabled":true}));
        f.write(".claude/aperture/t1-qa/TEAM", &serde_json::json!({"schema_version":1,"team":Self::TEAM,"role":"qa"}));
        write_private_bytes_atomic(&agent.join(".complete"), b"complete\n", true).unwrap();
        write_private_bytes_atomic(&agent.join("prompt.md"), b"fixture", true).unwrap();
        f.write(".aperture/teams/t1/team.json", &serde_json::json!({
            "schema_version":1,"team":Self::TEAM,"project":"project:aperture","repo":"aperture","mission":"fixture","acceptance":"fixture",
            "preset":{"id":null,"sha256":null},"lead":Self::SEAT,
            "seats":[{"name":Self::SEAT,"role":"qa","harness":"claude","model":crate::team_claude_launch::MODEL,"reasoning":null}],
            "fallbacks":[],"grants":[],"created_at":"2026-09-24T00:00:00Z","creation_request_id":uuid::Uuid::new_v4().to_string(),"staging_uuid":uuid::Uuid::new_v4().to_string()}));
        f.write(".aperture/teams/t1/state.json", &serde_json::json!({"schema_version":1,"state":"active","generation":1,"epic_id":"aperture-fixture","failure":null,"updated_at":"2026-09-24T00:00:00Z"}));
        let raw = std::fs::read(home.join(".aperture/teams/t1/team.json")).unwrap();
        let raw_sha = format!("{:x}", Sha256::digest(&raw));
        let typed: TeamSnapshot = serde_json::from_slice(&raw).unwrap();
        let typed_sha = smoke_hash(&typed).unwrap();
        let now = chrono::Utc::now().timestamp_millis();
        let g0 = uuid::Uuid::new_v4().to_string();
        f.write(".aperture/teams/t1/runtime-attempts/t1-qa/g0/admitted.json", &serde_json::json!({"schema_version":1,"attempt_id":g0,"team":Self::TEAM,"seat":Self::SEAT,"old_generation":0,"admitted_at_ms":now-300_000,"native_budget_ms":170000,"cleanup_reserve_ms":40000}));
        f.write(".aperture/teams/t1/runtime-attempts/t1-qa/g0/effects.json", &serde_json::json!({"schema_version":1,"attempt_id":g0,"kind":"effects_may_have_occurred"}));
        f.write(".aperture/teams/t1/runtime-attempts/t1-qa/g0/terminal.json", &serde_json::json!({"schema_version":1,"attempt_id":g0,"kind":"unknown"}));
        let attempt = crate::team_claude_launch::ClaudeAttempt {
            schema_version: 1, team: Self::TEAM.into(), seat: Self::SEAT.into(), generation: 1,
            reservation_nonce_sha256: "c".repeat(64), snapshot_sha256: raw_sha.clone(), team_generation: 1,
            token_id: Self::token(), root_pid: 900001, root_start_time_us: 42,
            session_id: uuid::Uuid::new_v4().to_string(), requested_model: crate::team_claude_launch::MODEL.into(),
            created_at_ms: now - 200_000, mode: crate::team_claude_launch::ClaudeLaunchMode::NormalPositional,
        };
        let r = Self { f, attempt, raw_sha, typed_sha };
        r.owner(|_| {});
        r.attempt_file(|_| {});
        r.launch(|_| {});
        r.release(|_| {});
        r.revocations(|_| {});
        ensure_private_dir(&home.join(".aperture/run/hub-tokens")).unwrap();
        r
    }
    fn home(&self) -> std::path::PathBuf { self.f.0.clone() }
    fn owner_value(&self) -> serde_json::Value {
        serde_json::json!({"schema_version":1,"seat":Self::SEAT,"generation":1,"state":"quarantined",
            "reservation_nonce_sha256":null,"provisional_token_id":Self::token(),
            "requested":{"harness":"claude","model":crate::team_claude_launch::MODEL,"reasoning":null},
            "incarnation":{"pid":900001,"start_time":42,"thread_id":"","token_id":Self::token(),"harness":"claude","model":crate::team_claude_launch::MODEL,"reasoning":null,"observed":false,
                "processes":[{"pid":900001,"start_time":42,"ppid":1,"pgid":900001,"cmdline_sha256":"b".repeat(64),"cwd":"/fixture"}]},
            "since":"2026-09-24T00:00:00Z","writer":"launcher"})
    }
    fn owner(&self, mutate: impl Fn(&mut serde_json::Value)) {
        let mut v = self.owner_value(); mutate(&mut v);
        self.f.write(".aperture/run/owner/t1-qa.json", &v);
    }
    fn attempt_file(&self, mutate: impl Fn(&mut serde_json::Value)) {
        let mut v = serde_json::to_value(&self.attempt).unwrap(); mutate(&mut v);
        self.f.write(".aperture/run/t1-qa.g1.claude-attempt.json", &v);
    }
    fn launch(&self, mutate: impl Fn(&mut serde_json::Value)) {
        let mut v = serde_json::json!({"schema_version":1,"team":Self::TEAM,"seat":Self::SEAT,"generation":1,"session_id":self.attempt.session_id,
            "token_id":Self::token(),"snapshot_sha256":self.typed_sha,"mode":"normal_positional","args":["--model",crate::team_claude_launch::MODEL]});
        mutate(&mut v);
        self.f.write(".aperture/run/managed/t1-qa/g1/claude-launch.json", &v);
    }
    fn release(&self, mutate: impl Fn(&mut serde_json::Value)) {
        let mut v = serde_json::json!({"schema_version":1,"attempt_sha256":smoke_hash(&self.attempt).unwrap(),"launch_sha256":"d".repeat(64),"root_pid":900001,"root_start_time_us":42});
        mutate(&mut v);
        self.f.write(".aperture/run/managed/t1-qa/g1/claude-release.json", &v);
    }
    fn revocations(&self, mutate: impl Fn(&mut serde_json::Value)) {
        let mut v = serde_json::json!({"schema_version":1,"seat":Self::SEAT,"revoked_through_generation":1,"revoked_token_ids":[Self::token()]});
        mutate(&mut v);
        self.f.write(".aperture/run/revocations/t1-qa.json", &v);
    }
    fn owner_record(&self) -> OwnerRecord { read_private_json(&self.home().join(".aperture/run/owner/t1-qa.json")).unwrap() }
    fn proof(&self) -> Result<(String, String), ReplacementError> {
        RecoveryAdmission::proof_locked(&self.home(), Self::TEAM, Self::SEAT, &self.owner_record(), RecoveryPhase::PreAdmission)
    }
    fn tuple() -> ExecutionTuple {
        ExecutionTuple { harness: Harness::Claude, model: crate::team_claude_launch::MODEL.into(), reasoning: None }
    }
    fn file_bytes(&self, relative: &str) -> Vec<u8> { std::fs::read(self.home().join(relative)).unwrap() }
    fn g0_bytes(&self) -> Vec<Vec<u8>> {
        ["admitted", "effects", "terminal"].iter()
            .map(|n| std::fs::read(self.home().join(format!(".aperture/teams/t1/runtime-attempts/t1-qa/g0/{n}.json"))).unwrap())
            .collect()
    }
}

#[test]
fn recovery_proof_accepts_only_the_factual_quarantined_first_claude_bootstrap() {
    let r = Recovery::new();
    let (typed, token) = r.proof().unwrap();
    assert_eq!((typed, token), (r.typed_sha.clone(), Recovery::token()));
    // Retained provisional equal to the incarnation token is the factual case;
    // None is also accepted; anything else is not the proved incarnation.
    r.owner(|o| o["provisional_token_id"] = serde_json::Value::Null);
    assert!(r.proof().is_ok());
    r.owner(|o| o["provisional_token_id"] = serde_json::json!("f".repeat(64)));
    assert_eq!(r.proof().unwrap_err(), ReplacementError::GenerationMismatch);
    let owner_cases: [(&str, fn(&mut serde_json::Value)); 8] = [
        ("active owner", |o| o["state"] = "active".into()),
        ("starting owner", |o| o["state"] = "starting".into()),
        ("generation 2", |o| o["generation"] = 2.into()),
        ("observed", |o| o["incarnation"]["observed"] = true.into()),
        ("thread bound", |o| o["incarnation"]["thread_id"] = uuid::Uuid::new_v4().to_string().into()),
        ("nonce present", |o| o["reservation_nonce_sha256"] = "e".repeat(64).into()),
        ("no recorded processes", |o| o["incarnation"]["processes"] = serde_json::json!([])),
        ("root not recorded", |o| o["incarnation"]["processes"][0]["pid"] = 900002.into()),
    ];
    for (name, mutate) in owner_cases {
        r.owner(mutate);
        assert_eq!(r.proof().unwrap_err(), ReplacementError::GenerationMismatch, "{name}");
    }
    // A recorded process that is not Gone (this test process, wrong birth) stops.
    let pid = std::process::id();
    r.owner(|o| {
        o["incarnation"]["pid"] = pid.into();
        o["incarnation"]["processes"][0]["pid"] = pid.into();
    });
    assert_eq!(r.proof().unwrap_err(), ReplacementError::StopUnverified);
    r.owner(|_| {});
    assert!(r.proof().is_ok());
    // Token/floor.
    write_private_bytes_atomic(&r.home().join(".aperture/run/hub-tokens/t1-qa.token"), b"x", true).unwrap();
    assert_eq!(r.proof().unwrap_err(), ReplacementError::RevocationUnverified);
    std::fs::remove_file(r.home().join(".aperture/run/hub-tokens/t1-qa.token")).unwrap();
    r.revocations(|v| v["revoked_through_generation"] = 2.into());
    assert_eq!(r.proof().unwrap_err(), ReplacementError::RevocationUnverified);
    r.revocations(|v| v["revoked_token_ids"] = serde_json::json!(["9".repeat(64)]));
    assert_eq!(r.proof().unwrap_err(), ReplacementError::RevocationUnverified);
    r.revocations(|_| {});
    // g0 must be an expired normal bootstrap with an Unknown terminal, never reconciled.
    r.f.write(".aperture/teams/t1/runtime-attempts/t1-qa/g0/terminal.json", &serde_json::json!({"schema_version":1,"attempt_id":"x","kind":"active"}));
    assert_eq!(r.proof().unwrap_err(), ReplacementError::OutcomeUnknown);
    let g0: serde_json::Value = read_private_json(&r.home().join(".aperture/teams/t1/runtime-attempts/t1-qa/g0/admitted.json")).unwrap();
    r.f.write(".aperture/teams/t1/runtime-attempts/t1-qa/g0/terminal.json", &serde_json::json!({"schema_version":1,"attempt_id":g0["attempt_id"],"kind":"unknown"}));
    assert!(r.proof().is_ok());
    r.f.write(".aperture/teams/t1/runtime-attempts/t1-qa/g0/reconciled.json", &serde_json::json!({"schema_version":1,"attempt_id":g0["attempt_id"],"kind":"stopped_reconciled"}));
    assert_eq!(r.proof().unwrap_err(), ReplacementError::OutcomeUnknown);
    std::fs::remove_file(r.home().join(".aperture/teams/t1/runtime-attempts/t1-qa/g0/reconciled.json")).unwrap();
    // Category comes from BOTH records being normal_positional and bound to this owner.
    r.attempt_file(|a| { a.as_object_mut().unwrap().remove("mode"); });
    assert_eq!(r.proof().unwrap_err(), ReplacementError::OutcomeUnknown, "diagnostic attempt");
    r.attempt_file(|a| a["token_id"] = "9".repeat(64).into());
    assert_eq!(r.proof().unwrap_err(), ReplacementError::OutcomeUnknown, "attempt of another token");
    r.attempt_file(|a| a["snapshot_sha256"] = r.typed_sha.clone().into());
    assert_eq!(r.proof().unwrap_err(), ReplacementError::OutcomeUnknown, "attempt binds the raw snapshot digest");
    r.attempt_file(|_| {});
    r.launch(|l| { l.as_object_mut().unwrap().remove("mode"); });
    assert_eq!(r.proof().unwrap_err(), ReplacementError::OutcomeUnknown, "diagnostic launch record");
    r.launch(|l| l["session_id"] = uuid::Uuid::new_v4().to_string().into());
    assert_eq!(r.proof().unwrap_err(), ReplacementError::OutcomeUnknown, "launch of another session");
    r.launch(|l| l["snapshot_sha256"] = r.raw_sha.clone().into());
    assert_eq!(r.proof().unwrap_err(), ReplacementError::OutcomeUnknown, "launch binds the typed snapshot digest");
    r.launch(|_| {});
    r.release(|v| v["attempt_sha256"] = "9".repeat(64).into());
    assert_eq!(r.proof().unwrap_err(), ReplacementError::OutcomeUnknown, "release of another attempt");
    r.release(|_| {});
    assert!(r.proof().is_ok());
    // Unobserved means no sample and no proven rejection; one admission means no g2 and no attempt g1.
    for (path, is_dir) in [
        (".aperture/run/t1-qa.g1.claude-observation.json", false),
        (".aperture/run/t1-qa.g1.claude-rejected.json", false),
        (".aperture/run/t1-qa.g2.claude-attempt.json", false),
        (".aperture/run/managed/t1-qa/g2", true),
        (".aperture/teams/t1/runtime-attempts/t1-qa/g1", true),
    ] {
        let p = r.home().join(path);
        if is_dir { ensure_private_dir(&p).unwrap(); } else { r.f.write(path, &serde_json::json!({})); }
        assert_eq!(r.proof().unwrap_err(), ReplacementError::OutcomeUnknown, "{path}");
        if is_dir { std::fs::remove_dir_all(&p).unwrap(); } else { std::fs::remove_file(&p).unwrap(); }
    }
    assert!(r.proof().is_ok());
    // The tuple never decides the category, but recovery closes a Claude first
    // bootstrap only: the same sealed snapshot with a coherent Codex seat is not
    // launchable here (a non-catalog literal is already rejected at parse time).
    let mut snapshot: serde_json::Value = read_private_json(&r.home().join(".aperture/teams/t1/team.json")).unwrap();
    snapshot["seats"][0]["harness"] = "codex".into();
    snapshot["seats"][0]["model"] = "gpt-6-astra".into();
    snapshot["seats"][0]["reasoning"] = "high".into();
    r.f.write(".aperture/teams/t1/team.json", &snapshot);
    assert_eq!(r.proof().unwrap_err(), ReplacementError::LaunchUnavailable);
    snapshot["seats"][0]["harness"] = "claude".into();
    snapshot["seats"][0]["model"] = "claude-opus-5-5".into();
    snapshot["seats"][0]["reasoning"] = serde_json::Value::Null;
    r.f.write(".aperture/teams/t1/team.json", &snapshot);
    assert!(r.proof().is_err(), "non-admitted literal never proves");
}

#[test]
fn recovery_deadline_admission_writes_only_runtime_attempt_g1_and_refuses_a_second() {
    let r = Recovery::new();
    let before = r.g0_bytes();
    let launcher = AuthenticatedActor::launcher();
    // The ordinary first-start admission never accepts the quarantined owner.
    assert_eq!(
        deadline::RuntimeAttempt::begin_bootstrap(&r.home(), &launcher, "t1", "t1-qa", deadline::Deadline::new()).err().unwrap(),
        ReplacementError::GenerationMismatch
    );
    let attempt = deadline::RuntimeAttempt::begin_bootstrap_recovery(&r.home(), &launcher, "t1", "t1-qa", deadline::Deadline::new()).unwrap();
    let g1 = r.home().join(".aperture/teams/t1/runtime-attempts/t1-qa/g1");
    let admitted: serde_json::Value = read_private_json(&g1.join("admitted.json")).unwrap();
    assert_eq!(admitted["old_generation"], 1);
    assert_eq!(admitted["seat"], "t1-qa");
    assert_eq!(r.g0_bytes(), before, "g0 facts are never rewritten");
    assert!(!g1.join("effects.json").exists());
    drop(attempt);
    // One admission only: the existing g1 refuses, and so does the proof afterwards.
    assert_eq!(
        deadline::RuntimeAttempt::begin_bootstrap_recovery(&r.home(), &launcher, "t1", "t1-qa", deadline::Deadline::new()).err().unwrap(),
        ReplacementError::OutcomeUnknown
    );
    assert_eq!(r.proof().unwrap_err(), ReplacementError::OutcomeUnknown);
    assert_eq!(r.g0_bytes(), before);
    // The deadline arm also rejects an observed or thread-bound quarantined owner.
    std::fs::remove_dir_all(&g1).unwrap();
    r.owner(|o| o["incarnation"]["observed"] = true.into());
    assert_eq!(
        deadline::RuntimeAttempt::begin_bootstrap_recovery(&r.home(), &launcher, "t1", "t1-qa", deadline::Deadline::new()).err().unwrap(),
        ReplacementError::GenerationMismatch
    );
    r.owner(|o| o["provisional_token_id"] = "f".repeat(64).into());
    assert_eq!(
        deadline::RuntimeAttempt::begin_bootstrap_recovery(&r.home(), &launcher, "t1", "t1-qa", deadline::Deadline::new()).err().unwrap(),
        ReplacementError::GenerationMismatch
    );
    assert!(!g1.exists());
}

/// The production order with an inert fixture: locked issue (pre-admission
/// proof), deadline admission + effects, pre-reserve proof against THAT
/// attempt, bound g2 reserve. No launch, gate, token, provider or process.
#[test]
fn recovery_production_order_reaches_a_bound_g2_reservation_and_only_once() {
    let r = Recovery::new();
    let home = r.home();
    let store = OwnerStore::new(home.join(".aperture/run/owner"));
    let g0_before = r.g0_bytes();
    let attempt_bytes = r.file_bytes(".aperture/run/t1-qa.g1.claude-attempt.json");
    let launch_bytes = r.file_bytes(".aperture/run/managed/t1-qa/g1/claude-launch.json");
    let admission = RecoveryAdmission::issue_checked(&home, "t1", "t1-qa", || Ok(())).unwrap();
    assert_eq!((admission.snapshot_sha256.clone(), admission.token_id.clone()), (r.typed_sha.clone(), Recovery::token()));
    assert!(admission.attempt_id.is_none());
    // An unbound admission never reserves.
    assert_eq!(reserve_recovery(&store, &admission, "t1-qa", &Recovery::tuple()).unwrap_err(), ReplacementError::OutcomeUnknown);
    // Pre-admission phase under the same locks production takes.
    {
        let _team = crate::owner::try_lock(&home.join(".aperture/run/team-locks"), "t1").unwrap();
        let _seat = store.lock("t1-qa").unwrap();
        admission.reprove_locked(&store, RecoveryPhase::PreAdmission).unwrap();
    }
    let mut attempt = deadline::RuntimeAttempt::begin_bootstrap_recovery(&home, &AuthenticatedActor::launcher(), "t1", "t1-qa", deadline::Deadline::new()).unwrap();
    attempt.admit_effects().unwrap();
    let g1 = home.join(".aperture/teams/t1/runtime-attempts/t1-qa/g1");
    assert!(g1.join("admitted.json").exists() && g1.join("effects.json").exists() && !g1.join("terminal.json").exists());
    // The regression GLaDOS found: after the admission the pre-admission phase
    // must refuse, and the pre-reserve phase accepts exactly this attempt.
    assert_eq!(admission.reprove_locked(&store, RecoveryPhase::PreAdmission).unwrap_err(), ReplacementError::OutcomeUnknown);
    let other = uuid::Uuid::new_v4().to_string();
    assert_eq!(admission.reprove_locked(&store, RecoveryPhase::PreReserve { attempt_id: &other }).unwrap_err(), ReplacementError::OutcomeUnknown);
    assert_eq!(admission.reprove_locked(&store, RecoveryPhase::PreReserve { attempt_id: "not-a-uuid" }).unwrap_err(), ReplacementError::OutcomeUnknown);
    let bound = admission.bind_attempt(attempt.id());
    bound.reprove_locked(&store, RecoveryPhase::PreReserve { attempt_id: attempt.id() }).unwrap();
    // The bound reserve, with the team lock held as in start_native.
    let reservation = {
        let _team = crate::owner::try_lock(&home.join(".aperture/run/team-locks"), "t1").unwrap();
        reserve_recovery(&store, &bound, "t1-qa", &Recovery::tuple()).unwrap()
    };
    assert_eq!(reservation.generation, 2);
    let owner = store.read_owner("t1-qa").unwrap();
    assert_eq!(owner.generation, 2);
    assert_eq!(owner.state, OwnerState::Starting);
    assert!(owner.incarnation.is_none() && owner.provisional_token_id.is_none());
    assert!(owner.reservation_nonce_sha256.is_some());
    assert_eq!(owner.requested, Recovery::tuple());
    // Every historical fact is intact; only the owner and the new g1 attempt moved.
    assert_eq!(r.g0_bytes(), g0_before);
    assert_eq!(r.file_bytes(".aperture/run/t1-qa.g1.claude-attempt.json"), attempt_bytes);
    assert_eq!(r.file_bytes(".aperture/run/managed/t1-qa/g1/claude-launch.json"), launch_bytes);
    assert!(!home.join(".aperture/run/t1-qa.g2.claude-attempt.json").exists());
    // Once: the proof is pre-reserve only, the admission is one-shot, and a
    // second reserve or admission never happens.
    assert_eq!(bound.reprove_locked(&store, RecoveryPhase::PreReserve { attempt_id: attempt.id() }).unwrap_err(), ReplacementError::GenerationMismatch);
    bound.revalidate_locked().unwrap();
    assert_eq!(reserve_recovery(&store, &bound, "t1-qa", &Recovery::tuple()).unwrap_err(), ReplacementError::GenerationMismatch);
    assert_eq!(
        deadline::RuntimeAttempt::begin_bootstrap_recovery(&home, &AuthenticatedActor::launcher(), "t1", "t1-qa", deadline::Deadline::new()).err().unwrap(),
        ReplacementError::GenerationMismatch
    );
    assert_eq!(RecoveryAdmission::issue_checked(&home, "t1", "t1-qa", || Ok(())).err().unwrap(), ReplacementError::GenerationMismatch);
    // A finished attempt (terminal written) is never a pre-reserve match.
    let r2 = Recovery::new();
    let home2 = r2.home();
    let store2 = OwnerStore::new(home2.join(".aperture/run/owner"));
    let admission2 = RecoveryAdmission::issue_checked(&home2, "t1", "t1-qa", || Ok(())).unwrap();
    let mut attempt2 = deadline::RuntimeAttempt::begin_bootstrap_recovery(&home2, &AuthenticatedActor::launcher(), "t1", "t1-qa", deadline::Deadline::new()).unwrap();
    attempt2.admit_effects().unwrap();
    let bound2 = admission2.bind_attempt(attempt2.id());
    bound2.reprove_locked(&store2, RecoveryPhase::PreReserve { attempt_id: attempt2.id() }).unwrap();
    assert_eq!(attempt2.finish_unknown().unwrap_err(), ReplacementError::OutcomeUnknown);
    assert_eq!(bound2.reprove_locked(&store2, RecoveryPhase::PreReserve { attempt_id: attempt2.id() }).unwrap_err(), ReplacementError::OutcomeUnknown);
    assert_eq!(reserve_recovery(&store2, &bound2, "t1-qa", &Recovery::tuple()).unwrap_err(), ReplacementError::OutcomeUnknown);
    assert_eq!(store2.read_owner("t1-qa").unwrap().state, OwnerState::Quarantined);
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
fn legacy_claude_alias_denies_before_runtime_io_without_affecting_normal_policy() {
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
        model: "sonnet".into(),
        reasoning: None,
    };
    // Legacy alias and deliberately missing HOME: reject before examining native
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
    assert!(crate::teams::managed_launch_enabled(&Harness::Claude), "normal launch does not make legacy aliases executable");
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

#[test]
fn retirement_authority_never_impersonates_operator_or_launches() {
    let f=Fixture::new();
    for actor in [AuthenticatedActor::operator_ui(),AuthenticatedActor::launcher()] {
        let authority=ReplacementAuthority::GladosRetirement(&actor);
        assert_eq!(authority.revalidate(&target()),Err(ReplacementError::AuthorizationRequired));
        assert!(authority.inspect(&f.0,&target(),&[]).is_err());
        assert_eq!(replace_authorized(&f.0,authority,target(),&selection(),&[]).unwrap_err(),ReplacementError::AuthorizationRequired);
        assert_eq!(stop_for_retirement(&f.0,&actor,"t1","t1-worker",1,true).unwrap_err(),ReplacementError::AuthorizationRequired);
    }
    assert_eq!(NativePlan::Retirement.revalidate(&deadline::Deadline::new()).unwrap_err(),ReplacementError::AuthorizationRequired);
    assert_eq!(std::fs::read_dir(&f.0).unwrap().count(),0);
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
    assert!(crate::teams::managed_launch_enabled(&Harness::Claude), "historical diagnostic tests never enable a different launch path");
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

#[test]
fn claude_observation_lock_contention_is_pending_not_false_model_failure() {
    let _envlock=crate::team_auth::tests::ENV_LOCK.lock().unwrap();let f=Fixture::new();let _env=SmokeEnv::set(&f);
    let _actor=smoke_fixture(&f);let store=OwnerStore::new(f.0.join(".aperture/run/owner"));
    let tuple=ExecutionTuple{harness:Harness::Claude,model:"claude-sonnet-5".into(),reasoning:None};
    let reservation=store.reserve_start(&AuthenticatedActor::launcher(),"t1-worker",0,tuple).unwrap();
    for team_locked in [true,false] {
        let lock=if team_locked {crate::owner::try_lock(&f.0.join(".aperture/run/team-locks"),"t1").unwrap()}
            else {store.lock("t1-worker").unwrap()};
        assert!(matches!(runtime_observation(&f.0,"t1",&reservation,&Harness::Claude),Ok(None)));
        drop(lock);
        // The fixture has no candidate: after lock release its identity is
        // invalid, so it MUST fail rather than treating invalid data as pending.
        assert!(matches!(runtime_observation(&f.0,"t1",&reservation,&Harness::Claude),Err(ReplacementError::ModelUnverified)));
    }
}

#[test]
fn inbox_probe_no_input_before_active_and_no_retry_after_uncertain_send() {
    use std::cell::RefCell;
    for fail in ["activate","kickoff","readback","none"] {
        let events=RefCell::new(Vec::new());
        let result=inbox_after_activation(
            ||{events.borrow_mut().push("active");if fail=="activate" {Err(ReplacementError::ModelUnverified)}else{Ok(())}},
            ||{events.borrow_mut().push("kickoff");if fail=="kickoff" {Err(ReplacementError::OutcomeUnknown)}else{Ok(())}},
            ||{events.borrow_mut().push("readback");if fail=="readback" {Err(ReplacementError::OutcomeUnknown)}else{Ok(())}},
        );
        let expected=match fail {"activate"=>vec!["active"],"kickoff"=>vec!["active","kickoff"],_=>vec!["active","kickoff","readback"]};
        assert_eq!(*events.borrow(),expected);assert_eq!(result.is_ok(),fail=="none");
    }
}

#[test]
fn inbox_probe_native_denies_non_glados_before_any_launch_or_kickoff() {
    let f=Fixture::new();
    for actor in [AuthenticatedActor::launcher(),AuthenticatedActor::operator_ui()] {
        assert!(matches!(bootstrap_claude_inbox_probe_authorized(&f.0,&actor,"t1","t1-worker",0),Err(ReplacementError::AuthorizationRequired)));
    }
    assert!(!f.0.join(".aperture/run/managed/t1-worker").exists());
    assert!(crate::teams::managed_launch_enabled(&Harness::Claude), "old inbox probe remains denied after normal launch admission");
}

#[test]
fn inbox_probe_finally_always_cleans_including_successful_input_window() {
    use std::cell::Cell;
    for sequence_ok in [false,true] { for cleanup_ok in [false,true] {
        let cleaned=Cell::new(0);
        let result=smoke_finally(if sequence_ok {Ok(())} else {Err(ReplacementError::OutcomeUnknown)},
            ||{cleaned.set(cleaned.get()+1);if cleanup_ok {Ok(())}else{Err(ReplacementError::StartCleanupUnverified)}});
        assert_eq!(cleaned.get(),1);
        if !cleanup_ok {assert_eq!(result,Err(ReplacementError::StartCleanupUnverified));}
        else {assert_eq!(result.is_ok(),sequence_ok);}
    }}
}
