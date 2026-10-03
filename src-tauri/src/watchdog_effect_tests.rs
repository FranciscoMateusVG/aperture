use super::*;
use crate::daemons::{RuntimeOwner, tests::{Home, client}};
fn inputs(pause: Option<(usize,std::sync::mpsc::SyncSender<()>,Mutex<std::sync::mpsc::Receiver<()>>)>) -> NudgeInputs {
    let me=crate::team_process::observe(std::process::id()).unwrap().unwrap();
    NudgeInputs{fixture:Some(NudgeFixture{
        target:EffectTarget{window:"@1".into(),pane:"%1".into(),pid:me.identity.pid,birth:me.identity.start_time,uid:me.uid},
        clients:(0..4).map(|_|client("ok")).collect(),pause,target_after:None,crash:None,
    })}
}
type Audits = Vec<Arc<Mutex<crate::daemons::ClientAudit>>>;
fn audit_clients(spec: &mut NudgeInputs) -> Audits {
    spec.fixture.as_mut().unwrap().clients.iter_mut().map(|input| {
        let audit=Arc::new(Mutex::new(crate::daemons::ClientAudit::default()));
        input.audit=Some(audit.clone());audit
    }).collect()
}
fn assert_prefix(audits: &Audits, count: usize) {
    assert_eq!(audits.len(),4);
    for (index,audit) in audits.iter().enumerate() {
        let audit=audit.lock().unwrap();
        assert_eq!(audit.pid>1,index<count,"native client index {index}, expected prefix {count}");
        if index>=count { assert_eq!(audit.pid,0);assert_eq!(audit.kills,0); }
        else { assert!(crate::team_process::observe(audit.pid).unwrap().is_none(),"client not reaped"); }
    }
}
fn retry_denied(owner:&RuntimeOwner,s:&Arc<Mutex<AppState>>,home:&std::path::Path) {
    let before=crate::agents::lifecycle_tests::tree(home);
    let work=owner.admit(Some("fixture")).unwrap();let _body=work.body().unwrap();
    let mut spec=inputs(None);let audits=audit_clients(&mut spec);
    assert!(work.nudge(s,"fixture",NudgeProducer::UnreadNudge,spec).is_err());
    assert_prefix(&audits,0);
    assert_eq!(crate::agents::lifecycle_tests::tree(home),before);
}
fn state() -> Arc<Mutex<AppState>> {
    let s=crate::agents::lifecycle_tests::state("fixture","opus");
    { let mut a=s.lock().unwrap();let a=a.agents.get_mut("fixture").unwrap();a.status="running".into();a.tmux_window_id=Some("@1".into()); }
    s
}
#[test]
fn d_same_actuator_accepts_four_clients_then_cross_producer_cooldown() {
    let h=Home::new();let owner=h.owner();let s=state();
    let work=owner.admit(Some("fixture")).unwrap();let _body=work.body().unwrap();
    let mut spec=inputs(None);let audits=audit_clients(&mut spec);
    assert_eq!(work.nudge(&s,"fixture",NudgeProducer::RekickNudge,spec).unwrap(),DispatchOutcome::DispatchAccepted);
    assert_prefix(&audits,4);
    let before=crate::agents::lifecycle_tests::tree(&h.0);
    assert!(work.nudge(&s,"fixture",NudgeProducer::UnreadNudge,inputs(None)).is_err());
    assert_eq!(crate::agents::lifecycle_tests::tree(&h.0),before);
}
#[test]
fn d_close_before_internal_or_delayed_enter_retains_unknown_on_restart() {
    for index in [0,1,2,3] {
        let h=Home::new();let owner=h.owner();let s=state();
        let (arrived,notice)=std::sync::mpsc::sync_channel(1);
        let (resume,go)=std::sync::mpsc::sync_channel(1);
        let mut spec=inputs(Some((index,arrived,Mutex::new(go))));
        let audits=audit_clients(&mut spec);
        std::thread::scope(|scope| {
            let work=owner.admit(Some("fixture")).unwrap();let state=s.clone();
            let task=scope.spawn(move || {
                let _body=work.body().unwrap();
                work.nudge(&state,"fixture",NudgeProducer::RekickNudge,spec)
            });
            notice.recv_timeout(Duration::from_secs(3)).unwrap();
            assert_eq!(owner.fixture_close_short().unwrap_err(),"E_RUNTIME_DRAIN_INCOMPLETE");
            resume.send(()).unwrap();
            assert_eq!(task.join().unwrap().unwrap(),if index==0 {DispatchOutcome::NoDispatch} else {DispatchOutcome::Unknown});
        });
        assert_prefix(&audits,index);
        owner.close().unwrap();drop(owner);
        let restarted=Arc::new(RuntimeOwner::new(crate::controller::ControllerLock::acquire(&h.0).unwrap()));
        if index==0 {
            let work=restarted.admit(Some("fixture")).unwrap();let _body=work.body().unwrap();
            let mut spec=inputs(None);let audits=audit_clients(&mut spec);
            assert_eq!(work.nudge(&s,"fixture",NudgeProducer::UnreadNudge,spec).unwrap(),DispatchOutcome::DispatchAccepted);
            assert_prefix(&audits,4);
        } else { retry_denied(&restarted,&s,&h.0); }
    }
}
#[test]
fn d_target_drift_and_partial_dispatch_are_unknown_not_retried() {
    let h=Home::new();let owner=h.owner();let s=state();
    let mut spec=inputs(None);
    let f=spec.fixture.as_mut().unwrap();
    let mut changed=f.target.clone();changed.pane="%2".into();f.target_after=Some((1,changed));
    let audits=audit_clients(&mut spec);
    let work=owner.admit(Some("fixture")).unwrap();let _body=work.body().unwrap();
    assert_eq!(work.nudge(&s,"fixture",NudgeProducer::RekickNudge,spec).unwrap(),DispatchOutcome::Unknown);
    assert_prefix(&audits,1);
    drop(_body);drop(work);
    retry_denied(&owner,&s,&h.0);
    owner.close().unwrap();drop(owner);
    retry_denied(&h.owner(),&s,&h.0);
}
#[test]
fn d_tools_membership_model_and_malformed_namespace_deny_before_dispatch() {
    let h=Home::new();let owner=h.owner();let s=state();
    let work=owner.admit(Some("fixture")).unwrap();let _body=work.body().unwrap();
    let before=crate::agents::lifecycle_tests::tree(&h.0);
    assert!(work.nudge(&s,"fixture",NudgeProducer::RekickNudge,NudgeInputs::production()).is_err());
    s.lock().unwrap().agents.get_mut("fixture").unwrap().model="codex/test".into();
    assert!(work.nudge(&s,"fixture",NudgeProducer::RekickNudge,inputs(None)).is_err());
    assert_eq!(crate::agents::lifecycle_tests::tree(&h.0),before);
    s.lock().unwrap().agents.get_mut("fixture").unwrap().model="opus".into();
    crate::journal::ensure_private_dir(&h.0.join(".aperture/run/watchdog")).unwrap();
    std::fs::write(h.0.join(".aperture/run/watchdog/orphan"),b"bad").unwrap();
    let before=crate::agents::lifecycle_tests::tree(&h.0);
    assert!(work.nudge(&s,"fixture",NudgeProducer::UnreadNudge,inputs(None)).is_err());
    assert_eq!(crate::agents::lifecycle_tests::tree(&h.0),before);
}

