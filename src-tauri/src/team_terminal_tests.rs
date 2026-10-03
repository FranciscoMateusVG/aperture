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
    let runtime = crate::daemons::RuntimeOwner::new(crate::controller::ControllerLock::acquire(&home).unwrap());
    let work = runtime.admit(None).unwrap();
    assert!(open(&home, input(), &work).is_err());
    drop(work); runtime.close().unwrap(); drop(runtime);
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
        client: crate::daemons::ToolPin::capture(&std::env::current_exe().unwrap()).unwrap(),
        client_fingerprint: "f".repeat(64),
        pid: 42,
        birth: 123,
    };
    let c = Client {
        client_path: b.client.path.clone(),
        client_fingerprint: b.client_fingerprint.clone(),
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
    for model in ["claude-fable-5-1", "claude-opus-5", "claude-opus-5-5"] {
        let mut r = claude_record();
        r.requested.model = model.into();
        assert!(claude_owner_valid(&claude_input(), &r).is_err(), "{model} requested but Sonnet observed");
        r.incarnation.as_mut().unwrap().model = model.into();
        assert!(claude_owner_valid(&claude_input(), &r).is_ok(), "{model} exact observed owner opens");
    }
    for model in ["fable", "claude-fable-5", "claude-opus-5.5", "claude-opus-5-5[1m]", "claude-opus-5-5-latest", "claude-fable-5-1[1m]"] {
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

// Hermetic HOME + kernel Unix peer fixture; never the installed coordination
// sockets or application servers. Synthetic metadata negatives use this same
// filesystem capture/recheck implementation, not a caller-created proof.
struct CoordinationFixture {
    home: PathBuf,
    _listener: std::os::unix::net::UnixListener,
}
impl CoordinationFixture {
    fn new() -> Self {
        use std::os::unix::{fs::PermissionsExt, net::UnixListener};
        let home = PathBuf::from(format!(
            "/private/tmp/ac-{}",
            &uuid::Uuid::new_v4().simple().to_string()[..10]
        ));
        for part in [
            "",
            ".aperture",
            ".aperture/run",
            ".claude",
            ".claude/aperture",
            ".claude/aperture/glados",
        ] {
            fs::create_dir(home.join(part)).unwrap();
            fs::set_permissions(home.join(part), fs::Permissions::from_mode(0o700)).unwrap();
        }
        for part in [".claude", ".claude/aperture/glados"] {
            fs::set_permissions(home.join(part), fs::Permissions::from_mode(0o755)).unwrap();
        }
        fs::write(
            home.join(".claude/aperture/glados/manifest.json"),
            br#"{"name":"GLaDOS","enabled":true,"role":"orchestrator"}"#,
        )
        .unwrap();
        fs::set_permissions(
            home.join(".claude/aperture/glados/manifest.json"),
            fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        let socket = home.join(".aperture/run/glados.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
        Self {
            home,
            _listener: listener,
        }
    }
    fn manifest(&self) -> PathBuf {
        self.home.join(".claude/aperture/glados/manifest.json")
    }
    fn socket(&self) -> PathBuf {
        self.home.join(".aperture/run/glados.sock")
    }
    fn capture(&self) -> Result<CoordinationPeer> {
        capture_coordination_peer(
            &self.home,
            "glados",
            &resolve_socket,
            &peer_pid,
            &team_process::observe,
        )?
        .ok_or(ERROR.into())
    }
}
impl Drop for CoordinationFixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.home).unwrap();
    }
}
#[test]
#[cfg(target_os = "macos")]
fn coordination_kernel_peer_is_bound_and_only_fixed_missing_sockets_skip() {
    let f = CoordinationFixture::new();
    let peers = capture_coordination_peers(&f.home).unwrap();
    assert_eq!(peers.len(), 1);
    assert_eq!(peers[0].seat, "glados");
    assert_eq!(
        peers[0].identity,
        team_process::observe(std::process::id())
            .unwrap()
            .unwrap()
            .identity
    );
    peers[0].recheck().unwrap();
    assert!(capture_coordination_peer(
        &f.home,
        "someone",
        &resolve_socket,
        &peer_pid,
        &team_process::observe
    )
    .is_err());
    fs::remove_file(f.socket()).unwrap();
    assert!(peers[0].recheck().is_err());
    assert!(capture_coordination_peers(&f.home).unwrap().is_empty());
    std::os::unix::fs::symlink("missing", f.socket()).unwrap();
    assert!(
        capture_coordination_peers(&f.home).is_err(),
        "dangling socket is invalid, not absent"
    );
}
#[test]
fn coordination_manifest_and_team_fail_closed() {
    for bytes in [
        r#"{"name":"GLaDOS","enabled":false}"#,
        r#"{"name":"other","enabled":true}"#,
        r#"{"name":"GLaDOS"}"#,
        "{broken",
        r#"{"name":"GLaDOS","enabled":true,"enabled":false}"#,
    ] {
        let f = CoordinationFixture::new();
        fs::write(f.manifest(), bytes).unwrap();
        assert!(f.capture().is_err());
    }
    let f = CoordinationFixture::new();
    fs::write(f.home.join(".claude/aperture/glados/TEAM"), b"").unwrap();
    assert!(f.capture().is_err());
    fs::remove_file(f.home.join(".claude/aperture/glados/TEAM")).unwrap();
    std::os::unix::fs::symlink("missing", f.home.join(".claude/aperture/glados/TEAM")).unwrap();
    assert!(f.capture().is_err());
}
#[test]
fn coordination_manifest_pins_reject_links_writes_and_replacement() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    for mutation in 0..5 {
        let f = CoordinationFixture::new();
        let proof = f.capture().unwrap();
        match mutation {
            0 => fs::set_permissions(f.manifest(), fs::Permissions::from_mode(0o666)).unwrap(),
            1 => fs::hard_link(f.manifest(), f.home.join("alias")).unwrap(),
            2 => {
                fs::rename(f.manifest(), f.home.join("original")).unwrap();
                symlink(f.home.join("original"), f.manifest()).unwrap();
            }
            3 => {
                let bytes = fs::read(f.manifest()).unwrap();
                fs::rename(f.manifest(), f.home.join("old")).unwrap();
                fs::write(f.manifest(), bytes).unwrap();
            }
            _ => fs::write(
                f.manifest(),
                br#"{"name":"GLaDOS","enabled":true,"role":"changed"}"#,
            )
            .unwrap(),
        }
        assert!(proof.recheck().is_err(), "mutation {mutation}");
    }
}
#[test]
fn coordination_parent_chain_and_socket_mode_are_pinned() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    for relative in [
        ".aperture",
        ".aperture/run",
        ".claude/aperture",
        ".claude/aperture/glados",
    ] {
        let f = CoordinationFixture::new();
        let proof = f.capture().unwrap();
        fs::set_permissions(f.home.join(relative), fs::Permissions::from_mode(0o777)).unwrap();
        assert!(proof.recheck().is_err(), "{relative}");
        assert!(f.capture().is_err());
    }
    let f = CoordinationFixture::new();
    let proof = f.capture().unwrap();
    fs::rename(
        f.home.join(".aperture/run"),
        f.home.join(".aperture/old-run"),
    )
    .unwrap();
    symlink("old-run", f.home.join(".aperture/run")).unwrap();
    assert!(proof.recheck().is_err());
    assert!(f.capture().is_err());
    let f = CoordinationFixture::new();
    fs::set_permissions(f.socket(), fs::Permissions::from_mode(0o666)).unwrap();
    assert!(f.capture().is_err());
}
#[test]
fn coordination_process_uid_birth_peer_and_midcapture_drift_deny() {
    use std::cell::Cell;
    let f = CoordinationFixture::new();
    let proof = f.capture().unwrap();
    let original = team_process::observe(std::process::id()).unwrap().unwrap();
    for mutation in 0..5 {
        let observe = |_: u32| {
            let mut p = original.clone();
            match mutation {
                0 => p.uid += 1,
                1 => p.identity.start_time.push('0'),
                2 => return Ok(None),
                3 => return Err(crate::team_replacement::ReplacementError::StopUnverified),
                _ => p.identity.pid += 1,
            }
            Ok(Some(p))
        };
        assert!(proof
            .recheck_with(&resolve_socket, &peer_pid, &observe)
            .is_err());
    }
    let calls = Cell::new(0);
    let peer = |_: &Path| {
        calls.set(calls.get() + 1);
        Ok(original.identity.pid + u32::from(calls.get() > 1))
    };
    assert!(capture_coordination_peer(
        &f.home,
        "glados",
        &resolve_socket,
        &peer,
        &team_process::observe
    )
    .is_err());
    let calls = Cell::new(0);
    let observe = |_: u32| {
        calls.set(calls.get() + 1);
        let mut p = original.clone();
        if calls.get() > 1 {
            p.identity.start_time.push('0');
        }
        Ok(Some(p))
    };
    assert!(
        capture_coordination_peer(&f.home, "glados", &resolve_socket, &peer_pid, &observe).is_err()
    );
}
#[test]
fn coordination_symlink_binding_reuses_native_shape_and_rechecks_target() {
    use std::os::unix::fs::symlink;
    let f = CoordinationFixture::new();
    let daemon = SocketFixture::new();
    fs::remove_file(f.socket()).unwrap();
    symlink(&daemon.target, f.socket()).unwrap();
    let resolve = |p: &Path| pin_socket(p, &daemon.daemon, unsafe { libc::geteuid() });
    let proof = capture_coordination_peer(
        &f.home,
        "glados",
        &resolve,
        &peer_pid,
        &team_process::observe,
    )
    .unwrap()
    .unwrap();
    proof
        .recheck_with(&resolve, &peer_pid, &team_process::observe)
        .unwrap();
    fs::remove_file(f.socket()).unwrap();
    symlink(daemon.daemon.join("b".repeat(64)), f.socket()).unwrap();
    assert!(proof
        .recheck_with(&resolve, &peer_pid, &team_process::observe)
        .is_err());
}

