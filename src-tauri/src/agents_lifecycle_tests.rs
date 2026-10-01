//! C3 actual synchronous ingress, private HOME, inert effect seams. No provider/CLI.
use super::*;
use crate::{controller::ControllerLock, journal::ensure_private_dir};
use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

pub(crate) fn state(name: &str, model: &str) -> Arc<Mutex<AppState>> {
    let agent: AgentDef = serde_json::from_value(serde_json::json!({
        "name":name,"model":model,"role":"backend","prompt_file":"never-read",
        "tmux_window_id":null,"status":"stopped"
    }))
    .unwrap();
    Arc::new(Mutex::new(AppState {
        agents: [(name.into(), agent)].into_iter().collect(),
        tmux_session: "inert".into(),
        mcp_server_path: String::new(),
        mcp_sentry_server_path: String::new(),
        db_path: String::new(),
        project_dir: String::new(),
        team_preparations: Arc::new(Mutex::new(crate::state::RuntimePermitStore::new())),
    }))
}
pub(crate) fn fixture(root: &Path) -> LifecycleFixture {
    LifecycleFixture {
        spec: None,
        preparation_file: root.join("prepared"),
        fail_preparation: false,
        preparation_count: AtomicUsize::new(0),
        effects: Mutex::new(vec![]),
        pause: None,
    }
}
struct Home(PathBuf);
impl Home {
    fn new() -> Self {
        let p = PathBuf::from(format!("/private/tmp/aperture-c3-{}", uuid::Uuid::new_v4()));
        // Explicit fixture setup before any state snapshot/ingress. Exclusive
        // creation of only our UUID leaf, private from the creation syscall.
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new().mode(0o700).create(&p).unwrap();
        ensure_private_dir(&p).unwrap();
        ensure_private_dir(&p.join(".claude/aperture/fixture")).unwrap();
        Self(p)
    }
}
impl Drop for Home {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
        assert!(!self.0.exists());
    }
}
pub(crate) fn tree(root: &Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
    fn walk(root: &Path, at: &Path, out: &mut std::collections::BTreeMap<PathBuf, Vec<u8>>) {
        for e in fs::read_dir(at).unwrap() {
            let p = e.unwrap().path();
            let m = fs::symlink_metadata(&p).unwrap();
            let bytes = if m.is_file() {
                fs::read(&p).unwrap()
            } else if m.file_type().is_symlink() {
                fs::read_link(&p)
                    .unwrap()
                    .as_os_str()
                    .as_encoded_bytes()
                    .to_vec()
            } else {
                vec![]
            };
            out.insert(p.strip_prefix(root).unwrap().into(), bytes);
            if m.is_dir() {
                walk(root, &p, out);
            }
        }
    }
    let mut out = std::collections::BTreeMap::new();
    walk(root, root, &mut out);
    out
}
#[test]
fn c3_all_shared_ingresses_deny_codex_before_state_files_or_effects() {
    for model in ["codex/test", "opus"] {
        let h = Home::new();
        let lease = ControllerLock::acquire(&h.0).unwrap();
        let mut ctx = LifecycleContext::fixture_context(&lease).unwrap();
        let fixture = fixture(&h.0);
        ctx.fixture = Some(&fixture);
        let app = state("fixture", model);
        if model == "opus" {
            ensure_private_dir(&h.0.join(".aperture/run/daemons/codex-fixture")).unwrap();
            fs::write(
                h.0.join(".aperture/run/daemons/codex-fixture/orphan"),
                b"unknown",
            )
            .unwrap();
        }
        let before = tree(&h.0);
        let plan = serde_json::to_value(&app.lock().unwrap().agents["fixture"]).unwrap();
        for call in [start_agent_shared, stop_agent_shared, restart_agent_shared] {
            assert_eq!(
                call("fixture".into(), &app, &ctx).unwrap_err(),
                LifecycleRefusal::InputsUnverified.code()
            );
        }
        assert!(update_agent_model_shared(
            "fixture".into(),
            if model == "opus" {
                "codex/test"
            } else {
                "opus"
            }
            .into(),
            &app,
            &ctx
        )
        .is_err());
        assert!(update_agent_model_shared("fixture".into(), model.into(), &app, &ctx).is_ok());
        crate::watchdog::c3_rekick_fixture(&app, "fixture", &h.0, false); // stale caller flag cannot bypass model/history
        assert!(fixture.effects.lock().unwrap().is_empty());
        assert_eq!(fixture.preparation_count.load(Ordering::SeqCst), 0);
        assert_eq!(tree(&h.0), before);
        assert_eq!(
            serde_json::to_value(&app.lock().unwrap().agents["fixture"]).unwrap(),
            plan
        );
    }
}
#[test]
fn c3_external_contention_zero_effects_internal_borrows_without_reacquire() {
    let h = Home::new();
    let lease = ControllerLock::acquire(&h.0).unwrap();
    let app = state("fixture", "opus");
    let before = tree(&h.0);
    assert!(crate::boot_agent_headless_external(&h.0, "fixture", &app).is_err());
    assert_eq!(tree(&h.0), before);
    let fixture = fixture(&h.0);
    let mut ctx = LifecycleContext::fixture_context(&lease).unwrap();
    ctx.fixture = Some(&fixture);
    start_agent_shared("fixture".into(), &app, &ctx).unwrap();
    assert_eq!(*fixture.effects.lock().unwrap(), vec!["legacy-boot"]);
    assert_eq!(app.lock().unwrap().agents["fixture"].status, "running");
    assert_eq!(tree(&h.0), before); // no external command/pane/file operation by inert seam
}
#[test]
fn c3_stale_advisory_plan_is_rechecked_under_slot_both_harness_directions() {
    for (old, new) in [("opus", "codex/test"), ("codex/test", "opus")] {
        let h = Home::new();
        let lease = ControllerLock::acquire(&h.0).unwrap();
        let app = state("fixture", old);
        let (arrived, notice) = std::sync::mpsc::sync_channel(1);
        let (resume, go) = std::sync::mpsc::sync_channel(1);
        let mut fixture = fixture(&h.0);
        fixture.pause = Some(LifecyclePause {
            arrived,
            resume: Mutex::new(go),
        });
        let mut ctx = LifecycleContext::fixture_context(&lease).unwrap();
        ctx.fixture = Some(&fixture);
        let before = tree(&h.0);
        std::thread::scope(|scope| {
            let task = scope.spawn(|| start_agent_shared("fixture".into(), &app, &ctx));
            notice.recv_timeout(Duration::from_secs(3)).unwrap();
            // Competing REAL model request is denied; it never creates the drift.
            let plain = LifecycleContext::fixture_context(&lease).unwrap();
            assert!(update_agent_model_shared("fixture".into(), new.into(), &app, &plain).is_err());
            assert_eq!(app.lock().unwrap().agents["fixture"].model, old);
            // Explicit advisory-state drift injection, not a claimed allowed model change.
            let slot = lease.codex_slot("fixture").unwrap();
            let op = slot.enter().unwrap();
            app.lock().unwrap().agents.get_mut("fixture").unwrap().model = new.into();
            resume.send(()).unwrap();
            drop(op);
            assert_eq!(
                task.join().unwrap().unwrap_err(),
                LifecycleRefusal::PlanChanged.code()
            );
        });
        assert!(fixture.effects.lock().unwrap().is_empty());
        assert_eq!(tree(&h.0), before);
    }
}
#[test]
fn c3_model_into_codex_and_missing_target_leave_no_mutation() {
    let h = Home::new();
    let lease = ControllerLock::acquire(&h.0).unwrap();
    let ctx = LifecycleContext::fixture_context(&lease).unwrap();
    let app = state("fixture", "opus");
    let before = tree(&h.0);
    assert!(update_agent_model_shared("fixture".into(), "codex/test".into(), &app, &ctx).is_err());
    assert_eq!(app.lock().unwrap().agents["fixture"].model, "opus");
    assert!(start_agent_shared("absent".into(), &app, &ctx).is_err());
    assert_eq!(tree(&h.0), before);
    assert!(crate::codex_appserver::spawn_app_server("fixture", "never-read").is_err());
    assert!(crate::codex_appserver::stop_app_server("fixture").is_err());
    crate::codex_appserver::shutdown();
    assert_eq!(tree(&h.0), before);
}