#[test]
fn d_managed_reclassification_before_enter_and_no_dispatch_retry() {
    let h=Home::new();let owner=h.owner();let s=state();
    let (arrived,notice)=std::sync::mpsc::sync_channel(1);
    let (resume,go)=std::sync::mpsc::sync_channel(1);
    let mut spec=inputs(Some((1,arrived,Mutex::new(go))));
    let audits=audit_clients(&mut spec);
    std::thread::scope(|scope| {
        let work=owner.admit(Some("fixture")).unwrap();let state=s.clone();
        let task=scope.spawn(move || { let _body=work.body().unwrap();work.nudge(&state,"fixture",NudgeProducer::RekickNudge,spec) });
        notice.recv_timeout(Duration::from_secs(3)).unwrap();
        // Actual classifier rejects an orphan TEAM marker; keep model Claude
        // so the membership guard, not a simultaneous model change, is tested.
        std::fs::write(h.0.join(".claude/aperture/fixture/TEAM"),b"{}").unwrap();
        resume.send(()).unwrap();
        assert_eq!(task.join().unwrap().unwrap(),DispatchOutcome::Unknown);
    });
    assert_prefix(&audits,1);
    // Remove only the fixture-owned marker to prove the persisted Unknown,
    // rather than membership alone, blocks a cross-producer/new-owner retry.
    std::fs::remove_file(h.0.join(".claude/aperture/fixture/TEAM")).unwrap();
    retry_denied(&owner,&s,&h.0);
    owner.close().unwrap();drop(owner);
    retry_denied(&h.owner(),&s,&h.0);
    let h=Home::new();let owner=h.owner();let s=state();
    let work=owner.admit(Some("fixture")).unwrap();let _body=work.body().unwrap();
    let mut fail=inputs(None);
    fail.fixture.as_mut().unwrap().clients[0]=crate::daemons::ClientInput::fixture(h.0.join("absent-owned-executable"),vec![],vec![]);
    let audits=audit_clients(&mut fail);
    assert_eq!(work.nudge(&s,"fixture",NudgeProducer::RekickNudge,fail).unwrap(),DispatchOutcome::NoDispatch);
    assert_prefix(&audits,0);
    let mut retry=inputs(None);let audits=audit_clients(&mut retry);
    assert_eq!(work.nudge(&s,"fixture",NudgeProducer::UnreadNudge,retry).unwrap(),DispatchOutcome::DispatchAccepted);
    assert_prefix(&audits,4);
}
#[test]
#[ignore = "owned crash child entry only"]
fn inert_effect_crash_entry() {
    if std::env::var("APERTURE_D_CLIENT_EXIT").as_deref()==Ok("23") { std::process::exit(23); }
    if std::env::var("APERTURE_D_KICKOFF").as_deref()==Ok("matrix") {
        kickoff_matrix_child();return;
    }
    let root=std::path::PathBuf::from(std::env::var_os("APERTURE_D_ROOT").unwrap());
    assert!(root.file_name().unwrap().to_string_lossy().starts_with("aperture-d-"));
    let stage=std::env::var("APERTURE_D_CRASH").unwrap().parse::<u8>().unwrap();
    assert!(stage<=1);
    let owner=RuntimeOwner::new(crate::controller::ControllerLock::acquire(&root).unwrap());
    let work=owner.admit(Some("fixture")).unwrap();let _body=work.body().unwrap();
    let mut spec=inputs(None);spec.fixture.as_mut().unwrap().crash=Some(stage);
    let _=work.nudge(&state(),"fixture",NudgeProducer::RekickNudge,spec);
    panic!("crash seam not reached");
}
#[test]
fn d_crash_on_both_effect_edges_persists_unknown_across_new_owner_and_target() {
    for stage in [0,1] {
        let h=Home::new();
        let mut child=std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact","watchdog::effect_tests::inert_effect_crash_entry","--ignored","--nocapture"])
            .env_clear().env("APERTURE_D_ROOT",&h.0).env("APERTURE_D_CRASH",stage.to_string())
            .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().unwrap();
        let identity=crate::team_process::observe(child.id()).unwrap().unwrap().identity;
        let deadline=std::time::Instant::now()+Duration::from_secs(5);
        let status=loop {
            if let Some(status)=child.try_wait().unwrap() { break status; }
            if std::time::Instant::now()>=deadline {
                if crate::team_process::state(&identity)==crate::team_replacement::ProcessState::Same { child.kill().unwrap(); }
                let _=child.wait(); panic!("owned crash fixture deadline");
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(status.code(),Some(73+stage));
        assert_eq!(crate::team_process::state(&identity),crate::team_replacement::ProcessState::Gone);
        let owner=h.owner();let work=owner.admit(Some("fixture")).unwrap();let _body=work.body().unwrap();
        let s=state();let before=crate::agents::lifecycle_tests::tree(&h.0);
        // New UUID would be generated by the real actuator; a new live target
        // identity and producer cannot bypass the existing incomplete intent.
        assert!(work.nudge(&s,"fixture",NudgeProducer::UnreadNudge,inputs(None)).is_err());
        assert_eq!(crate::agents::lifecycle_tests::tree(&h.0),before);
        let history=effect_history(&h.0.join(".aperture/run/watchdog"),"fixture",effect_now().unwrap()).unwrap();
        assert!(history.blocked);assert_eq!(history.entries,2);
    }
}

#[test]
fn d_fact_capacity_shape_and_outcome_reservation_never_prune_or_dispatch() {
    fn facts(root:&std::path::Path,seat:&str,pairs:usize) {
        let dir=root.join(seat);crate::journal::ensure_private_dir(&dir).unwrap();
        let target=inputs(None).fixture.unwrap().target;
        for _ in 0..pairs {
            let id=uuid::Uuid::new_v4().to_string();
            write_effect(&dir.join(format!("{id}.intent.json")),&EffectFact::Intent {
                version:1,id:id.clone(),seat:seat.into(),producer:NudgeProducer::RekickNudge,
                payload:"boot_nudge".into(),target:target.clone(),plan:"0".repeat(64),at_ms:1,
            }).unwrap();
            write_effect(&dir.join(format!("{id}.outcome.json")),&EffectFact::Outcome {
                version:1,id,seat:seat.into(),outcome:DispatchOutcome::NoDispatch,observations:vec![],at_ms:2,
            }).unwrap();
        }
    }
    for cap in ["seats","entries","future","bytes"] {
        let h=Home::new();let owner=h.owner();let work=owner.admit(Some("fixture")).unwrap();let _body=work.body().unwrap();
        let root=h.0.join(".aperture/run/watchdog");crate::journal::ensure_private_dir(&root).unwrap();
        match cap {
            "seats" => for i in 0..128 { facts(&root,&format!("seat-{i}"),1); },
            "entries" => facts(&root,"fixture",255), // 511 entries; outcome budget needs two
            _ => {
                facts(&root,"fixture",1);
                let path=std::fs::read_dir(root.join("fixture")).unwrap().map(|e|e.unwrap().path()).find(|p|p.to_string_lossy().ends_with("intent.json")).unwrap();
                let mut value:serde_json::Value=serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                if cap=="future" { value["version"]=serde_json::json!(2); }
                else { value["plan"]=serde_json::json!("x".repeat(8193)); }
                // Corruption fixture is explicit setup, not a writer/cleanup policy.
                std::fs::write(path,serde_json::to_vec(&value).unwrap()).unwrap();
            }
        }
        let before=crate::agents::lifecycle_tests::tree(&h.0);
        assert!(work.nudge(&state(),"fixture",NudgeProducer::UnreadNudge,inputs(None)).is_err(),"{cap}");
        assert_eq!(crate::agents::lifecycle_tests::tree(&h.0),before);
    }
}

#[test]
fn d_final_audit_closing_badge_and_disconnected_unread_are_denied() {
    let h=Home::new();let owner=h.owner();let s=state();
    let worker=owner.fixture_worker();let work=owner.admit(None).unwrap();
    let (started,notice)=std::sync::mpsc::sync_channel(1);
    std::thread::scope(|scope| {
        let held_state=s.lock().unwrap();
        let state=s.clone();
        let task=scope.spawn(move || {
            let _body=work.body().unwrap();started.send(()).unwrap();
            ring_operator(&state,"fixture",&worker);
        });
        notice.recv_timeout(Duration::from_secs(3)).unwrap();
        assert_eq!(owner.fixture_close_short().unwrap_err(),"E_RUNTIME_DRAIN_INCOMPLETE");
        drop(held_state);task.join().unwrap();
    });
    owner.close().unwrap();
    assert!(!s.lock().unwrap().agents["fixture"].attention);
    let shared=Arc::new(Mutex::new(Shared::new()));
    {
        let mut value=shared.lock().unwrap();value.subscriber_connected=true;
        apply_presence_event(&mut value.presence,"fixture","join",now());
        assert!(unread_online(&value,"fixture"));
    }
    mark_disconnected(&shared);
    let value=shared.lock().unwrap();
    assert!(value.presence["fixture"].online); // retained observational flag is not trust
    assert!(!unread_online(&value,"fixture"));
}

#[test]
fn d_nonzero_and_timeout_at_first_enter_spawn_exact_partial_prefix() {
    for mode in ["nonzero","timeout"] {
        let h=Home::new();let owner=h.owner();let s=state();
        let work=owner.admit(Some("fixture")).unwrap();let body=work.body().unwrap();
        let mut spec=inputs(None);
        spec.fixture.as_mut().unwrap().clients[1]=if mode=="timeout" { client("sleep") } else {
            crate::daemons::ClientInput::fixture(std::env::current_exe().unwrap(),
                vec!["--exact".into(),"watchdog::effect_tests::inert_effect_crash_entry".into(),"--ignored".into(),"--nocapture".into()],
                vec![("APERTURE_D_CLIENT_EXIT".into(),"23".into())])
        };
        let audits=audit_clients(&mut spec);
        assert_eq!(work.nudge(&s,"fixture",NudgeProducer::RekickNudge,spec).unwrap(),DispatchOutcome::Unknown,"{mode}");
        assert_prefix(&audits,2);
        if mode=="timeout" { let audit=audits[1].lock().unwrap();assert_eq!(audit.kills,1);assert!(audit.gone); }
        else { assert_eq!(audits[1].lock().unwrap().kills,0); }
        drop(body);drop(work);
        retry_denied(&owner,&s,&h.0);
        owner.close().unwrap();drop(owner);
        retry_denied(&h.owner(),&s,&h.0);
    }
}

fn kickoff_matrix_child() {
    use std::os::unix::fs::{MetadataExt,OpenOptionsExt};
    use std::io::Write;
    let root=std::path::PathBuf::from(std::env::var_os("APERTURE_D_ROOT").unwrap());
    assert!(root.file_name().unwrap().to_string_lossy().starts_with("aperture-d-"));
    let owner=RuntimeOwner::new(crate::controller::ControllerLock::acquire(&root).unwrap());
    let worker=owner.fixture_worker();
    let run=root.join(".aperture/run");let leaf=run.join("fixture.kickoff");
    assert_eq!(read_kickoff_millis(&root,"fixture"),KickoffRead::Absent);
    let recent=effect_now().unwrap();
    for case in ["fifo","oversize","overflow","malformed","whitespace","empty","symlink","writable","hardlink","0600","0640","0644"] {
        let value=recent.to_string();
        let bytes=match case { "oversize"=>b"123456789012345678901".as_slice(),
            "overflow"=>b"18446744073709551616", "malformed"=>b"-1", "whitespace"=>b"1\n", "empty"=>b"", _=>value.as_bytes() };
        if case=="fifo" {
            let name=std::ffi::CString::new(leaf.as_os_str().as_encoded_bytes()).unwrap();
            assert_eq!(unsafe{libc::mkfifo(name.as_ptr(),0o600)},0);
        } else if case=="symlink" {
            std::os::unix::fs::symlink("retained-target",&leaf).unwrap();
        } else {
            let mode=match case {"0640"=>0o640,"0644"=>0o644,"writable"=>0o622,_=>0o600};
            let mut file=std::fs::OpenOptions::new().write(true).create_new(true).mode(mode).open(&leaf).unwrap();
            file.write_all(bytes).unwrap();
            // Fixture setup fixes only this newly-created leaf so ambient umask
            // cannot silently turn the writer-compatible mode oracle into0600.
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(mode)).unwrap();
            if case=="hardlink" { std::fs::hard_link(&leaf,run.join("owned-extra-link")).unwrap(); }
        }
        let before_meta=std::fs::symlink_metadata(&leaf).unwrap();
        let before_files=crate::agents::lifecycle_tests::tree(&root); // does not read FIFO bodies
        let shared=Arc::new(Mutex::new(Shared::new()));
        let old=Watch{tracked_kickoff_millis:Some(17),attempts:2,last_attempt_at:Some(UNIX_EPOCH),latched:true};
        shared.lock().unwrap().watch.insert("fixture".into(),old);
        let s=state();let before_state=serde_json::to_vec(&s.lock().unwrap().agents["fixture"]).unwrap();
        let started=std::time::Instant::now();
        tick(&shared,&s,&worker); // the real decision ingress, not a copied reader
        assert!(started.elapsed()<Duration::from_secs(1),"bounded tick: {case}");
        let valid=matches!(case,"0600"|"0640"|"0644");
        let observed=read_kickoff_millis(&root,"fixture");
        let shared_guard=shared.try_lock().expect("Shared must be available after tick");
        let watch=&shared_guard.watch["fixture"];
        if valid {
            assert_eq!(observed,KickoffRead::Millis(recent),"{case}");
            assert_eq!(watch.tracked_kickoff_millis,Some(recent));
            assert_eq!(watch.attempts,0);assert!(!watch.latched);assert_eq!(watch.last_attempt_at,None);
            assert_eq!(s.lock().unwrap().agents["fixture"].dot_state.as_deref(),Some("booting"));
        } else {
            assert_eq!(observed,KickoffRead::Unverified,"{case}");
            assert_eq!((watch.tracked_kickoff_millis,watch.attempts,watch.last_attempt_at,watch.latched),(Some(17),2,Some(UNIX_EPOCH),true),"{case}");
            assert_eq!(serde_json::to_vec(&s.lock().unwrap().agents["fixture"]).unwrap(),before_state,"{case}");
        }
        drop(shared_guard);
        assert_eq!(crate::agents::lifecycle_tests::tree(&root),before_files,"no intent/filesystem effects: {case}");
        let after_meta=std::fs::symlink_metadata(&leaf).unwrap();
        assert_eq!((after_meta.dev(),after_meta.ino(),after_meta.mode(),after_meta.nlink()),(before_meta.dev(),before_meta.ino(),before_meta.mode(),before_meta.nlink()));
        assert!(!run.join("watchdog").exists());
        mark_disconnected(&shared);
        // Explicit fixture cleanup only, after immutable-state assertions.
        std::fs::remove_file(&leaf).unwrap();
        if case=="hardlink" { std::fs::remove_file(run.join("owned-extra-link")).unwrap(); }
        println!("kickoff case={case} bounded shared_available=true no_facts=true");
    }
    // Missing parent is never downgraded to a conclusively absent leaf and the
    // reader must not recreate it (owned setup/cleanup, after releasing owner).
    drop(worker);owner.close().unwrap();drop(owner);
    std::fs::rename(&run,root.join(".aperture/retained-run")).unwrap();
    assert_eq!(read_kickoff_millis(&root,"fixture"),KickoffRead::Unverified);
    assert!(!run.exists());
}

#[test]
fn d_kickoff_actual_tick_is_bounded_and_preserves_unverified_seats() {
    let h=Home::new();
    let mut child=std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact","watchdog::effect_tests::inert_effect_crash_entry","--ignored","--nocapture"])
        .env_clear().env("APERTURE_D_ROOT",&h.0).env("APERTURE_D_KICKOFF","matrix")
        .stdin(std::process::Stdio::null()).spawn().unwrap();
    let identity=crate::team_process::observe(child.id()).unwrap().unwrap().identity;
    let deadline=std::time::Instant::now()+Duration::from_secs(5);
    let status=loop {
        if let Some(status)=child.try_wait().unwrap() {break status;}
        if std::time::Instant::now()>=deadline {
            if crate::team_process::state(&identity)==crate::team_replacement::ProcessState::Same {child.kill().unwrap();}
            let _=child.wait();panic!("owned kickoff fixture deadline");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(status.success(),"owned kickoff matrix failed: {status}");
    assert_eq!(crate::team_process::state(&identity),crate::team_replacement::ProcessState::Gone);
}


#[test]
fn managed_monitor_expiry_only_signals_after_trustworthy_silence() {
    let mut watch = HashMap::new();
    let at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
    assert!(!managed_silence_due(&mut watch, "team-qa", "owner-a", true, true, at));
    assert!(!managed_silence_due(&mut watch, "team-qa", "owner-a", true, false, at));
    assert!(!managed_silence_due(&mut watch, "team-qa", "owner-a", true, false, at+Duration::from_secs(59)));
    assert!(managed_silence_due(&mut watch, "team-qa", "owner-a", true, false, at+Duration::from_secs(60)));
    // Watchdog subscriber loss is not agent death; no expiry/recovery claim.
    assert!(!managed_silence_due(&mut watch, "team-qa", "owner-a", false, false, at+Duration::from_secs(61)));
    assert!(!managed_silence_due(&mut watch, "team-qa", "owner-a", true, false, at+Duration::from_secs(120)));
    assert!(!managed_silence_due(&mut watch, "team-qa", "owner-b", true, false, at+Duration::from_secs(181)));
    assert!(managed_silence_due(&mut watch, "team-qa", "owner-b", true, false, at+Duration::from_secs(241)));
    assert!(!managed_silence_due(&mut watch, "team-qa", "owner-b", true, true, at+Duration::from_secs(242)));
    assert!(watch.is_empty()); // join clears only silence timer, not readiness
}
