//! Private temporary facts and fake monotonic time only. No agent effects.
use super::*;
use std::os::unix::fs::{symlink, PermissionsExt};
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let home = std::env::temp_dir().join(format!("aperture-deadline-{}", uuid::Uuid::new_v4()));
        ensure_private_dir(&home).unwrap();
        Self(home)
    }
    fn attempt(&self) -> RuntimeAttempt {
        let dir = self.0.join("facts");
        ensure_private_dir(&dir).unwrap();
        RuntimeAttempt::publish(
            &self.0,
            dir,
            Admission {
                schema_version: 1,
                attempt_id: uuid::Uuid::new_v4().to_string(),
                team: "t1".into(),
                seat: "t1-worker".into(),
                old_generation: 1,
                admitted_at_ms: 1,
                native_budget_ms: 170_000,
                cleanup_reserve_ms: 40_000,
            },
            Deadline::new(),
        )
        .unwrap()
    }
    fn managed(&self) {
        let write = |rel: &str, v: serde_json::Value| {
            let p = self.0.join(rel);
            ensure_private_dir(p.parent().unwrap()).unwrap();
            write_private_json_atomic(&p, &v, false).unwrap();
        };
        write(
            ".aperture/teams/t1/team.json",
            serde_json::json!({
            "schema_version":1,"team":"t1","project":"project:aperture","repo":"aperture",
            "mission":"fixture","acceptance":"fixture","preset":{"id":null,"sha256":null},
            "lead":"t1-worker","seats":[{"name":"t1-worker","role":"lead","harness":"codex","model":"gpt-6-astra","reasoning":"high"}],
            "fallbacks":[],"grants":[],"created_at":"2026-09-20T00:00:00Z","creation_request_id":uuid::Uuid::new_v4().to_string(),"staging_uuid":uuid::Uuid::new_v4().to_string()}),
        );
        write(
            ".aperture/teams/t1/state.json",
            serde_json::json!({"schema_version":1,"state":"active","generation":1,"epic_id":"aperture-fixture","failure":null,"updated_at":"2026-09-20T00:00:00Z"}),
        );
        let runtime = self.0.join(".claude/aperture/t1-worker");
        ensure_private_dir(&runtime).unwrap();
        for name in ["TEAM", ".complete"] {
            crate::journal::write_private_bytes_atomic(&runtime.join(name), b"", false).unwrap();
        }
        write(
            ".aperture/run/owner/t1-worker.json",
            serde_json::json!({
            "schema_version":1,"seat":"t1-worker","generation":1,"state":"active",
            "reservation_nonce_sha256":null,"provisional_token_id":null,
            "requested":{"harness":"codex","model":"gpt-6-astra","reasoning":"high"},
            "incarnation":{"pid":900001,"start_time":42,"thread_id":"fixture","token_id":"a".repeat(64),
                "harness":"codex","model":"gpt-6-astra","reasoning":"high","observed":true,"processes":[]},
            "since":"2026-09-20T00:00:00Z","writer":"launcher"}),
        );
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
#[test]
fn fixed_budget_reserves_cleanup_and_never_renews() {
    let d = Deadline::new();
    assert!(d.forward_at(FORWARD_AFTER_FIRST_EFFECT, d.started).is_ok());
    assert_eq!(
        d.forward_at(
            FORWARD_AFTER_FIRST_EFFECT,
            d.started + Duration::from_secs(14)
        ),
        Err(ReplacementError::Deadline)
    );
    assert!(d
        .forward_at(Duration::ZERO, d.started + Duration::from_secs(129))
        .is_ok());
    assert_eq!(
        d.forward_at(Duration::ZERO, d.started + Duration::from_secs(130)),
        Err(ReplacementError::Deadline)
    );
    assert_eq!(
        d.remaining_at(d.started + Duration::from_secs(171)),
        Duration::ZERO
    );
    assert_eq!(d.cleanup_until(), d.started + Duration::from_secs(170));
}
#[test]
fn subprocess_deadline_cannot_spend_cleanup_reserve() {
    let d = Deadline::new();
    assert_eq!(
        d.forward_until(Duration::from_secs(1000)).unwrap(),
        d.started + Duration::from_secs(130)
    );
    assert!(d.forward_until(Duration::from_secs(2)).unwrap() < d.cleanup_until());
}
#[test]
fn crash_after_admission_retains_private_unknown_evidence() {
    let f = Fixture::new();
    let a = f.attempt();
    let path = a.dir.clone();
    let before = std::fs::read(path.join("admitted.json")).unwrap();
    drop(a);
    assert!(!path.join("terminal.json").exists());
    assert_eq!(
        std::fs::metadata(path.join("admitted.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(std::fs::read(path.join("admitted.json")).unwrap(), before);
    let admitted: Admission = read_private_json(&path.join("admitted.json")).unwrap();
    assert!(matches!(
        RuntimeAttempt::publish(&f.0, path, admitted, Deadline::new()),
        Err(ReplacementError::OutcomeUnknown)
    ));
}
#[test]
fn effect_admission_is_durable_and_idempotent_without_budget_reset() {
    let f = Fixture::new();
    let mut a = f.attempt();
    let start = a.budget.started;
    a.admit_effects().unwrap();
    let bytes = std::fs::read(a.dir.join("effects.json")).unwrap();
    a.admit_effects().unwrap();
    assert_eq!(a.budget.started, start);
    assert_eq!(std::fs::read(a.dir.join("effects.json")).unwrap(), bytes);
    assert_eq!(a.finish_failed(), Err(ReplacementError::OutcomeUnknown));
    let terminal: Fact = read_private_json(&a.dir.join("terminal.json")).unwrap();
    assert_eq!(terminal.kind, FactKind::Unknown);
}
#[test]
fn insufficient_budget_blocks_before_effect_intent() {
    let f = Fixture::new();
    let mut a = f.attempt();
    a.budget.started = Instant::now() - Duration::from_secs(25);
    assert_eq!(a.admit_effects(), Err(ReplacementError::Deadline));
    assert!(!a.dir.join("effects.json").exists());
    a.finish_failed().unwrap();
}
#[test]
fn no_active_success_after_forward_deadline_or_without_observed_owner() {
    let f = Fixture::new();
    let mut a = f.attempt();
    a.admit_effects().unwrap();
    assert_eq!(a.finish_active(), Err(ReplacementError::OutcomeUnknown));
    a.budget.started = Instant::now() - Duration::from_secs(130);
    assert_eq!(a.finish_active(), Err(ReplacementError::Deadline));
    assert!(!a.dir.join("terminal.json").exists());
    assert_eq!(a.finish_unknown(), Err(ReplacementError::OutcomeUnknown));
}
#[test]
fn terminal_is_append_only_and_cannot_be_reclassified() {
    let f = Fixture::new();
    let mut a = f.attempt();
    a.finish_failed().unwrap();
    let before = std::fs::read(a.dir.join("terminal.json")).unwrap();
    assert_eq!(a.finish_unknown(), Err(ReplacementError::OutcomeUnknown));
    assert_eq!(std::fs::read(a.dir.join("terminal.json")).unwrap(), before);
}
#[test]
fn corruption_or_symlink_cannot_authorize_effects() {
    for replace_with_symlink in [false, true] {
        let f = Fixture::new();
        let mut a = f.attempt();
        let p = a.dir.join("admitted.json");
        if replace_with_symlink {
            std::fs::remove_file(&p).unwrap();
            symlink(f.0.join("absent"), &p).unwrap();
        } else {
            std::fs::write(&p, b"{}").unwrap();
        }
        assert_eq!(a.admit_effects(), Err(ReplacementError::OutcomeUnknown));
        assert!(!a.dir.join("effects.json").exists());
    }
}
#[test]
fn operator_is_not_launcher_and_cannot_mint_attempt_directly() {
    let f = Fixture::new();
    assert!(matches!(
        RuntimeAttempt::begin(
            &f.0,
            &AuthenticatedActor::operator_ui(),
            "t1",
            "t1-worker",
            1,
            Deadline::new()
        ),
        Err(ReplacementError::AuthorizationRequired)
    ));
    assert!(!f.0.join(".aperture").exists());
}
#[test]
fn native_admission_is_single_use_even_after_known_pre_effect_failure() {
    let f = Fixture::new();
    f.managed();
    let actor = AuthenticatedActor::launcher();
    let mut a = RuntimeAttempt::begin(&f.0, &actor, "t1", "t1-worker", 1, Deadline::new()).unwrap();
    a.finish_failed().unwrap();
    assert!(matches!(
        RuntimeAttempt::begin(&f.0, &actor, "t1", "t1-worker", 1, Deadline::new()),
        Err(ReplacementError::OutcomeUnknown)
    ));
}
#[test]
fn native_admission_refuses_generation_drift_and_unsafe_parent() {
    let f = Fixture::new();
    f.managed();
    let actor = AuthenticatedActor::launcher();
    assert!(matches!(
        RuntimeAttempt::begin(&f.0, &actor, "t1", "t1-worker", 2, Deadline::new()),
        Err(ReplacementError::GenerationMismatch)
    ));
    let outside = f.0.join("outside");
    ensure_private_dir(&outside).unwrap();
    symlink(&outside, f.0.join(".aperture/teams/t1/runtime-attempts")).unwrap();
    assert!(RuntimeAttempt::begin(&f.0, &actor, "t1", "t1-worker", 1, Deadline::new()).is_err());
    assert_eq!(std::fs::read_dir(outside).unwrap().count(), 0);
}
#[test]
fn native_terminal_success_checks_real_owner_generation_and_observation() {
    let f = Fixture::new();
    f.managed();
    let actor = AuthenticatedActor::launcher();
    let mut a = RuntimeAttempt::begin(&f.0, &actor, "t1", "t1-worker", 1, Deadline::new()).unwrap();
    a.admit_effects().unwrap();
    assert_eq!(a.finish_active(), Err(ReplacementError::OutcomeUnknown));
    let path = f.0.join(".aperture/run/owner/t1-worker.json");
    let mut owner: OwnerRecord = read_private_json(&path).unwrap();
    owner.generation = 2;
    owner.incarnation.as_mut().unwrap().observed = false;
    write_private_json_atomic(&path, &owner, true).unwrap();
    assert_eq!(a.finish_active(), Err(ReplacementError::OutcomeUnknown));
    owner.incarnation.as_mut().unwrap().observed = true;
    write_private_json_atomic(&path, &owner, true).unwrap();
    a.finish_active().unwrap();
}

#[test]
fn admission_preserves_time_already_spent_in_repository_preflight() {
    let f = Fixture::new();
    f.managed();
    let mut budget = Deadline::new();
    budget.started = Instant::now() - Duration::from_secs(25);
    let original = budget.started;
    let mut a = RuntimeAttempt::begin(
        &f.0,
        &AuthenticatedActor::launcher(),
        "t1",
        "t1-worker",
        1,
        budget,
    )
    .unwrap();
    assert_eq!(a.budget.started, original);
    assert_eq!(a.admit_effects(), Err(ReplacementError::Deadline));
    assert!(!a.dir.join("effects.json").exists());
}

#[test]
fn bootstrap_g0_has_separate_exact_admission_and_never_relaxes_replace() {
    let f = Fixture::new();
    f.managed();
    let path = f.0.join(".aperture/run/owner/t1-worker.json");
    let mut owner: OwnerRecord = read_private_json(&path).unwrap();
    owner.generation = 0;
    owner.state = OwnerState::Stale;
    owner.incarnation = None;
    write_private_json_atomic(&path, &owner, true).unwrap();
    let actor = AuthenticatedActor::launcher();
    assert!(matches!(
        RuntimeAttempt::begin(&f.0, &actor, "t1", "t1-worker", 0, Deadline::new()),
        Err(ReplacementError::GenerationMismatch)
    ));
    let a =
        RuntimeAttempt::begin_bootstrap(&f.0, &actor, "t1", "t1-worker", Deadline::new()).unwrap();
    assert_eq!(a.admitted.old_generation, 0);
    assert!(a.dir.ends_with("t1-worker/g0"));
    assert!(matches!(
        RuntimeAttempt::begin_bootstrap(&f.0, &actor, "t1", "t1-worker", Deadline::new()),
        Err(ReplacementError::OutcomeUnknown)
    ));
    assert_eq!(read_private_json::<OwnerRecord>(&path).unwrap(), owner);
}
#[test]
fn bootstrap_rejects_every_nonvirgin_owner_before_admission() {
    for variant in 0..8 {
        let f = Fixture::new();
        f.managed();
        let path = f.0.join(".aperture/run/owner/t1-worker.json");
        let mut owner: OwnerRecord = read_private_json(&path).unwrap();
        owner.generation = 0;
        owner.state = OwnerState::Stale;
        owner.incarnation = None;
        match variant {
            0 => owner.generation = 1,
            1 => owner.state = OwnerState::Starting,
            2 => owner.state = OwnerState::Active,
            3 => owner.state = OwnerState::Quarantined,
            4 => owner.provisional_token_id = Some("a".repeat(64)),
            5 => owner.reservation_nonce_sha256 = Some("b".repeat(64)),
            6 => owner.schema_version = 99,
            _ => owner.requested.model = "other-model".into(),
        }
        write_private_json_atomic(&path, &owner, true).unwrap();
        assert!(RuntimeAttempt::begin_bootstrap(
            &f.0,
            &AuthenticatedActor::launcher(),
            "t1",
            "t1-worker",
            Deadline::new()
        )
        .is_err());
        assert!(!f.0.join(".aperture/teams/t1/runtime-attempts").exists());
    }
}
#[test]
fn bootstrap_missing_corrupt_owner_and_operator_direct_admission_fail_closed() {
    for corrupt in [false, true] {
        let f = Fixture::new();
        f.managed();
        let path = f.0.join(".aperture/run/owner/t1-worker.json");
        if corrupt {
            std::fs::write(&path, b"{}").unwrap();
        } else {
            std::fs::remove_file(&path).unwrap();
        }
        assert!(RuntimeAttempt::begin_bootstrap(
            &f.0,
            &AuthenticatedActor::launcher(),
            "t1",
            "t1-worker",
            Deadline::new()
        )
        .is_err());
        assert!(!f.0.join(".aperture/teams/t1/runtime-attempts").exists());
    }
    let f = Fixture::new();
    assert!(matches!(
        RuntimeAttempt::begin_bootstrap(
            &f.0,
            &AuthenticatedActor::operator_ui(),
            "t1",
            "t1-worker",
            Deadline::new()
        ),
        Err(ReplacementError::AuthorizationRequired)
    ));
    assert!(!f.0.join(".aperture").exists());
}

#[test]
fn human_ready_is_terminal_clock_free_and_start_gets_new_nonrenewable_budget() {
    let f = Fixture::new();
    f.managed();
    let actor = AuthenticatedActor::launcher();
    let mut a = RuntimeAttempt::begin(&f.0, &actor, "t1", "t1-worker", 1, Deadline::new()).unwrap();
    a.admit_effects().unwrap();
    let old_start = a.budget.started;
    let ready = a.finish_ready(&proof()).unwrap();
    let old_dir = ready.dir.clone();
    let terminal: Fact = read_private_json(&old_dir.join("terminal.json")).unwrap();
    assert_eq!(terminal.kind, FactKind::Ready);
    let new_budget = Deadline::new();
    let next_start = new_budget.started;
    assert!(next_start > old_start);
    let mut start = ready.start(new_budget).unwrap();
    assert_eq!(start.budget.started, next_start);
    assert!(start.dir.ends_with("g1/start"));
    assert!(!start.effects_admitted());
    start.finish_failed().unwrap();
    let old_terminal: Fact = read_private_json(&old_dir.join("terminal.json")).unwrap();
    assert_eq!(old_terminal, terminal);
}
#[test]
fn human_permit_corruption_owner_drift_or_consumed_path_is_expired_not_unknown() {
    for variant in 0..4 {
        let f = Fixture::new();
        f.managed();
        let actor = AuthenticatedActor::launcher();
        let mut a =
            RuntimeAttempt::begin(&f.0, &actor, "t1", "t1-worker", 1, Deadline::new()).unwrap();
        a.admit_effects().unwrap();
        let ready = a.finish_ready(&proof()).unwrap();
        match variant {
            0 => std::fs::write(ready.dir.join("terminal.json"), b"{}").unwrap(),
            1 => {
                let p = f.0.join(".aperture/run/owner/t1-worker.json");
                let mut o: OwnerRecord = read_private_json(&p).unwrap();
                o.generation = 2;
                write_private_json_atomic(&p, &o, true).unwrap();
            }
            2 => {
                ensure_private_dir(&ready.dir.join("start")).unwrap();
            }
            _ => {
                let p = f.0.join(".aperture/run/owner/t1-worker.json");
                let mut o: OwnerRecord = read_private_json(&p).unwrap();
                o.state = OwnerState::Quarantined;
                write_private_json_atomic(&p, &o, true).unwrap();
            }
        }
        assert!(matches!(
            ready.start(Deadline::new()),
            Err(ReplacementError::PreparationExpired)
        ));
    }
}

fn proof() -> crate::team_replacement::RevocationProof {
    crate::team_replacement::RevocationProof {
        generation: 1,
        durable: true,
        sockets_closed: true,
        close_code: 4001,
        close_elapsed_ms: 2,
        reconnect_code: 4003,
        reconnect_is_historical: false,
        token_deleted: true,
    }
}
#[test]
fn lost_human_permit_allows_fresh_ready_successor_not_reconstruction() {
    let f = Fixture::new();
    f.managed();
    let actor = AuthenticatedActor::launcher();
    let mut a = RuntimeAttempt::begin(&f.0, &actor, "t1", "t1-worker", 1, Deadline::new()).unwrap();
    a.admit_effects().unwrap();
    let ready = a.finish_ready(&proof()).unwrap();
    let old = std::fs::read(ready.dir.join("prepared.json")).unwrap();
    let next = RuntimeAttempt::begin(&f.0, &actor, "t1", "t1-worker", 1, Deadline::new()).unwrap();
    assert!(next.dir.ends_with("g1/reprepare"));
    assert!(next.prior_ready().is_some());
    assert!(!next.effects_admitted());
    assert_eq!(old, std::fs::read(ready.dir.join("prepared.json")).unwrap());
    assert!(matches!(
        ready.start(Deadline::new()),
        Err(ReplacementError::PreparationExpired)
    ));
    drop(next); // An incomplete successor is UNKNOWN, never retry admission.
    assert!(matches!(
        RuntimeAttempt::begin(&f.0, &actor, "t1", "t1-worker", 1, Deadline::new()),
        Err(ReplacementError::OutcomeUnknown)
    ));
}
#[test]
fn ready_reprepare_denies_corrupt_proof_changed_identity_and_started_effects() {
    for mode in 0..3 {
        let f = Fixture::new();
        f.managed();
        let actor = AuthenticatedActor::launcher();
        let mut a =
            RuntimeAttempt::begin(&f.0, &actor, "t1", "t1-worker", 1, Deadline::new()).unwrap();
        a.admit_effects().unwrap();
        let ready = a.finish_ready(&proof()).unwrap();
        match mode {
            0 => std::fs::write(ready.dir.join("prepared.json"), b"{}").unwrap(),
            1 => {
                let p = f.0.join(".aperture/run/owner/t1-worker.json");
                let mut o: OwnerRecord = read_private_json(&p).unwrap();
                o.incarnation.as_mut().unwrap().token_id = "b".repeat(64);
                write_private_json_atomic(&p, &o, true).unwrap();
            }
            _ => {
                let mut start = ready.start(Deadline::new()).unwrap();
                start.admit_effects().unwrap();
                let _ = start.finish_unknown();
            }
        }
        assert!(
            RuntimeAttempt::begin(&f.0, &actor, "t1", "t1-worker", 1, Deadline::new()).is_err()
        );
    }
}

#[test]
fn failed_start_without_effects_can_reprepare_and_preserves_all_old_facts() {
    let f = Fixture::new();
    f.managed();
    let actor = AuthenticatedActor::launcher();
    let mut a = RuntimeAttempt::begin(&f.0, &actor, "t1", "t1-worker", 1, Deadline::new()).unwrap();
    a.admit_effects().unwrap();
    let ready = a.finish_ready(&proof()).unwrap();
    let dir = ready.dir.clone();
    let prior = std::fs::read(dir.join("terminal.json")).unwrap();
    let mut start = ready.start(Deadline::new()).unwrap();
    start.finish_failed().unwrap();
    let mut next =
        RuntimeAttempt::begin(&f.0, &actor, "t1", "t1-worker", 1, Deadline::new()).unwrap();
    assert!(next.prior_ready().is_some());
    next.admit_effects().unwrap();
    let mut historical = proof();
    historical.reconnect_is_historical = true;
    let ready2 = next.finish_ready(&historical).unwrap();
    assert_eq!(std::fs::read(dir.join("terminal.json")).unwrap(), prior);
    let evidence: PriorReady = read_private_json(&ready2.dir.join("prepared.json")).unwrap();
    assert!(evidence.revocation.reconnect_is_historical);
    let third = RuntimeAttempt::begin(&f.0, &actor, "t1", "t1-worker", 1, Deadline::new()).unwrap();
    assert!(third.dir.ends_with("g1/reprepare/reprepare"));
}