#[test]
fn c3_watchdog_claude_pristine_respawn_denied_before_all_effects() {
    let h = Home::new();
    let lease = ControllerLock::acquire(&h.0).unwrap();
    let app = state("fixture", "opus");
    {
        let mut state = app.lock().unwrap();
        let agent = state.agents.get_mut("fixture").unwrap();
        agent.status = "running".into();
        agent.tmux_window_id = Some("owned-fixture-pane".into());
    }
    // This is genuinely legacy Claude/no-history, so the Codex/history guard
    // cannot accidentally satisfy the Respawn oracle.
    require_legacy_lifecycle_at(&h.0, &h.0.join(".claude/aperture"), "fixture").unwrap();
    assert!(!h.0.join(".aperture/run/daemons/codex-fixture").exists());
    fs::write(h.0.join("pane-sentinel"), b"owned pane retained").unwrap();
    let before = tree(&h.0);
    let state_before = serde_json::to_value(&app.lock().unwrap().agents["fixture"]).unwrap();
    let effects = crate::watchdog::c3_rekick_fixture(&app, "fixture", &h.0, false);
    assert_eq!(effects, crate::watchdog::C3RekickEffects::default());
    assert_eq!(tree(&h.0), before);
    assert_eq!(serde_json::to_value(&app.lock().unwrap().agents["fixture"]).unwrap(), state_before);
    lease.verify_live().unwrap();

    // Positive control of the SAME ingress and actual effect-site interception:
    // Nudge still emits its two key sends, never real tmux in a test build.
    let nudge = crate::watchdog::c3_nudge_fixture(&app, "fixture", &h.0);
    assert_eq!(nudge.pane_keys, 2);
    assert_eq!(nudge.pane_kills, 0);
    assert_eq!(nudge.external_boots, 0);
    assert_eq!(nudge.pane_sentinel, "keys-sent");
    assert_eq!(tree(&h.0), before);
    assert_eq!(serde_json::to_value(&app.lock().unwrap().agents["fixture"]).unwrap(), state_before);
}

