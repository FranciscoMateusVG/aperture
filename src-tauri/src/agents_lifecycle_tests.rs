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
