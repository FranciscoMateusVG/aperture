use super::*;
use crate::{
    owner::{Incarnation, ProcessIdentity},
    state::{ExecutionTuple, ReasoningEffort},
};
fn input() -> OpenSeatInput {
    OpenSeatInput {
        team: "test".into(),
        seat: "test-dev".into(),
        expected_generation: 1,
    }
}
fn record() -> OwnerRecord {
    let tuple = ExecutionTuple {
        harness: Harness::Codex,
        model: "gpt-5.6-sol".into(),
        reasoning: Some(ReasoningEffort::High),
    };
    OwnerRecord {
        schema_version: 1,
        seat: "test-dev".into(),
        generation: 1,
        state: OwnerState::Active,
        reservation_nonce_sha256: None,
        provisional_token_id: None,
        requested: tuple.clone(),
        incarnation: Some(Incarnation {
            pid: 42,
            start_time: 123,
            thread_id: "exact-thread-123".into(),
            token_id: "opaque".into(),
            harness: tuple.harness,
            model: tuple.model,
            reasoning: tuple.reasoning,
            observed: true,
            processes: vec![ProcessIdentity {
                pid: 42,
                start_time: 123,
                ppid: 1,
                pgid: 42,
                cmdline_sha256: "a".repeat(64),
                cwd: "/fixture".into(),
            }],
        }),
        since: "2026-09-22".into(),
        writer: "launcher".into(),
    }
}
#[test]
fn exact_active_observed_owner_only() {
    assert!(owner_valid(&input(), &record()).is_ok());
    for state in [
        OwnerState::Stale,
        OwnerState::Starting,
        OwnerState::Quarantined,
    ] {
        let mut r = record();
        r.state = state;
        assert!(owner_valid(&input(), &r).is_err())
    }
}
#[test]
fn reject_generation_and_identity_drift() {
    for kind in 0..9 {
        let mut r = record();
        match kind {
            0 => r.generation = 2,
            1 => r.seat = "foreign".into(),
            2 => r.incarnation.as_mut().unwrap().observed = false,
            3 => r.incarnation.as_mut().unwrap().model = "different".into(),
            4 => r.incarnation.as_mut().unwrap().reasoning = None,
            5 => r.incarnation.as_mut().unwrap().thread_id = "--last".into(),
            6 => r.incarnation.as_mut().unwrap().processes.clear(),
            7 => r.incarnation.as_mut().unwrap().start_time = 999,
            _ => r.requested.harness = Harness::Claude,
        }
        assert!(owner_valid(&input(), &r).is_err(), "mutation {kind}")
    }
}
#[test]
fn selectors_reject_payload_injection() {
    for team in ["../x", "a;echo", "", "this-team-name-is-far-too-long"] {
        let mut i = input();
        i.team = team.into();
        assert!(selectors(&i).is_err())
    }
    let mut i = input();
    i.expected_generation = 0;
    assert!(selectors(&i).is_err());
    assert!(serde_json::from_value::<OpenSeatInput>(serde_json::json!({"team":"test","seat":"test-dev","expected_generation":1,"thread":"caller"})).is_err());
}
#[test]
fn windows_are_exact_native_ids() {
    for s in ["", "@", "@1;kill", "test", "%42", "@-1"] {
        assert!(!window_id(s))
    }
    assert!(window_id("@123"))
}
#[test]
fn private_socket_directory_chain_fails_on_symlink() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let home = std::env::temp_dir().join(format!("aperture-terminal-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&home).unwrap();
    fs::set_permissions(&home, fs::Permissions::from_mode(0o700)).unwrap();
    journal::ensure_private_dir(&home.join(".aperture/run")).unwrap();
    assert!(private_chain(&home, ".aperture/run").is_ok());
    fs::rename(home.join(".aperture/run"), home.join(".aperture/real")).unwrap();
    symlink(home.join(".aperture/real"), home.join(".aperture/run")).unwrap();
    assert!(private_chain(&home, ".aperture/run").is_err());
    fs::remove_dir_all(home).unwrap();
}
#[test]
fn readonly_open_missing_owner_does_not_create_client_or_worker() {
    use std::os::unix::fs::PermissionsExt;
    let home = std::env::temp_dir().join(format!("aperture-terminal-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&home).unwrap();
    fs::set_permissions(&home, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(open(&home, input()).is_err());
    assert!(!home.join(".aperture/run/terminals").exists());
    assert!(!home.join(".aperture/run/managed").exists());
    assert!(!home.join(".aperture/run/owner/test-dev.json").exists());
    fs::remove_dir_all(home).unwrap();
}
#[test]
fn client_argv_is_exact_thread_only_no_start_or_prompt() {
    let v = tui_args("exact-thread-123", Path::new("/home/run/test-dev.sock"));
    assert_eq!(
        v,
        vec![
            "resume",
            "exact-thread-123",
            "--remote",
            "unix:///home/run/test-dev.sock"
        ]
    );
    for bad in [
        "app-server",
        "--last",
        "--latest",
        "--model",
        "--prompt",
        "--continue",
    ] {
        assert!(!v.iter().any(|a| a == bad))
    }
}
#[test]
#[cfg(target_os = "macos")]
fn socket_peer_is_kernel_bound_and_missing_or_foreign_is_denied() {
    use std::os::unix::net::UnixListener;
    let p = PathBuf::from(format!("/tmp/at-{}.sock", uuid::Uuid::new_v4()));
    let listener = UnixListener::bind(&p).unwrap();
    assert!(socket_peer(&p, std::process::id()).is_ok());
    assert!(socket_peer(&p, std::process::id() + 1).is_err());
    drop(listener);
    fs::remove_file(&p).unwrap();
    assert!(socket_peer(&p, std::process::id()).is_err());
}
#[test]
fn serialized_terminal_response_matches_shared_ui_fixture() {
    let value = serde_json::to_value(OpenSeatView {
        team: "t1".into(),
        seat: "t1-backend".into(),
        generation: 2,
        window_id: "@42".into(),
    })
    .unwrap();
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../tests/fixtures/team-terminal-response.json"
    ))
    .unwrap();
    assert_eq!(value, fixture);
}
#[test]
fn existing_client_reuse_requires_exact_binding_pid_birth_and_live_pane() {
    let pid = std::process::id();
    let p = team_process::observe(pid).unwrap().unwrap();
    let birth = team_process::birth_micros(&p.identity).unwrap();
    let b = Binding {
        hash: "a".repeat(64),
        thread: "exact-thread".into(),
        socket: "/fixture.sock".into(),
        socket_id: (1, 2),
        runtime: "/fixture".into(),
        executable: "/fixture/bin".into(),
        executable_id: (3, 4),
        pid: 42,
        birth: 123,
    };
    let c = Client {
        binding: b.hash.clone(),
        window: "@42".into(),
        pid,
        birth,
    };
    assert!(reusable(&c, &b, pid, false));
    assert!(reusable(&c, &b, pid, false));
    assert!(!reusable(&c, &b, pid, true));
    assert!(!reusable(&c, &b, pid + 1, false));
    let mut stale = c.clone();
    stale.birth += 1;
    assert!(!reusable(&stale, &b, pid, false));
    stale = c.clone();
    stale.binding = "b".repeat(64);
    assert!(!reusable(&stale, &b, pid, false));
}
