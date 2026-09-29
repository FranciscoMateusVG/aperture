use super::*;
use std::path::PathBuf;
use std::os::unix::fs::DirBuilderExt;
use tauri::Manager;

pub(crate) struct Home(pub PathBuf);
impl Home {
    pub(crate) fn new() -> Self {
        let root = PathBuf::from(format!("/private/tmp/aperture-d-{}", uuid::Uuid::new_v4()));
        std::fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
        crate::journal::ensure_private_dir(&root.join(".claude/aperture/fixture")).unwrap();
        Self(root)
    }
    pub(crate) fn owner(&self) -> Arc<RuntimeOwner> {
        Arc::new(RuntimeOwner::new(ControllerLock::acquire(&self.0).unwrap()))
    }
}
impl Drop for Home {
    fn drop(&mut self) { std::fs::remove_dir_all(&self.0).unwrap(); assert!(!self.0.exists()); }
}
pub(crate) fn client(mode: &str) -> ClientInput {
    ClientInput::fixture(std::env::current_exe().unwrap(),
        vec!["--exact".into(),"daemons::tests::inert_client_entry".into(),"--ignored".into(),"--nocapture".into()],
        vec![("APERTURE_D_INERT".into(),mode.into())])
}
#[test]
#[ignore = "Own inert child entry only, never a standalone gate"]
fn inert_client_entry() {
    match std::env::var("APERTURE_D_INERT").as_deref() {
        Ok("ok") => print!("owned"),
        Ok("short") => std::thread::sleep(Duration::from_millis(250)),
        Ok("sleep") => std::thread::sleep(Duration::from_secs(10)),
        Ok("overflow") => print!("{}", "x".repeat(100_000)),
        Ok("local-list") => {
            let home=PathBuf::from(std::env::var_os("HOME").unwrap());
            assert!(home.file_name().unwrap().to_string_lossy().starts_with("aperture-d-"));
            let tools=LocalTools::fixture(&home,&home.join("tmux-inert"));
            let owner=RuntimeOwner::local(ControllerLock::acquire(&home).unwrap(),tools).unwrap();
            let work=owner.admit(None).unwrap();let body=work.body().unwrap();
            let state=Arc::new(Mutex::new(crate::config::default_state()));
            let agents=crate::agents::list_agents_local(&state,&work).unwrap();
            assert_eq!(agents.len(),1);assert_eq!(agents[0].name,"fixture");assert_eq!(agents[0].status,"running");
            assert_eq!(agents[0].tmux_window_id.as_deref(),Some("@1"));
            drop(body);drop(work);owner.close().unwrap();drop(owner);
        },
        _ => panic!("missing finite fixture mode"),
    }
}
#[test]
fn d_admission_capacity_close_self_close_and_lease_retention() {
    let h=Home::new(); let owner=h.owner();
    let first=owner.admit(Some("fixture")).unwrap();
    assert!(owner.admit(Some("fixture")).is_err());
    let mut others=Vec::new();
    for _ in 0..31 { others.push(owner.admit(None).unwrap()); }
    assert!(owner.admit(None).is_err());
    {
        let _body=first.body().unwrap();
        assert_eq!(owner.close().unwrap_err(),"E_RUNTIME_SELF_CLOSE");
        assert_eq!(owner.core.admission.lock().unwrap().phase,RuntimePhase::Open);
    }
    assert_eq!(owner.close_until(Instant::now()+Duration::from_millis(20)).unwrap_err(),"E_RUNTIME_DRAIN_INCOMPLETE");
    assert!(owner.admit(None).is_err());
    assert!(ControllerLock::acquire(&h.0).is_err());
    drop(others); drop(first);
    owner.close().unwrap();
    assert_eq!(owner.core.admission.lock().unwrap().phase,RuntimePhase::Drained);
    assert!(ControllerLock::acquire(&h.0).is_err()); // owner still holds it
    drop(owner);
    drop(ControllerLock::acquire(&h.0).unwrap());
}
#[test]
fn d_direct_tauri_wrappers_are_real_accounted_bodies() {
    for which in ["create","select","list","clear","teams"] {
        let h=Home::new(); let owner=h.owner();
        let state=crate::agents::lifecycle_tests::state("fixture","opus");
        let (arrived,notice)=std::sync::mpsc::sync_channel(1);
        let (resume,go)=std::sync::mpsc::sync_channel(1);
        *owner.core.pause.lock().unwrap()=Some(Arc::new(BodyPause{arrived,resume:Mutex::new(go)}));
        let before=crate::agents::lifecycle_tests::tree(&h.0);
        std::thread::scope(|scope| {
            let owner_for_wrapper=owner.clone(); let state=state.clone();
            let task=scope.spawn(move || {
                // Native MockRuntime only: no window, plugin, run(), environment mutation.
                let app=tauri::test::mock_builder().manage(owner_for_wrapper).manage(state)
                    .build(tauri::test::mock_context(tauri::test::noop_assets())).unwrap();
                match which {
                    "create" => crate::tmux::tmux_create_session("fixture".into(),app.state()).map(|_| ()),
                    "select" => crate::tmux::tmux_select_window("@1".into(),app.state()),
                    "list" => crate::agents::list_agents(app.state(),app.state()).map(|_| ()),
                    "teams" => crate::teams::team_list(app.state(),app.state()).map(|_| ()).map_err(|e| e.message),
                    _ => crate::agents::clear_attention("fixture".into(),app.state(),app.state()),
                }
            });
            notice.recv_timeout(Duration::from_secs(3)).unwrap();
            assert_eq!(owner.close_until(Instant::now()+Duration::from_millis(20)).unwrap_err(),"E_RUNTIME_DRAIN_INCOMPLETE");
            assert!(owner.admit(None).is_err());
            resume.send(()).unwrap();
            assert_eq!(task.join().unwrap().unwrap_err(),"E_RUNTIME_CLOSING");
        });
        owner.close().unwrap();
        assert_eq!(crate::agents::lifecycle_tests::tree(&h.0),before);
    }
}
#[test]
fn d_client_native_success_timeout_overflow_and_closing() {
    let h=Home::new();let owner=h.owner();let work=owner.admit(Some("fixture")).unwrap();
    {
        let _body=work.body().unwrap();
        let good=run_client(&work,client("ok"),Duration::from_secs(2),64*1024,64*1024).unwrap();
        assert!(good.spawned&&good.accepted&&!good.unknown);
        let timeout=run_client(&work,client("sleep"),Duration::from_millis(150),64*1024,64*1024).unwrap();
        assert!(timeout.spawned&&timeout.unknown&&!timeout.accepted);
        let large=run_client(&work,client("overflow"),Duration::from_secs(2),1024,64*1024).unwrap();
        assert!(large.spawned&&large.unknown&&!large.accepted);
    }
    drop(work);owner.close().unwrap();
    assert!(owner.admit(None).is_err());
}
#[test]
fn d_partial_worker_start_and_four_workers_join() {
    let h=Home::new();let owner=h.owner();
    let state=crate::agents::lifecycle_tests::state("fixture","opus");
    // Production loops, no registry/hub record or E tools: no token/query/client.
    owner.start_workers(state.clone()).unwrap();
    assert_eq!(owner.core.workers.lock().unwrap().len(),4);
    owner.close().unwrap();
    assert!(owner.core.workers.lock().unwrap().is_empty());
    assert_eq!(owner.core.admission.lock().unwrap().phase,RuntimePhase::Drained);
    drop(owner);
    let second=h.owner();
    *second.core.fail_worker_at.lock().unwrap()=Some(2);
    assert_eq!(second.start_workers(state).unwrap_err(),"E_RUNTIME_WORKER_START");
    assert!(second.core.workers.lock().unwrap().is_empty());
    assert_eq!(second.core.admission.lock().unwrap().phase,RuntimePhase::Drained);
}

