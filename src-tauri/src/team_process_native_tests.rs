//! Synthetic collector cases plus explicitly read-only macOS metadata/SDK oracles.
//! No argument/environment reads or process signals in these tests.
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
    on_details: Option<Box<dyn FnOnce()>>,
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
            on_details: None,
        }
    }
}
impl ProcessSource for Fake {
    fn table(&mut self) -> Result<Vec<ProcessMetadata>, ReplacementError> {
        Ok(self.tables.remove(0))
    }
    fn details(&mut self, id: &ProcessIdentity) -> Result<(String, String), ReplacementError> {
        self.reads.push(id.pid);
        if let Some(effect) = self.on_details.take() {effect();}
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
fn foreign_rows_remain_in_topology_but_never_grant_signal_authority() {
    let mut outsider = meta(200, 1);
    outsider.uid = unsafe { libc::geteuid() }.wrapping_add(1);
    let mut f = Fake::new(vec![meta(100, 50), outsider.clone()]);
    let snapshot = collect(&owner(), &mut f).unwrap();
    assert_eq!(snapshot.processes.len(), 1);
    assert_eq!(f.reads, vec![100]); // no foreign arguments/environment reads
    outsider.ppid = 100;
    let mut f = Fake::new(vec![meta(100, 50), outsider]);
    assert!(matches!(collect(&owner(), &mut f), Err(ReplacementError::StopUnverified)));
    assert!(f.reads.is_empty()); // foreign descendant blocks before details/effects
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

#[cfg(all(target_os = "macos", target_pointer_width = "64", target_endian = "little",
    any(target_arch = "aarch64", target_arch = "x86_64")))]
mod darwin_table {
    use super::*;
    use process_table::*;

