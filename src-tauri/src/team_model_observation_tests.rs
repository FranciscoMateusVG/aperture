use super::*;
use crate::journal::{ensure_private_dir, write_private_bytes_atomic, write_private_json_atomic};
use crate::owner::{Incarnation, ProcessIdentity as StoredProcess};
use crate::team_auth::AuthenticatedActor;
use serde_json::{json, Value};
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::PathBuf;
struct Fixture {
    home: PathBuf,
    store: OwnerStore,
    reservation: StartReservation,
}
impl Fixture {
    fn new() -> Self {
        let home = std::env::temp_dir().join(format!(
            "aperture-model-observation-{}",
            uuid::Uuid::new_v4()
        ));
        let dir = home.join(".aperture/teams/t1");
        ensure_private_dir(&dir).unwrap();
        write_private_json_atomic(&dir.join("team.json"),&json!({"schema_version":1,"team":"t1","project":"project:aperture","mission":"fixture","acceptance":"fixture","preset":{"id":null,"sha256":null},"lead":"t1-worker","seats":[{"name":"t1-worker","role":"backend","harness":"codex","model":"gpt-6-astra","reasoning":"high"}],"fallbacks":[],"grants":[],"created_at":"2026-09-20T00:00:00Z","creation_request_id":uuid::Uuid::new_v4().to_string(),"staging_uuid":uuid::Uuid::new_v4().to_string()}),false).unwrap();
        write_private_json_atomic(&dir.join("state.json"),&json!({"schema_version":1,"state":"active","generation":1,"epic_id":"aperture-fixture","failure":null,"updated_at":"2026-09-20T00:00:00Z"}),false).unwrap();
        let seat = home.join(".claude/aperture/t1-worker");
        ensure_private_dir(&seat).unwrap();
        for name in ["TEAM", ".complete"] {
            write_private_bytes_atomic(&seat.join(name), b"", false).unwrap();
        }
        let store = OwnerStore::new(home.join(".aperture/run/owner"));
        let actor = AuthenticatedActor::launcher();
        let tuple = ExecutionTuple {
            harness: Harness::Codex,
            model: "gpt-6-astra".into(),
            reasoning: Some(ReasoningEffort::High),
        };
        store
            .initialize_owner(&actor, "t1-worker", tuple.clone())
            .unwrap();
        let reservation = store.reserve_start(&actor, "t1-worker", 0, tuple).unwrap();
        let token = crate::hub_auth::managed::provision(&home, "t1", &actor, &reservation).unwrap();
        store
            .record_start_candidate(
                &actor,
                &reservation,
                Incarnation {
                    pid: 900001,
                    start_time: 1_790_000_000_000_001,
                    thread_id: String::new(),
                    token_id: token.token_id().into(),
                    harness: Harness::Codex,
                    model: "gpt-6-astra".into(),
                    reasoning: Some(ReasoningEffort::High),
                    observed: false,
                    processes: vec![StoredProcess {
                        pid: 900001,
                        start_time: 1_790_000_000_000_001,
                        ppid: 1,
                        pgid: 900001,
                        cmdline_sha256: "c".repeat(64),
                        cwd: "/fixture".into(),
                    }],
                },
            )
            .unwrap();
        let f = Self {
            home,
            store,
            reservation,
        };
        let candidate = f.owner().incarnation.unwrap();
        f.write("start-attempt",json!({"schema_version":1,"seat":"t1-worker","generation":1,"token_id":candidate.token_id,"root_pid":candidate.pid,"root_start_time_us":candidate.start_time,"requested_model":"gpt-6-astra","requested_reasoning":"high"}));
        f.write("observation",json!({"schema_version":1,"seat":"t1-worker","generation":1,"token_id":candidate.token_id,"root_pid":candidate.pid,"root_start_time_us":candidate.start_time,"thread_id":"native-thread","actual_model":"gpt-6-astra","actual_reasoning":"high","observed_at_ms":chrono::Utc::now().timestamp_millis()}));
        f
    }
    fn owner(&self) -> OwnerRecord {
        self.store.read_owner("t1-worker").unwrap()
    }
    fn path(&self, kind: &str) -> PathBuf {
        self.home
            .join(".aperture/run")
            .join(format!("t1-worker.g1.managed-{kind}.json"))
    }
    fn write(&self, kind: &str, v: Value) {
        write_private_json_atomic(&self.path(kind), &v, true).unwrap();
    }
    fn alter(&self, kind: &str, key: &str, v: Value) {
        let mut value: Value = read_private_json(&self.path(kind)).unwrap();
        value[key] = v;
        self.write(kind, value);
    }
    fn read(&self) -> Result<VerifiedObservation, ObservationError> {
        read_checked(
            &self.home,
            "t1",
            &self.reservation,
            |_| ProcessState::Same,
            || chrono::Utc::now().timestamp_millis(),
            || {},
        )
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.home).unwrap();
    }
}
#[test]
fn exact_native_receipt_is_read_only_then_shared_owner_cas_binds_actual() {
    let f = Fixture::new();
    let before = f.owner();
    let observation = f.read().unwrap().into_runtime_observation();
    assert_eq!(f.owner(), before);
    assert_eq!(observation.thread_id, "native-thread");
    assert_eq!(observation.actual, before.requested);
    let stored = f
        .store
        .record_runtime_observation(&AuthenticatedActor::launcher(), &f.reservation, observation)
        .unwrap();
    assert_eq!(
        stored.incarnation.as_ref().unwrap().processes,
        before.incarnation.unwrap().processes
    );
    assert_eq!(
        f.store
            .commit_start(&AuthenticatedActor::launcher(), &f.reservation)
            .unwrap()
            .state,
        OwnerState::Active
    );
    assert!(f.read().is_err());
}
#[test]
fn mismatched_receipt_fields_never_return_observation_or_write_owner() {
    for (key, v) in [
        ("schema_version", json!(2)),
        ("seat", json!("other")),
        ("generation", json!(2)),
        ("token_id", json!("f".repeat(64))),
        ("root_pid", json!(900002)),
        ("root_start_time_us", json!(1)),
        ("actual_model", json!("gpt-wrong")),
        ("actual_reasoning", json!("low")),
        ("thread_id", json!("../bad")),
        ("unexpected", json!(true)),
    ] {
        let f = Fixture::new();
        let before = f.owner();
        f.alter("observation", key, v);
        assert!(f.read().is_err(), "{key}");
        assert_eq!(f.owner(), before);
    }
}
#[test]
fn missing_mismatched_or_corrupt_attempt_blocks_even_valid_receipt() {
    let f = Fixture::new();
    std::fs::remove_file(f.path("start-attempt")).unwrap();
    assert!(matches!(f.read(), Err(ObservationError::Missing)));
    let f = Fixture::new();
    f.alter("start-attempt", "requested_model", json!("wrong"));
    assert!(f.read().is_err());
    let f = Fixture::new();
    write_private_bytes_atomic(&f.path("start-attempt"), b"{", true).unwrap();
    assert!(f.read().is_err());
}
#[test]
fn receipt_clock_and_byte_caps_fail_closed() {
    for t in [0, 1, i64::MAX] {
        let f = Fixture::new();
        f.alter("observation", "observed_at_ms", json!(t));
        assert!(f.read().is_err());
    }
    let f = Fixture::new();
    write_private_bytes_atomic(&f.path("observation"), &vec![b' '; 8193], true).unwrap();
    assert!(matches!(f.read(), Err(ObservationError::Invalid)));
}
#[test]
fn private_receipt_leaf_symlink_hardlink_permissions_fail_closed() {
    let f = Fixture::new();
    std::fs::remove_file(f.path("observation")).unwrap();
    symlink(f.path("start-attempt"), f.path("observation")).unwrap();
    assert!(matches!(f.read(), Err(ObservationError::Unsafe)));
    let f = Fixture::new();
    std::fs::hard_link(f.path("observation"), f.home.join("extra")).unwrap();
    assert!(matches!(f.read(), Err(ObservationError::Unsafe)));
    let f = Fixture::new();
    std::fs::set_permissions(
        f.path("observation"),
        std::fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    assert!(matches!(f.read(), Err(ObservationError::Unsafe)));
}
#[test]
fn native_process_must_match_both_observations() {
    for result in [
        ProcessState::Gone,
        ProcessState::Recycled,
        ProcessState::Unreadable,
    ] {
        let f = Fixture::new();
        let calls = std::cell::Cell::new(0);
        assert!(matches!(
            read_checked(
                &f.home,
                "t1",
                &f.reservation,
                |_| {
                    let n = calls.get();
                    calls.set(n + 1);
                    if n == 0 {
                        ProcessState::Same
                    } else {
                        result
                    }
                },
                || chrono::Utc::now().timestamp_millis(),
                || {}
            ),
            Err(ObservationError::Process)
        ));
    }
}
#[test]
fn owner_or_token_change_during_read_blocks_and_preserves_evidence() {
    let f = Fixture::new();
    assert!(matches!(
        read_checked(
            &f.home,
            "t1",
            &f.reservation,
            |_| ProcessState::Same,
            || chrono::Utc::now().timestamp_millis(),
            || {
                let mut owner: OwnerRecord =
                    read_private_json(&f.store.record_path("t1-worker")).unwrap();
                owner.generation += 1;
                write_private_json_atomic(&f.store.record_path("t1-worker"), &owner, true).unwrap();
            }
        ),
        Err(ObservationError::Owner)
    ));
    let f = Fixture::new();
    assert!(read_checked(
        &f.home,
        "t1",
        &f.reservation,
        |_| ProcessState::Same,
        || chrono::Utc::now().timestamp_millis(),
        || {
            std::fs::remove_file(f.home.join(".aperture/run/hub-tokens/t1-worker.token")).unwrap();
        }
    )
    .is_err());
}
#[test]
fn revoked_identity_or_corrupt_or_symlink_store_cannot_be_observed() {
    let f = Fixture::new();
    let root = f.home.join(".aperture/run/revocations");
    ensure_private_dir(&root).unwrap();
    write_private_json_atomic(&root.join("t1-worker.json"),&json!({"schema_version":1,"seat":"t1-worker","revoked_through_generation":1,"revoked_token_ids":[f.owner().incarnation.unwrap().token_id]}),false).unwrap();
    assert!(matches!(f.read(), Err(ObservationError::Revoked)));
    let f = Fixture::new();
    let root = f.home.join(".aperture/run/revocations");
    ensure_private_dir(&root).unwrap();
    write_private_bytes_atomic(&root.join("t1-worker.json"), b"{", false).unwrap();
    assert!(f.read().is_err());
    let f = Fixture::new();
    symlink(
        f.home.join("missing"),
        f.home.join(".aperture/run/revocations"),
    )
    .unwrap();
    assert!(f.read().is_err());
}