#[test]
fn d_client_ambiguous_observation_never_signals_and_retains_until_actual_reap() {
    let h=Home::new(); let owner=h.owner();
    let work=owner.admit(Some("fixture")).unwrap(); let _body=work.body().unwrap();
    for (mode, unreadable, expected_kills) in [("short",true,0),("sleep",false,1)] {
        let mut spec=client(mode); let audit=Arc::new(Mutex::new(ClientAudit::default()));
        spec.unreadable=unreadable; spec.audit=Some(audit.clone());
        let result=run_client(&work,spec,Duration::from_millis(50),65536,65536).unwrap();
        assert!(result.unknown&&!result.accepted);
        let a=audit.lock().unwrap(); assert_eq!(a.kills,expected_kills); assert!(a.gone);
        assert!(a.pid>1 && crate::team_process::observe(a.pid).unwrap().is_none());
    }
}
#[test]
fn d_poller_is_read_only_absence_overflow_unsafe_and_closing() {
    use std::os::unix::fs::OpenOptionsExt;
    use std::io::Write;
    let h=Home::new(); let owner=h.owner(); let worker=owner.fixture_worker();
    let state=crate::agents::lifecycle_tests::state("fixture","opus");
    let before=crate::agents::lifecycle_tests::tree(&h.0);
    assert!(crate::poller::scan_once(&h.0,&state,&worker).is_err());
    assert_eq!(crate::agents::lifecycle_tests::tree(&h.0),before);
    let mailbox=h.0.join(".aperture/mailbox/operator");
    crate::journal::ensure_private_dir(&mailbox).unwrap();
    let sentinel=mailbox.join("1-fixture.md");
    let mut f=std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&sentinel).unwrap();
    f.write_all(b"synthetic message never consumed").unwrap(); drop(f);
    let before=crate::agents::lifecycle_tests::tree(&h.0);
    crate::poller::scan_once(&h.0,&state,&worker).unwrap();
    crate::poller::scan_once(&h.0,&state,&worker).unwrap();
    assert_eq!(crate::agents::lifecycle_tests::tree(&h.0),before);
    for i in 2..=513 { std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(mailbox.join(format!("{i}-fixture.md"))).unwrap(); }
    let before=crate::agents::lifecycle_tests::tree(&h.0);
    assert_eq!(crate::poller::scan_once(&h.0,&state,&worker).unwrap_err(),"E_MAILBOX_LIMIT");
    assert_eq!(crate::agents::lifecycle_tests::tree(&h.0),before);
    std::fs::rename(&mailbox,mailbox.with_file_name("owned-retained")).unwrap();
    std::os::unix::fs::symlink(mailbox.with_file_name("owned-retained"),&mailbox).unwrap();
    let before=crate::agents::lifecycle_tests::tree(&h.0);
    assert!(crate::poller::scan_once(&h.0,&state,&worker).is_err());
    owner.close().unwrap();
    assert_eq!(crate::poller::scan_once(&h.0,&state,&worker).unwrap_err(),"E_RUNTIME_CLOSING");
    assert_eq!(crate::agents::lifecycle_tests::tree(&h.0),before);
}