#[test]
fn d_accounted_context_is_same_seat_and_stops_before_later_mutation() {
    let h=Home::new();
    let owner=crate::daemons::RuntimeOwner::new(ControllerLock::acquire(&h.0).unwrap());
    let work=owner.admit(Some("fixture")).unwrap();
    let context=work.lifecycle("fixture").unwrap();
    let app=state("fixture","opus");
    let before=tree(&h.0);
    assert_eq!(context.classify("other").unwrap_err(),"E_RUNTIME_SELECTOR");
    assert_eq!(owner.fixture_close_short().unwrap_err(),"E_RUNTIME_DRAIN_INCOMPLETE");
    assert_eq!(update_agent_model_shared("fixture".into(),"sonnet".into(),&app,&context).unwrap_err(),"E_RUNTIME_CLOSING");
    assert_eq!(app.lock().unwrap().agents["fixture"].model,"opus");
    assert_eq!(tree(&h.0),before);
    drop(context);drop(work);owner.close().unwrap();
}

#[test]
fn local_codex_preparation_preserves_operator_semantics_and_validates_before_token() {
    let h=Home::new();let lease=ControllerLock::acquire(&h.0).unwrap();
    let tools=crate::daemons::LocalTools::fixture(&h.0,&std::env::current_exe().unwrap());
    let owner=crate::daemons::RuntimeOwner::local(lease,tools).unwrap();
    let work=owner.admit(Some("fixture")).unwrap();let body=work.body().unwrap();let context=work.lifecycle("fixture").unwrap();
    let app=state("fixture","codex/selected");
    let bus=h.0.join("bus.js");let sentry=h.0.join("sentry.js");fs::write(&bus,"synthetic").unwrap();fs::write(&sentry,"synthetic").unwrap();
    let home=h.0.join("codex-home");ensure_private_dir(&home).unwrap();
    let expected={let mut s=app.lock().unwrap();s.mcp_server_path=bus.to_string_lossy().into_owned();s.mcp_sentry_server_path=sentry.to_string_lossy().into_owned();s.project_dir=h.0.to_string_lossy().into_owned();s.agents["fixture"].clone()};
    let plan=LocalCodexPreparation{context:&context,state:&app,expected:&expected,codex_home:home.clone(),bus:bus.to_string_lossy().into_owned(),sentry:sentry.to_string_lossy().into_owned(),project:h.0.to_string_lossy().into_owned(),bus_pin:local_pinned_bytes(&bus,1024).unwrap().1,sentry_pin:local_pinned_bytes(&sentry,1024).unwrap().1,drift_point:None};
    let path=home.join("config.toml");
    fs::write(&path,"model='old'\n[mcp_servers.aperture-bus]\nenv=7\n[mcp_servers.sentry]\ncommand='old'\n").unwrap();
    assert!(plan.prepare().is_err());assert!(!h.0.join(".aperture/run/hub-tokens").exists());
    let original="model='old'\napproval_policy='on-request'\nsandbox_mode='workspace-write'\nunknown_future=['keep',3]\n[projects.'synthetic-project']\ntrust_level='untrusted'\n[model_providers.synthetic]\nname='fake'\nbase_url='http://127.0.0.1:1'\n[mcp_servers.aperture-bus]\ncommand='old'\nargs=['old']\ncustom_flag=true\n[mcp_servers.sentry]\ncommand='old'\nargs=['old']\n";
    fs::write(&path,original).unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&path,fs::Permissions::from_mode(0o600)).unwrap();
    plan.prepare().unwrap();
    let before:toml::Value=original.parse().unwrap();let after:toml::Value=fs::read_to_string(&path).unwrap().parse().unwrap();
    for key in ["approval_policy","sandbox_mode","unknown_future","projects","model_providers"]{assert_eq!(before[key],after[key]);}
    assert_eq!(after["model"].as_str(),Some("selected"));
    assert_eq!(after["mcp_servers"]["aperture-bus"]["custom_flag"].as_bool(),Some(true));
    assert_eq!(after["mcp_servers"]["sentry"]["args"][0].as_str(),sentry.to_str());
    assert!(h.0.join(".aperture/run/hub-tokens/fixture.token").is_file());
    drop(context);drop(body);drop(work);owner.close().unwrap();drop(owner);
}