    fn row(pid: u32, uid: u32) -> Vec<u8> {
        let mut b = vec![0; ROW_SIZE];
        for (at, value) in [(PID, pid), (UID, uid), (PPID, 1), (PGID, pid), (START_USEC, 123456)] {
            b[at..at+4].copy_from_slice(&value.to_ne_bytes());
        }
        b[START_SEC..START_SEC+8].copy_from_slice(&123i64.to_ne_bytes());
        b
    }
    fn parse(b: &[u8]) -> Result<Vec<ProcessMetadata>, ReplacementError> {
        decode(b, b.len()+ROW_SIZE, Instant::now()+Duration::from_secs(2))
    }
    #[test]
    fn complete_table_preserves_foreign_metadata_and_birth() {
        let uid = unsafe { libc::geteuid() };
        let mut b = row(0, 0);
        b.extend(row(1, 0));
        b.extend(row(20, uid));
        b.extend(row(21, uid.wrapping_add(1)));
        let t = parse(&b).unwrap();
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].identity.start_time, "123.123456");
        assert_eq!(t[1].uid, uid.wrapping_add(1));
        assert_eq!(t[1].ppid, 1);
        assert_eq!(t[1].pgid, 21);
    }
    #[test]
    fn incomplete_duplicate_overflow_and_deadline_fail_closed() {
        let b = row(20, 0);
        assert!(parse(&[]).is_err());
        assert!(parse(&b[..ROW_SIZE-1]).is_err());
        assert!(decode(&b, b.len(), Instant::now()+Duration::from_secs(1)).is_err());
        assert!(decode(&b, b.len()+ROW_SIZE, Instant::now()).is_err());
        let mut duplicate = b.clone(); duplicate.extend(&b);
        assert!(parse(&duplicate).is_err());
        let huge = vec![0; (MAX_PIDS+1)*ROW_SIZE];
        assert!(parse(&huge).is_err());
        for (at, value) in [(PID, u32::MAX), (PPID, u32::MAX), (PGID, u32::MAX),
            (START_USEC, 1_000_000), (START_USEC, u32::MAX)] {
            let mut bad = b.clone(); bad[at..at+4].copy_from_slice(&value.to_ne_bytes());
            assert!(parse(&bad).is_err());
        }
        for sec in [0i64, -1, i64::MAX] {
            let mut bad=b.clone(); bad[START_SEC..START_SEC+8].copy_from_slice(&sec.to_ne_bytes());
            assert!(parse(&bad).is_err());
        }
    }
    #[test]
    fn sdk_kinfo_layout_matches_decoder() {
        use std::os::unix::fs::PermissionsExt;
        let dir=std::env::temp_dir().join(format!("aperture-kinfo-sdk-{}",uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir,std::fs::Permissions::from_mode(0o700)).unwrap();
        let source=dir.join("layout.c"); let binary=dir.join("layout");
        std::fs::write(&source,r#"
#include <sys/types.h>
#include <sys/sysctl.h>
#include <stddef.h>
#include <stdio.h>
int main(void) {
 printf("%zu %zu %zu %zu %zu %zu %zu %zu %zu %zu %zu %zu %zu %zu %d %d %d\n",
 sizeof(struct kinfo_proc), _Alignof(struct kinfo_proc),
 offsetof(struct kinfo_proc,kp_proc.p_pid),
 offsetof(struct kinfo_proc,kp_proc.p_starttime.tv_sec),
 offsetof(struct kinfo_proc,kp_proc.p_starttime.tv_usec),
 offsetof(struct kinfo_proc,kp_eproc.e_ucred.cr_uid),
 offsetof(struct kinfo_proc,kp_eproc.e_ppid),
 offsetof(struct kinfo_proc,kp_eproc.e_pgid),
 sizeof(((struct kinfo_proc*)0)->kp_proc.p_pid),
 sizeof(((struct kinfo_proc*)0)->kp_proc.p_starttime.tv_sec),
 sizeof(((struct kinfo_proc*)0)->kp_proc.p_starttime.tv_usec),
 sizeof(((struct kinfo_proc*)0)->kp_eproc.e_ucred.cr_uid),
 sizeof(((struct kinfo_proc*)0)->kp_eproc.e_ppid),
 sizeof(((struct kinfo_proc*)0)->kp_eproc.e_pgid), CTL_KERN,KERN_PROC,KERN_PROC_ALL);
 return 0;
}
"#).unwrap();
        let compile=std::process::Command::new("/usr/bin/cc").arg(&source).arg("-o").arg(&binary).output().unwrap();
        assert!(compile.status.success(),"SDK layout oracle compilation failed");
        let result=std::process::Command::new(&binary).output().unwrap();
        assert!(result.status.success());
        let actual:Vec<usize>=std::str::from_utf8(&result.stdout).unwrap().split_whitespace().map(|s|s.parse().unwrap()).collect();
        assert_eq!(actual,vec![ROW_SIZE,8,PID,START_SEC,START_USEC,UID,PPID,PGID,4,8,4,4,4,4,
            libc::CTL_KERN as usize,libc::KERN_PROC as usize,libc::KERN_PROC_ALL as usize]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
    #[test]
    fn native_readonly_table_agrees_with_accessible_bsd_metadata() {
        let table=native_table(Instant::now()+Duration::from_secs(5)).unwrap();
        validate_table(&table).unwrap();
        let own=observe(std::process::id()).unwrap().unwrap();
        assert!(table.iter().any(|p|p.identity==own.identity && p.uid==own.uid && p.ppid==own.ppid && p.pgid==own.pgid));
        let mut compared=0; let mut denied=0; let mut gone=0; let mut recycled=0;
        for p in &table {
            let mut info=std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
            let size=std::mem::size_of::<libc::proc_bsdinfo>();
            let n=unsafe{libc::proc_pidinfo(p.identity.pid as i32,libc::PROC_PIDTBSDINFO,0,info.as_mut_ptr().cast(),size as i32)};
            if n==0 {
                match std::io::Error::last_os_error().raw_os_error() {
                    Some(libc::EPERM)=>{denied+=1;continue;},
                    Some(libc::ESRCH)=>{gone+=1;continue;},
                    _=>panic!("unexpected metadata API error"),
                }
            }
            assert_eq!(n,size as i32);
            let b=unsafe{info.assume_init()};
            let birth=format!("{}.{:06}",b.pbi_start_tvsec,b.pbi_start_tvusec);
            if birth!=p.identity.start_time {recycled+=1;continue;}
            assert_eq!((b.pbi_pid,b.pbi_uid,b.pbi_ppid,b.pbi_pgid),(p.identity.pid,p.uid,p.ppid,p.pgid));
            compared+=1;
        }
        assert!(compared>0);
        // This oracle counts inaccessible comparisons, NOT dropped table rows.
        assert_eq!(compared+denied+gone+recycled,table.len());
        println!("readonly table={} compared={} privileged_denied={} gone={} recycled={}",table.len(),compared,denied,gone,recycled);
    }
}

/// Explicitly authorized one-shot diagnostic; never run in routine suites.
/// The target is fixed, not a caller-supplied process or ownership assertion.
#[cfg(target_os = "macos")]
#[test]
#[ignore = "operator-authorized RO Sonnet diagnostic only; reads real HOME/process metadata"]
fn sonnet_diagnostic_collect_readonly_once() {
    let home = std::env::var_os("HOME").expect("HOME unavailable");
    match collect_native(Path::new(&home), "sonnet-smoke", "sonnet-smoke-qa", 1) {
        Ok(snapshot) => {
            let gone = snapshot.processes.iter()
                .filter(|p| state(&p.identity) == ProcessState::Gone).count();
            println!("diagnostic_collect=OK complete={} owned_count={} gone_count={} unowned_match_count={}",
                snapshot.complete, snapshot.processes.len(), gone, snapshot.unowned_matches.len());
            // Collection is evidence only; unowned matches remain blockers for
            // downstream policy. No persistence/stop/revoke follows this test.
        }
        Err(error) => {
            println!("diagnostic_collect={}", error.code());
            panic!("read-only collector returned fixed error {}", error.code());
        }
    }
}


// Native private-file/classifier composition, fake process metadata only.
// No real HOME, process enumeration, argv/env, signals or provider calls.
struct PeerFixture {
    home: std::path::PathBuf,
    target: OwnerRecord,
    peer: OwnerRecord,
}
fn peer_write<T: serde::Serialize>(path: &Path, value: &T) {
    crate::journal::ensure_private_dir(path.parent().unwrap()).unwrap();
    crate::journal::write_private_json_atomic(path, value, true).unwrap();
}
impl Drop for PeerFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.home);
    }
}
impl PeerFixture {
    fn new() -> Self {
        let home =
            std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("aperture-peer-process-{}", uuid::Uuid::new_v4()));
        let mut target = owner();
        target.requested.reasoning = Some(crate::state::ReasoningEffort::High);
        target.incarnation.as_mut().unwrap().reasoning = target.requested.reasoning.clone();
        target.incarnation.as_mut().unwrap().token_id = "a".repeat(64);
        let mut peer = target.clone();
        peer.seat = "t1-other".into();
        let inc = peer.incarnation.as_mut().unwrap();
        inc.pid = 200;
        inc.processes[0].pid = 200;
        inc.thread_id = "other-thread".into();
        inc.token_id = "b".repeat(64);
        for o in [&target, &peer] {
            peer_write(
                &home
                    .join(".aperture/run/owner")
                    .join(format!("{}.json", o.seat)),
                o,
            );
        }
        let snapshot = serde_json::json!({"schema_version":1,"team":"t1","project":"project:aperture","repo":"aperture","mission":"fixture","acceptance":"fixture","preset":{"id":null,"sha256":null},"lead":"t1-worker","seats":[{"name":"t1-worker","role":"qa","harness":"codex","model":"gpt-6-astra","reasoning":"high"},{"name":"t1-other","role":"frontend","harness":"codex","model":"gpt-6-astra","reasoning":"high"}],"fallbacks":[],"grants":[],"created_at":"2026-09-20T00:00:00Z","creation_request_id":uuid::Uuid::new_v4().to_string(),"staging_uuid":uuid::Uuid::new_v4().to_string()});
        peer_write(&home.join(".aperture/teams/t1/team.json"), &snapshot);
        peer_write(
            &home.join(".aperture/teams/t1/state.json"),
            &serde_json::json!({"schema_version":1,"state":"active","generation":1,"epic_id":"aperture-fixture","failure":null,"updated_at":"2026-09-20T00:00:00Z"}),
        );
        for name in [&target.seat, &peer.seat] {
            for leaf in ["TEAM", ".complete"] {
                let p = home.join(".claude/aperture").join(name).join(leaf);
                crate::journal::ensure_private_dir(p.parent().unwrap()).unwrap();
                crate::journal::write_private_bytes_atomic(&p, b"fixture", false).unwrap();
            }
        }
        Self { home, target, peer }
    }
    fn load(&self) -> Result<PeerBindings, ReplacementError> {
        PeerBindings::load(
            &self.home,
            &self.target,
            Instant::now() + Duration::from_secs(10),
        )
    }
    fn save_peer(&self) {
        peer_write(
            &self.home.join(".aperture/run/owner/t1-other.json"),
            &self.peer,
        );
    }
}
fn sibling_table() -> Vec<ProcessMetadata> {
    vec![
        meta(100, 50),
        meta(101, 100),
        meta(200, 1),
        meta(201, 200),
        meta(202, 201),
    ]
}

