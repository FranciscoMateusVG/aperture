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
fn private_chain_accepts_non_private_home_but_not_non_private_runtime() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let home = std::env::temp_dir().join(format!("aperture-terminal-home-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&home).unwrap();
    fs::set_permissions(&home, fs::Permissions::from_mode(0o700)).unwrap();
    journal::ensure_private_dir(&home.join(".aperture/run/managed/test-dev/g1")).unwrap();
    for mode in [0o700, 0o750, 0o755] {
        fs::set_permissions(&home, fs::Permissions::from_mode(mode)).unwrap();
        assert!(private_chain(&home, ".aperture/run").is_ok(), "home mode {mode:o}");
        assert!(private_chain(&home, ".aperture/run/managed/test-dev/g1").is_ok());
        assert_eq!(fs::metadata(&home).unwrap().mode() & 0o777, mode);
    }
    for part in [".aperture", ".aperture/run", ".aperture/run/managed/test-dev/g1"] {
        fs::set_permissions(home.join(part), fs::Permissions::from_mode(0o750)).unwrap();
        assert!(private_chain(&home, ".aperture/run/managed/test-dev/g1").is_err());
        fs::set_permissions(home.join(part), fs::Permissions::from_mode(0o700)).unwrap();
    }
    for mode in [0o770, 0o707, 0o777] {
        fs::set_permissions(&home, fs::Permissions::from_mode(mode)).unwrap();
        assert!(private_chain(&home, ".aperture/run").is_err());
    }
    fs::set_permissions(&home, fs::Permissions::from_mode(0o750)).unwrap();
    for relative in ["../run", "run", ".aperture/../run", "/.aperture/run"] {
        assert!(private_chain(&home, relative).is_err());
    }
    let linked_home = home.with_extension("symlink");
    symlink(&home, &linked_home).unwrap();
    assert!(private_chain(&linked_home, ".aperture/run").is_err());
    fs::remove_file(linked_home).unwrap();
    fs::remove_dir_all(home).unwrap();
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
        socket_binding: SocketBinding {
            path: "/fixture.sock".into(),
            pins: vec![],
            link: None,
        },
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

// ---- Claude: select the existing worker window; never create or key ----------
fn claude_input() -> OpenSeatInput {
    OpenSeatInput {
        team: "test".into(),
        seat: "test-qa".into(),
        expected_generation: 1,
    }
}
fn claude_record() -> OwnerRecord {
    let tuple = ExecutionTuple {
        harness: Harness::Claude,
        model: crate::team_claude_launch::MODEL.into(),
        reasoning: None,
    };
    let mut r = record();
    r.seat = "test-qa".into();
    r.requested = tuple.clone();
    let i = r.incarnation.as_mut().unwrap();
    i.harness = tuple.harness;
    i.model = tuple.model;
    i.reasoning = tuple.reasoning;
    i.thread_id = "12345678-1234-4234-8234-123456789012".into();
    r
}
const PANES_OK: &str = "aperture|@7|%3|42|0|test-qa-g1\naperture|@2|%1|7|0|glados\n";
struct FakeClaude {
    owners: Vec<Result<OwnerRecord>>,
    live_fail_at: Option<usize>,
    lives: usize,
    panes: String,
    events: Vec<&'static str>,
}
impl FakeClaude {
    fn ok() -> Self {
        Self {
            owners: vec![Ok(claude_record()), Ok(claude_record())],
            live_fail_at: None,
            lives: 0,
            panes: PANES_OK.into(),
            events: vec![],
        }
    }
}
impl ClaudeWindowIo for FakeClaude {
    fn owner(&mut self) -> Result<OwnerRecord> {
        self.events.push("owner");
        if self.owners.is_empty() {
            return Err(ERROR.into());
        }
        self.owners.remove(0)
    }
    fn live(&mut self, pid: u32, birth: u64) -> Result<()> {
        self.events.push("live");
        self.lives += 1;
        if self.live_fail_at == Some(self.lives) || pid != 42 || birth != 123 {
            return Err(ERROR.into());
        }
        Ok(())
    }
    fn panes(&mut self) -> Result<String> {
        self.events.push("panes");
        Ok(self.panes.clone())
    }
    fn select(&mut self, window: &str) -> Result<()> {
        self.events.push("select");
        assert!(window_id(window));
        Ok(())
    }
}
#[test]
fn claude_open_selects_existing_window_only_after_second_live_and_owner_recheck() {
    let mut io = FakeClaude::ok();
    let v = open_claude_with(&mut io, &claude_input()).unwrap();
    assert_eq!(v.window_id, "@7");
    assert_eq!(
        (v.team.as_str(), v.seat.as_str(), v.generation),
        ("test", "test-qa", 1)
    );
    assert_eq!(
        io.events,
        ["owner", "live", "panes", "live", "owner", "select"]
    );
    let value = serde_json::to_value(&v).unwrap();
    assert_eq!(
        value.as_object().unwrap().len(),
        4,
        "same response shape as Codex"
    );
}
#[test]
fn claude_open_negatives_never_reach_select() {
    let mut drifted = claude_record();
    drifted.incarnation.as_mut().unwrap().pid = 43;
    let cases: Vec<(&str, FakeClaude)> = vec![
        (
            "owner unavailable",
            FakeClaude {
                owners: vec![Err(ERROR.into())],
                ..FakeClaude::ok()
            },
        ),
        (
            "live fails before panes",
            FakeClaude {
                live_fail_at: Some(1),
                ..FakeClaude::ok()
            },
        ),
        (
            "live fails after panes",
            FakeClaude {
                live_fail_at: Some(2),
                ..FakeClaude::ok()
            },
        ),
        (
            "owner drifts after panes",
            FakeClaude {
                owners: vec![Ok(claude_record()), Ok(drifted)],
                ..FakeClaude::ok()
            },
        ),
        (
            "owner gone after panes",
            FakeClaude {
                owners: vec![Ok(claude_record())],
                ..FakeClaude::ok()
            },
        ),
        (
            "no matching pane",
            FakeClaude {
                panes: "aperture|@2|%1|7|0|glados\n".into(),
                ..FakeClaude::ok()
            },
        ),
        (
            "duplicate pid rows",
            FakeClaude {
                panes: "aperture|@7|%3|42|0|test-qa-g1\naperture|@8|%4|42|0|test-qa-g1\n".into(),
                ..FakeClaude::ok()
            },
        ),
        (
            "dead pane",
            FakeClaude {
                panes: "aperture|@7|%3|42|1|test-qa-g1\n".into(),
                ..FakeClaude::ok()
            },
        ),
        (
            "foreign session",
            FakeClaude {
                panes: "other|@7|%3|42|0|test-qa-g1\n".into(),
                ..FakeClaude::ok()
            },
        ),
        (
            "foreign window name",
            FakeClaude {
                panes: "aperture|@7|%3|42|0|test-qa-g2\n".into(),
                ..FakeClaude::ok()
            },
        ),
        (
            "malformed row",
            FakeClaude {
                panes: "aperture|@7;kill|%3|42|0|test-qa-g1\n".into(),
                ..FakeClaude::ok()
            },
        ),
        (
            "empty output",
            FakeClaude {
                panes: String::new(),
                ..FakeClaude::ok()
            },
        ),
    ];
    for (name, mut io) in cases {
        assert!(
            open_claude_with(&mut io, &claude_input()).is_err(),
            "{name}"
        );
        assert!(
            !io.events.contains(&"select"),
            "{name}: select must never run"
        );
    }
}
#[test]
fn claude_window_parser_is_exact_and_tolerates_pipes_in_foreign_names() {
    assert_eq!(claude_window(PANES_OK, 42, "test-qa-g1").unwrap(), "@7");
    assert_eq!(
        claude_window(
            "aperture|@1|%1|9|0|a|b|c\naperture|@7|%3|42|0|test-qa-g1\n",
            42,
            "test-qa-g1"
        )
        .unwrap(),
        "@7"
    );
    for (out, pid) in [
        ("aperture|@7|%3|42|0|test-qa-g1\n", 41u32),
        ("aperture|@7|%3|42|0\n", 42),
        ("aperture|@7|3|42|0|test-qa-g1\n", 42),
        ("aperture|@7|%3|x|0|test-qa-g1\n", 42),
        ("aperture|@7|%3|42|2|test-qa-g1\n", 42),
    ] {
        assert!(claude_window(out, pid, "test-qa-g1").is_err(), "{out:?}");
    }
    assert!(claude_window(&"a".repeat(8193), 42, "x").is_err());
}
#[test]
fn claude_owner_requires_exact_active_observed_sonnet_none() {
    assert!(claude_owner_valid(&claude_input(), &claude_record()).is_ok());
    for kind in 0..12 {
        let mut r = claude_record();
        match kind {
            0 => r.state = OwnerState::Starting,
            1 => r.state = OwnerState::Quarantined,
            2 => r.generation = 2,
            3 => r.requested.harness = Harness::Codex,
            4 => r.requested.model = "sonnet".into(),
            5 => r.requested.reasoning = Some(ReasoningEffort::High),
            6 => r.incarnation.as_mut().unwrap().observed = false,
            7 => r.incarnation.as_mut().unwrap().model = "different".into(),
            8 => r.incarnation.as_mut().unwrap().thread_id = "not-a-uuid".into(),
            9 => r.reservation_nonce_sha256 = Some("a".repeat(64)),
            10 => r.incarnation.as_mut().unwrap().processes.clear(),
            _ => r.incarnation.as_mut().unwrap().start_time = 999,
        }
        assert!(
            claude_owner_valid(&claude_input(), &r).is_err(),
            "mutation {kind}"
        );
    }
    for model in ["claude-fable-5-1", "claude-opus-5"] {
        let mut r = claude_record();
        r.requested.model = model.into();
        assert!(claude_owner_valid(&claude_input(), &r).is_err(), "{model} requested but Sonnet observed");
        r.incarnation.as_mut().unwrap().model = model.into();
        assert!(claude_owner_valid(&claude_input(), &r).is_ok(), "{model} exact observed owner opens");
    }
    for model in ["fable", "claude-fable-5", "claude-opus-5-5", "claude-fable-5-1[1m]"] {
        let mut r = claude_record();
        r.requested.model = model.into();
        r.incarnation.as_mut().unwrap().model = model.into();
        assert!(claude_owner_valid(&claude_input(), &r).is_err(), "{model} is not an exact literal");
    }
    // The Codex guard still rejects a Claude owner and vice versa.
    assert!(owner_valid(&claude_input(), &claude_record()).is_err());
}

// Isolated socket metadata fixtures: no app-server, TUI, tmux or provider.
struct SocketFixture {
    root: PathBuf,
    daemon: PathBuf,
    link: PathBuf,
    target: PathBuf,
    _listener: std::os::unix::net::UnixListener,
}
impl SocketFixture {
    fn new() -> Self {
        use std::os::unix::{
            fs::{symlink, PermissionsExt},
            net::UnixListener,
        };
        // Keep Unix pathname below sockaddr_un capacity, including 64-byte name.
        let root = PathBuf::from(format!(
            "/private/tmp/ats-{}",
            &uuid::Uuid::new_v4().simple().to_string()[..12]
        ));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let daemon = root.join("d");
        fs::create_dir(&daemon).unwrap();
        fs::set_permissions(&daemon, fs::Permissions::from_mode(0o700)).unwrap();
        let target = daemon.join("a".repeat(64));
        let listener = UnixListener::bind(&target).unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
        let link = root.join("run.sock");
        symlink(&target, &link).unwrap();
        Self {
            root,
            daemon,
            target,
            link,
            _listener: listener,
        }
    }
    fn pin(&self) -> Result<SocketBinding> {
        pin_socket(&self.link, &self.daemon, unsafe { libc::geteuid() })
    }
    fn relink(&self, path: &Path) {
        fs::remove_file(&self.link).unwrap();
        std::os::unix::fs::symlink(path, &self.link).unwrap();
    }
}
impl Drop for SocketFixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap();
    }
}
#[test]
fn native_daemon_socket_shape_and_direct_socket_are_pinned() {
    let f = SocketFixture::new();
    let pin = f.pin().unwrap();
    assert_eq!(pin.path, f.target);
    assert_eq!(pin.pins.len(), 3);
    assert_eq!(pin.link, Some((f.link.clone(), f.target.clone())));
    pin.recheck().unwrap();
    let direct = resolve_socket(&f.target).unwrap();
    assert_eq!(direct.path, f.target);
    assert!(direct.link.is_none());
    assert_eq!(
        tui_args("exact-thread", &pin.path)[3],
        format!("unix://{}", f.target.display())
    );
    // Production resolver cannot admit fixture parent paths through the link.
    assert!(resolve_socket(&f.link).is_err());
    assert_eq!(system_tmp_pins().unwrap().len(), 3);
}
#[test]
fn native_daemon_socket_rejects_all_non_native_link_targets() {
    let f = SocketFixture::new();
    for target in [
        f.daemon.join("socket"),
        f.daemon.join("a".repeat(63)),
        f.daemon.join("a".repeat(65)),
        f.daemon.join("A".repeat(64)),
        f.daemon.join("g".repeat(64)),
        f.root.join("foreign").join("a".repeat(64)),
        PathBuf::from("d").join("a".repeat(64)),
        PathBuf::from(format!("{}/./{}", f.daemon.display(), "a".repeat(64))),
        PathBuf::from(format!("{}//{}", f.daemon.display(), "a".repeat(64))),
        f.daemon.join("..").join("d").join("a".repeat(64)),
    ] {
        f.relink(&target);
        assert!(f.pin().is_err(), "non-native link must fail");
    }
}
#[test]
fn native_daemon_socket_rejects_unsafe_directory_leaf_and_uid() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let f = SocketFixture::new();
    for mode in [0o755, 0o770, 0o1700] {
        fs::set_permissions(&f.daemon, fs::Permissions::from_mode(mode)).unwrap();
        assert!(f.pin().is_err());
    }
    fs::set_permissions(&f.daemon, fs::Permissions::from_mode(0o700)).unwrap();
    for mode in [0o666, 0o660, 0o601, 0o1600] {
        fs::set_permissions(&f.target, fs::Permissions::from_mode(mode)).unwrap();
        assert!(f.pin().is_err());
        assert!(resolve_socket(&f.target).is_err());
    }
    fs::set_permissions(&f.target, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(pin_socket(&f.link, &f.daemon, unsafe { libc::geteuid() } + 1).is_err());
    let moved = f.root.join("old");
    fs::rename(&f.daemon, &moved).unwrap();
    symlink(&moved, &f.daemon).unwrap();
    assert!(f.pin().is_err());
    fs::remove_file(&f.daemon).unwrap();
    fs::rename(moved, &f.daemon).unwrap();
    let old = f.root.join("old.sock");
    fs::rename(&f.target, &old).unwrap();
    symlink(&old, &f.target).unwrap();
    assert!(f.pin().is_err());
    fs::remove_file(&f.target).unwrap();
    fs::write(&f.target, b"not a socket").unwrap();
    fs::set_permissions(&f.target, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(f.pin().is_err());
}
#[test]
fn native_socket_recheck_detects_same_target_link_recreation() {
    let f = SocketFixture::new();
    let before = f.pin().unwrap();
    fs::rename(&f.link, f.root.join("old.link")).unwrap();
    std::os::unix::fs::symlink(&f.target, &f.link).unwrap();
    assert!(before.recheck().is_err());
    assert_ne!(before, f.pin().unwrap());
    // Receipt hash input changes, even though owner/thread and destination match.
    assert_ne!(
        serde_json::to_vec(&before).unwrap(),
        serde_json::to_vec(&f.pin().unwrap()).unwrap()
    );
}
#[test]
fn native_socket_recheck_detects_leaf_and_directory_replacement() {
    use std::os::unix::{fs::PermissionsExt, net::UnixListener};
    let f = SocketFixture::new();
    let before = f.pin().unwrap();
    fs::rename(&f.target, f.root.join("old.sock")).unwrap();
    let _other = UnixListener::bind(&f.target).unwrap();
    fs::set_permissions(&f.target, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(before.recheck().is_err());
    assert_ne!(before, f.pin().unwrap());
    let before = f.pin().unwrap();
    fs::rename(&f.daemon, f.root.join("old-dir")).unwrap();
    fs::create_dir(&f.daemon).unwrap();
    fs::set_permissions(&f.daemon, fs::Permissions::from_mode(0o700)).unwrap();
    fs::rename(f.root.join("old-dir").join("a".repeat(64)), &f.target).unwrap();
    assert!(before.recheck().is_err());
    assert_ne!(before, f.pin().unwrap());
}
#[test]
#[cfg(target_os = "macos")]
fn native_socket_peer_and_post_connect_drift_are_enforced() {
    use std::os::unix::fs::PermissionsExt;
    let f = SocketFixture::new();
    let resolve = |p: &Path| pin_socket(p, &f.daemon, unsafe { libc::geteuid() });
    assert!(verify_socket_with(&f.link, std::process::id(), resolve, socket_peer).is_ok());
    assert!(verify_socket_with(&f.link, std::process::id() + 1, resolve, socket_peer).is_err());
    assert!(
        verify_socket_with(&f.link, std::process::id(), resolve, |p, pid| {
            socket_peer(p, pid)?;
            fs::set_permissions(&f.target, fs::Permissions::from_mode(0o660)).unwrap();
            Ok(())
        })
        .is_err()
    );
    fs::set_permissions(&f.target, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(
        verify_socket_with(&f.link, std::process::id(), resolve, |_, _| {
            fs::rename(&f.link, f.root.join("old.link")).unwrap();
            std::os::unix::fs::symlink(&f.target, &f.link).unwrap();
            Ok(())
        })
        .is_err()
    );
}