#[test]
fn local_thread_resume_requires_exact_existing_uuid_and_close_cancels_wait(){
    let h=Home::new();let owner=crate::daemons::RuntimeOwner::new(ControllerLock::acquire(&h.0).unwrap());
    let work=owner.admit(None).unwrap();let path=h.0.join("thread-id");
    fs::write(&path,b"00000000-0000-4000-8000-000000000001").unwrap();
    assert_eq!(wait_local_thread(&path,&work).unwrap(),"00000000-0000-4000-8000-000000000001");
    // codex-bridge::publishThreadReady writes exactly `${threadId}\n`.
    let published=b"00000000-0000-4000-8000-000000000001\n";
    fs::write(&path,published).unwrap();
    let pin=LocalInputPin::of(&fs::symlink_metadata(&path).unwrap());
    assert_eq!(wait_local_thread(&path,&work).unwrap(),"00000000-0000-4000-8000-000000000001");
    assert_eq!(fs::read(&path).unwrap(),published);pin.recheck(&path).unwrap();
    for invalid in ["bad;command", "00000000-0000-4000-8000-000000000001\n\n", "00000000-0000-4000-8000-000000000001\r\n", " 00000000-0000-4000-8000-000000000001", "00000000-0000-4000-8000-000000000001 ", "00000000-0000-4000-8000-000000000001\nother", "\n"] {
        fs::write(&path,invalid).unwrap();assert_eq!(wait_local_thread(&path,&work).unwrap_err(),"E_LOCAL_THREAD_UNVERIFIED");
    }
    fs::remove_file(&path).unwrap();
    std::thread::scope(|scope|{
        let task=scope.spawn(||wait_local_thread(&path,&work));
        // close immediately closes admission, even though this wait owns work.
        assert_eq!(owner.fixture_close_short().unwrap_err(),"E_RUNTIME_DRAIN_INCOMPLETE");
        assert_eq!(task.join().unwrap().unwrap_err(),"E_RUNTIME_CLOSING");
    });
    drop(work);owner.close().unwrap();drop(owner);
}

