//! Synthetic private fixture homes only. No actual bearer, process or provider.
use super::*;
use crate::journal::write_private_bytes_atomic;
use crate::owner::Incarnation;
use crate::state::{ExecutionTuple, Harness, ReasoningEffort};
use crate::team_checkpoint::{
    CheckpointContext, CheckpointPayload, CheckpointWriter, RemoteEffectRef,
};
use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
const TARGET: &str = "t1-worker";
const LEAD: &str = "t1-lead";
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let home =
            std::env::temp_dir().join(format!("aperture-remote-fixture-{}", uuid::Uuid::new_v4()));
        let dir = home.join(".aperture/teams/t1");
        ensure_private_dir(&dir).unwrap();
        write_private_json_atomic(&dir.join("team.json"),&serde_json::json!({
            "schema_version":1,"team":"t1","project":"project:aperture","repo":"aperture","mission":"fixture","acceptance":"fixture",
            "preset":{"id":null,"sha256":null},"lead":LEAD,
            "seats":[{"name":TARGET,"role":"backend","harness":"codex","model":"gpt-6-astra","reasoning":"high"},{"name":LEAD,"role":"lead","harness":"codex","model":"gpt-6-astra","reasoning":"high"}],
            "fallbacks":[],"grants":[],"created_at":"2026-09-20T00:00:00Z","creation_request_id":uuid::Uuid::new_v4().to_string(),"staging_uuid":uuid::Uuid::new_v4().to_string()
        }),false).unwrap();
        write_private_json_atomic(&dir.join("state.json"),&serde_json::json!({"schema_version":1,"state":"active","generation":1,"epic_id":"aperture-fixture","failure":null,"updated_at":"2026-09-20T00:00:00Z"}),false).unwrap();
        for seat in [TARGET, LEAD] {
            let runtime = home.join(".claude/aperture").join(seat);
            ensure_private_dir(&runtime).unwrap();
            for name in ["TEAM", ".complete"] {
                write_private_bytes_atomic(&runtime.join(name), b"", false).unwrap();
            }
            let store = OwnerStore::new(home.join(".aperture/run/owner"));
            let actor = AuthenticatedActor::launcher();
            let tuple = ExecutionTuple {
                harness: Harness::Codex,
                model: "gpt-6-astra".into(),
                reasoning: Some(ReasoningEffort::High),
            };
            store.initialize_owner(&actor, seat, tuple.clone()).unwrap();
            let reservation = store.reserve_start(&actor, seat, 0, tuple.clone()).unwrap();
            let token_id = if seat == TARGET { "a".repeat(64) } else { "b".repeat(64) };
            store
                .bind_and_publish_token(&actor, &reservation, token_id.clone(), || Ok(()))
                .unwrap();
            store
                .record_start_candidate(
                    &actor,
                    &reservation,
                    Incarnation {
                        pid: 900001,
                        start_time: 42,
                        thread_id: String::new(),
                        token_id: token_id.clone(),
                        harness: tuple.harness.clone(),
                        model: tuple.model.clone(),
                        reasoning: tuple.reasoning.clone(),
                        observed: false,
                        processes: vec![crate::owner::ProcessIdentity {
                            pid: 900001,
                            start_time: 42,
                            ppid: 1,
                            pgid: 900001,
                            cmdline_sha256: "a".repeat(64),
                            cwd: "/fixture".into(),
                        }],
                    },
                )
                .unwrap();
            store
                .record_runtime_observation(
                    &actor,
                    &reservation,
                    crate::owner::RuntimeObservation {
                        pid: 900001,
                        start_time: 42,
                        token_id,
                        thread_id: "synthetic-thread".into(),
                        actual: tuple,
                    },
                )
                .unwrap();
            store.commit_start(&actor, &reservation).unwrap();
        }
        Self(home)
    }
    fn target(&self) -> RemoteTarget {
        RemoteTarget {
            team: "t1".into(),
            seat: TARGET.into(),
            expected_generation: 1,
        }
    }
    fn dir(&self) -> PathBuf {
        self.0.join(".aperture/teams/t1/checkpoints").join(TARGET)
    }
    fn facts(&self) -> PathBuf {
        self.dir().join(".remote-resolution")
    }
    fn view(&self) -> RemoteInventoryView {
        inspect_native(&self.0, &self.target(), &[]).unwrap()
    }
    fn request(&self, reference: Option<&str>) -> ResolutionRequest {
        ResolutionRequest {
            expected_inventory_hash: self.view().inventory_hash,
            scope: if reference.is_some() {
                ResolutionScope::EffectResolution
            } else {
                ResolutionScope::InventoryRiskAcceptance
            },
            reference: reference.map(str::to_string),
            decision: if reference.is_some() {
                ResolutionDecision::Finished
            } else {
                ResolutionDecision::ProceedWithUnobservedEffects
            },
            evidence_ref: "beads:aperture-fixture/decision-1".into(),
        }
    }
    fn resolve(&self, r: &ResolutionRequest) -> Result<ResolutionReceipt, RemoteError> {
        resolve_native(
            &self.0,
            ResolutionAuthority::Operator(&AuthenticatedActor::operator_ui()),
            &self.target(),
            r,
            &[],
        )
    }
    fn checkpoint(&self, seq: u64, refs: &[(&str, &str)]) {
        let ctx = CheckpointContext {
            team: "t1".into(),
            seat: TARGET.into(),
            generation: 1,
            authenticated_generation: 1,
            harness: "codex".into(),
            writer: CheckpointWriter::Explicit,
        };
        let payload = CheckpointPayload {
            task_id: "aperture-fixture".into(),
            worktree: "aperture-worktrees/fixture".into(),
            branch: "aperture-fixture".into(),
            head_sha: "a".repeat(40),
            dirty_files: vec![],
            open_pr: None,
            running_procs: vec![],
            decisions: vec![],
            next_step: format!("Continue fixture {seq}"),
            remote_effects: refs
                .iter()
                .map(|(r, s)| RemoteEffectRef {
                    kind: "shell".into(),
                    reference: (*r).into(),
                    state: (*s).into(),
                })
                .collect(),
        };
        crate::team_checkpoint::native::write_native(
            &self.0,
            &ctx,
            1,
            payload,
            seq * 6000,
            &[],
            || Ok(()),
        )
        .unwrap();
    }
    fn change_owner(&self, seat: &str, change: impl FnOnce(&mut OwnerRecord)) {
        let path = self
            .0
            .join(".aperture/run/owner")
            .join(format!("{seat}.json"));
        let mut row: OwnerRecord = read_private_json(&path).unwrap();
        change(&mut row);
        write_private_json_atomic(&path, &row, true).unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
#[test]
fn empty_history_requires_operator_decision_never_observation() {
    let f = Fixture::new();
    let view = f.view();
    assert!(!view.complete_observation);
    assert!(view.effects.is_empty());
    assert!(!f.dir().exists());
    assert!(!project_native(&f.0, &f.target(), &[]).unwrap().may_proceed);
    let r = f.resolve(&f.request(None)).unwrap();
    assert_eq!(r.source, "authorized_decision");
    assert!(!r.complete_observation);
    let p = project_native(&f.0, &f.target(), &[]).unwrap();
    assert!(p.may_proceed);
    assert!(!p.inventory.complete_observation);
    let core = p.into_core();
    assert!(!core.complete_observation);
    assert!(core.explicit_resolution.unwrap().permits(1, &[], false));
    let m = std::fs::metadata(f.facts().join("1.json")).unwrap();
    assert_eq!(m.mode() & 0o777, 0o600);
    assert_eq!(m.nlink(), 1);
    assert_eq!(std::fs::metadata(f.facts()).unwrap().mode() & 0o777, 0o700);
}
#[test]
fn checkpoint_finished_is_unknown_and_dropped_refs_remain_sorted() {
    let f = Fixture::new();
    f.checkpoint(1, &[("job-z", "finished"), ("job-a", "cancelled")]);
    let first = f.view();
    f.checkpoint(2, &[("job-a", "finished")]);
    let second = f.view();
    assert_eq!(first, second);
    assert_eq!(
        second
            .effects
            .iter()
            .map(|e| e.reference.as_str())
            .collect::<Vec<_>>(),
        vec!["job-a", "job-z"]
    );
    assert!(second
        .effects
        .iter()
        .all(|e| e.observed_state == ObservedState::Unknown));
    assert!(!second.complete_observation);
}
#[test]
fn both_inventory_and_each_named_reference_are_required() {
    let f = Fixture::new();
    f.checkpoint(1, &[("job-a", "unknown"), ("job-b", "finished")]);
    f.resolve(&f.request(Some("job-a"))).unwrap();
    f.resolve(&f.request(Some("job-b"))).unwrap();
    assert!(!project_native(&f.0, &f.target(), &[]).unwrap().may_proceed);
    f.resolve(&f.request(None)).unwrap();
    let p = project_native(&f.0, &f.target(), &[]).unwrap();
    assert!(p.may_proceed);
    assert!(!p.inventory.complete_observation);
    let core = p.clone().into_core();
    assert!(core
        .effects
        .iter()
        .all(|e| e.resolution == super::super::RemoteResolution::Unknown));
    assert!(p.permits(1, &core.effects, false));
    assert!(!p.permits(2, &core.effects, false));
    assert!(!p.permits(1, &core.effects, true));
    let f = Fixture::new();
    f.checkpoint(1, &[("job-a", "unknown")]);
    f.resolve(&f.request(None)).unwrap();
    assert!(!project_native(&f.0, &f.target(), &[]).unwrap().may_proceed);
}
#[test]
fn inventory_change_stales_facts_and_cas_without_rewriting_history() {
    let f = Fixture::new();
    let request = f.request(None);
    f.resolve(&request).unwrap();
    let bytes = std::fs::read(f.facts().join("1.json")).unwrap();
    f.checkpoint(1, &[("job-new", "unknown")]);
    assert_eq!(f.resolve(&request), Err(RemoteError::StaleInventory));
    assert!(!project_native(&f.0, &f.target(), &[]).unwrap().may_proceed);
    assert_eq!(bytes, std::fs::read(f.facts().join("1.json")).unwrap());
    f.change_owner(TARGET, |o| o.generation = 2);
    assert!(matches!(
        inspect_native(&f.0, &f.target(), &[]),
        Err(RemoteError::Generation)
    ));
    let t = RemoteTarget {
        expected_generation: 2,
        ..f.target()
    };
    assert!(!project_native(&f.0, &t, &[]).unwrap().may_proceed);
}
#[test]
fn lead_may_resolve_known_other_ref_but_not_accept_inventory_or_self() {
    let f = Fixture::new();
    f.checkpoint(1, &[("job-a", "unknown")]);
    let who = Principal::Lead {
        seat: LEAD.into(),
        generation: 1,
    };
    resolve_checked(
        &f.0,
        &who,
        &f.target(),
        &f.request(Some("job-a")),
        &[],
        || Ok(()),
    )
    .unwrap();
    assert_eq!(
        resolve_checked(&f.0, &who, &f.target(), &f.request(None), &[], || Ok(())),
        Err(RemoteError::Authority)
    );
    let self_lead = Principal::Lead {
        seat: TARGET.into(),
        generation: 1,
    };
    assert_eq!(
        resolve_checked(
            &f.0,
            &self_lead,
            &f.target(),
            &f.request(Some("job-a")),
            &[],
            || Ok(())
        ),
        Err(RemoteError::Authority)
    );
    let wrong = Principal::Lead {
        seat: "t2-lead".into(),
        generation: 1,
    };
    assert_eq!(
        resolve_checked(
            &f.0,
            &wrong,
            &f.target(),
            &f.request(Some("job-a")),
            &[],
            || Ok(())
        ),
        Err(RemoteError::Authority)
    );
    let t = RemoteTarget {
        seat: LEAD.into(),
        ..f.target()
    };
    let r = ResolutionRequest {
        expected_inventory_hash: inspect_native(&f.0, &t, &[]).unwrap().inventory_hash,
        ..f.request(None)
    };
    resolve_native(
        &f.0,
        ResolutionAuthority::Operator(&AuthenticatedActor::operator_ui()),
        &t,
        &r,
        &[],
    )
    .unwrap();
}
#[test]
fn absent_reference_invalid_evidence_and_forged_dto_grant_nothing() {
    let f = Fixture::new();
    assert_eq!(
        f.resolve(&f.request(Some("unknown-ref"))),
        Err(RemoteError::Invalid)
    );
    for bad in [
        "",
        "https://host/secret",
        "token=value",
        "../escape",
        "beads:\nraw",
        "sk_live_fixture",
        "evidence\u{202e}",
    ] {
        let mut r = f.request(None);
        r.evidence_ref = bad.into();
        assert_eq!(f.resolve(&r), Err(RemoteError::Invalid));
    }
    let r = f.request(None);
    for field in [
        "actor",
        "team",
        "seat",
        "generation",
        "authenticated",
        "inventory",
        "complete_observation",
    ] {
        let mut value = serde_json::to_value(&r).unwrap();
        value[field] = serde_json::json!(true);
        assert!(serde_json::from_value::<ResolutionRequest>(value).is_err());
    }
    assert_eq!(
        resolve_native(
            &f.0,
            ResolutionAuthority::Operator(&AuthenticatedActor::launcher()),
            &f.target(),
            &r,
            &[]
        ),
        Err(RemoteError::Authority)
    );
    assert_eq!(
        resolve_native(
            &f.0,
            ResolutionAuthority::Operator(&AuthenticatedActor::operator_ui()),
            &f.target(),
            &r,
            &["aperture-fixture".into()]
        ),
        Err(RemoteError::Invalid)
    );
    assert!(!f.facts().exists());
}
#[test]
fn exact_replay_is_idempotent_conflicting_decisions_never_latest_wins() {
    let f = Fixture::new();
    f.checkpoint(1, &[("job-a", "unknown")]);
    let r = f.request(Some("job-a"));
    let first = f.resolve(&r).unwrap();
    let replay = f.resolve(&r).unwrap();
    assert!(!first.replay);
    assert!(replay.replay);
    assert_eq!(first.fact_id, replay.fact_id);
    let mut conflict = r.clone();
    conflict.decision = ResolutionDecision::Cancelled;
    assert_eq!(f.resolve(&conflict), Err(RemoteError::Conflict));
    conflict = r.clone();
    conflict.evidence_ref = "beads:different".into();
    assert_eq!(f.resolve(&conflict), Err(RemoteError::Conflict));
    assert_eq!(
        resolve_checked(
            &f.0,
            &Principal::Lead {
                seat: LEAD.into(),
                generation: 1
            },
            &f.target(),
            &r,
            &[],
            || Ok(())
        ),
        Err(RemoteError::Conflict)
    );
    assert_eq!(std::fs::read_dir(f.facts()).unwrap().count(), 1);
}
#[test]
fn revoked_or_changed_resolver_between_locks_prevents_fact() {
    for fail_at in [1, 2, 3, 4] {
        let f = Fixture::new();
        f.checkpoint(1, &[("job-a", "unknown")]);
        let mut calls = 0;
        let result = resolve_checked(
            &f.0,
            &Principal::Lead {
                seat: LEAD.into(),
                generation: 1,
            },
            &f.target(),
            &f.request(Some("job-a")),
            &[],
            || {
                calls += 1;
                if calls == fail_at {
                    Err(RemoteError::Authority)
                } else {
                    Ok(())
                }
            },
        );
        assert_eq!(result, Err(RemoteError::Authority));
        assert!(!f.facts().join("1.json").exists());
    }
    let f = Fixture::new();
    f.checkpoint(1, &[("job-a", "unknown")]);
    let mut calls = 0;
    let result = resolve_checked(
        &f.0,
        &Principal::Lead {
            seat: LEAD.into(),
            generation: 1,
        },
        &f.target(),
        &f.request(Some("job-a")),
        &[],
        || {
            calls += 1;
            if calls == 3 {
                f.change_owner(LEAD, |o| o.generation = 2);
            }
            Ok(())
        },
    );
    assert_eq!(result, Err(RemoteError::Generation));
    assert!(!f.facts().exists());
}
#[test]
fn malformed_conflicting_or_symlink_facts_fail_closed() {
    let f = Fixture::new();
    f.resolve(&f.request(None)).unwrap();
    let path = f.facts().join("1.json");
    let bytes = std::fs::read(&path).unwrap();
    write_private_bytes_atomic(&path, b"{}", true).unwrap();
    assert!(matches!(
        project_native(&f.0, &f.target(), &[]),
        Err(RemoteError::Corrupt)
    ));
    write_private_bytes_atomic(&path, &bytes, true).unwrap();
    let mut fact: RemoteResolutionAuthorizationFact = read_private_json(&path).unwrap();
    fact.fact_seq = 2;
    write_private_json_atomic(&f.facts().join("2.json"), &fact, false).unwrap();
    assert!(matches!(
        project_native(&f.0, &f.target(), &[]),
        Err(RemoteError::Conflict)
    ));
    std::fs::remove_file(f.facts().join("2.json")).unwrap();
    std::fs::remove_file(&path).unwrap();
    symlink("missing", &path).unwrap();
    assert!(project_native(&f.0, &f.target(), &[]).is_err());
}
#[test]
fn native_fact_directory_privacy_and_checkpoint_integrity_are_required() {
    let f = Fixture::new();
    f.resolve(&f.request(None)).unwrap();
    std::fs::set_permissions(f.facts(), std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(project_native(&f.0, &f.target(), &[]).is_err());
    let f = Fixture::new();
    f.checkpoint(1, &[("job-a", "unknown")]);
    let path = f.dir().join("1-1.json");
    let mut cp: CheckpointEntry = read_private_json(&path).unwrap();
    cp.payload.remote_effects.clear();
    write_private_json_atomic(&path, &cp, true).unwrap();
    assert!(matches!(
        inspect_native(&f.0, &f.target(), &[]),
        Err(RemoteError::Corrupt)
    ));
    let f = Fixture::new();
    f.checkpoint(1, &[]);
    std::fs::hard_link(f.dir().join("1-1.json"), f.dir().join("2-1.json")).unwrap();
    assert!(inspect_native(&f.0, &f.target(), &[]).is_err());
}
#[test]
fn concurrent_conflicting_authorizations_have_only_one_winner() {
    let f = Fixture::new();
    f.checkpoint(1, &[("job-a", "unknown")]);
    let request = f.request(Some("job-a"));
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let results = std::thread::scope(|scope| {
        let mut handles = vec![];
        for decision in [ResolutionDecision::Finished, ResolutionDecision::Cancelled] {
            let root = f.0.clone();
            let b = barrier.clone();
            let mut r = request.clone();
            r.decision = decision;
            handles.push(scope.spawn(move || {
                b.wait();
                resolve_native(
                    &root,
                    ResolutionAuthority::Operator(&AuthenticatedActor::operator_ui()),
                    &RemoteTarget {
                        team: "t1".into(),
                        seat: TARGET.into(),
                        expected_generation: 1,
                    },
                    &r,
                    &[],
                )
            }));
        }
        handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(std::fs::read_dir(f.facts()).unwrap().count(), 1);
}
#[test]
fn facts_do_not_break_subsequent_checkpoint_append_or_expand_replace_dto() {
    let f = Fixture::new();
    f.checkpoint(1, &[]);
    f.resolve(&f.request(None)).unwrap();
    f.checkpoint(2, &[]);
    assert_eq!(f.view().effects.len(), 0);
    let value = serde_json::json!({"target_seat":TARGET,"expected_generation":1,"selection":{"harness":"codex","model":"gpt-6-astra","reasoning":"high"},"expected_inventory_hash":"a".repeat(64)});
    assert!(serde_json::from_value::<crate::teams::AgentReplaceInput>(value).is_err());
}

#[test]
fn publication_rechecks_inventory_and_owner_after_last_auth() {
    for changed_owner in [false, true] {
        let f = Fixture::new();
        f.checkpoint(1, &[("job-a", "unknown")]);
        let r = f.request(None);
        let mut calls = 0;
        let result = resolve_checked(&f.0, &Principal::Operator, &f.target(), &r, &[], || {
            calls += 1;
            if calls == 4 {
                if changed_owner {
                    f.change_owner(TARGET, |o| o.generation = 2);
                } else {
                    let path = f.dir().join("1-1.json");
                    let mut cp: CheckpointEntry = read_private_json(&path).unwrap();
                    cp.payload.remote_effects.push(RemoteEffectRef {
                        kind: "shell".into(),
                        reference: "job-b".into(),
                        state: "unknown".into(),
                    });
                    cp.content_hash =
                        digest(&serde_json::to_vec(&(cp.schema_version, &cp.payload)).unwrap());
                    write_private_json_atomic(&path, &cp, true).unwrap();
                }
            }
            Ok(())
        });
        assert_eq!(
            result,
            Err(if changed_owner {
                RemoteError::Generation
            } else {
                RemoteError::StaleInventory
            })
        );
        assert!(!f.facts().join("1.json").exists());
    }
}

#[test]
fn excessive_refs_or_files_stop_instead_of_truncating_inventory() {
    let f = Fixture::new();
    let names: Vec<_> = (0..64).map(|i| format!("job-{i}")).collect();
    let refs: Vec<_> = names.iter().map(|s| (s.as_str(), "unknown")).collect();
    f.checkpoint(1, &refs);
    assert_eq!(f.view().effects.len(), 64);
    f.checkpoint(2, &[("job-extra", "unknown")]);
    assert!(matches!(
        inspect_native(&f.0, &f.target(), &[]),
        Err(RemoteError::Limit)
    ));
    let f = Fixture::new();
    f.checkpoint(1, &[]);
    for n in 1..=MAX_ENTRIES {
        write_private_bytes_atomic(&f.dir().join(format!("2-{n}.json")), b"{}", false).unwrap();
    }
    assert!(matches!(
        inspect_native(&f.0, &f.target(), &[]),
        Err(RemoteError::Limit)
    ));
}

#[test]
fn operator_inventory_read_uses_authority_and_generation_guards() {
    let f = Fixture::new();
    f.checkpoint(1, &[("job-a", "unknown")]);
    let view = inspect_authorized(
        &f.0,
        ResolutionAuthority::Operator(&AuthenticatedActor::operator_ui()),
        &f.target(),
        &[],
    )
    .unwrap();
    assert_eq!(view, f.view());
    assert!(matches!(
        inspect_authorized(
            &f.0,
            ResolutionAuthority::Operator(&AuthenticatedActor::launcher()),
            &f.target(),
            &[]
        ),
        Err(RemoteError::Authority)
    ));
    assert!(matches!(
        open(
            &f.0,
            &f.target(),
            Some(&Principal::Lead {
                seat: TARGET.into(),
                generation: 1
            })
        ),
        Err(RemoteError::Authority)
    ));
    assert!(matches!(
        open(
            &f.0,
            &f.target(),
            Some(&Principal::Lead {
                seat: LEAD.into(),
                generation: 2
            })
        ),
        Err(RemoteError::Generation)
    ));
    assert!(!f.facts().exists());
}
