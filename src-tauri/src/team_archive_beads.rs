//! Bounded native BEADS inventory for archive. Read-only CLI, no notes parsing,
//! no mutation and no inference that closed means acceptance/metric truth.
use super::record::{ArchiveRecord, RecordDisposition, TASK_CAP};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::Path;
use std::time::{Duration, Instant};
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum InventoryError {
    Binding,
    Shape,
    Limit,
    Unavailable,
    Drift,
    Record,
    RecordMissing,
}
impl InventoryError {
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::RecordMissing => "E_RECONCILIATION_RECORD_MISSING",
            Self::Binding => "E_ARCHIVE_BINDING",
            Self::Shape | Self::Record => "E_RECONCILIATION_INVALID",
            Self::Limit => "E_RECONCILIATION_LIMIT",
            Self::Unavailable => "E_RECONCILIATION_UNAVAILABLE",
            Self::Drift => "E_RECONCILIATION_DRIFT",
        }
    }
    pub(crate) fn message(&self) -> &'static str {
        match self {
            Self::RecordMissing => "Archive record is missing. GLaDOS must reconcile task dispositions, reviews and metric evidence before archival.",
            Self::Record => "Archive record is invalid. GLaDOS must correct the epic-bound reconciliation record.",
            Self::Unavailable => "BEADS inventory could not be read. No archival was performed.",
            Self::Drift => "Archive evidence changed. Refresh the reconciliation before approval.",
            Self::Binding => "Archive evidence does not match the team and epic.",
            Self::Limit => "Archive evidence exceeds the bounded inventory limits.",
            Self::Shape => "BEADS inventory has an unsupported structure.",
        }
    }
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Status {
    Open,
    InProgress,
    Blocked,
    Deferred,
    Closed,
}
#[derive(Clone, Serialize, PartialEq, Eq)]
struct Issue {
    id: String,
    status: Status,
    assignee: Option<String>,
    created_by: String,
    issue_type: String,
    updated_at: String,
    parents: Vec<String>,
    acceptance_sha256: String,
}
#[derive(Deserialize)]
#[serde(bound(deserialize = "D: Deserialize<'de>"))]
struct RawIssue<D = Dependency> {
    id: String,
    status: Status,
    assignee: Option<String>,
    created_by: String,
    issue_type: String,
    updated_at: String,
    #[serde(default)]
    dependencies: Vec<D>,
    #[serde(default)]
    acceptance_criteria: String,
}
#[derive(Deserialize)]
struct Dependency {
    id: String,
    dependency_type: String,
}
// bd list embeds edge rows; bd show embeds the referenced issue instead.
// Keep the wire formats distinct. Only hydrated show rows become evidence.
#[derive(Deserialize)]
struct ListedDependency {
    issue_id: String,
    depends_on_id: String,
    #[serde(rename = "type")]
    dependency_type: String,
}
fn id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 96
        && s.as_bytes()[0].is_ascii_alphanumeric()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}
