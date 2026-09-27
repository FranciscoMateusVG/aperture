use super::*;
use crate::daemons::{RuntimeOwner, tests::{Home, client}};
fn inputs(pause: Option<(usize,std::sync::mpsc::SyncSender<()>,Mutex<std::sync::mpsc::Receiver<()>>)>) -> NudgeInputs {
    let me=crate::team_process::observe(std::process::id()).unwrap().unwrap();
    NudgeInputs{fixture:Some(NudgeFixture{
        target:EffectTarget{window:"@1".into(),pane:"%1".into(),pid:me.identity.pid,birth:me.identity.start_time,uid:me.uid},
        clients:(0..4).map(|_|client("ok")).collect(),pause,target_after:None,crash:None,
    })}
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
    assert_eq!(work.nudge(&s,"fixture",NudgeProducer::RekickNudge,inputs(None)).unwrap(),DispatchOutcome::DispatchAccepted);
    let before=crate::agents::lifecycle_tests::tree(&h.0);
    assert!(work.nudge(&s,"fixture",NudgeProducer::UnreadNudge,inputs(None)).is_err());
    assert_eq!(crate::agents::lifecycle_tests::tree(&h.0),before);
}
#[test]
fn d_close_before_internal_or_delayed_enter_retains_unknown_on_restart() {
    for index in [1,2,3] {
        let h=Home::new();let owner=h.owner();let s=state();
        let (arrived,notice)=std::sync::mpsc::sync_channel(1);
        let (resume,go)=std::sync::mpsc::sync_channel(1);
        let spec=inputs(Some((index,arrived,Mutex::new(go))));
        std::thread::scope(|scope| {
            let work=owner.admit(Some("fixture")).unwrap();let state=s.clone();
            let task=scope.spawn(move || {
                let _body=work.body().unwrap();
                work.nudge(&state,"fixture",NudgeProducer::RekickNudge,spec)
            });
            notice.recv_timeout(Duration::from_secs(3)).unwrap();
            assert_eq!(owner.fixture_close_short().unwrap_err(),"E_RUNTIME_DRAIN_INCOMPLETE");
            resume.send(()).unwrap();
            assert_eq!(task.join().unwrap().unwrap(),DispatchOutcome::Unknown);
        });
        owner.close().unwrap();drop(owner);
        let restarted=Arc::new(RuntimeOwner::new(crate::controller::ControllerLock::acquire(&h.0).unwrap()));
        let before=crate::agents::lifecycle_tests::tree(&h.0);
        let work=restarted.admit(Some("fixture")).unwrap();let _body=work.body().unwrap();
        assert!(work.nudge(&s,"fixture",NudgeProducer::UnreadNudge,inputs(None)).is_err());
        assert_eq!(crate::agents::lifecycle_tests::tree(&h.0),before);
    }
}
#[test]
fn d_target_drift_and_partial_dispatch_are_unknown_not_retried() {
    let h=Home::new();let owner=h.owner();let s=state();
    let mut spec=inputs(None);
    let f=spec.fixture.as_mut().unwrap();
    let mut changed=f.target.clone();changed.pane="%2".into();f.target_after=Some((1,changed));
    let work=owner.admit(Some("fixture")).unwrap();let _body=work.body().unwrap();
    assert_eq!(work.nudge(&s,"fixture",NudgeProducer::RekickNudge,spec).unwrap(),DispatchOutcome::Unknown);
    let before=crate::agents::lifecycle_tests::tree(&h.0);
    assert!(work.nudge(&s,"fixture",NudgeProducer::UnreadNudge,inputs(None)).is_err());
    assert_eq!(crate::agents::lifecycle_tests::tree(&h.0),before);
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
    let spec=inputs(Some((1,arrived,Mutex::new(go))));
    std::thread::scope(|scope| {
        let work=owner.admit(Some("fixture")).unwrap();let state=s.clone();
        let task=scope.spawn(move || { let _body=work.body().unwrap();work.nudge(&state,"fixture",NudgeProducer::RekickNudge,spec) });
        notice.recv_timeout(Duration::from_secs(3)).unwrap();
        // Actual membership classifier now sees unknown managed inventory.
        crate::journal::ensure_private_dir(&h.0.join(".aperture/teams")).unwrap();
        std::fs::write(h.0.join(".aperture/teams/broken.json"),b"invalid").unwrap();
        s.lock().unwrap().agents.get_mut("fixture").unwrap().model="codex/test".into();
        resume.send(()).unwrap();
        assert_eq!(task.join().unwrap().unwrap(),DispatchOutcome::Unknown);
    });
    let h=Home::new();let owner=h.owner();let s=state();
    let work=owner.admit(Some("fixture")).unwrap();let _body=work.body().unwrap();
    let mut fail=inputs(None);
    fail.fixture.as_mut().unwrap().clients[0]=crate::daemons::ClientInput::fixture(h.0.join("absent-owned-executable"),vec![],vec![]);
    assert_eq!(work.nudge(&s,"fixture",NudgeProducer::RekickNudge,fail).unwrap(),DispatchOutcome::NoDispatch);
    assert_eq!(work.nudge(&s,"fixture",NudgeProducer::UnreadNudge,inputs(None)).unwrap(),DispatchOutcome::DispatchAccepted);
}
#[test]
#[ignore = "owned crash child entry only"]
fn inert_effect_crash_entry() {
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