#[test]
fn local_real_client_selectors_positive_dispatch_and_drift_are_not_empty_tool_guards() {
    use std::os::unix::fs::PermissionsExt;
    let h=Home::new();let executable=h.0.join("tmux-inert");let events=h.0.join("events");
    // Native own short-lived process on the actual run_client path. No tmux,
    // provider or inherited environment. Shell uses builtins only.
    let script=format!("#!/bin/sh\nprintf '%s\\n' \"$1\" >> '{}'\ncase \"$1\" in\n list-sessions) printf 'aperture\\n';;\n list-windows) printf '@1||fixture||inert\\n';;\n select-window) exit 0;;\n *) exit 9;;\nesac\n",events.display());
    std::fs::write(&executable,script).unwrap();std::fs::set_permissions(&executable,std::fs::Permissions::from_mode(0o700)).unwrap();
    let tools=LocalTools::fixture(&h.0,&executable);
    let owner=RuntimeOwner::local(ControllerLock::acquire(&h.0).unwrap(),tools).unwrap();
    let work=owner.admit(None).unwrap();let body=work.body().unwrap();
    assert!(crate::tmux::tmux_create_session_shared("-bad".into(),&work).is_err());
    assert!(crate::tmux::tmux_select_window_shared(";bad".into(),&work).is_err());assert!(!events.exists());
    assert_eq!(crate::tmux::tmux_create_session_shared("aperture".into(),&work).unwrap(),"already exists");
    assert_eq!(crate::tmux::list_windows_local("aperture",&work).unwrap()[0].window_id,"@1");
    crate::tmux::tmux_select_window_shared("@1".into(),&work).unwrap();
    assert_eq!(std::fs::read_to_string(&events).unwrap(),"list-sessions\nlist-windows\nselect-window\n");
    std::fs::set_permissions(&executable,std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(crate::tmux::tmux_select_window_shared("@1".into(),&work).is_err());
    assert_eq!(std::fs::read_to_string(&events).unwrap(),"list-sessions\nlist-windows\nselect-window\n");
    drop(body);drop(work);owner.close().unwrap();drop(owner);
}

#[test]
fn local_list_ingress_loads_real_private_registry_and_queries_real_inert_clients(){
    use std::os::unix::fs::PermissionsExt;
    let h=Home::new();let root=h.0.join(".claude/aperture/fixture");
    std::fs::write(root.join("manifest.json"),br#"{"name":"fixture","model":"opus","role":"backend","window":"fixture"}"#).unwrap();
    std::fs::write(root.join("prompt.md"),"owned synthetic prompt").unwrap();
    let executable=h.0.join("tmux-inert");let events=h.0.join("events");
    let script=format!("#!/bin/sh\nprintf '%s\\n' \"$1\" >> '{}'\ncase \"$1\" in\n list-windows) printf '@1||fixture||claude\\n';;\n list) printf '[]';;\n *) exit 9;;\nesac\n",events.display());
    std::fs::write(&executable,script).unwrap();std::fs::set_permissions(&executable,std::fs::Permissions::from_mode(0o700)).unwrap();
    let parent=Home::new();let owner=parent.owner();let work=owner.admit(None).unwrap();let body=work.body().unwrap();
    let mut input=client("local-list");input.env.push(("HOME".into(),h.0.to_string_lossy().into_owned()));
    let result=run_client(&work,input,Duration::from_secs(5),64*1024,64*1024).unwrap();
    assert!(result.accepted,"inert list child failed: {}",String::from_utf8_lossy(&result.stdout));
    assert_eq!(std::fs::read_to_string(&events).unwrap(),"list-windows\nlist\n");
    drop(body);drop(work);owner.close().unwrap();drop(owner);
}

#[test]
fn local_tool_ctime_denies_in_place_overwrite_with_restored_mtime() {
    use std::os::unix::fs::{PermissionsExt,MetadataExt};use std::io::Write;
    let h=Home::new();let tool=h.0.join("native-inert");
    std::fs::write(&tool,b"#!/bin/sh\nexit 0\n").unwrap();std::fs::set_permissions(&tool,std::fs::Permissions::from_mode(0o700)).unwrap();
    let tools=LocalTools::fixture(&h.0,&tool);let pin=tools.tmux.clone();let before=std::fs::metadata(&tool).unwrap();
    let owner=RuntimeOwner::local(ControllerLock::acquire(&h.0).unwrap(),tools).unwrap();let work=owner.admit(None).unwrap();let body=work.body().unwrap();
    let mut input=work.client(&pin,vec![]).unwrap();let audit=Arc::new(Mutex::new(ClientAudit::default()));input.audit=Some(audit.clone());
    let mut file=std::fs::OpenOptions::new().write(true).open(&tool).unwrap();file.write_all(b"#!/bin/sh\nexit 1\n").unwrap();
    file.set_times(std::fs::FileTimes::new().set_modified(before.modified().unwrap())).unwrap();drop(file);
    let after=std::fs::metadata(&tool).unwrap();
    assert_eq!((before.ino(),before.len(),before.mode(),before.mtime(),before.mtime_nsec()),(after.ino(),after.len(),after.mode(),after.mtime(),after.mtime_nsec()));
    assert!(matches!(run_client(&work,input,Duration::from_secs(1),1024,1024),Err(e) if e=="E_LOCAL_TOOL_CHANGED"));
    assert_eq!(audit.lock().unwrap().pid,0);drop(body);drop(work);owner.close().unwrap();drop(owner);
}