fn hash<T: Serialize>(v: &T) -> Result<String, InventoryError> {
    use sha2::{Digest, Sha256};
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(v).map_err(|_| InventoryError::Shape)?)
    ))
}
fn issues(raw: &[u8]) -> Result<Vec<Issue>, InventoryError> {
    if raw.len() > 8 * 1024 * 1024 {
        return Err(InventoryError::Limit);
    }
    let rows: Vec<RawIssue> = serde_json::from_slice(raw).map_err(|_| InventoryError::Shape)?;
    project_issues(rows)
}
fn listed_issues(raw: &[u8]) -> Result<Vec<Issue>, InventoryError> {
    if raw.len() > 8 * 1024 * 1024 {
        return Err(InventoryError::Limit);
    }
    let rows: Vec<RawIssue<ListedDependency>> =
        serde_json::from_slice(raw).map_err(|_| InventoryError::Shape)?;
    if rows.len() > TASK_CAP {
        return Err(InventoryError::Limit);
    }
    let mut normalized = Vec::with_capacity(rows.len());
    for r in rows {
        if r.dependencies.len() > TASK_CAP {
            return Err(InventoryError::Limit);
        }
        let mut dependencies = vec![];
        for d in r.dependencies {
            if d.issue_id != r.id || !id(&d.depends_on_id) {
                return Err(InventoryError::Shape);
            }
            dependencies.push(Dependency {
                id: d.depends_on_id,
                dependency_type: d.dependency_type,
            });
        }
        normalized.push(RawIssue {
            id: r.id,
            status: r.status,
            assignee: r.assignee,
            created_by: r.created_by,
            issue_type: r.issue_type,
            updated_at: r.updated_at,
            acceptance_criteria: r.acceptance_criteria,
            dependencies,
        });
    }
    project_issues(normalized)
}
fn project_issues(rows: Vec<RawIssue>) -> Result<Vec<Issue>, InventoryError> {
    if rows.len() > TASK_CAP {
        return Err(InventoryError::Limit);
    }
    let mut ids = BTreeSet::new();
    let mut result = vec![];
    for r in rows {
        if !id(&r.id)
            || !ids.insert(r.id.clone())
            || !id(&r.created_by)
            || r.issue_type.len() > 32
            || r.issue_type.chars().any(char::is_control)
            || r.updated_at.len() > 40
            || chrono::DateTime::parse_from_rfc3339(&r.updated_at).is_err()
            || r.assignee
                .as_ref()
                .is_some_and(|s| !s.is_empty() && !crate::agent_loader::is_valid_seat_name(s))
            || r.dependencies.len() > TASK_CAP
            || r.acceptance_criteria.len() > 128 * 1024
        {
            return Err(InventoryError::Shape);
        }
        let mut parents = vec![];
        for d in r.dependencies {
            if d.dependency_type == "parent-child" {
                if !id(&d.id) || d.id == r.id || parents.contains(&d.id) {
                    return Err(InventoryError::Shape);
                }
                parents.push(d.id);
            }
        }
        if parents.len() > 1 {
            return Err(InventoryError::Shape);
        }
        parents.sort();
        result.push(Issue {
            id: r.id,
            status: r.status,
            assignee: r.assignee.filter(|s| !s.is_empty()),
            created_by: r.created_by,
            issue_type: r.issue_type,
            updated_at: r.updated_at,
            parents,
            acceptance_sha256: hash(&r.acceptance_criteria)?,
        });
    }
    Ok(result)
}
/// No Deserialize: metadata selector is derived from immutable team state.
pub(crate) struct BeadsEvidence {
    record: ArchiveRecord,
    rows: BTreeMap<String, Issue>,
    mission: BTreeSet<String>,
    pub(crate) record_sha256: String,
    pub(crate) inventory_sha256: String,
}
impl BeadsEvidence {
    pub(crate) fn counts(&self) -> (usize, usize) {
        (self.mission.len(), self.rows.len())
    }
    pub(crate) fn record(&self) -> &ArchiveRecord {
        &self.record
    }
    pub(crate) fn structural_blockers(&self, seats: &[String]) -> Vec<super::ArchiveBlocker> {
        let mut out = vec![];
        let mut add = |code: &str, reference: &str| {
            out.push(super::ArchiveBlocker {
                code: code.into(),
                reference: reference.into(),
            })
        };
        let record_ids: BTreeSet<_> = self
            .record
            .items
            .iter()
            .map(|i| i.task_id().to_string())
            .collect();
        for task in self.mission.iter().filter(|i| **i != self.record.epic_id) {
            if !record_ids.contains(task) {
                add("E_RECONCILIATION_COVERAGE", task);
            }
        }
        for item in &self.record.items {
            let task = item.task_id();
            let Some(row) = self.rows.get(task) else {
                add("E_RECONCILIATION_COVERAGE", task);
                continue;
            };
            if row.created_by != "glados" {
                add("E_CREATION_GATE_VIOLATION", task);
            }
            if row.status != Status::Closed
                && row.assignee.as_ref().is_some_and(|a| seats.contains(a))
            {
                add("E_UNFINISHED_SEAT_WORK", task);
            }
            match item {
                RecordDisposition::Completed { .. } if row.status != Status::Closed => {
                    add("E_COMPLETED_WITHOUT_EVIDENCE", task)
                }
                RecordDisposition::Cancelled { .. } if row.status != Status::Closed => {
                    add("E_CANCEL_UNAPPROVED", task)
                }
                RecordDisposition::Transferred {
                    owner,
                    task: destination,
                    ..
                } => {
                    if seats.contains(owner)
                        || row.assignee.as_deref() != Some(owner)
                        || self.mission.contains(task)
                        || !self.rows.contains_key(destination)
                    {
                        add("E_TRANSFER_UNACCEPTED", task);
                    }
                }
                _ => {}
            }
        }
        for review in &self.record.reviews {
            if !self.rows.get(&review.task_id).is_some_and(|r| {
                r.status == Status::Closed && r.assignee.as_deref() == Some(&review.reviewer)
            }) {
                add("E_REVIEW_MISSING", &review.task_id);
            }
        }
        for task in &self.mission {
            if *task != self.record.epic_id
                && self
                    .rows
                    .get(task)
                    .is_some_and(|r| r.status != Status::Closed && !r.parents.is_empty())
            {
                add("E_OPEN_CHILDREN", task);
            }
        }
        out
    }
}
trait ReadBeads {
    fn query(&mut self, args: &[String]) -> Result<Vec<u8>, InventoryError>;
}
struct NativeBeads<'a> {
    home: &'a Path,
    deadline: Instant,
    bytes: usize,
}
impl ReadBeads for NativeBeads<'_> {
    fn query(&mut self, args: &[String]) -> Result<Vec<u8>, InventoryError> {
        let c = native_command(self.home, args);
        // Shared native bounded subprocess handling; stderr is never surfaced.
        let bytes = crate::team_replacement::repository::bounded_command(c, self.deadline)
            .map_err(|_| InventoryError::Unavailable)?;
        self.bytes = self
            .bytes
            .checked_add(bytes.len())
            .ok_or(InventoryError::Limit)?;
        if self.bytes > 8 * 1024 * 1024 {
            return Err(InventoryError::Limit);
        }
        Ok(bytes)
    }
}
fn native_command(home: &Path, args: &[String]) -> std::process::Command {
    let mut c = std::process::Command::new("/opt/homebrew/bin/bd");
    c.args(args)
        .args(["--readonly", "--sandbox", "--json"])
        .env("BEADS_DIR", home.join(".aperture/.beads"))
        .env("BD_ACTOR", "watchdog")
        .env("BEADS_ACTOR", "watchdog")
        .env("PAGER", "cat")
        .env("NO_COLOR", "1");
    c
}