#[test]
fn local_claude_uncertain_create_retains_shell_and_denies_repeat_start() {
    use std::os::unix::fs::PermissionsExt;
    let h=Home::new();
    let tool=h.0.join("tmux-inert");let events=h.0.join("events");let pane=h.0.join("retained-pane");
    // Same native bounded client as production. The own inert process records
    // its create effect, then reports failure; no real tmux or provider runs.
    let script=format!("#!/bin/sh\nprintf '%s\\n' \"$1\" >> '{}'\ncase \"$1\" in\n list-windows) if test -f '{}'; then printf '@42||fixture||zsh\\n'; fi;;\n new-window) printf 'owned shell' > '{}'; exit 9;;\n *) exit 8;;\nesac\n",events.display(),pane.display(),pane.display());
    fs::write(&tool,script).unwrap();fs::set_permissions(&tool,fs::Permissions::from_mode(0o700)).unwrap();
    ensure_private_dir(&h.0.join(".aperture/run/hub-tokens")).unwrap();
    let token=h.0.join(".aperture/run/hub-tokens/fixture.token");fs::write(&token,b"synthetic untouched token").unwrap();
    fs::write(h.0.join("configuration-sentinel"),b"unchanged").unwrap();
    let app=state("fixture","opus");
    let bus=h.0.join("bus.js");let sentry=h.0.join("sentry.js");fs::write(&bus,b"inert").unwrap();fs::write(&sentry,b"inert").unwrap();
    {let mut a=app.lock().unwrap();a.mcp_server_path=bus.to_string_lossy().into_owned();a.mcp_sentry_server_path=sentry.to_string_lossy().into_owned();}
    let tools=crate::daemons::LocalTools::fixture(&h.0,&tool);
    let owner=crate::daemons::RuntimeOwner::local(ControllerLock::acquire(&h.0).unwrap(),tools).unwrap();
    let work=owner.admit(Some("fixture")).unwrap();let body=work.body().unwrap();let ctx=work.lifecycle("fixture").unwrap();
    let before=serde_json::to_value(&app.lock().unwrap().agents["fixture"]).unwrap();
    assert_eq!(start_agent_shared("fixture".into(),&app,&ctx).unwrap_err(),"E_TMUX_OUTCOME_UNKNOWN");
    assert_eq!(fs::read(&pane).unwrap(),b"owned shell");
    assert_eq!(fs::read_to_string(&events).unwrap(),"list-windows\nnew-window\n");
    let mut retained=tree(&h.0);retained.remove(Path::new("events"));
    assert_eq!(start_agent_shared("fixture".into(),&app,&ctx).unwrap_err(),"E_LOCAL_PANE_ALREADY_PRESENT");
    let mut after=tree(&h.0);after.remove(Path::new("events"));assert_eq!(retained,after);
    drop(ctx);drop(body);drop(work);owner.close().unwrap();drop(owner);
    // A fresh owner/context still asks the real client; no in-memory marker
    // manufactured the denial. A shell remains stopped for UI classification.
    let tools=crate::daemons::LocalTools::fixture(&h.0,&tool);
    let owner=crate::daemons::RuntimeOwner::local(ControllerLock::acquire(&h.0).unwrap(),tools).unwrap();
    let work=owner.admit(Some("fixture")).unwrap();let body=work.body().unwrap();let ctx=work.lifecycle("fixture").unwrap();
    assert_eq!(start_agent_shared("fixture".into(),&app,&ctx).unwrap_err(),"E_LOCAL_PANE_ALREADY_PRESENT");
    let mut after=tree(&h.0);after.remove(Path::new("events"));assert_eq!(retained,after);
    assert_eq!(fs::read_to_string(&events).unwrap(),"list-windows\nnew-window\nlist-windows\nlist-windows\n");
    assert_eq!(serde_json::to_value(&app.lock().unwrap().agents["fixture"]).unwrap(),before);
    assert_eq!(fs::read(&token).unwrap(),b"synthetic untouched token");
    assert!(find_running_window(&[tmux::WindowInfo{window_id:"@42".into(),name:"fixture".into(),command:"zsh".into()}],"fixture").is_none());
    drop(ctx);drop(body);drop(work);owner.close().unwrap();drop(owner);
}