#[test]
fn peer_native_binding_removes_only_proven_sibling_matches_never_target_authority() {
    let f = PeerFixture::new();
    let peers = f.load().unwrap();
    let old = collect(&f.target, &mut Fake::new(sibling_table())).unwrap();
    let new = collect_with_peers(&f.target, &mut Fake::new(sibling_table()), Some(&peers)).unwrap();
    assert_eq!(old.processes, new.processes);
    assert_eq!(old.unowned_matches, vec![id(200), id(201), id(202)]);
    assert!(new.unowned_matches.is_empty());
    let mut table = sibling_table();
    table.push(meta(300, 1));
    let snapshot = collect_with_peers(&f.target, &mut Fake::new(table), Some(&peers)).unwrap();
    assert_eq!(snapshot.processes, new.processes);
    assert_eq!(snapshot.unowned_matches, vec![id(300)]);
    // No-home/locked caller remains conservative by construction.
    assert_eq!(
        collect(&f.target, &mut Fake::new(sibling_table()))
            .unwrap()
            .unowned_matches,
        old.unowned_matches
    );
}

#[test]
fn peer_root_reuse_or_topology_drift_is_not_an_exemption() {
    for case in 0..5 {
        let f = PeerFixture::new();
        let peers = f.load().unwrap();
        let mut source = Fake::new(sibling_table());
        match case {
            0 => source.tables[0][2].identity.start_time = "2.000001".into(),
            1 => source.tables[1][3].ppid = 1,
            2 => {
                source.states.insert(200, ProcessState::Unreadable);
            }
            3 => {
                source.tables[1].retain(|p| p.identity.pid != 200);
                source.states.insert(200, ProcessState::Gone);
            }
            _ => source.tables[1].push(meta(203, 200)),
        }
        assert!(collect_with_peers(&f.target, &mut source, Some(&peers)).is_err());
    }
}