fn args(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| s.to_string()).collect()
}
fn read<C: ReadBeads>(c: &mut C, a: Vec<String>) -> Result<Vec<Issue>, InventoryError> {
    let mut bytes = c.query(&a)?;
    let parsed = issues(&bytes);
    bytes.fill(0);
    parsed
}
/// List is discovery only: bd show embeds the dependency records needed to
/// verify parent links. Do not infer a parent solely from the list selector.
fn discover<C: ReadBeads>(c: &mut C, a: Vec<String>) -> Result<Vec<Issue>, InventoryError> {
    let mut bytes = c.query(&a)?;
    let listed = listed_issues(&bytes);
    bytes.fill(0);
    let listed = listed?;
    if listed.is_empty() {
        return Ok(vec![]);
    }
    let mut a = vec!["show".to_string()];
    a.extend(listed.iter().map(|r| r.id.clone()));
    let shown = read(c, a)?;
    if listed.len() != shown.len() {
        return Err(InventoryError::Drift);
    }
    for l in &listed {
        if !shown.iter().any(|s| {
            s.id == l.id
                && s.status == l.status
                && s.assignee == l.assignee
                && s.created_by == l.created_by
                && s.issue_type == l.issue_type
                && s.updated_at == l.updated_at
        }) {
            return Err(InventoryError::Drift);
        }
    }
    Ok(shown)
}

