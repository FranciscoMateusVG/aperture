use super::*;
use crate::journal::{ensure_private_dir, write_private_json_atomic};
use std::path::{Path, PathBuf};
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let p =
            std::env::temp_dir().join(format!("aperture-k310b-lifecycle-{}", uuid::Uuid::new_v4()));
        ensure_private_dir(&p).unwrap();
        ensure_private_dir(&p.join(".claude/aperture/seat")).unwrap();
        Self(p)
    }
    fn roots(&self) -> PathBuf {
        self.0.join(".claude/aperture")
    }
    fn team(&self, status: &str) {
        let dir = self.0.join(".aperture/teams/t1");
        ensure_private_dir(&dir).unwrap();
        write_private_json_atomic(&dir.join("team.json"),&serde_json::json!({
                "schema_version":1,"team":"t1","project":"project:aperture","repo":"aperture","mission":"Fixture mission","acceptance":"Fixture gate",
                "preset":{"id":null,"sha256":null},"lead":"seat","seats":[{"name":"seat","role":"backend","harness":"claude","model":"opus","reasoning":null}],
                "fallbacks":[],"grants":[],"created_at":"2026-09-20T00:00:00Z","creation_request_id":uuid::Uuid::new_v4().to_string(),"staging_uuid":uuid::Uuid::new_v4().to_string()
            }),false).unwrap();
        write_private_json_atomic(&dir.join("state.json"),&serde_json::json!({"schema_version":1,"state":status,"generation":if status=="active"{1}else{0},"epic_id":if status=="active"{Some("aperture-fixture")}else{None},"failure":if status=="failed"{Some(serde_json::json!({"code":"E_FIXTURE","completed_moves":0}))}else{None},"updated_at":"2026-09-20T00:00:00Z"}),false).unwrap();
        if status == "active" {
            for name in ["TEAM", ".complete"] {
                crate::journal::write_private_bytes_atomic(
                    &self.roots().join("seat").join(name),
                    b"",
                    false,
                )
                .unwrap();
            }
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap()
    }
}
fn denied(home: &Path, root: &Path) {
    assert_eq!(
        require_legacy_lifecycle_at(home, root, "seat"),
        Err(legacy_lifecycle_guard::DENIED.into())
    );
}
#[test]
fn authoritative_active_pending_failed_archived_and_invalid_reject_legacy() {
    for lifecycle in ["active", "pending", "failed", "archived", "invalid"] {
        let f = Fixture::new();
        f.team(lifecycle);
        denied(&f.0, &f.roots());
    }
}
#[test]
fn missing_active_marker_and_corrupt_registry_are_not_standing() {
    let f = Fixture::new();
    f.team("active");
    std::fs::remove_file(f.roots().join("seat/TEAM")).unwrap();
    denied(&f.0, &f.roots());
    std::fs::write(f.0.join(".aperture/teams/t1/state.json"), b"invalid").unwrap();
    denied(&f.0, &f.roots());
}
#[test]
fn standing_registry_remains_allowed_but_orphan_or_broken_marker_denies() {
    let f = Fixture::new();
    assert!(require_legacy_lifecycle_at(&f.0, &f.roots(), "seat").is_ok());
    std::os::unix::fs::symlink("missing", f.roots().join("seat/TEAM")).unwrap();
    denied(&f.0, &f.roots());
}
#[test]
fn all_legacy_entrypoints_guard_before_state_or_native_effects() {
    let source = include_str!("agents.rs");
    // Supplemental source oracle: these thin wrappers have a column-zero
    // closing brace. Bound the selected function before inspecting statements;
    // a guard in a later function/comment cannot satisfy this assertion.
    fn function_body<'a>(source: &'a str, name: &str) -> &'a str {
        let signature = format!("pub fn {name}(");
        let start = source.lines().position(|line| line.starts_with(&signature)).unwrap();
        let offset = source.split_inclusive('\n').take(start).map(str::len).sum::<usize>();
        let function = source[offset..].split_once("\n}").unwrap().0;
        assert!(!function.contains("\npub fn "));
        function.split_once(" {\n").unwrap().1
    }
    for name in ["start_agent", "stop_agent", "restart_agent", "update_agent_model"] {
        let body = function_body(source, name);
        let statements: Vec<_> = body.lines().map(str::trim).filter(|line| !line.is_empty()).collect();
        let call = if name == "update_agent_model" {
            "update_agent_model_shared(name.clone(), model, state.inner(), &work.lifecycle(&name)?)".to_string()
        } else {
            format!("{name}_shared(name.clone(), state.inner(), &work.lifecycle(&name)?)")
        };
        // Exact statements also reject extra effects or a textual match in a
        // comment. Runtime admission/body are RO lifecycle accounting; membership
        // still precedes the first shared/native/state-effect entry.
        assert_eq!(statements, vec![
            "let work = runtime.admit(Some(&name))?;",
            "let _body = work.body()?;",
            "require_legacy_lifecycle(&name)?;",
            call.as_str(),
        ], "unguarded or unaccounted {name}");
    }
    assert!(function_body(source, "boot_agent_process")
        .trim_start().starts_with("require_legacy_lifecycle(&agent.name)?;"));
}

#[test]
fn archived_directory_membership_denies_without_runtime_marker() {
    let f = Fixture::new();
    f.team("archived");
    ensure_private_dir(&f.0.join(".aperture/teams/archive")).unwrap();
    std::fs::rename(
        f.0.join(".aperture/teams/t1"),
        f.0.join(".aperture/teams/archive/t1"),
    )
    .unwrap();
    denied(&f.0, &f.roots());
}

#[test]
fn watchdog_guard_precedes_nudge_teardown_and_shared_boot() {
    let source = include_str!("watchdog.rs");
    let body = source.split("fn execute_rekick(").nth(1).unwrap()
        .split("fn ring_operator(").next().unwrap();
    let gate = body.find("if crate::agents::require_legacy_lifecycle(name).is_err()").unwrap();
    let after_gate = &body[gate..];
    assert!(after_gate.find("return;").unwrap() < after_gate.find("match tier").unwrap());
    let detached = body.find("detached_codex_denied_at(").unwrap();
    for effect in ["tmux_send_keys(", "tmux_kill_window(", "boot_agent_headless("] {
        assert!(detached < body.find(effect).unwrap());
        assert!(gate < body.find(effect).unwrap(), "effect before guard: {effect}");
    }
}