#[test]
fn coordination_native_manifest_symlink_is_allowed_and_pinned() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let f = CoordinationFixture::new();
    let target = f.home.join("source-manifest.json");
    fs::rename(f.manifest(), &target).unwrap();
    symlink(&target, f.manifest()).unwrap();
    let proof = f.capture().unwrap();
    proof.recheck().unwrap();
    // Case-fold only for the fixed native display name, not a caller selector.
    fs::write(&target, br#"{"name":"glados","enabled":true}"#).unwrap();
    assert!(
        proof.recheck().is_err(),
        "changed bytes deny even with valid semantic name"
    );
    let proof = f.capture().unwrap();
    let other = f.home.join("other-manifest.json");
    fs::write(&other, fs::read(&target).unwrap()).unwrap();
    fs::remove_file(f.manifest()).unwrap();
    symlink(&other, f.manifest()).unwrap();
    assert!(
        proof.recheck().is_err(),
        "identical bytes at a different target deny"
    );
    fs::set_permissions(&other, fs::Permissions::from_mode(0o666)).unwrap();
    assert!(f.capture().is_err());
}
#[test]
fn coordination_manifest_link_chain_unsafe_parent_and_uid_deny() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let f = CoordinationFixture::new();
    let dir = f.home.join("source");
    fs::create_dir(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
    let target = dir.join("manifest.json");
    fs::rename(f.manifest(), &target).unwrap();
    symlink(&target, f.manifest()).unwrap();
    assert!(f.capture().is_ok());
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o777)).unwrap();
    assert!(f.capture().is_err());
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
    let real = dir.join("real.json");
    fs::rename(&target, &real).unwrap();
    symlink(&real, &target).unwrap();
    assert!(f.capture().is_err(), "secondary symlink denied");
    // A resolver with a foreign socket UID cannot pass canonical pinning.
    assert!(pin_socket(&f.socket(), &dir, unsafe { libc::geteuid() } + 1).is_err());
}

