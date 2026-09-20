use crate::agents::legacy_lifecycle_guard as team_legacy_guard;
use crate::tmux as native_tmux;
use crate::{team_archive, team_checkpoint, team_process, team_replacement};
use std::collections::HashMap;
use team_replacement::*;

struct Fake {
    clock: u64,
    owner: u64,
    snapshot: OwnershipSnapshot,
    states: HashMap<u32, ProcessState>,
    turn: TurnState,
    rate: Option<u64>,
    checkpoint: CheckpointRecovery,
    requested: u32,
    signals: Vec<(u32, Signal, u64)>,
    kill_survives: bool,
    revoked: bool,
    revoke_valid: bool,
    remote_complete: bool,
    remote_unknown: bool,
    unowned: bool,
    unowned_after_wait: bool,
    new_owner: u32,
    tokens: u32,
    threads: u32,
    windows: u32,
    actual_model: Option<String>,
    actual_harness: Option<String>,
    actual_reasoning: Option<String>,
    aborted: u32,
    activate_fails: bool,
    events: Vec<ReplacementPhase>,
    authorized: bool,
}
impl Fake {
    fn new() -> Self {
        let processes = vec![
            OwnedProcess {
                identity: ProcessIdentity {
                    pid: 100,
                    start_time: "birth-a".into(),
                },
                parent_pid: 50,
                process_group: 100,
                depth: 0,
                cmdline_sha256: "a".repeat(64),
                cwd: "/fixture".into(),
            },
            OwnedProcess {
                identity: ProcessIdentity {
                    pid: 101,
                    start_time: "birth-b".into(),
                },
                parent_pid: 100,
                process_group: 100,
                depth: 1,
                cmdline_sha256: "a".repeat(64),
                cwd: "/fixture".into(),
            },
        ];
        Self {
            clock: 0,
            owner: 3,
            snapshot: OwnershipSnapshot {
                seat: "t1-backend".into(),
                generation: 3,
                thread_id: "old-thread".into(),
                processes,
                complete: true,
                unowned_matches: vec![],
            },
            states: HashMap::from([(100, ProcessState::Same), (101, ProcessState::Same)]),
            turn: TurnState::Idle,
            rate: None,
            checkpoint: CheckpointRecovery::None,
            requested: 0,
            signals: vec![],
            kill_survives: false,
            revoked: false,
            revoke_valid: true,
            remote_complete: true,
            remote_unknown: false,
            unowned: false,
            unowned_after_wait: false,
            new_owner: 0,
            tokens: 0,
            threads: 0,
            windows: 0,
            actual_model: Some("model-a".into()),
            actual_harness: Some("codex".into()),
            actual_reasoning: Some("high".into()),
            aborted: 0,
            activate_fails: false,
            events: vec![],
            authorized: true,
        }
    }
    fn no_new(&self) {
        assert_eq!(
            (self.new_owner, self.tokens, self.threads, self.windows),
            (0, 0, 0, 0)
        );
    }
}
impl ReplacementRuntime for Fake {
    fn now_ms(&self) -> u64 {
        self.clock
    }
    fn wait_ms(&mut self, ms: u64) {
        self.clock += ms;
    }
    fn event(&mut self, p: ReplacementPhase) {
        self.events.push(p);
    }
    fn expected_owner(&mut self, _: &str, g: u64) -> Result<(), ReplacementError> {
        if self.owner == g {
            Ok(())
        } else {
            Err(ReplacementError::GenerationMismatch)
        }
    }
    fn snapshot(&mut self, _: &str, _: u64) -> Result<OwnershipSnapshot, ReplacementError> {
        Ok(self.snapshot.clone())
    }
    fn turn_state(&self) -> TurnState {
        self.turn
    }
    fn rate_limit_at_ms(&self) -> Option<u64> {
        self.rate
    }
    fn request_checkpoint(&mut self) -> Result<(), ReplacementError> {
        self.requested += 1;
        Ok(())
    }
    fn checkpoint_recovery(&mut self) -> CheckpointRecovery {
        self.checkpoint
    }
    fn process_state(&mut self, p: &ProcessIdentity) -> ProcessState {
        *self.states.get(&p.pid).unwrap_or(&ProcessState::Gone)
    }
    fn signal(&mut self, p: &ProcessIdentity, s: Signal) -> Result<(), ReplacementError> {
        self.signals.push((p.pid, s, self.clock));
        if s == Signal::Kill && !self.kill_survives {
            self.states.insert(p.pid, ProcessState::Gone);
        }
        Ok(())
    }
    fn unowned_matches(&mut self, _: &OwnershipSnapshot) -> Result<bool, ReplacementError> {
        Ok(self.unowned || (self.unowned_after_wait && self.clock > 0))
    }
    fn revoke(&mut self, s: &OwnershipSnapshot) -> Result<RevocationProof, ReplacementError> {
        self.revoked = true;
        Ok(RevocationProof {
            generation: s.generation,
            durable: self.revoke_valid,
            sockets_closed: true,
            close_code: 4001,
            close_elapsed_ms: 10,
            reconnect_code: 4003,
            token_deleted: true,
        })
    }
    fn revocation_still_valid(&mut self, _: &OwnershipSnapshot) -> Result<bool, ReplacementError> {
        Ok(self.revoked && self.revoke_valid)
    }
    fn remote_effects(
        &mut self,
        _: &OwnershipSnapshot,
    ) -> Result<RemoteInventory, ReplacementError> {
        Ok(RemoteInventory {
            complete_observation: self.remote_complete,
            explicit_resolution: None,
            effects: if self.remote_unknown {
                vec![RemoteEffect {
                    reference: "fixture-job".into(),
                    resolution: RemoteResolution::Unknown,
                }]
            } else {
                vec![]
            },
        })
    }
    fn authorize_selection(
        &mut self,
        _: &OwnershipSnapshot,
        _: &StartSelection,
    ) -> Result<(), ReplacementError> {
        if self.authorized {
            Ok(())
        } else {
            Err(ReplacementError::AuthorizationRequired)
        }
    }
    fn start_fresh(
        &mut self,
        s: &OwnershipSnapshot,
        x: &StartSelection,
    ) -> Result<StartedCandidate, ReplacementError> {
        self.tokens += 1;
        self.threads += 1;
        self.windows += 1;
        Ok(StartedCandidate {
            actual_harness: self.actual_harness.clone(),
            actual_reasoning: self.actual_reasoning.clone(),
            observed: StartedReplacement {
                generation: s.generation + 1,
                thread_id: "fresh-thread".into(),
                requested_model: x.model.clone(),
                actual_model: self.actual_model.clone(),
                model_verified: false,
            },
            process: ProcessIdentity {
                pid: 200,
                start_time: "new-birth".into(),
            },
            token_id: "new-token-id".into(),
        })
    }
    fn activate_started(&mut self, _: &StartedCandidate) -> Result<(), ReplacementError> {
        if self.activate_fails {
            return Err(ReplacementError::NativeFailure);
        }
        self.new_owner += 1;
        Ok(())
    }
    fn abort_started(&mut self, c: &StartedCandidate) -> Result<(), ReplacementError> {
        assert_eq!(c.process.pid, 200);
        assert_eq!(c.observed.generation, 4);
        assert_eq!(c.token_id, "new-token-id");
        self.aborted += 1;
        self.new_owner = 0;
        Ok(())
    }
}
fn selection() -> StartSelection {
    StartSelection {
        harness: "codex".into(),
        model: "model-a".into(),
        reasoning: Some("high".into()),
    }
}
#[test]
fn busy_checkpoint_timeout_then_term_children_first_kill_and_fresh_start() {
    let mut r = Fake::new();
    r.turn = TurnState::Busy;
    let p = prepare(&mut r, "t1-backend", 3, &ReplacementPolicy::default()).unwrap();
    assert_eq!(r.requested, 1);
    assert_eq!(r.clock, 100_000);
    assert_eq!(p.checkpoint_recovery(), CheckpointRecovery::None);
    assert_eq!(
        r.signals,
        vec![
            (101, Signal::Term, 90_000),
            (100, Signal::Term, 90_000),
            (101, Signal::Kill, 100_000),
            (100, Signal::Kill, 100_000)
        ]
    );
    r.no_new();
    let out = start(&mut r, p, &selection()).unwrap();
    assert!(out.model_verified);
    assert_eq!(out.generation, 4);
    assert_eq!(r.new_owner, 1);
}
#[test]
fn dead_root_does_not_skip_recorded_orphan() {
    let mut r = Fake::new();
    r.turn = TurnState::Dead;
    r.states.insert(100, ProcessState::Gone);
    prepare(&mut r, "t1-backend", 3, &ReplacementPolicy::default()).unwrap();
    assert_eq!(r.requested, 0);
    assert_eq!(
        r.signals.iter().map(|x| x.0).collect::<Vec<_>>(),
        vec![101, 101]
    );
    r.no_new();
}
#[test]
fn dead_without_trustworthy_owned_set_is_blocked() {
    let mut r = Fake::new();
    r.turn = TurnState::Dead;
    r.snapshot.complete = false;
    assert!(matches!(
        prepare(&mut r, "t1-backend", 3, &ReplacementPolicy::default()),
        Err(ReplacementError::InvalidSnapshot)
    ));
    assert!(r.signals.is_empty());
    r.no_new();
}
#[test]
fn unowned_match_never_signalled() {
    let mut r = Fake::new();
    r.unowned = true;
    assert!(matches!(
        prepare(&mut r, "t1-backend", 3, &ReplacementPolicy::default()),
        Err(ReplacementError::UnownedProcess)
    ));
    assert!(r.signals.is_empty());
    r.no_new();
}
#[test]
fn recycled_or_unreadable_blocks_before_any_signal() {
    for state in [ProcessState::Recycled, ProcessState::Unreadable] {
        let mut r = Fake::new();
        r.states.insert(100, state);
        assert!(matches!(
            prepare(&mut r, "t1-backend", 3, &ReplacementPolicy::default()),
            Err(ReplacementError::StopUnverified)
        ));
        assert!(r.signals.is_empty());
        r.no_new();
    }
}
#[test]
fn survivor_blocks_revocation_and_start() {
    let mut r = Fake::new();
    r.kill_survives = true;
    assert!(matches!(
        prepare(&mut r, "t1-backend", 3, &ReplacementPolicy::default()),
        Err(ReplacementError::StopUnverified)
    ));
    assert!(!r.revoked);
    r.no_new();
}
#[test]
fn unknown_remote_or_unobservable_shell_is_not_zero() {
    for unobservable in [false, true] {
        let mut r = Fake::new();
        r.remote_unknown = !unobservable;
        r.remote_complete = !unobservable;
        assert!(matches!(
            prepare(&mut r, "t1-backend", 3, &ReplacementPolicy::default()),
            Err(ReplacementError::RemoteUncertain)
        ));
        assert!(r.revoked);
        r.no_new();
    }
}
#[test]
fn bad_revoke_is_not_token_deletion_success() {
    let mut r = Fake::new();
    r.revoke_valid = false;
    assert!(matches!(
        prepare(&mut r, "t1-backend", 3, &ReplacementPolicy::default()),
        Err(ReplacementError::RevocationUnverified)
    ));
    r.no_new();
}
#[test]
fn prepare_does_not_authorize_stale_generation_start() {
    let mut r = Fake::new();
    let p = prepare(&mut r, "t1-backend", 3, &ReplacementPolicy::default()).unwrap();
    r.owner = 4;
    assert!(matches!(
        start(&mut r, p, &selection()),
        Err(ReplacementError::GenerationMismatch)
    ));
    r.no_new();
}
#[test]
fn fresh_remote_unknown_between_buttons_blocks_start() {
    let mut r = Fake::new();
    let p = prepare(&mut r, "t1-backend", 3, &ReplacementPolicy::default()).unwrap();
    r.remote_unknown = true;
    assert!(matches!(
        start(&mut r, p, &selection()),
        Err(ReplacementError::RemoteUncertain)
    ));
    r.no_new();
}
#[test]
fn selection_authorization_rechecked_before_new_authority() {
    let mut r = Fake::new();
    let p = prepare(&mut r, "t1-backend", 3, &ReplacementPolicy::default()).unwrap();
    r.authorized = false;
    assert!(matches!(
        start(&mut r, p, &selection()),
        Err(ReplacementError::AuthorizationRequired)
    ));
    r.no_new();
}
#[test]
fn rate_limit_signature_has_sixty_second_ttl() {
    for (age, requests, recovery) in [
        (60_000, 1, CheckpointRecovery::Stale),
        (60_001, 0, CheckpointRecovery::Valid),
    ] {
        let mut r = Fake::new();
        r.clock = age;
        r.rate = Some(0);
        r.checkpoint = CheckpointRecovery::Valid;
        let p = prepare(&mut r, "t1-backend", 3, &ReplacementPolicy::default()).unwrap();
        assert_eq!(r.requested, requests);
        assert_eq!(p.checkpoint_recovery(), recovery);
    }
}