pub(super) fn local_config_drift(plan:&LocalCodexPreparation<'_>,point:&str){
    if plan.drift_point==Some(point){
        assert!(plan.codex_home.parent().unwrap().file_name().unwrap().to_string_lossy().starts_with("aperture-c3-"));
        fs::write(plan.codex_home.join("config.toml"),b"model='operator-change'\n").unwrap();
    }
}
#[test]
fn local_codex_config_drift_preserves_operator_edit_at_both_write_boundaries(){
    use std::os::unix::fs::PermissionsExt;
    for point in ["before-token","before-replace"] {
        let h=Home::new();let tools=crate::daemons::LocalTools::fixture(&h.0,&std::env::current_exe().unwrap());
        let owner=crate::daemons::RuntimeOwner::local(ControllerLock::acquire(&h.0).unwrap(),tools).unwrap();
        let work=owner.admit(Some("fixture")).unwrap();let body=work.body().unwrap();let context=work.lifecycle("fixture").unwrap();
        let app=state("fixture","codex/selected");let bus=h.0.join("bus.js");let sentry=h.0.join("sentry.js");
        fs::write(&bus,b"inert").unwrap();fs::write(&sentry,b"inert").unwrap();
        let home=h.0.join("codex-home");ensure_private_dir(&home).unwrap();let config=home.join("config.toml");
        fs::write(&config,b"model='initial'\n[mcp_servers.aperture-bus]\ncommand='old'\n[mcp_servers.sentry]\ncommand='old'\n").unwrap();fs::set_permissions(&config,fs::Permissions::from_mode(0o600)).unwrap();
        let expected={let mut s=app.lock().unwrap();s.mcp_server_path=bus.to_string_lossy().into_owned();s.mcp_sentry_server_path=sentry.to_string_lossy().into_owned();s.project_dir=h.0.to_string_lossy().into_owned();s.agents["fixture"].clone()};
        let mut plan=LocalCodexPreparation{context:&context,state:&app,expected:&expected,codex_home:home,bus:bus.to_string_lossy().into_owned(),sentry:sentry.to_string_lossy().into_owned(),project:h.0.to_string_lossy().into_owned(),bus_pin:local_pinned_bytes(&bus,1024).unwrap().1,sentry_pin:local_pinned_bytes(&sentry,1024).unwrap().1,drift_point:Some(point)};
        assert!(plan.prepare().is_err());assert_eq!(fs::read(&config).unwrap(),b"model='operator-change'\n");
        assert_eq!(h.0.join(".aperture/run/hub-tokens/fixture.token").exists(),point=="before-replace");
        // After-token drift is partial, not zero-effect rollback. Script drift
        // also denies against its previously loaded pin, not a freshly relabeled file.
        plan.drift_point=None;fs::write(&bus,b"other").unwrap();assert!(plan.recheck().is_err());
        drop(context);drop(body);drop(work);owner.close().unwrap();drop(owner);
    }
}
#[test]
fn local_claude_staging_denies_existing_leaves_before_effects_and_writes_private_modes(){
    use std::os::unix::fs::{PermissionsExt,MetadataExt};
    for leaf in ["mcp.json","prompt.md","launch.sh"] {for symlink in [false,true] {
        let h=Home::new();let tool=h.0.join("tmux-inert");let events=h.0.join("events");
        fs::write(&tool,format!("#!/bin/sh\nprintf '%s\\n' \"$1\" >> '{}'\ntest \"$1\" = list-windows\n",events.display())).unwrap();fs::set_permissions(&tool,fs::Permissions::from_mode(0o700)).unwrap();
        let tools=crate::daemons::LocalTools::fixture(&h.0,&tool);let owner=crate::daemons::RuntimeOwner::local(ControllerLock::acquire(&h.0).unwrap(),tools).unwrap();
        let work=owner.admit(Some("fixture")).unwrap();let body=work.body().unwrap();let ctx=work.lifecycle("fixture").unwrap();let app=state("fixture","opus");
        let staging=h.0.join(".aperture/run/launch/fixture");ensure_private_dir(&staging).unwrap();let target=h.0.join("untouched");fs::write(&target,b"synthetic only").unwrap();
        if symlink{std::os::unix::fs::symlink(&target,staging.join(leaf)).unwrap();}else{fs::write(staging.join(leaf),b"permissive existing").unwrap();fs::set_permissions(staging.join(leaf),fs::Permissions::from_mode(0o644)).unwrap();}
        let before=tree(&h.0);assert_eq!(start_agent_shared("fixture".into(),&app,&ctx).unwrap_err(),"E_LOCAL_STAGING_EXISTS");
        let mut after=tree(&h.0);after.remove(Path::new("events"));assert_eq!(before,after);
        assert_eq!(fs::read_to_string(&events).unwrap(),"list-windows\n");assert!(!h.0.join(".aperture/run/hub-tokens").exists());
        drop(ctx);drop(body);drop(work);owner.close().unwrap();drop(owner);
    }}
    let h=Home::new();let owner=crate::daemons::RuntimeOwner::new(ControllerLock::acquire(&h.0).unwrap());
    let work=owner.admit(Some("fixture")).unwrap();let body=work.body().unwrap();let ctx=work.lifecycle("fixture").unwrap();
    let slot=ctx.lease.codex_slot("fixture").unwrap();let op=slot.enter().unwrap();let staging=LocalClaudeStaging::prepare(&ctx,&op,"fixture").unwrap();
    for (leaf,mode) in [("mcp.json",0o600),("prompt.md",0o600),("launch.sh",0o700)]{
        staging.write_new(&ctx,&op,leaf,b"synthetic",mode).unwrap();assert_eq!(fs::metadata(staging.root.join(leaf)).unwrap().mode()&0o7777,mode);
    }
    assert_eq!(fs::metadata(&staging.root).unwrap().mode()&0o7777,0o700);staging.recheck_written(&ctx,&op).unwrap();
    assert!(staging.write_new(&ctx,&op,"mcp.json",b"overwrite denied",0o600).is_err());
    drop(op);drop(slot);drop(ctx);drop(body);drop(work);owner.close().unwrap();drop(owner);
}