fn insert(rows: &mut BTreeMap<String, Issue>, r: Issue) -> Result<(), InventoryError> {
    if let Some(prior) = rows.get(&r.id) {
        if *prior != r {
            return Err(InventoryError::Drift);
        }
    } else {
        rows.insert(r.id.clone(), r);
    }
    if rows.len() > TASK_CAP {
        return Err(InventoryError::Limit);
    }
    Ok(())
}
fn collect<C: ReadBeads>(
    c: &mut C,
    team: &str,
    g: u64,
    epic: &str,
    seats: &[String],
    now: i64,
    sentinels: &[String],
) -> Result<BeadsEvidence, InventoryError> {
    if !id(epic)
        || seats.is_empty()
        || seats.len() > 32
        || seats
            .iter()
            .any(|s| !crate::agent_loader::is_valid_seat_name(s))
    {
        return Err(InventoryError::Binding);
    }
    let mut raw = c.query(&args(&["show", epic]))?;
    let record = super::record::parse_epic_record(&raw, team, g, epic, now, sentinels).map_err(
        |e| match e {
            super::record::RecordError::Missing => InventoryError::RecordMissing,
            _ => InventoryError::Record,
        },
    );
    let root = issues(&raw);
    raw.fill(0);
    let record = record?;
    let root = root?;
    if root.len() != 1 || root[0].id != epic || root[0].issue_type != "epic" {
        return Err(InventoryError::Binding);
    }
    let mut rows = BTreeMap::new();
    insert(&mut rows, root[0].clone())?;
    let mut mission = BTreeSet::from([epic.to_string()]);
    let mut queue = VecDeque::from([epic.to_string()]);
    while let Some(parent) = queue.pop_front() {
        for row in discover(
            c,
            args(&[
                "list",
                "--all",
                "--flat",
                "--include-gates",
                "--parent",
                &parent,
                "--limit",
                "257",
                "--no-pager",
            ]),
        )? {
            if row.parents != vec![parent.clone()] {
                return Err(InventoryError::Drift);
            }
            if !mission.insert(row.id.clone()) {
                return Err(InventoryError::Shape);
            }
            queue.push_back(row.id.clone());
            insert(&mut rows, row)?;
        }
    }
    for seat in seats {
        for row in discover(
            c,
            args(&[
                "list",
                "--all",
                "--flat",
                "--include-gates",
                "--assignee",
                seat,
                "--limit",
                "257",
                "--no-pager",
            ]),
        )? {
            if row.assignee.as_deref() != Some(seat) {
                return Err(InventoryError::Drift);
            }
            mission.insert(row.id.clone());
            insert(&mut rows, row)?;
        }
    }
    let mut referenced = BTreeSet::new();
    for item in &record.items {
        referenced.insert(item.task_id().to_string());
        if let RecordDisposition::Transferred { task, .. } = item {
            referenced.insert(task.clone());
        }
    }
    for r in &record.reviews {
        referenced.insert(r.task_id.clone());
    }
    let missing: Vec<_> = referenced
        .difference(&rows.keys().cloned().collect())
        .cloned()
        .collect();
    if !missing.is_empty() {
        let mut a = vec!["show".to_string()];
        a.extend(missing.iter().cloned());
        let more = read(c, a)?;
        if more.iter().map(|r| r.id.clone()).collect::<BTreeSet<_>>()
            != missing.into_iter().collect()
        {
            return Err(InventoryError::Drift);
        }
        for row in more {
            insert(&mut rows, row)?;
        }
    }
    let record_sha256 = record.sha256().map_err(|_| InventoryError::Record)?;
    let inventory_sha256 = hash(&(team, g, epic, &mission, &rows))?;
    Ok(BeadsEvidence {
        record,
        rows,
        mission,
        record_sha256,
        inventory_sha256,
    })
}
fn stable<C: ReadBeads>(
    c: &mut C,
    team: &str,
    g: u64,
    epic: &str,
    seats: &[String],
    now: i64,
    sentinels: &[String],
) -> Result<BeadsEvidence, InventoryError> {
    let first = collect(c, team, g, epic, seats, now, sentinels)?;
    let second = collect(c, team, g, epic, seats, now, sentinels)?;
    if first.record_sha256 != second.record_sha256
        || first.inventory_sha256 != second.inventory_sha256
    {
        return Err(InventoryError::Drift);
    }
    Ok(second)
}
/// Exactly this native team's epic/seats; no caller-provided inventory/path.
/// Two matching reads are stability evidence, NOT a transactional DB snapshot.
pub(crate) fn collect_native(
    home: &Path,
    team: &str,
    g: u64,
    epic: &str,
    sentinels: &[String],
) -> Result<BeadsEvidence, InventoryError> {
    use crate::journal::read_private_json;
    if !crate::agent_loader::is_valid_seat_name(team) || team.len() > 16 {
        return Err(InventoryError::Binding);
    }
    let dir = home.join(".aperture/teams").join(team);
    let snapshot: crate::teams::TeamSnapshot =
        read_private_json(&dir.join("team.json")).map_err(|_| InventoryError::Binding)?;
    let state: crate::teams::TeamStateFile =
        read_private_json(&dir.join("state.json")).map_err(|_| InventoryError::Binding)?;
    if snapshot.team != team
        || state.state != crate::teams::TeamLifecycle::Active
        || state.generation != g
        || state.epic_id.as_deref() != Some(epic)
    {
        return Err(InventoryError::Binding);
    }
    let seats: Vec<_> = snapshot.seats.iter().map(|s| s.name.clone()).collect();
    let now = chrono::Utc::now().timestamp_millis();
    let mut c = NativeBeads {
        home,
        deadline: Instant::now() + Duration::from_secs(30),
        bytes: 0,
    };
    let second = stable(&mut c, team, g, epic, &seats, now, sentinels)?;
    let current: crate::teams::TeamSnapshot =
        read_private_json(&dir.join("team.json")).map_err(|_| InventoryError::Binding)?;
    let current_state: crate::teams::TeamStateFile =
        read_private_json(&dir.join("state.json")).map_err(|_| InventoryError::Binding)?;
    if current != snapshot || current_state != state {
        return Err(InventoryError::Drift);
    }
    Ok(second)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    const EPIC: &str = "aperture-epic";
    const WORK: &str = "aperture-work";
    const REVIEW: &str = "aperture-review";
    const SEAT: &str = "t1-worker";
    fn row(id: &str, assignee: &str, parent: Option<&str>) -> Value {
        json!({"id":id,"status":"closed","assignee":assignee,"created_by":"glados",
            "issue_type":if id==EPIC {"epic"} else {"task"},"updated_at":"2026-09-20T12:00:00Z",
            "acceptance_criteria":"bounded acceptance",
            "dependencies":parent.map(|p| vec![json!({"id":p,"dependency_type":"parent-child"})]).unwrap_or_default()})
    }
    fn record() -> Value {
        json!({"schema_version":1,"team":"t1","generation":1,"epic_id":EPIC,
            "items":[{"kind":"completed","task_id":WORK,"evidence_refs":["artifact:work-sha"]}],
            "reviews":[{"task_id":REVIEW,"reviewer":"izzy","verdict":"pass","at":"2026-09-20T12:00:00Z","evidence_ref":"beads:aperture-review"}],
            "metrics":[{"metric":"bounded outcome","observed_at":"2026-09-20T12:00:00Z","evidence_ref":"artifact:metric-sha"}]})
    }
    struct Fake {
        data: BTreeMap<String, Value>,
        calls: Vec<Vec<String>>,
        wrong_parent: bool,
        show_drift: bool,
        extra: bool,
        over_cap: bool,
        change_second: bool,
        epic_reads: usize,
    }
    impl Fake {
        fn new() -> Self {
            let mut epic = row(EPIC, "glados", None);
            epic["metadata"] = json!({"aperture_archive_v1": record()});
            Self {
                data: BTreeMap::from([
                    (EPIC.into(), epic),
                    (WORK.into(), row(WORK, SEAT, Some(EPIC))),
                    (REVIEW.into(), row(REVIEW, "izzy", None)),
                ]),
                calls: vec![],
                wrong_parent: false,
                show_drift: false,
                extra: false,
                over_cap: false,
                change_second: false,
                epic_reads: 0,
            }
        }
    }
    impl ReadBeads for Fake {
        fn query(&mut self, args: &[String]) -> Result<Vec<u8>, InventoryError> {
            self.calls.push(args.to_vec());
            let mut result = vec![];
            if args[0] == "show" {
                for key in &args[1..] {
                    let mut r = self
                        .data
                        .get(key)
                        .ok_or(InventoryError::Unavailable)?
                        .clone();
                    if key == EPIC {
                        self.epic_reads += 1;
                        if self.change_second && self.epic_reads == 2 {
                            r["metadata"]["aperture_archive_v1"]["metrics"][0]["metric"] =
                                "changed outcome".into();
                        }
                    }
                    if key == WORK {
                        if self.wrong_parent {
                            r["dependencies"] = json!([]);
                        }
                        if self.show_drift {
                            r["status"] = "open".into();
                        }
                    }
                    result.push(r);
                }
            } else {
                assert_eq!(args[0], "list");
                assert_eq!(&args[1..4], &["--all", "--flat", "--include-gates"]);
                assert_eq!(&args[6..], &["--limit", "257", "--no-pager"]);
                assert!(matches!(args[4].as_str(), "--parent" | "--assignee"));
                if (args[4] == "--parent" && args[5] == EPIC)
                    || (args[4] == "--assignee" && args[5] == SEAT)
                {
                    let mut r = self.data[WORK].clone();
                    r["dependencies"] =
                        json!([{"issue_id":WORK,"depends_on_id":EPIC,"type":"parent-child"}]);
                    result.push(r);
                    if self.extra {
                        result.push(row("aperture-extra", SEAT, None));
                    }
                    if self.over_cap {
                        result = (0..257)
                            .map(|i| row(&format!("task-{i}"), SEAT, None))
                            .collect();
                    }
                }
            }
            Ok(serde_json::to_vec(&result).unwrap())
        }
    }
    fn run(f: &mut Fake) -> Result<BeadsEvidence, InventoryError> {
        stable(f, "t1", 1, EPIC, &[SEAT.into()], i64::MAX, &[])
    }
    #[test]
    fn two_native_shaped_reads_hydrate_links_and_do_not_claim_acceptance() {
        let mut f = Fake::new();
        let e = run(&mut f).unwrap();
        assert_eq!(e.counts(), (2, 3));
        assert_eq!(e.record_sha256.len(), 64);
        assert_eq!(e.inventory_sha256.len(), 64);
        assert!(e.structural_blockers(&[SEAT.into()]).is_empty());
        assert_eq!(f.epic_reads, 2);
        assert_eq!(
            f.calls
                .iter()
                .filter(|a| a == &&args(&["show", WORK]))
                .count(),
            4
        );
        assert_eq!(
            f.calls
                .iter()
                .filter(|a| a == &&args(&["show", REVIEW]))
                .count(),
            2
        );
        assert!(f
            .calls
            .iter()
            .all(|a| matches!(a[0].as_str(), "show" | "list")));
        // Result is structural evidence only: no approval or archive actuator exists here.
    }
    #[test]
    fn command_uses_fixed_binding_and_readonly_flags_without_execution() {
        let c = native_command(Path::new("/private/fixture-home"), &args(&["show", EPIC]));
        assert_eq!(c.get_program(), "/opt/homebrew/bin/bd");
        let argv: Vec<_> = c.get_args().map(|x| x.to_str().unwrap()).collect();
        assert_eq!(
            argv,
            vec!["show", EPIC, "--readonly", "--sandbox", "--json"]
        );
        let env: BTreeMap<_, _> = c
            .get_envs()
            .map(|(k, v)| (k.to_str().unwrap(), v.unwrap().to_str().unwrap()))
            .collect();
        assert_eq!(env["BEADS_DIR"], "/private/fixture-home/.aperture/.beads");
        assert_eq!(env["BEADS_ACTOR"], "watchdog");
    }
    #[test]
    fn list_parent_selector_never_substitutes_for_show_link() {
        let mut f = Fake::new();
        f.wrong_parent = true;
        assert!(matches!(run(&mut f), Err(InventoryError::Drift)));
        assert_eq!(f.epic_reads, 1);
    }
    #[test]
    fn discovery_edges_use_list_wire_format_not_show_issue_format() {
        let mut r = row(WORK, SEAT, None);
        r["dependencies"] = json!([{"issue_id":WORK,"depends_on_id":EPIC,"type":"parent-child",
            "created_by":"glados","created_at":"2026-09-20T12:00:00Z","metadata":{}}]);
        let bytes = serde_json::to_vec(&vec![r.clone()]).unwrap();
        assert!(
            matches!(issues(&bytes), Err(InventoryError::Shape)),
            "old show parser reproduces installed failure"
        );
        let listed = listed_issues(&bytes).unwrap();
        assert_eq!(listed[0].parents, vec![EPIC]);
        assert!(listed_issues(b"[]").unwrap().is_empty());
        assert!(matches!(listed_issues(b"null"), Err(InventoryError::Shape)));
        for field in ["issue_id", "depends_on_id", "type"] {
            let mut bad = r.clone();
            bad["dependencies"][0]
                .as_object_mut()
                .unwrap()
                .remove(field);
            assert!(matches!(
                listed_issues(&serde_json::to_vec(&vec![bad]).unwrap()),
                Err(InventoryError::Shape)
            ));
        }
        r["dependencies"][0]["issue_id"] = "aperture-foreign".into();
        assert!(matches!(
            listed_issues(&serde_json::to_vec(&vec![r]).unwrap()),
            Err(InventoryError::Shape)
        ));
        let show = serde_json::to_vec(&vec![row(WORK, SEAT, Some(EPIC))]).unwrap();
        assert!(issues(&show).is_ok());
        assert!(matches!(listed_issues(&show), Err(InventoryError::Shape)));
    }
    #[test]
    fn list_show_or_second_projection_drift_fail_closed() {
        let mut f = Fake::new();
        f.show_drift = true;
        assert!(matches!(run(&mut f), Err(InventoryError::Drift)));
        let mut f = Fake::new();
        f.change_second = true;
        assert!(matches!(run(&mut f), Err(InventoryError::Drift)));
    }
    #[test]
    fn cardinality_sentinel_stops_before_show_or_second_collection() {
        let mut f = Fake::new();
        f.over_cap = true;
        assert!(matches!(run(&mut f), Err(InventoryError::Limit)));
        assert_eq!(f.calls.len(), 2);
        let raw = serde_json::to_vec(&vec![row(WORK, SEAT, None); 2]).unwrap();
        assert!(matches!(issues(&raw), Err(InventoryError::Shape)));
    }
    #[test]
    fn coverage_and_status_are_required_but_not_acceptance_truth() {
        let mut f = Fake::new();
        f.data.get_mut(EPIC).unwrap()["metadata"]["aperture_archive_v1"]["items"][0]["task_id"] =
            REVIEW.into();
        let e = run(&mut f).unwrap();
        assert!(e
            .structural_blockers(&[SEAT.into()])
            .iter()
            .any(|b| b.code == "E_RECONCILIATION_COVERAGE"));
        let mut f = Fake::new();
        f.data.get_mut(WORK).unwrap()["status"] = "open".into();
        let e = run(&mut f).unwrap();
        let b = e.structural_blockers(&[SEAT.into()]);
        for code in [
            "E_UNFINISHED_SEAT_WORK",
            "E_COMPLETED_WITHOUT_EVIDENCE",
            "E_OPEN_CHILDREN",
        ] {
            assert!(b.iter().any(|b| b.code == code));
        }
    }
    #[test]
    fn missing_record_or_unknown_status_never_falls_back_to_notes() {
        let mut f = Fake::new();
        f.data.get_mut(EPIC).unwrap()["metadata"] = json!({});
        f.data.get_mut(EPIC).unwrap()["notes"] = record().to_string().into();
        let error = run(&mut f).err().unwrap();
        assert_eq!(error, InventoryError::RecordMissing);
        assert_eq!(error.code(), "E_RECONCILIATION_RECORD_MISSING");
        assert!(error.message().contains("GLaDOS"));
        assert_ne!(error.code(), InventoryError::Unavailable.code());
        let mut f = Fake::new();
        f.data.get_mut(WORK).unwrap()["status"] = "unknown".into();
        assert!(matches!(run(&mut f), Err(InventoryError::Shape)));
    }
    #[test]
    fn malformed_record_is_not_missing_or_inventory_unavailable() {
        let mut f = Fake::new();
        f.data.get_mut(EPIC).unwrap()["metadata"]["aperture_archive_v1"] =
            json!({"schema_version":99});
        let error = run(&mut f).err().unwrap();
        assert_eq!(error, InventoryError::Record);
        assert_eq!(error.code(), "E_RECONCILIATION_INVALID");
        assert!(error.message().contains("invalid"));
        assert!(InventoryError::Unavailable.message().contains("BEADS"));
    }
    #[test]
    fn caller_selector_injection_fails_before_any_read() {
        let mut f = Fake::new();
        assert!(matches!(
            collect(&mut f, "t1", 1, "--help", &[SEAT.into()], i64::MAX, &[]),
            Err(InventoryError::Binding)
        ));
        assert!(f.calls.is_empty());
    }
}