#[test]
fn peer_target_or_peer_peer_overlap_is_denied() {
    let f = PeerFixture::new();
    let peers = f.load().unwrap();
    let mut table = sibling_table();
    table[2].ppid = 100;
    assert!(matches!(
        collect_with_peers(&f.target, &mut Fake::new(table), Some(&peers)),
        Err(ReplacementError::UnownedProcess)
    ));
    let mut f = PeerFixture::new();
    f.peer
        .incarnation
        .as_mut()
        .unwrap()
        .processes
        .push(f.target.incarnation.as_ref().unwrap().processes[0].clone());
    f.save_peer();
    let peers = f.load().unwrap();
    assert!(collect_with_peers(&f.target, &mut Fake::new(sibling_table()), Some(&peers)).is_err());
    let f = PeerFixture::new();
    let mut peers = f.load().unwrap();
    let mut duplicate = f.peer.clone();
    duplicate.seat = "t1-third".into();
    peers.peers.push(PeerBinding {
        owner: duplicate,
        team: "t1".into(),
        team_generation: 1,
    });
    assert!(collect_with_peers(&f.target, &mut Fake::new(sibling_table()), Some(&peers)).is_err());
}

#[test]
fn peer_owner_snapshot_marker_or_directory_drift_during_collection_denies() {
    for case in 0..5 {
        let f = PeerFixture::new();
        let peers = f.load().unwrap();
        let home = f.home.clone();
        let mut source = Fake::new(sibling_table());
        source.on_details = Some(Box::new(move || {
            let ownerpath = home.join(".aperture/run/owner/t1-other.json");
            match case {
                0 => {
                    let mut owner: OwnerRecord =
                        crate::journal::read_private_json(&ownerpath).unwrap();
                    owner.generation += 1;
                    peer_write(&ownerpath, &owner);
                }
                1 => {
                    let p = home.join(".aperture/teams/t1/team.json");
                    let mut v: serde_json::Value = crate::journal::read_private_json(&p).unwrap();
                    v["mission"] = "changed".into();
                    peer_write(&p, &v);
                }
                2 => {
                    let p = home.join(".aperture/teams/t1/state.json");
                    let mut v: serde_json::Value = crate::journal::read_private_json(&p).unwrap();
                    v["state"] = "failed".into();
                    peer_write(&p, &v);
                }
                3 => {
                    std::fs::remove_file(home.join(".claude/aperture/t1-other/TEAM")).unwrap();
                }
                _ => {
                    let mut owner: OwnerRecord =
                        crate::journal::read_private_json(&ownerpath).unwrap();
                    owner.seat = "new-owner".into();
                    peer_write(&home.join(".aperture/run/owner/new-owner.json"), &owner);
                }
            }
        }));
        assert!(collect_with_peers(&f.target, &mut source, Some(&peers)).is_err());
    }
}