#[test]
fn actual_model_mismatch_or_absent_stops_new_incarnation_without_second_start() {
    for actual in [None, Some("wrong-model".into())] {
        let mut r = Fake::new();
        r.actual_model = actual;
        let p = prepare(&mut r, "t1-backend", 3, &ReplacementPolicy::default()).unwrap();
        assert!(matches!(
            start(&mut r, p, &selection()),
            Err(ReplacementError::ModelUnverified)
        ));
        assert_eq!(r.aborted, 1);
        assert_eq!(r.new_owner, 0);
        assert_eq!((r.tokens, r.threads, r.windows), (1, 1, 1));
        assert!(!r.events.contains(&ReplacementPhase::Started));
    }
}
#[test]
fn failed_owner_commit_cleans_exact_spawn_once() {
    let mut r = Fake::new();
    r.activate_fails = true;
    let p = prepare(&mut r, "t1-backend", 3, &ReplacementPolicy::default()).unwrap();
    assert!(start(&mut r, p, &selection()).is_err());
    assert_eq!(r.aborted, 1);
    assert_eq!(r.new_owner, 0);
    assert_eq!(r.threads, 1);
}
mod archive_tests {
    use super::team_archive::*;
    fn evidence() -> ArchiveEvidence {
        ArchiveEvidence {
            team: "t1".into(),
            generation: 1,
            reconciliation_complete: true,
            inventoried_task_ids: vec!["aperture-task".into()],
            items: vec![ReconciliationItem {
                task_id: "aperture-task".into(),
                created_by: "glados".into(),
                assigned_to_closing_team: true,
                unfinished: false,
                disposition: Some(Disposition::Completed {
                    evidence_ref: "artifact:receipt".into(),
                    acceptance_met: true,
                }),
            }],
            required_review_ids: vec!["review".into()],
            reviews: vec![RequiredReview {
                review_id: "review".into(),
                reviewer: "izzy".into(),
                verdict: "pass".into(),
                at_ms: 10,
            }],
            required_metrics: vec!["acceptance".into()],
            metrics: vec![MetricEvidence {
                metric: "acceptance".into(),
                observed_at_ms: 10,
                evidence_ref: "artifact:receipt".into(),
                met: true,
            }],
            expected_seats: vec!["t1-backend".into()],
            seats: vec![SeatArchiveEvidence {
                seat: "t1-backend".into(),
                exact_processes_gone: true,
                owner_stale_verified: true,
                revocation_durable: true,
                remote_reconciled: true,
                worktree_clean: true,
                protected_marker_verified: false,
            }],
            open_child_count: 0,
        }
    }
    struct NativeFake {
        evidence: ArchiveEvidence,
        journal_calls: u32,
        verify_calls: u32,
        journal_fails: bool,
    }
    impl ArchiveRuntime for NativeFake {
        fn lock_and_collect(&mut self, _: &str, _: u64) -> Result<ArchiveEvidence, ArchiveBlocker> {
            Ok(self.evidence.clone())
        }
        fn now_ms(&self) -> u64 {
            20
        }
        fn shared_journal_archive(&mut self, _: &ArchiveEvidence) -> Result<(), ArchiveBlocker> {
            self.journal_calls += 1;
            if self.journal_fails {
                Err(ArchiveBlocker {
                    code: "E_JOURNAL_INCONSISTENT".into(),
                    reference: "t1".into(),
                })
            } else {
                Ok(())
            }
        }
        fn verify_canonical_archive(&mut self, _: &str) -> Result<(), ArchiveBlocker> {
            self.verify_calls += 1;
            Ok(())
        }
    }
    #[test]
    fn archive_uses_fresh_gates_and_one_shared_journal_only() {
        let mut r = NativeFake {
            evidence: evidence(),
            journal_calls: 0,
            verify_calls: 0,
            journal_fails: false,
        };
        r.evidence.seats[0].owner_stale_verified = false;
        assert!(archive(&mut r, "t1", 1).is_err());
        assert_eq!(r.journal_calls, 0);
        r.evidence.seats[0].owner_stale_verified = true;
        assert!(archive(&mut r, "t1", 2).is_err());
        assert_eq!(r.journal_calls, 0);
        r.journal_fails = true;
        assert_eq!(
            archive(&mut r, "t1", 1).unwrap_err()[0].code,
            "E_JOURNAL_INCONSISTENT"
        );
        assert_eq!(r.journal_calls, 1);
        assert_eq!(r.verify_calls, 0);
    }
    #[test]
    fn archive_success_requires_canonical_readback() {
        let mut r = NativeFake {
            evidence: evidence(),
            journal_calls: 0,
            verify_calls: 0,
            journal_fails: false,
        };
        archive(&mut r, "t1", 1).unwrap();
        assert_eq!((r.journal_calls, r.verify_calls), (1, 1));
    }
    #[test]
    fn completed_reviewed_reconciled_archive_allowed() {
        assert!(check_archive(&evidence(), 20).is_empty());
    }
    #[test]
    fn historical_closed_assignee_preserved() {
        let e = evidence();
        assert!(e.items[0].assigned_to_closing_team);
        assert!(check_archive(&e, 20).is_empty());
    }
    #[test]
    fn unfinished_is_not_completed_by_assertion() {
        let mut e = evidence();
        e.items[0].unfinished = true;
        assert!(check_archive(&e, 20)
            .iter()
            .any(|x| x.code == "E_UNFINISHED_SEAT_WORK"));
    }
    #[test]
    fn completed_without_evidence_blocks() {
        let mut e = evidence();
        e.items[0].disposition = Some(Disposition::Completed {
            evidence_ref: "".into(),
            acceptance_met: true,
        });
        assert!(check_archive(&e, 20)
            .iter()
            .any(|x| x.code == "E_COMPLETED_WITHOUT_EVIDENCE"));
    }
    #[test]
    fn unapproved_cancel_blocks() {
        let mut e = evidence();
        e.items[0].disposition = Some(Disposition::Cancelled {
            disposition_ref: "".into(),
            approved: false,
        });
        assert!(check_archive(&e, 20)
            .iter()
            .any(|x| x.code == "E_CANCEL_UNAPPROVED"));
    }
    #[test]
    fn transfer_requires_acceptance_and_reparent_and_history() {
        for missing in 0..4 {
            let mut e = evidence();
            e.items[0].assigned_to_closing_team = false;
            e.items[0].unfinished = true;
            e.items[0].disposition = Some(Disposition::Transferred {
                owner: "outside".into(),
                task: "other-task".into(),
                approval_ref: "approved".into(),
                acceptance_ref: if missing == 0 { "" } else { "accepted" }.into(),
                history_ref: if missing == 1 { "" } else { "history" }.into(),
                reparented: missing != 2,
                open_child_of_closing_epic: missing == 3,
            });
            assert!(check_archive(&e, 20)
                .iter()
                .any(|x| x.code == "E_TRANSFER_UNACCEPTED"));
        }
    }
    #[test]
    fn accepted_transferred_work_can_remain_unfinished_elsewhere() {
        let mut e = evidence();
        e.items[0].assigned_to_closing_team = false;
        e.items[0].unfinished = true;
        e.items[0].disposition = Some(Disposition::Transferred {
            owner: "outside".into(),
            task: "other-task".into(),
            approval_ref: "approved".into(),
            acceptance_ref: "accepted".into(),
            history_ref: "history".into(),
            reparented: true,
            open_child_of_closing_epic: false,
        });
        assert!(check_archive(&e, 20).is_empty());
    }
    #[test]
    fn missing_review_or_unmet_metric_blocks() {
        let mut e = evidence();
        e.reviews.clear();
        e.metrics[0].met = false;
        let b = check_archive(&e, 20);
        assert!(b.iter().any(|x| x.code == "E_REVIEW_MISSING"));
        assert!(b.iter().any(|x| x.code == "E_METRIC_UNMET"));
    }
    #[test]
    fn live_process_remote_unknown_or_dirty_unprotected_blocks() {
        let mut e = evidence();
        e.seats[0].exact_processes_gone = false;
        e.seats[0].remote_reconciled = false;
        e.seats[0].worktree_clean = false;
        let b = check_archive(&e, 20);
        assert!(b.iter().any(|x| x.code == "E_STOP_UNVERIFIED"));
        assert!(b.iter().any(|x| x.code == "E_REMOTE_UNCERTAIN"));
        assert!(b.iter().any(|x| x.code == "E_WORKTREE_UNPROTECTED"));
    }
    #[test]
    fn partial_coverage_and_duplicate_inventory_blocks() {
        let mut e = evidence();
        e.inventoried_task_ids.push("missing".into());
        assert!(check_archive(&e, 20)
            .iter()
            .any(|x| x.code == "E_RECONCILIATION_COVERAGE"));
    }
}
mod checkpoint_tests {
    use super::team_checkpoint::*;
    #[derive(Default)]
    struct Store(Vec<CheckpointEntry>);
    impl CheckpointStore for Store {
        fn entries(
            &mut self,
            _: &str,
            _: &str,
            _: u64,
        ) -> Result<Vec<CheckpointEntry>, CheckpointError> {
            Ok(self.0.clone())
        }
        fn digest(&self, b: &[u8]) -> Result<String, CheckpointError> {
            Ok(format!("fixture:{}", String::from_utf8_lossy(b)))
        }
        fn append(&mut self, e: &CheckpointEntry) -> Result<(), CheckpointError> {
            self.0.push(e.clone());
            Ok(())
        }
    }
    fn context() -> CheckpointContext {
        CheckpointContext {
            team: "t1".into(),
            seat: "t1-backend".into(),
            generation: 2,
            authenticated_generation: 2,
            harness: "codex".into(),
            writer: CheckpointWriter::Explicit,
        }
    }
    fn payload() -> CheckpointPayload {
        CheckpointPayload {
            task_id: "aperture-fixture".into(),
            worktree: "aperture-fixture-work".into(),
            branch: "aperture-fixture".into(),
            head_sha: "a".repeat(40),
            dirty_files: vec!["src/file.rs".into()],
            open_pr: None,
            running_procs: vec![],
            decisions: vec![Decision {
                code: "keep".into(),
                text: "Preserve the reviewed branch.".into(),
                evidence_ref: Some("artifact:receipt".into()),
            }],
            next_step: "Run focused fixtures.".into(),
            remote_effects: vec![],
        }
    }
    #[test]
    fn launcher_sequence_and_cross_writer_five_second_dedupe() {
        let mut s = Store::default();
        let mut c = context();
        let a = write(&mut s, &c, 1, payload(), 0, &[]).unwrap();
        c.writer = CheckpointWriter::ClaudeStopHook;
        c.harness = "claude".into();
        assert_eq!(
            write(&mut s, &c, 1, payload(), 5000, &[])
                .unwrap()
                .checkpoint_id,
            a.checkpoint_id
        );
        assert_eq!(write(&mut s, &c, 1, payload(), 5001, &[]).unwrap().seq, 2);
        assert_eq!(s.0.len(), 2);
    }
    #[test]
    fn corrupted_checkpoint_hash_or_payload_never_advances_sequence() {
        for mutation in [0, 1, 2] {
            let mut s = Store::default();
            write(&mut s, &context(), 1, payload(), 0, &[]).unwrap();
            match mutation {
                0 => s.0[0].content_hash = "wrong".into(),
                1 => s.0[0].payload.next_step = "changed".into(),
                _ => s.0[0].payload.worktree = "../outside".into(),
            };
            assert_eq!(
                write(&mut s, &context(), 1, payload(), 6000, &[]),
                Err(CheckpointError::Corrupt)
            );
            assert_eq!(s.0.len(), 1);
        }
    }
    #[test]
    fn codex_has_no_stop_hook() {
        let mut s = Store::default();
        let mut c = context();
        c.writer = CheckpointWriter::ClaudeStopHook;
        assert_eq!(
            write(&mut s, &c, 1, payload(), 0, &[]),
            Err(CheckpointError::HookHarness)
        );
        assert!(s.0.is_empty());
    }
    #[test]
    fn revoked_or_wrong_generation_cannot_write() {
        let mut s = Store::default();
        let mut c = context();
        c.authenticated_generation = 1;
        assert_eq!(
            write(&mut s, &c, 1, payload(), 0, &[]),
            Err(CheckpointError::Generation)
        );
        assert!(s.0.is_empty());
    }
    #[test]
    fn unknown_schema_retained_rejected_not_recoverable() {
        let mut s = Store::default();
        let e = write(&mut s, &context(), 99, payload(), 0, &[]).unwrap();
        assert!(matches!(
            e.validation,
            CheckpointValidation::Rejected { .. }
        ));
        assert_eq!(s.0.len(), 1);
        assert!(latest_valid(&s.0).is_none());
    }
    #[test]
    fn valid_highest_sequence_not_latest_pending() {
        let mut s = Store::default();
        let e = write(&mut s, &context(), 1, payload(), 0, &[]).unwrap();
        s.0[0].validation = CheckpointValidation::Ok;
        write(&mut s, &context(), 1, payload(), 6000, &[]).unwrap();
        assert_eq!(latest_valid(&s.0).unwrap().seq, e.seq);
        assert!(stale(&s.0[0], 900001, 900000));
    }
    #[test]
    fn lead_artifact_comparison_marks_divergence() {
        let mut s = Store::default();
        let e = write(&mut s, &context(), 1, payload(), 0, &[]).unwrap();
        assert_eq!(
            validate_against_artifacts(
                &e,
                &ArtifactObservation {
                    head_sha: "b".repeat(40),
                    dirty_files: vec![],
                    open_pr: None
                }
            ),
            CheckpointValidation::Divergent {
                fields: vec!["head_sha".into(), "dirty_files".into()]
            }
        );
    }
    #[test]
    fn unsafe_text_url_env_controls_and_sentinel_rejected_before_write() {
        for bad in [
            "https://provider.invalid/capability",
            "TOKEN=value",
            "line\nnext",
            "sentinel-fixture",
            "ssh example",
            "",
            " ",
            "hidden\u{202e}text",
        ] {
            let mut s = Store::default();
            let mut p = payload();
            p.next_step = bad.into();
            assert!(write(&mut s, &context(), 1, p, 0, &["sentinel-fixture".into()]).is_err());
            assert!(s.0.is_empty());
        }
    }
    #[test]
    fn paths_unknown_fields_and_caps_are_not_silently_repaired() {
        for path in ["../secret", "/root", "a/../b", "a\\b"] {
            let mut p = payload();
            p.worktree = path.into();
            assert!(validate_payload(&p, &[]).is_err())
        }
        let mut p = payload();
        p.next_step = "x".repeat(1025);
        assert!(validate_payload(&p, &[]).is_err());
        let mut raw = serde_json::to_value(payload()).unwrap();
        raw["environment"] = serde_json::json!({"bad":"sentinel"});
        assert!(serde_json::from_value::<CheckpointPayload>(raw).is_err());
    }
}
mod process_tests {
    use super::{team_process::*, team_replacement::*};
    fn p(pid: u32, ppid: u32, pgid: u32) -> ProcessMetadata {
        ProcessMetadata {
            identity: ProcessIdentity {
                pid,
                start_time: format!("{pid}.000001"),
            },
            ppid,
            pgid,
            uid: unsafe { libc::geteuid() },
        }
    }
    #[test]
    fn descendants_and_owned_group_not_cwd_matches() {
        let table = vec![
            p(100, 50, 100),
            p(101, 100, 100),
            p(102, 101, 102),
            p(200, 50, 200),
        ];
        let s = capture_owned(
            "t1-backend",
            1,
            "thread",
            &table[0].identity,
            &table,
            &[],
            &[table[3].identity.clone()],
            true,
        )
        .unwrap();
        assert_eq!(
            s.processes
                .iter()
                .map(|p| p.identity.pid)
                .collect::<Vec<_>>(),
            vec![100, 101, 102]
        );
        assert_eq!(s.unowned_matches[0].pid, 200);
    }
    #[test]
    fn shared_tmux_group_does_not_authorize_siblings() {
        let table = vec![p(100, 50, 50), p(200, 50, 50)];
        let s = capture_owned(
            "t1-backend",
            1,
            "thread",
            &table[0].identity,
            &table,
            &[],
            &[table[1].identity.clone()],
            true,
        )
        .unwrap();
        assert_eq!(s.processes.len(), 1);
        assert_eq!(s.unowned_matches.len(), 1);
    }
    #[test]
    fn reparented_recorded_orphan_remains_owned() {
        let root = p(100, 50, 100);
        let child = p(101, 1, 100);
        let old = OwnedProcess {
            identity: child.identity.clone(),
            parent_pid: 100,
            process_group: 100,
            depth: 1,
            cmdline_sha256: "a".repeat(64),
            cwd: "/fixture".into(),
        };
        let s = capture_owned(
            "t1-backend",
            1,
            "thread",
            &root.identity,
            &[child],
            &[old],
            &[],
            true,
        )
        .unwrap();
        assert_eq!(s.processes[0].identity.pid, 101);
    }
    #[test]
    fn recycled_recorded_pid_or_missing_snapshot_fails_closed() {
        let root = p(100, 50, 100);
        assert!(capture_owned(
            "t1-backend",
            1,
            "thread",
            &root.identity,
            &[],
            &[],
            &[],
            true
        )
        .is_err());
        let mut changed = root.clone();
        changed.identity.start_time = "other".into();
        assert!(capture_owned(
            "t1-backend",
            1,
            "thread",
            &root.identity,
            &[changed],
            &[],
            &[],
            true
        )
        .is_err());
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn native_birth_identity_reads_only_this_test_process() {
        let pid = std::process::id();
        let p = observe(pid).unwrap().unwrap();
        assert_eq!(p.identity.pid, pid);
        assert_eq!(state(&p.identity), ProcessState::Same);
        let wrong = ProcessIdentity {
            pid,
            start_time: "wrong-birth".into(),
        };
        assert_eq!(state(&wrong), ProcessState::Recycled);
    }
}

#[test]
fn checkpoint_wait_new_unowned_process_blocks_before_signal() {
    let mut r = Fake::new();
    r.turn = TurnState::Busy;
    r.unowned_after_wait = true;
    assert!(matches!(
        prepare(&mut r, "t1-backend", 3, &ReplacementPolicy::default()),
        Err(ReplacementError::UnownedProcess)
    ));
    assert!(r.signals.is_empty());
    r.no_new();
}
#[test]
fn empty_or_unhydrated_owned_snapshot_cannot_signal_or_spawn() {
    for empty in [false, true] {
        let mut r = Fake::new();
        if empty {
            r.snapshot.processes.clear()
        } else {
            r.snapshot.processes[0].cmdline_sha256.clear()
        }
        assert!(matches!(
            prepare(&mut r, "t1-backend", 3, &ReplacementPolicy::default()),
            Err(ReplacementError::InvalidSnapshot)
        ));
        assert!(r.signals.is_empty());
        r.no_new();
    }
}
#[test]
fn process_metadata_completion_requires_exact_private_metadata() {
    let mut s = Fake::new().snapshot;
    s.complete = false;
    s.processes[0].cmdline_sha256.clear();
    assert!(
        team_process::complete_metadata(s.clone(), |_| Err(ReplacementError::StopUnverified))
            .is_err()
    );
    let out = team_process::complete_metadata(s, |p| {
        assert_eq!(p.pid, 100);
        Ok(("f".repeat(64), "/fixture".into()))
    })
    .unwrap();
    assert!(out.complete);
}

#[test]
fn observed_execution_tuple_must_match_not_only_model_string() {
    for field in [0, 1, 2] {
        let mut r = Fake::new();
        match field {
            0 => r.actual_harness = None,
            1 => r.actual_harness = Some("claude".into()),
            _ => r.actual_reasoning = Some("low".into()),
        };
        let p = prepare(&mut r, "t1-backend", 3, &ReplacementPolicy::default()).unwrap();
        assert!(matches!(
            start(&mut r, p, &selection()),
            Err(ReplacementError::ModelUnverified)
        ));
        assert_eq!(r.aborted, 1);
        assert_eq!(r.new_owner, 0);
        assert_eq!(r.threads, 1);
    }
}

mod legacy_guard_tests {
    use super::team_legacy_guard::*;
    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!(
                "aperture-k310b-guard-{}", uuid::Uuid::new_v4()
            ));
            std::fs::create_dir(&p).unwrap();
            std::fs::create_dir(p.join("seat")).unwrap();
            Self(p)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap()
        }
    }
    #[test]
    fn parallel_fixtures_do_not_share_clock_derived_paths() {
        let fixtures = std::thread::scope(|s| {
            let handles: Vec<_> = (0..16).map(|_| s.spawn(Fixture::new)).collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect::<Vec<_>>()
        });
        let mut paths = std::collections::HashSet::new();
        for f in &fixtures {
            assert!(paths.insert(f.0.clone()));
            let name = f.0.file_name().unwrap().to_str().unwrap();
            assert!(uuid::Uuid::parse_str(name.strip_prefix("aperture-k310b-guard-").unwrap()).is_ok());
        }
    }
    #[test]
    fn standing_seat_without_marker_preserves_legacy_access() {
        let f = Fixture::new();
        assert!(ensure_legacy(&f.0, "seat", Membership::Standing).is_ok())
    }
    #[test]
    fn active_pending_archived_and_unknown_are_never_standing() {
        let f = Fixture::new();
        for status in ["active", "pending", "archived", "failed", "invalid"] {
            let membership = if status == "invalid" {
                Membership::Unknown
            } else {
                Membership::Team
            };
            assert_eq!(ensure_legacy(&f.0, "seat", membership), Err(DENIED.into()));
        }
    }
    #[test]
    fn marker_blocks_even_when_membership_cache_says_standing() {
        let f = Fixture::new();
        std::fs::write(f.0.join("seat/TEAM"), b"").unwrap();
        assert!(ensure_legacy(&f.0, "seat", Membership::Standing).is_err())
    }
    #[test]
    fn unsafe_marker_or_seat_and_invalid_selector_fail_closed() {
        let f = Fixture::new();
        std::os::unix::fs::symlink("missing", f.0.join("seat/TEAM")).unwrap();
        assert!(ensure_legacy(&f.0, "seat", Membership::Standing).is_err());
        assert!(ensure_legacy(&f.0, "../seat", Membership::Standing).is_err());
        assert!(ensure_legacy(&f.0, "missing", Membership::Standing).is_err())
    }
}