#[test]
fn coordinator_stop_real_ingress_reaps_only_selected_native_pane() {
    use std::os::unix::fs::PermissionsExt;
    let tmux=Path::new("/opt/homebrew/bin/tmux");
    assert!(tmux.is_file(),"native tmux is required for this macOS regression");
    let h=Home::new();let label=format!("yvcnp-{}",uuid::Uuid::new_v4());
    struct Cleanup<'a>(&'a Path,String);
    impl Drop for Cleanup<'_>{fn drop(&mut self){let _=std::process::Command::new(self.0).args(["-L",&self.1,"kill-server"]).output();}}
    let _cleanup=Cleanup(tmux,label.clone());
    let invoke=|args:&[&str]|{
        let output=std::process::Command::new(tmux).env_clear().env("HOME",&h.0).env("PATH","/usr/bin:/bin")
            .args(["-L",&label,"-f","/dev/null"]).args(args).output().unwrap();
        assert!(output.status.success(),"own tmux status {:?}",output.status);output.stdout
    };
    invoke(&["new-session","-d","-s","inert","-n","fixture","/bin/sleep 60"]);
    invoke(&["new-window","-d","-t","inert:","-n","unrelated","/bin/sleep 60"]);
    let tool=h.0.join("tmux-own-server");
    fs::write(&tool,format!("#!/bin/sh\nexec '{}' -L '{}' \"$@\"\n",tmux.display(),label)).unwrap();
    fs::set_permissions(&tool,fs::Permissions::from_mode(0o700)).unwrap();
    let owner=crate::daemons::RuntimeOwner::local(ControllerLock::acquire(&h.0).unwrap(),crate::daemons::LocalTools::fixture(&h.0,&tool)).unwrap();
    let work=owner.admit(Some("fixture")).unwrap();let body=work.body().unwrap();let ctx=work.lifecycle("fixture").unwrap();
    let app=state("fixture","opus");
    let before=local_panes("inert","fixture",&work).unwrap();assert_eq!(before.len(),1);
    let id=crate::team_process::observe(before[0].pid).unwrap().unwrap().identity;
    // Deliberately stopped cached status: native facts, not stale UI, govern Stop.
    stop_agent_shared("fixture".into(),&app,&ctx).unwrap();
    assert_eq!(crate::team_process::state(&id),crate::team_replacement::ProcessState::Gone);
    assert!(local_panes("inert","fixture",&work).unwrap().is_empty());
    let unrelated=local_panes("inert","unrelated",&work).unwrap();assert_eq!(unrelated.len(),1);
    assert!(crate::team_process::observe(unrelated[0].pid).unwrap().is_some());
    assert_eq!(app.lock().unwrap().agents["fixture"].status,"stopped");
    drop(ctx);drop(body);drop(work);owner.close().unwrap();drop(owner);
}

