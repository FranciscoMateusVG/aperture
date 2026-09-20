//! No real OS process enumeration/argument reads/signals. Synthetic native seam.
use super::*;
use crate::owner::ProcessIdentity as StoredProcess;
fn id(pid: u32) -> ProcessIdentity {
    identity_from_owner(pid, 1_000_001).unwrap()
}
fn meta(pid: u32, ppid: u32) -> ProcessMetadata {
    ProcessMetadata {
        identity: id(pid),
        ppid,
        pgid: 50,
        uid: unsafe { libc::geteuid() },
    }
}
fn owner() -> OwnerRecord {
    serde_json::from_value(serde_json::json!({"schema_version":1,"seat":"t1-worker","generation":1,"state":"active","since":"2026-09-20T00:00:00Z","writer":"launcher","reservation_nonce_sha256":null,"requested":{"harness":"codex","model":"gpt-6-astra","reasoning":null},"incarnation":{"pid":100,"start_time":1000001,"thread_id":"fixture-thread","token_id":"fixture-token","harness":"codex","model":"gpt-6-astra","reasoning":null,"observed":true,"processes":[{"pid":100,"start_time":1000001,"ppid":50,"pgid":50,"cmdline_sha256":"a".repeat(64),"cwd":"/fixture/owned"}]}})).unwrap()
}
struct Fake {
    tables: Vec<Vec<ProcessMetadata>>,
    control: u32,
    states: HashMap<u32, ProcessState>,
    details: HashMap<u32, (String, String)>,
    reads: Vec<u32>,
    expired: bool,
    recycle_after_details: Option<u32>,
}
impl Fake {
    fn new(table: Vec<ProcessMetadata>) -> Self {
        Self {
            tables: vec![table.clone(), table],
            control: 999,
            states: HashMap::new(),
            details: HashMap::new(),
            reads: vec![],
            expired: false,
            recycle_after_details: None,
        }
    }
}
impl ProcessSource for Fake {
    fn table(&mut self) -> Result<Vec<ProcessMetadata>, ReplacementError> {
        Ok(self.tables.remove(0))
    }
    fn details(&mut self, id: &ProcessIdentity) -> Result<(String, String), ReplacementError> {
        self.reads.push(id.pid);
        if self.recycle_after_details == Some(id.pid) {
            self.states.insert(id.pid, ProcessState::Recycled);
        }
        Ok(self
            .details
            .get(&id.pid)
            .cloned()
            .unwrap_or(("a".repeat(64), "/fixture/owned".into())))
    }
    fn observe(&mut self, id: &ProcessIdentity) -> ProcessState {
        self.states
            .get(&id.pid)
            .copied()
            .unwrap_or(ProcessState::Same)
    }
    fn control(&mut self) -> Result<ProcessIdentity, ReplacementError> {
        Ok(id(self.control))
    }
    fn deadline(&self) -> Result<(), ReplacementError> {
        if self.expired {
            Err(ReplacementError::StopUnverified)
        } else {
            Ok(())
        }
    }
}
#[test]
fn native_shape_hydrates_descendants_without_spawning_or_signaling() {
    let mut f = Fake::new(vec![meta(100, 50), meta(101, 100), meta(102, 101)]);
    let s = collect(&owner(), &mut f).unwrap();
    assert!(s.complete);
    assert_eq!(s.processes.len(), 3);
    assert_eq!(f.reads, vec![100, 101, 102]);
    assert!(s.unowned_matches.is_empty());
}
#[test]
fn reparented_persisted_child_remains_owned_after_root_gone() {
    let mut o = owner();
    o.incarnation
        .as_mut()
        .unwrap()
        .processes
        .push(StoredProcess {
            pid: 101,
            start_time: 1_000_001,
            ppid: 100,
            pgid: 50,
            cmdline_sha256: "a".repeat(64),
            cwd: "/fixture/owned".into(),
        });
    let mut f = Fake::new(vec![meta(101, 1)]);
    f.states.insert(100, ProcessState::Gone);
    let s = collect(&o, &mut f).unwrap();
    assert_eq!(s.processes.len(), 2);
    assert!(s.processes.iter().any(|p| p.identity.pid == 100));
    assert!(s.processes.iter().any(|p| p.identity.pid == 101));
    assert_eq!(f.reads, vec![101]);
}
#[test]
fn recycled_pid_unreadable_or_partial_table_fails_closed() {
    let mut f = Fake::new(vec![meta(100, 50)]);
    f.states.insert(100, ProcessState::Recycled);
    assert!(collect(&owner(), &mut f).is_err());
    let mut f = Fake::new(vec![meta(100, 50)]);
    f.states.insert(100, ProcessState::Unreadable);
    assert!(collect(&owner(), &mut f).is_err());
    let mut f = Fake::new(vec![meta(100, 50), meta(100, 50)]);
    assert!(collect(&owner(), &mut f).is_err());
    let mut f = Fake::new(vec![meta(100, 50)]);
    f.recycle_after_details = Some(100);
    assert!(collect(&owner(), &mut f).is_err());
}
#[test]
fn changed_topology_and_new_unchecked_outsider_block_snapshot() {
    for parent in [100, 1] {
        let mut f = Fake::new(vec![meta(100, 50)]);
        f.tables[1].push(meta(101, parent));
        assert!(collect(&owner(), &mut f).is_err());
    }
}
#[test]
fn control_in_stop_set_denied_before_private_argument_reads() {
    let mut f = Fake::new(vec![meta(100, 50), meta(101, 100)]);
    f.control = 101;
    assert!(matches!(
        collect(&owner(), &mut f),
        Err(ReplacementError::UnownedProcess)
    ));
    assert!(f.reads.is_empty());
}
#[test]
fn cwd_or_command_match_is_only_a_blocker_never_added_to_owned() {
    let mut f = Fake::new(vec![meta(100, 50), meta(200, 1)]);
    let s = collect(&owner(), &mut f).unwrap();
    assert_eq!(s.processes.len(), 1);
    assert_eq!(s.unowned_matches, vec![id(200)]);
    let mut f = Fake::new(vec![meta(100, 50), meta(200, 1)]);
    f.details.insert(200, ("b".repeat(64), "/unrelated".into()));
    assert!(collect(&owner(), &mut f)
        .unwrap()
        .unowned_matches
        .is_empty());
}
fn args(argv: &[&[u8]], env: &[u8]) -> Vec<u8> {
    let mut b = (argv.len() as i32).to_ne_bytes().to_vec();
    b.extend_from_slice(b"/fixture/bin\0\0\0");
    for a in argv {
        b.extend_from_slice(a);
        b.push(0);
    }
    b.extend_from_slice(env);
    b
}
#[test]
fn argument_digest_excludes_environment_and_is_length_delimited() {
    let a = args(
        &[b"fixture", b"--flag", b"value"],
        b"SENTINEL=private-one\0",
    );
    let b = args(
        &[b"fixture", b"--flag", b"value"],
        b"SENTINEL=private-two\0",
    );
    assert_eq!(argv_digest(&a).unwrap(), argv_digest(&b).unwrap());
    assert_ne!(
        argv_digest(&args(&[b"a", b"bc"], b"")),
        argv_digest(&args(&[b"ab", b"c"], b""))
    );
    assert_eq!(argv_digest(&a).unwrap().len(), 64);
}
#[test]
fn argv_malformed_caps_and_deadline_are_not_empty_success() {
    for bad in [
        vec![],
        vec![0; 5],
        args(&[b"one"], b"")[..5].to_vec(),
        vec![0; MAX_ARGS + 1],
    ] {
        assert!(argv_digest(&bad).is_err());
    }
    let mut bad = args(&[b"one"], b"");
    bad[..4].copy_from_slice(&((MAX_ARGC + 1) as i32).to_ne_bytes());
    assert!(argv_digest(&bad).is_err());
    let mut f = Fake::new(vec![meta(100, 50)]);
    f.expired = true;
    assert!(collect(&owner(), &mut f).is_err());
    assert!(f.reads.is_empty());
}

#[test]
fn refreshed_depths_preserve_children_first_and_missing_live_row_blocks() {
    let mut o = owner();
    for (pid, ppid) in [(101, 100), (102, 101)] {
        o.incarnation
            .as_mut()
            .unwrap()
            .processes
            .push(StoredProcess {
                pid,
                start_time: 1_000_001,
                ppid,
                pgid: 50,
                cmdline_sha256: "a".repeat(64),
                cwd: "/fixture/owned".into(),
            });
    }
    let mut f = Fake::new(vec![meta(100, 50), meta(101, 100), meta(102, 101)]);
    let s = collect(&o, &mut f).unwrap();
    assert_eq!(
        s.processes
            .iter()
            .find(|p| p.identity.pid == 102)
            .unwrap()
            .depth,
        2
    );
    let mut f = Fake::new(vec![]);
    assert!(collect(&owner(), &mut f).is_err());
    let mut f = Fake::new(vec![meta(100, 50)]);
    f.tables[1].clear();
    assert!(collect(&owner(), &mut f).is_err());
}
