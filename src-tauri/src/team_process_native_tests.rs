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
