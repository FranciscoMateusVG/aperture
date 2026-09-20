//! Private filesystem fixtures: no OS process collection, spawn or signal.
use super::*;
use crate::journal::{
    ensure_private_dir, read_private_json, write_private_bytes_atomic, write_private_json_atomic,
};
use crate::owner::{Incarnation, OwnerRecord, OwnerStore};
use crate::state::{ExecutionTuple, Harness, ReasoningEffort};
use crate::team_auth::AuthenticatedActor;
use std::path::PathBuf;

const SEAT: &str = "t1-backend";
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let home =
            std::env::temp_dir().join(format!("aperture-k310b-process-{}", uuid::Uuid::new_v4()));
        let team = home.join(".aperture/teams/t1");
        ensure_private_dir(&team).unwrap();
        write_private_json_atomic(&team.join("team.json"), &serde_json::json!({
            "schema_version":1,"team":"t1","project":"project:aperture",
            "mission":"Fixture","acceptance":"Fixture","preset":{"id":null,"sha256":null},
            "lead":SEAT,"seats":[{"name":SEAT,"role":"backend","harness":"codex","model":"gpt-6-astra","reasoning":"high"}],
            "fallbacks":[],"grants":[],"created_at":"2026-09-20T00:00:00Z",
            "creation_request_id":uuid::Uuid::new_v4().to_string(),"staging_uuid":uuid::Uuid::new_v4().to_string()
        }), false).unwrap();
        write_private_json_atomic(
            &team.join("state.json"),
            &serde_json::json!({
                "schema_version":1,"state":"active","generation":1,"epic_id":"aperture-fixture",
                "failure":null,"updated_at":"2026-09-20T00:00:00Z"
            }),
            false,
        )
        .unwrap();
        let seat = home.join(".claude/aperture").join(SEAT);
        ensure_private_dir(&seat).unwrap();
        for name in ["TEAM", ".complete"] {
            write_private_bytes_atomic(&seat.join(name), b"", false).unwrap();
        }
        let owners = OwnerStore::new(home.join(".aperture/run/owner"));
        let actor = AuthenticatedActor::launcher();
        let tuple = ExecutionTuple {
            harness: Harness::Codex,
            model: "gpt-6-astra".into(),
            reasoning: Some(ReasoningEffort::High),
        };
        owners
            .initialize_owner(&actor, SEAT, tuple.clone())
            .unwrap();
        let reservation = owners.reserve_start(&actor, SEAT, 0, tuple.clone()).unwrap();
        let token_id = "a".repeat(64);
        owners
            .bind_and_publish_token(&actor, &reservation, token_id.clone(), || Ok(()))
            .unwrap();
        owners
            .record_start_candidate(
                &actor,
                &reservation,
                Incarnation {
                    pid: 900001,
                    start_time: 1_000_001,
                    thread_id: String::new(),
                    token_id: token_id.clone(),
                    harness: Harness::Codex,
                    model: "gpt-6-astra".into(),
                    reasoning: Some(ReasoningEffort::High),
                    observed: false,
                    processes: vec![
                        owner_process(900001, 1_000_001, 1),
                        owner_process(900002, 1_000_002, 900001),
                    ],
                },
            )
            .unwrap();
        owners
            .record_runtime_observation(
                &actor,
                &reservation,
                crate::owner::RuntimeObservation {
                    pid: 900001,
                    start_time: 1_000_001,
                    token_id,
                    thread_id: "fixture-thread".into(),
                    actual: tuple,
                },
            )
            .unwrap();
        owners.commit_start(&actor, &reservation).unwrap();
        Self(home)
    }
    fn store(&self) -> OwnerStore {
        OwnerStore::new(self.0.join(".aperture/run/owner"))
    }
    fn owner(&self) -> OwnerRecord {
        read_private_json(&self.store().record_path(SEAT)).unwrap()
    }
    fn bytes(&self) -> Vec<u8> {
        std::fs::read(self.store().record_path(SEAT)).unwrap()
    }
    fn snapshot(&self) -> OwnershipSnapshot {
        let owner = self.owner();
        let inc = owner.incarnation.unwrap();
        OwnershipSnapshot {
            seat: SEAT.into(),
            generation: owner.generation,
            thread_id: inc.thread_id,
            processes: inc
                .processes
                .iter()
                .map(|p| OwnedProcess {
                    identity: identity_from_owner(p.pid, p.start_time).unwrap(),
                    parent_pid: p.ppid,
                    process_group: p.pgid,
                    depth: if p.pid == inc.pid { 0 } else { 1 },
                    cmdline_sha256: p.cmdline_sha256.clone(),
                    cwd: p.cwd.clone(),
                })
                .collect(),
            complete: true,
            unowned_matches: vec![],
        }
    }
    fn unchanged_failure(
        &self,
        snapshot: OwnershipSnapshot,
        state: ProcessState,
    ) -> ReplacementError {
        let before = self.bytes();
        let result = persist_for_stop_checked(
            &self.0,
            "t1",
            &AuthenticatedActor::launcher(),
            snapshot,
            |_| state,
        );
        let error = match result {
            Err(e) => e,
            Ok(_) => panic!("unexpected stop authority"),
        };
        assert_eq!(
            self.bytes(),
            before,
            "failed guard must not change owner bytes"
        );
        error
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
fn owner_process(pid: u32, birth: u64, ppid: u32) -> crate::owner::ProcessIdentity {
    crate::owner::ProcessIdentity {
        pid,
        start_time: birth,
        ppid,
        pgid: 900001,
        cmdline_sha256: "a".repeat(64),
        cwd: "/fixture".into(),
    }
}

#[test]
fn persisted_stop_guard_keeps_orphan_identity_and_both_locks_until_drop() {
    let f = Fixture::new();
    let before = f.owner();
    let mut snapshot = f.snapshot();
    snapshot.processes[1].parent_pid = 1; // reparented, not missing or unowned
    let authority =
        persist_for_stop_checked(&f.0, "t1", &AuthenticatedActor::launcher(), snapshot, |p| {
            if p.pid == 900001 {
                ProcessState::Gone
            } else {
                ProcessState::Same
            }
        })
        .unwrap();
    assert_eq!(authority.snapshot().processes.len(), 2);
    let after = f.owner();
    let inc = after.incarnation.as_ref().unwrap();
    assert_eq!(inc.processes.len(), 2);
    assert_eq!(inc.processes[1].pid, 900002);
    assert_eq!(inc.processes[1].start_time, 1_000_002);
    assert_eq!(
        inc.thread_id,
        before.incarnation.as_ref().unwrap().thread_id
    );
    assert_eq!(inc.token_id, before.incarnation.as_ref().unwrap().token_id);
    assert_eq!(after.generation, before.generation);
    assert!(f.store().lock(SEAT).is_err());
    assert!(crate::owner::try_lock(&f.0.join(".aperture/run/team-locks"), "t1").is_err());
    drop(authority);
    assert!(f.store().lock(SEAT).is_ok());
    assert!(crate::owner::try_lock(&f.0.join(".aperture/run/team-locks"), "t1").is_ok());
}

#[test]
fn missing_root_or_persisted_orphan_never_becomes_signal_authority() {
    for missing in [0, 1] {
        let f = Fixture::new();
        let mut s = f.snapshot();
        s.processes.remove(missing);
        assert_eq!(
            f.unchanged_failure(s, ProcessState::Same),
            ReplacementError::InvalidSnapshot
        );
    }
}
#[test]
fn recycled_or_unreadable_identity_preserves_preimage() {
    for state in [ProcessState::Recycled, ProcessState::Unreadable] {
        let f = Fixture::new();
        assert_eq!(
            f.unchanged_failure(f.snapshot(), state),
            ReplacementError::StopUnverified
        );
    }
}
#[test]
fn duplicate_pid_changed_birth_and_noncanonical_birth_fail_closed() {
    for variant in 0..3 {
        let f = Fixture::new();
        let mut s = f.snapshot();
        match variant {
            0 => s.processes.push(s.processes[1].clone()),
            1 => s.processes[1].identity.start_time = "1.000003".into(),
            _ => s.processes[1].identity.start_time = "01.000002".into(),
        }
        assert_eq!(
            f.unchanged_failure(s, ProcessState::Same),
            ReplacementError::InvalidSnapshot
        );
    }
}
#[test]
fn incomplete_unowned_wrong_thread_and_stale_generation_preserve_owner() {
    for variant in 0..4 {
        let f = Fixture::new();
        let mut s = f.snapshot();
        let expected = match variant {
            0 => {
                s.complete = false;
                ReplacementError::InvalidSnapshot
            }
            1 => {
                s.unowned_matches
                    .push(identity_from_owner(900003, 1_000_003).unwrap());
                ReplacementError::UnownedProcess
            }
            2 => {
                s.thread_id = "other-thread".into();
                ReplacementError::InvalidSnapshot
            }
            _ => {
                s.generation = 2;
                ReplacementError::GenerationMismatch
            }
        };
        assert_eq!(f.unchanged_failure(s, ProcessState::Same), expected);
    }
}
#[test]
fn nonlauncher_and_wrong_team_cannot_observe_or_mutate() {
    for wrong_team in [false, true] {
        let f = Fixture::new();
        let before = f.bytes();
        let mut observations = 0;
        let actor = if wrong_team {
            AuthenticatedActor::launcher()
        } else {
            AuthenticatedActor::operator_ui()
        };
        let result = persist_for_stop_checked(
            &f.0,
            if wrong_team { "t2" } else { "t1" },
            &actor,
            f.snapshot(),
            |_| {
                observations += 1;
                ProcessState::Same
            },
        );
        assert!(result.is_err());
        assert_eq!(observations, 0);
        assert_eq!(f.bytes(), before);
    }
}
#[test]
fn concurrent_owner_transition_before_cas_yields_no_authority() {
    let f = Fixture::new();
    let mut observed = 0;
    let result = persist_for_stop_checked(
        &f.0,
        "t1",
        &AuthenticatedActor::launcher(),
        f.snapshot(),
        |_| {
            observed += 1;
            if observed == 1 {
                f.store()
                    .mark_stale(&AuthenticatedActor::launcher(), SEAT, 1)
                    .unwrap();
            }
            ProcessState::Same
        },
    );
    assert!(matches!(result, Err(ReplacementError::StopUnverified)));
    assert_eq!(f.owner().state, crate::state::OwnerState::Stale);
}
#[test]
fn corrupt_or_missing_owner_has_no_reset_or_signal_authority() {
    for missing in [true, false] {
        let f = Fixture::new();
        let s = f.snapshot();
        let path = f.store().record_path(SEAT);
        if missing {
            std::fs::remove_file(&path).unwrap();
        } else {
            write_private_bytes_atomic(&path, b"invalid fixture", true).unwrap();
        }
        let result =
            persist_for_stop_checked(&f.0, "t1", &AuthenticatedActor::launcher(), s, |_| {
                panic!("must not observe")
            });
        assert!(result.is_err());
        if missing {
            assert!(!path.exists());
        } else {
            assert_eq!(std::fs::read(&path).unwrap(), b"invalid fixture");
        }
    }
}