#[test]
fn peer_unobserved_wrong_tuple_and_corrupt_or_unsafe_files_fail_closed() {
    for case in 0..5 {
        let mut f = PeerFixture::new();
        match case {
            0 => {
                f.peer.incarnation.as_mut().unwrap().observed = false;
                f.save_peer();
            }
            1 => {
                f.peer.incarnation.as_mut().unwrap().model = "other".into();
                f.save_peer();
            }
            2 => {
                std::fs::write(f.home.join(".aperture/run/owner/t1-other.json"), b"{").unwrap();
            }
            3 => {
                let p = f.home.join(".aperture/run/owner/t1-other.json");
                std::fs::rename(&p, p.with_extension("saved")).unwrap();
                std::os::unix::fs::symlink(p.with_extension("saved"), &p).unwrap();
            }
            _ => {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(
                    f.home.join(".aperture/run/owner/t1-other.json"),
                    std::fs::Permissions::from_mode(0o644),
                )
                .unwrap();
            }
        }
        assert!(f.load().is_err());
    }
}

#[test]
fn peer_stopped_active_or_nonactive_does_not_erase_unknown_children() {
    let f = PeerFixture::new();
    let peers = f.load().unwrap();
    let mut source = Fake::new(vec![meta(100, 50), meta(201, 1)]);
    source.states.insert(200, ProcessState::Gone);
    let snapshot = collect_with_peers(&f.target, &mut source, Some(&peers)).unwrap();
    assert_eq!(snapshot.unowned_matches, vec![id(201)]);
    let mut f = PeerFixture::new();
    f.peer.state = OwnerState::Quarantined;
    f.save_peer();
    let peers = f.load().unwrap();
    let snapshot =
        collect_with_peers(&f.target, &mut Fake::new(sibling_table()), Some(&peers)).unwrap();
    assert_eq!(snapshot.unowned_matches, vec![id(200), id(201), id(202)]);
}