#[test]
fn exact_tmux_pane_metadata_never_selects_by_name_or_ambiguity() {
    let p = native_tmux::parse_pane_process("@12", b"@12\t%13\t777\n").unwrap();
    assert_eq!((p.pane_id.as_str(), p.pid), ("%13", 777));
    for output in [
        b"@12\t%13\t777\n@12\t%14\t778\n".as_slice(),
        b"@13\t%13\t777",
        b"@12\tname\t777",
        b"@12\t%13\t1",
        b"@12\t%13\t00777",
        b"",
        b"@12\t%13\t777\textra",
    ] {
        assert!(native_tmux::parse_pane_process("@12", output).is_err());
    }
    assert!(native_tmux::parse_pane_process("seat", b"seat\t%1\t777").is_err());
}

#[test]
fn native_owner_birth_unit_roundtrips_without_rounding_or_elapsed_guess() {
    for micros in [1u64, 1234567890123456, u64::MAX] {
        let p = team_process::identity_from_owner(777, micros).unwrap();
        assert_eq!(team_process::birth_micros(&p).unwrap(), micros);
    }
    for bad in [
        "birth",
        "12.5",
        "012.500000",
        "0.000000",
        "18446744073709551615.999999",
        "1.0000000",
    ] {
        assert!(team_process::birth_micros(&ProcessIdentity {
            pid: 777,
            start_time: bad.into()
        })
        .is_err());
    }
}
