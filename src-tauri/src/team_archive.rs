//! Archive acceptance is checked against a complete, fresh reconciliation
//! projection. Actual moves/recovery belong exclusively to Rex's shared journal.
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Disposition {
    Completed {
        evidence_ref: String,
        acceptance_met: bool,
    },
    Cancelled {
        disposition_ref: String,
        approved: bool,
    },
    Transferred {
        owner: String,
        task: String,
        approval_ref: String,
        acceptance_ref: String,
        history_ref: String,
        reparented: bool,
        open_child_of_closing_epic: bool,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReconciliationItem {
    pub task_id: String,
    pub created_by: String,
    pub assigned_to_closing_team: bool,
    pub unfinished: bool,
    pub disposition: Option<Disposition>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequiredReview {
    pub review_id: String,
    pub reviewer: String,
    pub verdict: String,
    pub at_ms: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricEvidence {
    pub metric: String,
    pub observed_at_ms: u64,
    pub evidence_ref: String,
    pub met: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeatArchiveEvidence {
    pub seat: String,
    pub exact_processes_gone: bool,
    pub owner_stale_verified: bool,
    pub revocation_durable: bool,
    pub remote_reconciled: bool,
    pub worktree_clean: bool,
    pub protected_marker_verified: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchiveEvidence {
    pub team: String,
    pub generation: u64,
    pub reconciliation_complete: bool,
    pub inventoried_task_ids: Vec<String>,
    pub items: Vec<ReconciliationItem>,
    pub required_review_ids: Vec<String>,
    pub reviews: Vec<RequiredReview>,
    pub required_metrics: Vec<String>,
    pub metrics: Vec<MetricEvidence>,
    pub expected_seats: Vec<String>,
    pub seats: Vec<SeatArchiveEvidence>,
    pub open_child_count: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArchiveBlocker {
    pub code: String,
    pub reference: String,
}
fn blocker(out: &mut Vec<ArchiveBlocker>, code: &str, reference: &str) {
    out.push(ArchiveBlocker {
        code: code.into(),
        reference: reference.into(),
    });
}
fn unique_nonempty(xs: &[String]) -> bool {
    !xs.iter().any(|x| x.is_empty()) && xs.iter().collect::<HashSet<_>>().len() == xs.len()
}

/// Pure acceptance check. Caller is the trusted native collector, not a UI
/// request carrying self-attested booleans. Re-run under team+seat locks before
/// the shared journal starts; a previous green checklist is not authorization.
pub fn check_archive(e: &ArchiveEvidence, now_ms: u64) -> Vec<ArchiveBlocker> {
    let mut out = Vec::new();
    if e.team.is_empty()
        || e.generation == 0
        || !e.reconciliation_complete
        || !unique_nonempty(&e.inventoried_task_ids)
    {
        blocker(&mut out, "E_RECONCILIATION_INCOMPLETE", &e.team);
    }
    let ids: Vec<String> = e.items.iter().map(|x| x.task_id.clone()).collect();
    if !unique_nonempty(&ids)
        || ids.iter().collect::<HashSet<_>>()
            != e.inventoried_task_ids.iter().collect::<HashSet<_>>()
    {
        blocker(&mut out, "E_RECONCILIATION_COVERAGE", &e.team);
    }
    for item in &e.items {
        if item.created_by != "glados" {
            blocker(&mut out, "E_CREATION_GATE_VIOLATION", &item.task_id);
        }
        if item.unfinished && item.assigned_to_closing_team {
            blocker(&mut out, "E_UNFINISHED_SEAT_WORK", &item.task_id);
        }
        match &item.disposition {
            None => blocker(&mut out, "E_DISPOSITION_MISSING", &item.task_id),
            Some(Disposition::Completed {
                evidence_ref,
                acceptance_met,
            }) => {
                if !acceptance_met || evidence_ref.is_empty() || item.unfinished {
                    blocker(&mut out, "E_COMPLETED_WITHOUT_EVIDENCE", &item.task_id);
                }
            }
            Some(Disposition::Cancelled {
                disposition_ref,
                approved,
            }) => {
                if !approved || disposition_ref.is_empty() || item.unfinished {
                    blocker(&mut out, "E_CANCEL_UNAPPROVED", &item.task_id);
                }
            }
            Some(Disposition::Transferred {
                owner,
                task,
                approval_ref,
                acceptance_ref,
                history_ref,
                reparented,
                open_child_of_closing_epic,
            }) => {
                if owner.is_empty()
                    || task.is_empty()
                    || approval_ref.is_empty()
                    || acceptance_ref.is_empty()
                    || history_ref.is_empty()
                    || !reparented
                    || *open_child_of_closing_epic
                    || item.assigned_to_closing_team
                    || e.expected_seats.contains(owner)
                {
                    blocker(&mut out, "E_TRANSFER_UNACCEPTED", &item.task_id);
                }
            }
        }
    }
    if e.open_child_count != 0 {
        blocker(&mut out, "E_OPEN_CHILDREN", &e.team);
    }
    if e.required_review_ids.is_empty() || !unique_nonempty(&e.required_review_ids) {
        blocker(&mut out, "E_REVIEW_MISSING", &e.team);
    }
    for id in &e.required_review_ids {
        let found: Vec<_> = e.reviews.iter().filter(|r| &r.review_id == id).collect();
        if found.len() != 1
            || found
                .iter()
                .any(|r| r.reviewer.is_empty() || r.verdict != "pass" || r.at_ms > now_ms)
        {
            blocker(&mut out, "E_REVIEW_MISSING", id);
        }
    }
    if e.required_metrics.is_empty() || !unique_nonempty(&e.required_metrics) {
        blocker(&mut out, "E_METRIC_UNMET", &e.team);
    }
    for metric in &e.required_metrics {
        let found: Vec<_> = e.metrics.iter().filter(|m| &m.metric == metric).collect();
        if found.len() != 1
            || found
                .iter()
                .any(|m| !m.met || m.evidence_ref.is_empty() || m.observed_at_ms > now_ms)
        {
            blocker(&mut out, "E_METRIC_UNMET", metric);
        }
    }
    let seats: Vec<String> = e.seats.iter().map(|s| s.seat.clone()).collect();
    if e.expected_seats.is_empty()
        || !unique_nonempty(&e.expected_seats)
        || !unique_nonempty(&seats)
        || seats.iter().collect::<HashSet<_>>() != e.expected_seats.iter().collect::<HashSet<_>>()
    {
        blocker(&mut out, "E_SEAT_COVERAGE", &e.team);
    }
    for seat in &e.seats {
        if !seat.exact_processes_gone || !seat.owner_stale_verified {
            blocker(&mut out, "E_STOP_UNVERIFIED", &seat.seat);
        }
        if !seat.revocation_durable {
            blocker(&mut out, "E_REVOCATION_UNVERIFIED", &seat.seat);
        }
        if !seat.remote_reconciled {
            blocker(&mut out, "E_REMOTE_UNCERTAIN", &seat.seat);
        }
        if !seat.worktree_clean && !seat.protected_marker_verified {
            blocker(&mut out, "E_WORKTREE_UNPROTECTED", &seat.seat);
        }
    }
    out
}

pub trait ArchiveRuntime {
    /// Acquire team lock, then all seat locks lexicographically. This guard is
    /// retained by the adapter through fresh evidence and journal completion.
    fn lock_and_collect(
        &mut self,
        team: &str,
        generation: u64,
    ) -> Result<ArchiveEvidence, ArchiveBlocker>;
    fn now_ms(&self) -> u64;
    /// The ONE shared journal writes manifest preimages and performs no-replace
    /// moves, fsyncs, final markers/state, recovery and inverse-rename rollback.
    fn shared_journal_archive(&mut self, evidence: &ArchiveEvidence) -> Result<(), ArchiveBlocker>;
    fn verify_canonical_archive(&mut self, team: &str) -> Result<(), ArchiveBlocker>;
}
pub fn archive<R: ArchiveRuntime>(
    r: &mut R,
    team: &str,
    generation: u64,
) -> Result<(), Vec<ArchiveBlocker>> {
    let e = r.lock_and_collect(team, generation).map_err(|e| vec![e])?;
    if e.team != team || e.generation != generation {
        return Err(vec![ArchiveBlocker {
            code: "E_GENERATION_MISMATCH".into(),
            reference: team.into(),
        }]);
    }
    let blockers = check_archive(&e, r.now_ms());
    if !blockers.is_empty() {
        return Err(blockers);
    }
    r.shared_journal_archive(&e).map_err(|e| vec![e])?;
    r.verify_canonical_archive(team).map_err(|e| vec![e])
}

#[path = "team_archive_beads.rs"]
pub(crate) mod beads;
#[path = "team_archive_record.rs"]
pub(crate) mod record;