#[test]
fn coordinator_foreign_pane_after_gone_reports_unknown_but_not_running() {
    use std::os::unix::fs::PermissionsExt;
    let h=Home::new();let tool=h.0.join("tmux-inert");let count=h.0.join("count");
    // First two observations prove no old pane. A concurrent new pane appears
    // only at the post-stop sweep; it must not inherit any signal authority.
    let mut foreign=std::process::Command::new("/bin/sleep").arg("60").spawn().unwrap();
    let id=crate::team_process::observe(foreign.id()).unwrap().unwrap().identity;
    fs::write(&tool,format!("#!/bin/sh\nn=0; test ! -f '{}' || read n < '{}'\nn=$((n+1)); printf '%s' \"$n\" > '{}'\nif test \"$n\" -ge 3; then printf '@9||fixture||%%9||{}||0\\n'; fi\n",count.display(),count.display(),count.display(),foreign.id())).unwrap();
    fs::set_permissions(&tool,fs::Permissions::from_mode(0o700)).unwrap();
    let owner=crate::daemons::RuntimeOwner::local(ControllerLock::acquire(&h.0).unwrap(),crate::daemons::LocalTools::fixture(&h.0,&tool)).unwrap();
    let work=owner.admit(Some("fixture")).unwrap();let body=work.body().unwrap();let ctx=work.lifecycle("fixture").unwrap();
    let app=state("fixture","opus");app.lock().unwrap().agents.get_mut("fixture").unwrap().status="running".into();
    let result=stop_agent_shared("fixture".into(),&app,&ctx);
    let observed=crate::team_process::state(&id);foreign.kill().unwrap();foreign.wait().unwrap();
    assert_eq!(result.unwrap_err(),"E_LIFECYCLE_OUTCOME_UNKNOWN");
    assert_eq!(observed,crate::team_replacement::ProcessState::Same);
    assert_eq!(app.lock().unwrap().agents["fixture"].status,"stopped");
    assert!(app.lock().unwrap().agents["fixture"].tmux_window_id.is_none());
    drop(ctx);drop(body);drop(work);owner.close().unwrap();drop(owner);
}