#[test]
fn peer_repeated_snapshot_binding_cannot_hide_mid_load_change() {
    let f = PeerFixture::new();
    let mut peers = f.load().unwrap();
    let path = f.home.join(".aperture/teams/t1/team.json");
    let bytes = peer_bytes(&path).unwrap();
    peers.bind_file(path.clone(), &bytes).unwrap();
    let mut changed = bytes.clone(); changed.push(b' ');
    assert!(peers.bind_file(path, &changed).is_err());
}

#[test]
fn older_outsider_only_strictly_earlier_native_birth_exempts_or_match() {
    let baseline = collect(&owner(), &mut Fake::new(vec![meta(100, 50)])).unwrap();
    let bytes = serde_json::to_vec(&baseline.processes).unwrap();
    for birth in [1_000_000, 1_000_001, 1_000_002] {
        // Same hash/different cwd, different hash/same cwd, and both matching.
        for (hash, cwd) in [
            ("a".repeat(64), "/different"),
            ("b".repeat(64), "/fixture/owned"),
            ("a".repeat(64), "/fixture/owned"),
        ] {
            let mut other = meta(200, 1);
            other.identity = identity_from_owner(200, birth).unwrap();
            let mut f = Fake::new(vec![meta(100, 50), other.clone()]);
            f.details.insert(200, (hash, cwd.into()));
            let actual = collect(&owner(), &mut f).unwrap();
            assert_eq!(serde_json::to_vec(&actual.processes).unwrap(), bytes);
            assert!(actual.complete);
            if birth < 1_000_001 {
                assert!(actual.unowned_matches.is_empty());
                assert!(!f.reads.contains(&200));
            } else {
                assert_eq!(actual.unowned_matches, vec![other.identity]);
                assert!(f.reads.contains(&200));
            }
        }
    }
}
#[test]
fn older_outsider_unreadable_recycled_or_malformed_is_not_exempt() {
    for state in [ProcessState::Unreadable, ProcessState::Recycled] {
        let mut other = meta(200, 1);
        other.identity = identity_from_owner(200, 1_000_000).unwrap();
        let mut f = Fake::new(vec![meta(100, 50), other]);
        f.states.insert(200, state);
        assert!(matches!(collect(&owner(), &mut f), Err(ReplacementError::StopUnverified)));
    }
    let mut other = meta(200, 1);
    other.identity.start_time = "unknown".into();
    assert!(collect(&owner(), &mut Fake::new(vec![meta(100, 50), other])).is_err());
}
#[test]
fn older_outsider_exemption_does_not_filter_persisted_owned_identity() {
    let mut o = owner();
    o.incarnation.as_mut().unwrap().processes.push(StoredProcess {
        pid: 101, start_time: 1_000_000, ppid: 100, pgid: 50,
        cmdline_sha256: "a".repeat(64), cwd: "/fixture/owned".into(),
    });
    let mut persisted = meta(101, 1);
    persisted.identity = identity_from_owner(101, 1_000_000).unwrap();
    let table = vec![meta(100, 50), persisted.clone()];
    let baseline = collect(&o, &mut Fake::new(table.clone())).unwrap();
    let mut with_outsider = table;
    let mut other = meta(200, 1);
    other.identity = identity_from_owner(200, 1_000_000).unwrap();
    with_outsider.push(other);
    let actual = collect(&o, &mut Fake::new(with_outsider)).unwrap();
    assert_eq!(serde_json::to_vec(&baseline.processes).unwrap(), serde_json::to_vec(&actual.processes).unwrap());
    assert!(actual.processes.iter().any(|p| p.identity == persisted.identity));
    assert_eq!(actual.processes.len(), 2);
    assert!(actual.unowned_matches.is_empty());
}