#[test]
fn coordination_recheck_rejects_team_marker_and_mutated_public_identity() {
    let f = CoordinationFixture::new();
    let mut proof = f.capture().unwrap();
    proof.identity.start_time.push('0');
    assert!(proof.recheck().is_err());
    let proof = f.capture().unwrap();
    fs::write(f.home.join(".claude/aperture/glados/TEAM"), b"unexpected").unwrap();
    assert!(proof.recheck().is_err());
}

#[test]
#[cfg(target_os = "macos")]
fn coordination_cipher_requires_the_same_native_binding_and_recheck() {
    let f = CoordinationFixture::new();
    let agent = f.home.join(".claude/aperture/cipher");
    fs::rename(f.home.join(".claude/aperture/glados"), &agent).unwrap();
    fs::write(agent.join("manifest.json"), br#"{"name":"Cipher","enabled":true}"#).unwrap();
    let socket = f.home.join(".aperture/run/cipher.sock");
    fs::rename(f.socket(), &socket).unwrap();
    let peers = capture_coordination_peers(&f.home).unwrap();
    assert_eq!(peers.len(), 1);
    assert_eq!(peers[0].seat, "cipher");
    assert_eq!(peers[0].identity, team_process::observe(std::process::id()).unwrap().unwrap().identity);
    peers[0].recheck().unwrap();
    fs::write(agent.join("TEAM"), b"not-coordination").unwrap();
    assert!(peers[0].recheck().is_err());
    assert!(capture_coordination_peers(&f.home).is_err());
    fs::remove_file(agent.join("TEAM")).unwrap();
    fs::write(agent.join("manifest.json"), br#"{"name":"Other","enabled":true}"#).unwrap();
    assert!(capture_coordination_peers(&f.home).is_err());
    fs::remove_file(socket).unwrap();
    assert!(capture_coordination_peers(&f.home).unwrap().is_empty());
}

#[test]
fn c2b_native_pin_mapping_and_fixed_link_unlink_retain_target() {
    use std::os::unix::fs::PermissionsExt;
    let mut f = SocketFixture::new();
    // C2 native policy has a direct child of pinned /private/tmp; do not omit
    // an intermediate fixture directory from the persisted parent order.
    f.daemon = f.root.clone();
    f.target = f.root.join("a".repeat(64));
    let listener = std::os::unix::net::UnixListener::bind(&f.target).unwrap();
    fs::set_permissions(&f.target, fs::Permissions::from_mode(0o600)).unwrap();
    f._listener = listener;
    f.relink(&f.target);
    fs::create_dir_all(f.root.join(".aperture/run")).unwrap();
    for p in [f.root.join(".aperture"), f.root.join(".aperture/run")] {
        fs::set_permissions(p, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let binding = f.pin().unwrap();
    let values = codex_pin_values(&f.root, &f.link, &binding).unwrap();
    assert_eq!(
        values.parents,
        coordination_runtime_dirs(&f.root)
            .unwrap()
            .iter()
            .map(pin_value)
            .collect::<Vec<_>>()
    );
    let target = values.native_target.unwrap();
    assert_eq!(target.basename, "a".repeat(64));
    let mut parents = system_tmp_pins().unwrap();
    parents.push(PathPin::read(&f.daemon).unwrap());
    assert_eq!(
        target.parents,
        parents.iter().map(pin_value).collect::<Vec<_>>()
    );
    assert_eq!(target.leaf, pin_value(&PathPin::read(&f.target).unwrap()));
    assert_eq!(target.leaf.links, 1);
    assert!(target.parents.iter().all(|p| p.links == 0));
    let before = codex_unlink_calls();
    assert!(
        native_explicit_refusal(&f.target).is_err(),
        "live listener never refusal"
    );
    assert_eq!(codex_unlink_calls(), before);
    let old = std::mem::replace(
        &mut f._listener,
        std::os::unix::net::UnixListener::bind(f.root.join("other.sock")).unwrap(),
    );
    drop(old);
    native_explicit_refusal(&f.target).unwrap();
    let proof = CodexCleanup::new(f.link.clone(), binding).unwrap();
    assert_eq!(
        proof.unlink().unwrap(),
        crate::daemon_registry::CleanupOutcomeV2::FixedLinkRemovedTargetRetained
    );
    assert!(!f.link.symlink_metadata().is_ok());
    assert!(f.target.exists());
    assert_eq!(codex_unlink_calls(), before + 1);
}

#[test]
fn c2b_cleanup_fd_path_swap_absence_and_nonrefusal_never_unlink() {
    use std::os::unix::{fs::PermissionsExt, net::UnixListener};
    for change in ["link", "parent", "parent-replaced", "leaf", "absent"] {
        let f = SocketFixture::new();
        let proof = CodexCleanup::new(f.link.clone(), f.pin().unwrap()).unwrap();
        let before = codex_unlink_calls();
        if change == "parent-replaced" {
            let moved = f.root.with_extension("moved");
            fs::rename(&f.root, &moved).unwrap();
            fs::create_dir(&f.root).unwrap();
            fs::set_permissions(&f.root, fs::Permissions::from_mode(0o700)).unwrap();
            let denied = proof.unlink().is_err();
            // Restore own tree before asserting, so fixture teardown is faithful.
            fs::remove_dir(&f.root).unwrap();
            fs::rename(moved, &f.root).unwrap();
            assert!(denied);
            assert_eq!(codex_unlink_calls(), before);
            continue;
        }
        match change {
            "link" => f.relink(&f.target),
            "parent" => fs::set_permissions(&f.root, fs::Permissions::from_mode(0o755)).unwrap(),
            "leaf" => fs::set_permissions(&f.target, fs::Permissions::from_mode(0o640)).unwrap(),
            "absent" => fs::remove_file(&f.link).unwrap(),
            _ => unreachable!(),
        }
        assert!(proof.unlink().is_err());
        assert_eq!(codex_unlink_calls(), before);
    }
    let f = SocketFixture::new();
    assert!(native_explicit_refusal(&f.root.join("missing.sock")).is_err());
    assert!(native_explicit_refusal(&f.root).is_err());
    let live = UnixListener::bind(f.root.join("live.sock")).unwrap();
    assert!(native_explicit_refusal(&f.root.join("live.sock")).is_err());
    drop(live);
    native_explicit_refusal(&f.root.join("live.sock")).unwrap();
}

#[test]
fn local_attach_helper_is_adjacent_validated_and_never_checkout_fallback() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let f = SocketFixture::new();
    let bin = f.root.join("bin"); fs::create_dir(&bin).unwrap();
    let current = bin.join("aperture-server");
    let helper = bin.join("aperture-boot");
    // Even a valid checkout-looking file cannot substitute for a missing sibling.
    let checkout = f.root.join("target/release"); fs::create_dir_all(&checkout).unwrap();
    fs::write(checkout.join("aperture-boot"), b"fixture").unwrap();
    fs::set_permissions(checkout.join("aperture-boot"), fs::Permissions::from_mode(0o700)).unwrap();
    assert!(boot_helper_adjacent(&current).is_err());
    symlink(checkout.join("aperture-boot"), &helper).unwrap();
    assert!(boot_helper_adjacent(&current).is_err()); fs::remove_file(&helper).unwrap();
    fs::write(&helper, b"own inert executable fixture").unwrap();
    fs::set_permissions(&helper, fs::Permissions::from_mode(0o722)).unwrap();
    assert!(boot_helper_adjacent(&current).is_err());
    fs::set_permissions(&helper, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(boot_helper_adjacent(&current).is_err());
    fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(boot_helper_adjacent(&current).unwrap().path, helper);
    assert!(boot_helper_adjacent(Path::new("relative/aperture-server")).is_err());
}

// Native filesystem/owner/socket proof remains production code. Only the
// tmux executable and the last-boundary drift trigger are fixture-local.
pub(super) struct OpenFixture {
    home: PathBuf,
    pub(super) current: PathBuf,
    client: PathBuf,
    tmux: PathBuf,
    drift: std::cell::RefCell<Option<(&'static str, bool)>>,
    listener: Option<std::os::unix::net::UnixListener>,
    child: Option<(std::process::Child, crate::team_replacement::ProcessIdentity)>,
}
impl OpenFixture {
    fn new() -> Self {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        let home = PathBuf::from(format!("/private/tmp/ato-{}", &uuid::Uuid::new_v4().simple().to_string()[..10]));
        fs::DirBuilder::new().mode(0o700).create(&home).unwrap();
        for dir in ["bin", ".aperture/run/managed/test-dev/g1", ".aperture/run/owner", ".aperture/teams/test", ".claude/aperture/test-dev"] {
            journal::ensure_private_dir(&home.join(dir)).unwrap();
        }
        let tuple = record().requested;
        let snapshot = teams::TeamSnapshot {
            schema_version: 1, team:"test".into(), project:"project:aperture".into(), repo:"aperture".into(),
            mission:"inert terminal fixture".into(), acceptance:"no worker launch".into(),
            preset:teams::PresetSnapshotRef{id:None,sha256:None}, lead:"test-dev".into(),
            seats:vec![teams::TeamSeat{name:"test-dev".into(),role:"backend".into(),harness:tuple.harness,model:tuple.model,reasoning:tuple.reasoning}],
            fallbacks:vec![],grants:vec![],created_at:"2026-09-28T00:00:00Z".into(),
            creation_request_id:uuid::Uuid::new_v4().to_string(), staging_uuid:uuid::Uuid::new_v4().to_string(),
        };
        journal::write_private_json_atomic(&home.join(".aperture/teams/test/team.json"),&snapshot,false).unwrap();
        journal::write_private_json_atomic(&home.join(".aperture/teams/test/state.json"),&teams::TeamStateFile {
            schema_version:1,state:teams::TeamLifecycle::Active,generation:1,epic_id:Some("aperture-fixture".into()),failure:None,updated_at:snapshot.created_at.clone(),
        },false).unwrap();
        for leaf in ["TEAM",".complete"] { fs::write(home.join(".claude/aperture/test-dev").join(leaf),b"test").unwrap(); }
        let current=home.join("bin/aperture-server");let client=home.join("client");let tmux=home.join("tmux-inert");
        for p in [home.join("bin/aperture-boot"),client.clone()] {
            fs::write(&p,b"owned executable fixture").unwrap(); fs::set_permissions(p,fs::Permissions::from_mode(0o700)).unwrap();
        }
        let script=format!("#!/bin/sh\nprintf '%s\n' \"$1\" >> '{}/events'\ncase \"$1\" in\n new-window) printf 'created' > '{}/pane'; printf '@42\n';;\n list-panes) test -f '{}/pane' || exit 9; printf '{}|0\n';;\n select-window) test -f '{}/pane' || exit 9;;\n *) exit 8;;\nesac\n",home.display(),home.display(),home.display(),std::process::id(),home.display());
        fs::write(&tmux,script).unwrap();fs::set_permissions(&tmux,fs::Permissions::from_mode(0o700)).unwrap();
        let listener=std::os::unix::net::UnixListener::bind(home.join(".aperture/run/test-dev.sock")).unwrap();
        fs::set_permissions(home.join(".aperture/run/test-dev.sock"),fs::Permissions::from_mode(0o600)).unwrap();
        let f=Self{home,current,client,tmux,drift:Default::default(),listener:Some(listener),child:None};f.publish_owner(std::process::id());f
    }
    fn publish_owner(&self,pid:u32) {
        let observed=team_process::observe(pid).unwrap().unwrap();let birth=team_process::birth_micros(&observed.identity).unwrap();
        let mut r=record();let i=r.incarnation.as_mut().unwrap();i.pid=pid;i.start_time=birth;i.processes[0].pid=pid;i.processes[0].start_time=birth;
        let p=self.home.join(".aperture/run/owner/test-dev.json");let replace=p.exists();journal::write_private_json_atomic(&p,&r,replace).unwrap();
    }
    fn runtime(&self)->crate::daemons::RuntimeOwner {
        let tools=crate::daemons::LocalTools::fixture(&self.home,&self.client);
        crate::daemons::RuntimeOwner::local(crate::controller::ControllerLock::acquire(&self.home).unwrap(),tools).unwrap()
    }
    pub(super) fn tmux(&self,args:&[String])->Result<String> {
        // Actual native bounded process call, with an inert script instead of tmux.
        let mut command=Command::new(&self.tmux);command.args(args).env_clear().env("HOME",&self.home);
        let bytes=crate::team_replacement::repository::bounded_command(command,Instant::now()+Duration::from_secs(3)).map_err(|_| "E_FIXTURE_TMUX_UNKNOWN".to_string())?;
        String::from_utf8(bytes).map_err(|_|ERROR.into())
    }
    pub(super) fn before_dispatch(&self) {
        use std::io::Write;
        if let Some((target,replace))=self.drift.borrow_mut().take() {
            let path=if target=="client" {self.client.clone()} else {self.home.join("bin/aperture-boot")};
            let before=fs::metadata(&path).unwrap();
            if replace {fs::rename(&path,path.with_extension("retained")).unwrap();fs::copy(path.with_extension("retained"),&path).unwrap();}
            else {
                let mut f=fs::OpenOptions::new().write(true).open(&path).unwrap();
                f.write_all(&vec![b'X';before.len() as usize]).unwrap();
                f.set_times(fs::FileTimes::new().set_modified(before.modified().unwrap())).unwrap();
                let after=f.metadata().unwrap();assert_eq!((before.ino(),before.len(),before.mode(),before.mtime(),before.mtime_nsec()),(after.ino(),after.len(),after.mode(),after.mtime(),after.mtime_nsec()));
            }
        }
    }
    fn unlinked_server(&mut self) {
        use std::os::unix::fs::PermissionsExt;
        drop(self.listener.take());fs::remove_file(self.home.join(".aperture/run/test-dev.sock")).unwrap();
        let bin=self.home.join("old-appserver");fs::copy(std::env::current_exe().unwrap(),&bin).unwrap();
        fs::set_permissions(&bin,fs::Permissions::from_mode(0o700)).unwrap();
        let child=Command::new(&bin).args(["--exact","team_terminal::tests::local_terminal_inert_server_entry","--ignored","--test-threads=1"])
            .env_clear().env("APERTURE_TERMINAL_FIXTURE_HOME",&self.home)
            .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().unwrap();
        let identity=team_process::observe(child.id()).unwrap().unwrap().identity;self.child=Some((child,identity));
        let until=Instant::now()+Duration::from_secs(5);
        while !self.home.join("ready").exists() {assert!(Instant::now()<until);std::thread::sleep(Duration::from_millis(5));}
        let pid=self.child.as_ref().unwrap().0.id();self.publish_owner(pid);
        fs::remove_file(&bin).unwrap();assert!(!bin.exists());
        assert_eq!(team_process::state(&self.child.as_ref().unwrap().1),crate::team_replacement::ProcessState::Same);
    }
}
impl Drop for OpenFixture {
    fn drop(&mut self) {
        if let Some((child,id))=&mut self.child {
            assert_eq!(team_process::state(id),crate::team_replacement::ProcessState::Same);
            child.kill().unwrap();child.wait().unwrap();
            assert_eq!(team_process::state(id),crate::team_replacement::ProcessState::Gone);
        }
        fs::remove_dir_all(&self.home).unwrap();
    }
}
#[test]
#[ignore="own inert AF_UNIX fixture child only"]
fn local_terminal_inert_server_entry() {
    use std::os::unix::{fs::PermissionsExt,net::UnixListener};
    let home=PathBuf::from(std::env::var_os("APERTURE_TERMINAL_FIXTURE_HOME").unwrap());
    assert!(home.starts_with("/private/tmp")&&home.file_name().unwrap().to_str().unwrap().starts_with("ato-"));
    let socket=home.join(".aperture/run/test-dev.sock");let listener=UnixListener::bind(&socket).unwrap();
    fs::set_permissions(socket,fs::Permissions::from_mode(0o600)).unwrap();fs::write(home.join("ready"),b"ready").unwrap();
    for stream in listener.incoming(){drop(stream.unwrap());}
}
#[test]
fn local_terminal_separate_client_survives_unlinked_server_and_reuses_receipt() {
    let mut f=OpenFixture::new();f.unlinked_server();let owner_before=fs::read(f.home.join(".aperture/run/owner/test-dev.json")).unwrap();
    let runtime=f.runtime();let work=runtime.admit(None).unwrap();let body=work.body().unwrap();let dispatch=OpenDispatch{work:&work,fixture:Some(&f)};
    let first=open_with(&f.home,input(),&dispatch).unwrap();assert_eq!(first.window_id,"@42");
    let receipt_before=fs::read(client_path(&f.home,&input())).unwrap();let c:Client=serde_json::from_slice(&receipt_before).unwrap();
    let selected=receipt_client(&c).unwrap();assert_eq!(selected.path,f.client);assert_eq!(selected.fingerprint().unwrap(),c.client_fingerprint);
    assert_eq!(open_with(&f.home,input(),&dispatch).unwrap().window_id,"@42");
    assert_eq!(fs::read_to_string(f.home.join("events")).unwrap(),"new-window\nlist-panes\nselect-window\nlist-panes\nselect-window\n");
    assert_eq!(fs::read(client_path(&f.home,&input())).unwrap(),receipt_before);
    assert_eq!(fs::read(f.home.join(".aperture/run/owner/test-dev.json")).unwrap(),owner_before);
    drop(body);drop(work);runtime.close().unwrap();
}
#[test]
fn local_terminal_client_and_helper_drift_deny_before_real_dispatch() {
    for target in ["client","helper"] {for replace in [false,true] {
        let f=OpenFixture::new();let owner_before=fs::read(f.home.join(".aperture/run/owner/test-dev.json")).unwrap();
        let runtime=f.runtime();let work=runtime.admit(None).unwrap();let body=work.body().unwrap();let dispatch=OpenDispatch{work:&work,fixture:Some(&f)};
        *f.drift.borrow_mut()=Some((target,replace));
        let e=open_with(&f.home,input(),&dispatch).err().unwrap();
        assert_eq!(e,if target=="client" {"E_TERMINAL_CLIENT_CHANGED"} else {"E_TERMINAL_HELPER_CHANGED"});
        assert!(!f.home.join("events").exists());assert!(!f.home.join("pane").exists());assert!(!client_path(&f.home,&input()).exists());
        assert_eq!(fs::read(f.home.join(".aperture/run/owner/test-dev.json")).unwrap(),owner_before);
        drop(body);drop(work);runtime.close().unwrap();
    }}
}
#[test]
fn local_terminal_legacy_or_drifted_receipt_denies_without_any_dispatch() {
    let f=OpenFixture::new();let runtime=f.runtime();let work=runtime.admit(None).unwrap();let body=work.body().unwrap();let dispatch=OpenDispatch{work:&work,fixture:Some(&f)};
    journal::ensure_private_dir(&f.home.join(".aperture/run/terminals")).unwrap();
    let path=client_path(&f.home,&input());
    let legacy=serde_json::json!({"binding":"a".repeat(64),"pid":std::process::id(),"birth":1,"window":"@42"});
    journal::write_private_json_atomic(&path,&legacy,false).unwrap();
    assert_eq!(open_with(&f.home,input(),&dispatch).err().unwrap(),"E_TERMINAL_CLIENT_RECEIPT_REQUIRED");
    let mut stale=legacy.clone();stale["client_path"]=serde_json::json!(f.client);stale["client_fingerprint"]=serde_json::json!("0".repeat(64));
    journal::write_private_json_atomic(&path,&stale,true).unwrap();
    assert_eq!(open_with(&f.home,input(),&dispatch).err().unwrap(),"E_TERMINAL_CLIENT_CHANGED");
    assert!(!f.home.join("events").exists());assert!(!f.home.join("pane").exists());
    assert_eq!(journal::read_private_json::<serde_json::Value>(&path).unwrap(),stale);
    drop(body);drop(work);runtime.close().unwrap();
}
