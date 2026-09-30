//! Explicit operator Stop for standing coordinators, not managed team retirement.
//! No PGID/name/pattern signaling. Freeze the exact native ancestry closure,
//! journal it, terminate individual identities, and verify Gone before reuse.
use crate::{
    controller::{CodexOperation, ControllerLock},
    team_process::{self, ProcessMetadata},
    team_replacement::{ProcessIdentity, ProcessState},
};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::PathBuf,
    time::{Duration, Instant},
};
const ERROR: &str = "E_LIFECYCLE_PROCESS_UNKNOWN";
const OUTCOME: &str = "E_LIFECYCLE_OUTCOME_UNKNOWN";
const LIMIT: usize = 256;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StopRecord {
    schema: u32,
    seat: String,
    operation: String,
    roots: Vec<ProcessIdentity>,
    processes: Vec<ProcessIdentity>,
    outcome: String,
}
fn record_path(lease: &ControllerLock, seat: &str) -> Result<PathBuf, String> {
    if !crate::daemon_registry::valid_name(seat) {
        return Err(ERROR.into());
    }
    Ok(lease
        .run_dir()?
        .join("coordinator-stops")
        .join(format!("{seat}.json")))
}
pub(super) fn check_start(lease: &ControllerLock, seat: &str) -> Result<(), String> {
    let path = record_path(lease, seat)?;
    match fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(ERROR.into()),
        Ok(_) => {
            let r: StopRecord = crate::journal::read_private_json(&path)?;
            if r.schema != 1 || r.seat != seat || r.outcome != "gone" {
                return Err(OUTCOME.into());
            }
            for id in &r.processes {
                if !matches!(
                    team_process::state(id),
                    ProcessState::Gone | ProcessState::Recycled
                ) {
                    return Err(OUTCOME.into());
                }
            }
            Ok(())
        }
    }
}
/// Value cannot be constructed by a caller from a PID list or receipt JSON.
pub(crate) struct Closed {
    seat: String,
    ids: Vec<ProcessIdentity>,
    path: PathBuf,
}
impl Closed {
    pub(crate) fn verifies(&self, seat: &str, root: &ProcessIdentity) -> Result<(), String> {
        if self.seat != seat || !self.ids.contains(root) {
            return Err(OUTCOME.into());
        }
        self.recheck()
    }
    pub(crate) fn recheck(&self) -> Result<(), String> {
        let r: StopRecord = crate::journal::read_private_json(&self.path)?;
        if r.schema != 1 || r.seat != self.seat || r.outcome != "gone" || r.processes != self.ids {
            return Err(OUTCOME.into());
        }
        if self
            .ids
            .iter()
            .any(|p| team_process::state(p) != ProcessState::Gone)
        {
            return Err(OUTCOME.into());
        }
        Ok(())
    }
}
fn closure(
    roots: &[ProcessIdentity],
    retained: &[ProcessIdentity],
    table: &[ProcessMetadata],
    protected: u32,
) -> Result<Vec<ProcessIdentity>, String> {
    let mut ids = retained.to_vec();
    for root in roots {
        if !ids.contains(root) {
            ids.push(root.clone());
        }
    }
    for id in &ids {
        if let Some(row) = table.iter().find(|r| r.identity.pid == id.pid) {
            if row.identity != *id || row.uid != unsafe { libc::geteuid() } {
                return Err(ERROR.into());
            }
        }
    }
    loop {
        let mut changed = false;
        for row in table {
            if ids.iter().any(|r| r.pid == row.ppid) && !ids.contains(&row.identity) {
                if row.uid != unsafe { libc::geteuid() } {
                    return Err(ERROR.into());
                }
                ids.push(row.identity.clone());
                changed = true;
            }
        }
        if ids.len() > LIMIT {
            return Err(ERROR.into());
        }
        if !changed {
            break;
        }
    }
    if ids.iter().any(|p| p.pid == protected || p.pid <= 1) {
        return Err("E_COORDINATOR_SELF_STOP".into());
    }
    Ok(ids)
}
fn signal(id: &ProcessIdentity, sig: i32) -> Result<(), String> {
    match team_process::state(id) {
        ProcessState::Gone => return Ok(()),
        ProcessState::Same => {}
        _ => return Err(ERROR.into()),
    }
    if unsafe { libc::kill(id.pid as i32, sig) } != 0 {
        if team_process::state(id) == ProcessState::Gone {
            return Ok(());
        }
        return Err(ERROR.into());
    }
    Ok(())
}
#[cfg(target_os = "macos")]
fn stopped(id: &ProcessIdentity) -> Result<bool, String> {
    if team_process::state(id) == ProcessState::Gone {
        return Ok(true);
    }
    if team_process::state(id) != ProcessState::Same {
        return Err(ERROR.into());
    }
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let size = std::mem::size_of::<libc::proc_bsdinfo>();
    if unsafe {
        libc::proc_pidinfo(
            id.pid as i32,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size as i32,
        )
    } != size as i32
    {
        return Err(ERROR.into());
    }
    let info = unsafe { info.assume_init() };
    if team_process::state(id) != ProcessState::Same {
        return Err(ERROR.into());
    }
    // Darwin sys/proc.h: SSTOP=4. Zombies cannot fork either, but must still reap.
    Ok(info.pbi_status == 4 || info.pbi_status == 5)
}
#[cfg(not(target_os = "macos"))]
fn stopped(_: &ProcessIdentity) -> Result<bool, String> {
    Err(ERROR.into())
}