#[test]
#[ignore = "operator-authorized single read-only retirement diagnosis; never routine"]
fn retirement_lead_unowned_ro_once() {
    let home=std::path::Path::new("/Users/franciscomateus");
    std::env::set_current_dir("/").unwrap();
    let snapshot=collect_native(home,"teams-live","teams-live-frontend",1).unwrap();
    println!("RO complete={} owned={} unowned={}",snapshot.complete,snapshot.processes.len(),snapshot.unowned_matches.len());
    for id in &snapshot.unowned_matches {
        let meta=observe(id.pid).unwrap().unwrap();assert_eq!(meta.identity,*id);
        let (hash,cwd)=native_details(id).unwrap();
        println!("unowned pid={} birth={} ppid={} pgid={} uid={} hash_match={} cwd_match={}",id.pid,id.start_time,meta.ppid,meta.pgid,meta.uid,
            snapshot.processes.iter().any(|p|p.cmdline_sha256==hash),snapshot.processes.iter().any(|p|p.cwd==cwd));
    }
}

#[test]
#[ignore = "operator-authorized single read-only retirement diagnosis; never routine"]
fn convites_lead_unowned_ro_once() {
    let home=std::path::Path::new("/Users/franciscomateus");
    std::env::set_current_dir("/").unwrap();
    let snapshot=collect_native(home,"eunenem-convites","eunenem-convites-frontend",1).unwrap();
    println!("RO complete={} owned={} unowned={}",snapshot.complete,snapshot.processes.len(),snapshot.unowned_matches.len());
    for id in &snapshot.unowned_matches {
        let meta=observe(id.pid).unwrap().unwrap();assert_eq!(meta.identity,*id);
        let (hash,cwd)=native_details(id).unwrap();
        println!("unowned pid={} birth={} ppid={} pgid={} uid={} hash_match={} cwd_match={}",id.pid,id.start_time,meta.ppid,meta.pgid,meta.uid,
            snapshot.processes.iter().any(|p|p.cmdline_sha256==hash),snapshot.processes.iter().any(|p|p.cwd==cwd));
    }
}

#[test]
fn coordination_socket_root_attribution_never_expands_target_and_keeps_unknown_match() {
    let o = owner();
    let table = sibling_table();
    let roots = vec![("glados".to_string(), id(200))];
    let old = collect(&o, &mut Fake::new(table.clone())).unwrap();
    let new = collect_with_roots(&o, &mut Fake::new(table.clone()), None, &roots).unwrap();
    assert_eq!(old.processes, new.processes);
    assert!(!old.unowned_matches.is_empty());
    assert!(new.unowned_matches.is_empty());
    let mut unknown = table;
    unknown.push(meta(300, 1));
    let still_denied = collect_with_roots(&o, &mut Fake::new(unknown), None, &roots).unwrap();
    assert_eq!(still_denied.processes, new.processes);
    assert_eq!(still_denied.unowned_matches, vec![id(300)]);
}

#[test]
fn coordination_root_overlap_recycle_topology_or_identity_drift_never_exempts() {
    let o = owner();
    for mode in 0..9 {
        let mut roots = vec![("glados".to_string(), id(200))];
        let mut f = Fake::new(sibling_table());
        match mode {
            0 => roots[0].1 = id(100), // target overlap
            1 => roots.push(("peppy".into(), id(200))), // peer overlap
            2 => { f.states.insert(200, ProcessState::Recycled); }
            3 => { f.states.insert(201, ProcessState::Unreadable); }
            4 => { f.tables[1].iter_mut().find(|m| m.identity.pid==201).unwrap().ppid=1; }
            5 => { f.tables[1].iter_mut().find(|m| m.identity.pid==200).unwrap().identity.start_time="2.000001".into(); }
            6 => { f.tables[0].iter_mut().find(|m| m.identity.pid==200).unwrap().uid+=1; }
            7 => { f.tables[1].retain(|m| m.identity.pid!=200); }
            _ => { f.tables[0].iter_mut().find(|m| m.identity.pid==202).unwrap().uid+=1; }
        }
        assert!(collect_with_roots(&o, &mut f, None, &roots).is_err(), "mode {mode}");
    }
}

#[test]
fn coordination_root_cannot_overlap_a_managed_peer() {
    let f = PeerFixture::new();
    let peers = f.load().unwrap();
    assert!(matches!(collect_with_roots(&f.target, &mut Fake::new(sibling_table()),
        Some(&peers), &[("glados".into(), id(200))]), Err(ReplacementError::UnownedProcess)));
}
