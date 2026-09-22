//! Local private fixture only. No installed auth, harness, hub or provider.
use super::*;
use crate::journal::write_private_json_atomic;
use crate::state::ReasoningEffort;
use std::os::unix::fs::{symlink, PermissionsExt};
struct Fixture {
    home: PathBuf,
    infra: PathBuf,
    cwd: PathBuf,
    executable: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let home =
            std::env::temp_dir().join(format!("al-{}", &uuid::Uuid::new_v4().to_string()[..8]));
        ensure_private_dir(&home).unwrap();
        let home = home.canonicalize().unwrap();
        let f = Self {
            infra: home.join("infra"),
            cwd: home.join("projects/aperture"),
            executable: home.join("tools/codex-fixture"),
            home,
        };
        ensure_private_dir(&f.cwd).unwrap();
        let status = std::process::Command::new("/usr/bin/git")
            .args(["init", "-q"])
            .arg(&f.cwd)
            .status()
            .unwrap();
        assert!(status.success());
        f.write(".codex/auth.json", b"{}");
        f.write("tools/codex-fixture", b"synthetic never executed");
        f.write("tools/node-fixture", b"synthetic node never executed");
        std::fs::set_permissions(
            f.home.join("tools/node-fixture"),
            std::fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        f.write(
            ".volta/bin/node",
            format!(
                "#!/bin/sh\nprintf '%s\\n' '{}'\n",
                f.home.join("tools/node-fixture").display()
            )
            .as_bytes(),
        );
        std::fs::set_permissions(
            f.home.join(".volta/bin/node"),
            std::fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        std::fs::set_permissions(&f.executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        for p in [
            "mcp-server/start.sh",
            "mcp-server/dist/index.js",
            "mcp-server-sentry/dist/index.js",
        ] {
            f.write(&format!("infra/{p}"), b"fixture output");
        }
        f.json(".aperture/teams/t1/team.json",serde_json::json!({
            "schema_version":1,"team":"t1","project":"project:aperture","repo":"aperture","mission":"fixture","acceptance":"fixture",
            "preset":{"id":null,"sha256":null},"lead":"t1-worker","seats":[{"name":"t1-worker","role":"lead","harness":"codex","model":"gpt-6-astra","reasoning":"high"}],
            "fallbacks":[],"grants":[],"created_at":"2026-09-20T00:00:00Z","creation_request_id":uuid::Uuid::new_v4().to_string(),"staging_uuid":uuid::Uuid::new_v4().to_string()}));
        f.json(".aperture/teams/t1/state.json",serde_json::json!({"schema_version":1,"state":"active","generation":1,"epic_id":"aperture-fixture","failure":null,"updated_at":"2026-09-20T00:00:00Z"}));
        f.json(".claude/aperture/t1-worker/manifest.json",serde_json::json!({"name":"t1-worker","role":"lead","model":"codex/gpt-6-astra","enabled":true}));
        f.json(
            ".claude/aperture/t1-worker/TEAM",
            serde_json::json!({"schema_version":1,"team":"t1","role":"lead"}),
        );
        for (p, b) in [
            ("prompt.md", "fixture mission"),
            (".complete", ""),
            ("resident.txt", "constitution\nlead-core\n"),
            ("skills/constitution/SKILL.md", "fixture constitution"),
            ("skills/lead-core/SKILL.md", "fixture lead"),
        ] {
            f.write(&format!(".claude/aperture/t1-worker/{p}"), b.as_bytes());
        }
        f
    }
    fn write(&self, p: &str, b: &[u8]) {
        let p = self.home.join(p);
        ensure_private_dir(p.parent().unwrap()).unwrap();
        write_private_bytes_atomic(&p, b, true).unwrap();
    }
    fn json(&self, p: &str, v: serde_json::Value) {
        let p = self.home.join(p);
        ensure_private_dir(p.parent().unwrap()).unwrap();
        write_private_json_atomic(&p, &v, true).unwrap();
    }
    fn plan(&self) -> Result<NativeLaunchBinding, ReplacementError> {
        NativeLaunchBinding::preflight_at(
            &self.home,
            "t1",
            "t1-worker",
            &selected(),
            self.cwd.clone(),
            &Deadline::new(),
            self.infra.clone(),
            self.executable.clone(),
        )
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.home);
    }
}
fn selected() -> ExecutionTuple {
    ExecutionTuple {
        harness: Harness::Codex,
        model: "gpt-6-astra".into(),
        reasoning: Some(ReasoningEffort::High),
    }
}
#[test]
fn launch_preflight_has_no_publication_and_preserves_source() {
    let f = Fixture::new();
    let p = f.plan().unwrap();
    p.revalidate(&Deadline::new()).unwrap();
    assert_eq!(p.cwd, f.cwd);
    assert_eq!(p.skills.len(), 2);
    assert!(!f.home.join(".aperture/run/managed").exists());
    assert_eq!(
        std::fs::read(f.home.join(".codex/auth.json")).unwrap(),
        b"{}"
    );
}
#[test]
fn generated_config_is_exact_toml_and_values_are_not_syntax_or_argv() {
    let f = Fixture::new();
    let dest = f.home.join("private generation");
    let token = f.home.join(".aperture/run/hub-tokens/t1-worker.token");
    let canary = "fixture\"\n[evil]\nvalue='not-real'";
    let output = config(
        &f.home,
        &f.infra,
        &f.home.join("tools/node-fixture"),
        "t1-worker",
        "lead",
        &selected(),
        1,
        &f.cwd,
        &dest,
        &token,
        canary,
    )
    .unwrap();
    let parsed: toml::Value = toml::from_str(std::str::from_utf8(&output.0).unwrap()).unwrap();
    assert!(parsed.get("evil").is_none());
    assert_eq!(parsed["model"].as_str(), Some("gpt-6-astra"));
    assert_eq!(parsed["model_reasoning_effort"].as_str(), Some("high"));
    for server in ["aperture-bus", "sentry"] {
        assert_eq!(
            parsed["mcp_servers"][server]["command"].as_str(),
            f.home.join("tools/node-fixture").to_str()
        );
        let script = if server == "aperture-bus" {
            "mcp-server/dist/index.js"
        } else {
            "mcp-server-sentry/dist/index.js"
        };
        assert_eq!(
            parsed["mcp_servers"][server]["args"][0].as_str(),
            f.infra.join(script).to_str()
        );
        assert_eq!(
            parsed["mcp_servers"][server]["env"]["APERTURE_TEAM_GENERATION"].as_str(),
            Some("1")
        );
        assert_eq!(
            parsed["mcp_servers"][server]["env"]["BEADS_DOLT_PASSWORD"].as_str(),
            Some(canary)
        );
        assert!(!parsed["mcp_servers"][server]["args"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x.as_str() == Some(canary)));
    }
}
#[test]
fn launch_preflight_rejects_source_identity_and_unapproved_tuple() {
    let f = Fixture::new();
    f.json(
        ".claude/aperture/t1-worker/TEAM",
        serde_json::json!({"schema_version":1,"team":"other","role":"lead"}),
    );
    assert!(f.plan().is_err());
    let f = Fixture::new();
    let mut t = selected();
    t.model = "unapproved".into();
    assert!(NativeLaunchBinding::preflight_at(
        &f.home,
        "t1",
        "t1-worker",
        &t,
        f.cwd.clone(),
        &Deadline::new(),
        f.infra.clone(),
        f.executable.clone()
    )
    .is_err());
}
#[test]
fn launch_revalidation_rejects_output_and_skill_drift() {
    for changed in [
        "infra/mcp-server/dist/index.js",
        ".claude/aperture/t1-worker/prompt.md",
        ".claude/aperture/t1-worker/skills/lead-core/SKILL.md",
    ] {
        let f = Fixture::new();
        let p = f.plan().unwrap();
        f.write(changed, b"changed fixture");
        assert!(p.revalidate(&Deadline::new()).is_err());
    }
}
#[test]
fn launch_private_paths_reject_symlinks_hardlinks_and_loose_permissions() {
    for variant in 0..3 {
        let f = Fixture::new();
        let p = f.home.join(".codex/auth.json");
        match variant {
            0 => {
                std::fs::remove_file(&p).unwrap();
                symlink(f.executable.clone(), &p).unwrap();
            }
            1 => std::fs::hard_link(&p, f.home.join("alias")).unwrap(),
            _ => std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap(),
        }
        assert!(f.plan().is_err());
    }
    let f = Fixture::new();
    symlink(
        &f.executable,
        f.home.join(".claude/aperture/t1-worker/skills/swap"),
    )
    .unwrap();
    assert!(f.plan().is_err());
}
#[test]
fn launch_revalidation_rejects_replaced_cwd_identity() {
    let f = Fixture::new();
    let p = f.plan().unwrap();
    std::fs::rename(&f.cwd, f.home.join("old-repo")).unwrap();
    ensure_private_dir(&f.cwd).unwrap();
    assert!(p.revalidate(&Deadline::new()).is_err());
}
#[test]
fn launch_publish_requires_exact_bound_owner_before_generation_files() {
    let f = Fixture::new();
    let p = f.plan().unwrap();
    let actor = crate::team_auth::AuthenticatedActor::launcher();
    let store = OwnerStore::new(f.home.join(".aperture/run/owner"));
    store
        .initialize_owner(&actor, "t1-worker", selected())
        .unwrap();
    let res = store
        .reserve_start(&actor, "t1-worker", 0, selected())
        .unwrap();
    let token = crate::hub_auth::managed::provision(&f.home, "t1", &actor, &res).unwrap();
    store.abort_start(&actor, &res).unwrap();
    assert!(p
        .publish_with_password(&res, &token, &Deadline::new(), "synthetic-fixture-only")
        .is_err());
    assert!(!f.home.join(".aperture/run/managed").exists());
}
#[test]
fn launch_publish_private_generation_is_no_reuse_and_never_executes_fixture() {
    let f = Fixture::new();
    let p = f.plan().unwrap();
    let actor = crate::team_auth::AuthenticatedActor::launcher();
    let store = OwnerStore::new(f.home.join(".aperture/run/owner"));
    store
        .initialize_owner(&actor, "t1-worker", selected())
        .unwrap();
    let res = store
        .reserve_start(&actor, "t1-worker", 0, selected())
        .unwrap();
    let token = crate::hub_auth::managed::provision(&f.home, "t1", &actor, &res).unwrap();
    let spec = p
        .publish_with_password(&res, &token, &Deadline::new(), "synthetic-fixture-only")
        .unwrap();
    assert_eq!(spec.program, f.executable);
    assert_eq!(spec.cwd, f.cwd);
    assert_eq!(spec.args[0], "app-server");
    let dest = f.home.join(".aperture/run/managed/t1-worker/g1");
    for name in ["config.toml", "auth.json", "prompt.md"] {
        assert_eq!(
            std::fs::metadata(dest.join(name)).unwrap().mode() & 0o777,
            0o600
        );
    }
    assert_eq!(std::fs::metadata(&dest).unwrap().mode() & 0o777, 0o700);
    assert!(
        std::str::from_utf8(&std::fs::read(dest.join("prompt.md")).unwrap())
            .unwrap()
            .contains("fixture constitution")
    );
    assert!(p
        .publish_with_password(&res, &token, &Deadline::new(), "synthetic-fixture-only")
        .is_err());
    assert!(store.read_owner("t1-worker").unwrap().incarnation.is_none());
}

#[test]
fn socket_cleanup_metadata_boundary_rejects_files_links_and_cross_seat_paths() {
    let f = Fixture::new();
    ensure_private_dir(&f.home.join(".aperture/run")).unwrap();
    assert!(unlink_socket(&f.home, "../other").is_err());
    unlink_socket(&f.home, "t1-worker").unwrap();
    let path = f.home.join(".aperture/run/t1-worker.sock");
    write_private_bytes_atomic(&path, b"not a socket", false).unwrap();
    assert!(unlink_socket(&f.home, "t1-worker").is_err());
    assert!(path.exists());
    std::fs::remove_file(&path).unwrap();
    symlink(&f.executable, &path).unwrap();
    assert!(unlink_socket(&f.home, "t1-worker").is_err());
    assert!(f.executable.exists());
    std::fs::remove_file(&path).unwrap();
    let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
    drop(listener);
    unlink_socket(&f.home, "t1-worker").unwrap();
    assert!(!path.exists());
}

#[test]
fn codex_large_executable_is_pinned_at_preflight_and_revalidation_only() {
    let f = Fixture::new();
    let binary = std::fs::OpenOptions::new()
        .write(true)
        .open(&f.executable)
        .unwrap();
    // Sparse local fixture; never executed, no installed credentials/provider.
    binary.set_len(INSTALLED_FILE_CAP + 1).unwrap();
    assert!(installed_file(&f.executable).is_err());
    let plan = f.plan().unwrap();
    plan.revalidate(&Deadline::new()).unwrap();
    assert!(!f.home.join(".aperture/run/managed").exists());
    let script = f.infra.join("mcp-server/dist/index.js");
    std::fs::OpenOptions::new()
        .write(true)
        .open(&script)
        .unwrap()
        .set_len(INSTALLED_FILE_CAP + 1)
        .unwrap();
    assert!(plan.revalidate(&Deadline::new()).is_err());
    assert!(f.plan().is_err());
    binary.set_len(CODEX_EXECUTABLE_CAP + 1).unwrap();
    assert!(installed_pin(&f.executable, &f.executable).is_err());
}

#[test]
fn codex_executable_larger_cap_keeps_installed_identity_guards() {
    for variant in 0..3 {
        let f = Fixture::new();
        match variant {
            0 => {
                std::fs::remove_file(&f.executable).unwrap();
                symlink(f.infra.join("mcp-server/start.sh"), &f.executable).unwrap();
            }
            1 => std::fs::hard_link(&f.executable, f.home.join("binary-alias")).unwrap(),
            _ => std::fs::set_permissions(&f.executable, std::fs::Permissions::from_mode(0o777))
                .unwrap(),
        }
        assert!(installed_pin(&f.executable, &f.executable).is_err());
        assert!(f.plan().is_err());
    }
}

#[test]
fn volta_only_node_is_absolute_pinned_and_drift_fails_closed() {
    let f = Fixture::new();
    let p = f.plan().unwrap();
    assert_eq!(p.node, f.home.join("tools/node-fixture"));
    assert!(p.pins.contains_key(&p.node));
    f.write("tools/node-fixture", b"changed node");
    assert!(p.revalidate(&Deadline::new()).is_err());
}
#[test]
fn node_absent_unsafe_or_shim_response_denied_without_fallback() {
    let f = Fixture::new();
    assert!(node_from_candidates(&f.home, &[f.home.join("missing")], &Deadline::new()).is_err());
    let candidate = f.home.join(".volta/bin/node");
    f.write(".volta/bin/node", b"#!/bin/sh\nprintf node\n");
    std::fs::set_permissions(&candidate, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(node_from_candidates(&f.home, &[candidate.clone()], &Deadline::new()).is_err());
    std::fs::set_permissions(&candidate, std::fs::Permissions::from_mode(0o777)).unwrap();
    assert!(node_from_candidates(&f.home, &[candidate], &Deadline::new()).is_err());
}
#[test]
fn node_removed_or_no_longer_executable_invalidates_preflight() {
    for remove in [false, true] {
        let f = Fixture::new();
        let p = f.plan().unwrap();
        if remove {
            std::fs::remove_file(&p.node).unwrap();
        } else {
            std::fs::set_permissions(&p.node, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        assert!(p.revalidate(&Deadline::new()).is_err());
    }
}