pub(crate) fn stop(
    lease: &ControllerLock,
    op: &mut CodexOperation<'_>,
    roots: Vec<ProcessIdentity>,
    mut check: impl FnMut() -> Result<(), String>,
) -> Result<Closed, String> {
    check()?;
    op.verify_for(lease)?;
    check_start(lease, op.seat())?;
    let table = crate::team_process::native::native_table(Instant::now() + Duration::from_secs(2))
        .map_err(|_| ERROR)?;
    // No already-gone seed gets silently enrolled as an empty observation.
    for root in &roots {
        if !table.iter().any(|r| r.identity == *root) {
            return Err(ERROR.into());
        }
    }
    let ids = closure(&roots, &[], &table, lease.identity()?.pid)?;
    let path = record_path(lease, op.seat())?;
    crate::journal::ensure_private_dir(path.parent().ok_or(ERROR)?)?;
    if path.exists() {
        let old: StopRecord = crate::journal::read_private_json(&path)?;
        if old.outcome != "gone" {
            return Err(OUTCOME.into());
        }
        let archive = path.with_file_name(format!("{}-{}.json", op.seat(), uuid::Uuid::new_v4()));
        crate::journal::rename_no_replace(&path, &archive)?;
    }
    let mut record = StopRecord {
        schema: 1,
        seat: op.seat().into(),
        operation: op.nonce().into(),
        roots,
        processes: ids,
        outcome: "unknown".into(),
    };
    crate::journal::write_private_json_atomic(&path, &record, false)?;
    let mut frozen = Vec::new();
    let result = (|| {
        let until = Instant::now() + Duration::from_secs(3);
        loop {
            check()?;
            op.verify_for(lease)?;
            for id in &record.processes {
                if !frozen.contains(id) {
                    // Authority was durably recorded before every signal.
                    signal(id, libc::SIGSTOP)?;
                    frozen.push(id.clone());
                }
            }
            if Instant::now() >= until {
                return Err(OUTCOME.to_string());
            }
            if !record
                .processes
                .iter()
                .map(stopped)
                .collect::<Result<Vec<_>, _>>()?
                .iter()
                .all(|v| *v)
            {
                std::thread::sleep(Duration::from_millis(10));
                continue;
            }
            let table = crate::team_process::native::native_table(until).map_err(|_| ERROR)?;
            let ids = closure(
                &record.roots,
                &record.processes,
                &table,
                lease.identity()?.pid,
            )?;
            if ids == record.processes {
                break;
            }
            record.processes = ids;
            crate::journal::write_private_json_atomic(&path, &record, true)?;
        }
        check()?;
        op.verify_for(lease)?;
        // Force-stop is deliberate. Resuming a TERM handler could fork after
        // the closed snapshot. Never advertise this as graceful persistence of
        // outstanding provider/build work. Children first; no group signal.
        for id in record.processes.iter().rev() {
            signal(id, libc::SIGKILL)?;
        }
        let until = Instant::now() + Duration::from_secs(4);
        loop {
            op.try_wait()?;
            if record
                .processes
                .iter()
                .all(|id| team_process::state(id) == ProcessState::Gone)
            {
                break;
            }
            if Instant::now() >= until {
                return Err(OUTCOME.to_string());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        record.outcome = "gone".into();
        crate::journal::write_private_json_atomic(&path, &record, true)?;
        Ok(Closed {
            seat: record.seat.clone(),
            ids: record.processes.clone(),
            path: path.clone(),
        })
    })();
    if result.is_err() {
        // Do not leave a preflight failure freezing a live agent forever. Only
        // identities we stopped may be resumed; unknown journal blocks retry.
        for id in frozen {
            let _ = signal(&id, libc::SIGCONT);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    fn row(pid: u32, ppid: u32) -> ProcessMetadata {
        ProcessMetadata {
            identity: ProcessIdentity {
                pid,
                start_time: format!("{pid}.000001"),
            },
            ppid,
            pgid: 17,
            uid: unsafe { libc::geteuid() },
        }
    }
    #[test]
    fn native_closure_never_uses_shared_group_or_recycled_identity() {
        let t = vec![row(101, 1), row(102, 101), row(103, 102), row(104, 1)];
        let root = t[0].identity.clone();
        let ids = closure(&[root.clone()], &[], &t, 999).unwrap();
        assert_eq!(
            ids.iter().map(|i| i.pid).collect::<Vec<_>>(),
            vec![101, 102, 103]
        );
        assert!(closure(&[root.clone()], &[], &t, 102).is_err());
        let mut changed = t.clone();
        changed[0].identity.start_time = "changed".into();
        assert!(closure(&[root], &[], &changed, 999).is_err());
    }
}

#[cfg(test)]
mod native_tests {
    use super::*;
    use std::{
        os::unix::process::CommandExt,
        process::{Command, Stdio},
    };
    #[test]
    #[ignore = "owned child entry only"]
    fn owned_parent_entry() {
        let Ok(root) = std::env::var("APERTURE_COORDINATOR_FIXTURE") else {
            return;
        };
        let mut child = Command::new("/bin/sleep")
            .arg("60")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let id = team_process::observe(child.id()).unwrap().unwrap().identity;
        fs::write(
            PathBuf::from(&root).join("descendant.json"),
            serde_json::to_vec(&id).unwrap(),
        )
        .unwrap();
        child.wait().unwrap();
    }
    #[test]
    fn exact_native_tree_gone_before_retry_and_unrelated_child_untouched() {
        let home = PathBuf::from(format!(
            "/private/tmp/ac-stop-{}",
            &uuid::Uuid::new_v4().simple().to_string()[..12]
        ));
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new().mode(0o700).create(&home).unwrap();
        let lease = ControllerLock::acquire(&home).unwrap();
        let slot = lease.codex_slot("fixture").unwrap();
        let mut op = slot.enter().unwrap();
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .env_clear()
            .env("APERTURE_COORDINATOR_FIXTURE", &home)
            .args([
                "--exact",
                "agents::coordinator_lifecycle::native_tests::owned_parent_entry",
                "--ignored",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 {
                    Err(std::io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
        let id = op.spawn_retained(&mut command).unwrap();
        let until = Instant::now() + Duration::from_secs(5);
        let descendant: ProcessIdentity = loop {
            if let Ok(b) = fs::read(home.join("descendant.json")) {
                if let Ok(v) = serde_json::from_slice(&b) {
                    break v;
                }
            }
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(10));
        };
        let mut unrelated = Command::new("/bin/sleep").arg("60").spawn().unwrap();
        let other = team_process::observe(unrelated.id())
            .unwrap()
            .unwrap()
            .identity;
        let result = stop(&lease, &mut op, vec![id.clone()], || Ok(()));
        // Always reap our unrelated fixture even when an assertion fails.
        let other_state = team_process::state(&other);
        unrelated.kill().unwrap();
        unrelated.wait().unwrap();
        let closed = result.unwrap();
        assert_eq!(other_state, ProcessState::Same);
        closed.verifies("fixture", &id).unwrap();
        assert_eq!(team_process::state(&descendant), ProcessState::Gone);
        check_start(&lease, "fixture").unwrap();
        op.release_exited().unwrap();
        let mut record: StopRecord = crate::journal::read_private_json(&closed.path).unwrap();
        record.outcome = "unknown".into();
        crate::journal::write_private_json_atomic(&closed.path, &record, true).unwrap();
        assert!(check_start(&lease, "fixture").is_err());
        fs::remove_dir_all(home).unwrap();
    }
    #[test]
    fn self_or_missing_root_denies_without_record_or_signal() {
        let home = PathBuf::from(format!("/private/tmp/ac-stop-{}", uuid::Uuid::new_v4()));
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new().mode(0o700).create(&home).unwrap();
        let lease = ControllerLock::acquire(&home).unwrap();
        let slot = lease.codex_slot("fixture").unwrap();
        let mut op = slot.enter().unwrap();
        assert!(
            matches!(stop(&lease,&mut op,vec![lease.identity().unwrap().clone()],||Ok(())),Err(e) if e=="E_COORDINATOR_SELF_STOP")
        );
        assert!(!record_path(&lease, "fixture").unwrap().exists());
        fs::remove_dir_all(home).unwrap();
    }
}
