use super::*;
use std::time::Instant;
use tungstenite::Message;

fn text(v: serde_json::Value) -> Message {
    Message::Text(v.to_string())
}
fn presence(name: &str) -> Message {
    text(
        serde_json::json!({"type":"presence","agent":name,"event":"join","ts":"2026-09-27T12:00:00Z"}),
    )
}
fn end(count: usize) -> Message {
    text(
        serde_json::json!({"type":"subscriber_snapshot_end","protocol_version":1,"hub_pid":1234,"snapshot_count":count}),
    )
}

#[test]
fn empty_and_nonempty_snapshots_complete_only_on_matching_end_marker() {
    let at = Instant::now();
    let mut d = SnapshotDecoder::new(at);
    assert_eq!(d.check_deadline(at), Ok(()));
    assert_eq!(d.feed(&Message::Ping(vec![]), at), Ok(None));
    assert_eq!(
        d.feed(&end(0), at),
        Ok(Some(SnapshotCompletion {
            claimed_hub_pid: 1234,
            entries: 0
        }))
    );
    let mut d = SnapshotDecoder::new(at);
    assert_eq!(d.feed(&presence("fixture-a"), at), Ok(None));
    assert_eq!(d.feed(&presence("fixture-b"), at), Ok(None));
    assert_eq!(
        d.feed(&end(2), at),
        Ok(Some(SnapshotCompletion {
            claimed_hub_pid: 1234,
            entries: 2
        }))
    );
    // Completion explicitly retains a CLAIM, even for a syntactically valid
    // impostor's frame. Nothing in this decoder grants daemon adoption.
}
#[test]
fn count_order_duplicate_and_live_frame_after_end_fail_closed() {
    let at = Instant::now();
    let mut d = SnapshotDecoder::new(at);
    assert_eq!(d.feed(&end(1), at), Err("E_HUB_SNAPSHOT_END"));
    assert!(d.feed(&end(0), at).is_err(), "failure stays latched");
    let mut d = SnapshotDecoder::new(at);
    d.feed(&presence("a"), at).unwrap();
    assert_eq!(d.feed(&end(0), at), Err("E_HUB_SNAPSHOT_END"));
    let mut d = SnapshotDecoder::new(at);
    d.feed(&end(0), at).unwrap();
    assert_eq!(d.feed(&end(0), at), Err("E_HUB_SNAPSHOT_AFTER_END"));
    let mut d = SnapshotDecoder::new(at);
    d.feed(&end(0), at).unwrap();
    assert_eq!(
        d.feed(&presence("live-update"), at),
        Err("E_HUB_SNAPSHOT_AFTER_END")
    );
    let mut d = SnapshotDecoder::new(at);
    d.feed(&presence("a"), at).unwrap();
    assert_eq!(d.feed(&presence("a"), at), Err("E_HUB_SNAPSHOT_ENTRY"));
}
#[test]
fn malformed_types_extra_fields_versions_and_pid_claims_are_rejected() {
    let at = Instant::now();
    for msg in [
        Message::Text("{".into()),
        Message::Binary(vec![]),
        Message::Close(None),
        text(
            serde_json::json!({"type":"subscriber_snapshot_end","protocol_version":1,"hub_pid":2,"snapshot_count":0,"token":"synthetic-forbidden"}),
        ),
        text(
            serde_json::json!({"type":"subscriber_snapshot_end","protocol_version":1,"hub_pid":"1234","snapshot_count":0}),
        ),
        text(serde_json::json!({"type":"unknown"})),
    ] {
        assert_eq!(
            SnapshotDecoder::new(at).feed(&msg, at),
            Err("E_HUB_SNAPSHOT_FRAME")
        );
    }
    for version in [0, 2] {
        let m = text(
            serde_json::json!({"type":"subscriber_snapshot_end","protocol_version":version,"hub_pid":2,"snapshot_count":0}),
        );
        assert_eq!(
            SnapshotDecoder::new(at).feed(&m, at),
            Err("E_HUB_SNAPSHOT_VERSION")
        );
    }
    for pid in [0u32, 1, u32::MAX] {
        let m = text(
            serde_json::json!({"type":"subscriber_snapshot_end","protocol_version":1,"hub_pid":pid,"snapshot_count":0}),
        );
        assert_eq!(
            SnapshotDecoder::new(at).feed(&m, at),
            Err("E_HUB_SNAPSHOT_END")
        );
    }
}
#[test]
fn presence_shape_state_and_timestamp_are_checked() {
    let at = Instant::now();
    for (name, event, ts) in [
        ("", "join", "2026-09-27T12:00:00Z"),
        ("a", "leave", "2026-09-27T12:00:00Z"),
        ("a", "join", "invalid"),
        ("a\n", "join", "2026-09-27T12:00:00Z"),
    ] {
        let m = text(serde_json::json!({"type":"presence","agent":name,"event":event,"ts":ts}));
        assert_eq!(
            SnapshotDecoder::new(at).feed(&m, at),
            Err("E_HUB_SNAPSHOT_ENTRY")
        );
    }
}
#[test]
fn frame_bytes_total_bytes_frame_count_and_entry_limits_are_finite() {
    let at = Instant::now();
    assert_eq!(
        SnapshotDecoder::new(at).feed(&Message::Text(" ".repeat(SNAPSHOT_FRAME_BYTES + 1)), at),
        Err("E_HUB_SNAPSHOT_LIMIT")
    );
    let mut d = SnapshotDecoder::new(at);
    // Valid zero-payload pings still consume the frame budget.
    for _ in 0..SNAPSHOT_FRAMES {
        assert_eq!(d.feed(&Message::Ping(vec![]), at), Ok(None));
    }
    assert_eq!(d.feed(&end(0), at), Err("E_HUB_SNAPSHOT_LIMIT"));
    let mut d = SnapshotDecoder::new(at);
    // JSON whitespace is legal but counts toward the byte budget. Use unique
    // presence entries so this test reaches the byte limit, not duplicate guard.
    for n in 0..SNAPSHOT_TOTAL_BYTES / SNAPSHOT_FRAME_BYTES {
        let Message::Text(mut m) = presence(&format!("fixture-{n}")) else {
            unreachable!()
        };
        m.push_str(&" ".repeat(SNAPSHOT_FRAME_BYTES - m.len()));
        assert_eq!(d.feed(&Message::Text(m), at), Ok(None));
    }
    assert_eq!(d.feed(&end(64), at), Err("E_HUB_SNAPSHOT_LIMIT"));
    let mut d = SnapshotDecoder::new(at);
    for n in 0..SNAPSHOT_ENTRIES {
        assert_eq!(d.feed(&presence(&format!("fixture-{n}")), at), Ok(None));
    }
    assert_eq!(
        d.feed(&presence("overflow"), at),
        Err("E_HUB_SNAPSHOT_ENTRY")
    );
}
#[test]
fn silence_timeout_clock_regression_and_control_frames_never_complete() {
    let at = Instant::now();
    let mut d = SnapshotDecoder::new(at);
    assert_eq!(
        d.check_deadline(at + SNAPSHOT_DEADLINE),
        Err("E_HUB_SNAPSHOT_TIMEOUT")
    );
    assert!(d.feed(&end(0), at).is_err());
    assert_eq!(
        SnapshotDecoder::new(at).feed(&end(0), at + SNAPSHOT_DEADLINE),
        Err("E_HUB_SNAPSHOT_TIMEOUT")
    );
    assert_eq!(
        SnapshotDecoder::new(at).check_deadline(at - Duration::from_millis(1)),
        Err("E_HUB_SNAPSHOT_TIMEOUT")
    );
    assert_eq!(
        SnapshotDecoder::new(at).feed(&Message::Pong(vec![]), at),
        Ok(None)
    );
}

// B2 fixtures are inert modes of this existing test binary. They never run the
// operational Node hub, CLI/provider, daemon fleet or an external harness.
use crate::{
    daemon_registry::{Endpoint, Provenance, Registry},
    team_replacement::{ProcessIdentity, ProcessState},
};
use std::os::unix::process::CommandExt;
use std::{
    fs,
    net::{Ipv4Addr, SocketAddr, TcpListener},
    path::PathBuf,
    process::{Child, Command, Stdio},
};
const CHILD_TEST: &str = "ws_hub::snapshot_tests::inert_process_entry";

struct NativeFixture {
    home: PathBuf,
    endpoint: SocketAddr,
    children: Vec<(Child, ProcessIdentity)>,
}
impl NativeFixture {
    fn new() -> Self {
        let home = std::env::temp_dir().join(format!("aperture-b2-hub-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&home).unwrap();
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let endpoint = listener.local_addr().unwrap();
        drop(listener);
        let lease = ControllerLock::acquire(&home).unwrap();
        let dir = lease.run_dir().unwrap().join("hub-tokens");
        crate::journal::ensure_private_dir(&dir).unwrap();
        crate::journal::write_private_bytes_atomic(
            &dir.join("watchdog.token"),
            b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            false,
        )
        .unwrap();
        drop(lease);
        Self {
            home,
            endpoint,
            children: Vec::new(),
        }
    }
    fn spec(&self, mode: &str) -> HubSpec {
        HubSpec {
            command: std::env::current_exe().unwrap(),
            args: vec![
                "--exact".into(),
                CHILD_TEST.into(),
                "--ignored".into(),
                "--nocapture".into(),
                "--test-threads=1".into(),
            ],
            endpoint: self.endpoint,
            home: self.home.clone(),
            env:vec![],
            provenance: Provenance::LegacyUnknown,
            fault: None,
            fixture_mode: String::new(),
        }
        .with_fixture_mode(mode)
    }
    fn launch(&mut self, mode: &str) -> ProcessIdentity {
        let mut cmd = Command::new(std::env::current_exe().unwrap());
        cmd.args([
            "--exact",
            CHILD_TEST,
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("HOME", &self.home)
        .env("APERTURE_RUN_DIR", self.home.join(".aperture/run"))
        .env("APERTURE_WS_PORT", self.endpoint.port().to_string())
        .env("APERTURE_FIXTURE_MODE", mode)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    Err(std::io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
        let child = cmd.spawn().unwrap();
        let p = team_process::observe(child.id()).unwrap().unwrap().identity;
        self.children.push((child, p.clone()));
        p
    }
    fn wait_file(&self, name: &str) -> Vec<u8> {
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(b) = fs::read(self.home.join(".aperture/run").join(name)) {
                return b;
            }
            assert!(Instant::now() < until, "fixture file timeout: {name}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    fn published(&self, lease: &ControllerLock, identity: &ProcessIdentity) {
        let r = Registry::open(lease).unwrap();
        let reservation = r
            .reserve(
                Endpoint::Hub {
                    port: self.endpoint.port(),
                },
                Provenance::LegacyUnknown,
                1,
            )
            .unwrap();
        let record = r.record(&reservation, identity, 2).unwrap();
        r.publish_current(&record).unwrap();
    }
    fn cleanup_identity(&self, identity: &ProcessIdentity) {
        if team_process::state(identity) == ProcessState::Same {
            assert_eq!(unsafe { libc::kill(identity.pid as i32, libc::SIGKILL) }, 0);
        }
        let until = Instant::now() + Duration::from_secs(3);
        loop {
            // May be our detached direct child or reparented under launchd.
            unsafe {
                libc::waitpid(identity.pid as i32, std::ptr::null_mut(), libc::WNOHANG);
            }
            if team_process::state(identity) == ProcessState::Gone {
                break;
            }
            assert!(Instant::now() < until, "own fixture cleanup not verified");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
// Mode selects inert behavior only in this test build; no production switch.
impl HubSpec {
    fn with_fixture_mode(mut self, mode: &str) -> Self {
        self.fixture_mode = mode.into();
        self
    }
}
impl Drop for NativeFixture {
    fn drop(&mut self) {
        for (child, identity) in &mut self.children {
            if team_process::state(identity) == ProcessState::Same {
                unsafe {
                    libc::kill(identity.pid as i32, libc::SIGKILL);
                }
            }
            let _ = child.wait();
            assert_eq!(team_process::state(identity), ProcessState::Gone);
        }
        // Catch any child spawned through the actual supervisor, whose handle
        // was intentionally detached. The fixture writes only its own identity.
        if let Ok(raw) = fs::read(self.home.join(".aperture/run/fixture-daemon.json")) {
            let p: ProcessIdentity = serde_json::from_slice(&raw).unwrap();
            self.cleanup_identity(&p);
        }
        fs::remove_dir_all(&self.home).unwrap();
    }
}

#[test]
#[ignore = "inert child entry; launched only by scoped B2 tests"]
fn inert_process_entry() {
    let home = PathBuf::from(std::env::var_os("HOME").unwrap());
    assert!(home
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("aperture-b2-hub-"));
    let mode = std::env::var("APERTURE_FIXTURE_MODE").unwrap();
    let port: u16 = std::env::var("APERTURE_WS_PORT").unwrap().parse().unwrap();
    let run = home.join(".aperture/run");
    let identity = team_process::observe(std::process::id())
        .unwrap()
        .unwrap()
        .identity;
    if mode == "controller" {
        let lease = ControllerLock::acquire(&home).unwrap();
        let mut spec = HubSpec {
            command: std::env::current_exe().unwrap(),
            args: vec![
                "--exact".into(),
                CHILD_TEST.into(),
                "--ignored".into(),
                "--nocapture".into(),
                "--test-threads=1".into(),
            ],
            endpoint: SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
            home: home.clone(),
            env:vec![],
            provenance: Provenance::LegacyUnknown,
            fault: None,
            fixture_mode: "normal".into(),
        };
        spec.fixture_mode = "normal".into();
        let result = Supervisor::new(&lease, spec).unwrap().reconcile().unwrap();
        crate::journal::write_private_json_atomic(
            &run.join("fixture-controller-ready.json"),
            &result.identity,
            false,
        )
        .unwrap();
        loop {
            std::thread::sleep(Duration::from_secs(1));
        }
    }
    if let Some(upstream) = mode.strip_prefix("proxy:") {
        let upstream: u16 = upstream.parse().unwrap();
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port)).unwrap();
        fs::write(run.join("fixture-ready"), b"ready").unwrap();
        for stream in listener.incoming() {
            let incoming = stream.unwrap();
            let outgoing = TcpStream::connect((Ipv4Addr::LOCALHOST, upstream)).unwrap();
            let mut in_read = incoming.try_clone().unwrap();
            let mut out_write = outgoing.try_clone().unwrap();
            std::thread::spawn(move || {
                let _ = std::io::copy(&mut in_read, &mut out_write);
            });
            std::thread::spawn(move || {
                let _ = std::io::copy(&mut &outgoing, &mut &incoming);
            });
        }
        return;
    }
    if mode == "no-bind" {
        loop {
            std::thread::sleep(Duration::from_secs(1));
        }
    }
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port)).unwrap();
    crate::journal::write_private_json_atomic(&run.join("fixture-daemon.json"), &identity, true)
        .unwrap();
    if mode != "impostor" {
        crate::journal::write_private_json_atomic(&run.join("presence.json"), &serde_json::json!({"hub_pid":identity.pid,"updated_at":"2026-09-27T12:00:00Z","agents":{}}), true).unwrap();
    }
    fs::write(run.join("fixture-ready"), b"ready").unwrap();
    for stream in listener.incoming() {
        let stream = stream.unwrap();
        let mode = mode.clone();
        let run = run.clone();
        std::thread::spawn(move || {
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut ws = match tungstenite::accept(stream) {
                Ok(v) => v,
                Err(_) => return,
            };
            let mut final_msg = serde_json::json!({"type":"subscriber_snapshot_end","protocol_version":1,"hub_pid":std::process::id(),"snapshot_count":0});
            if mode == "impostor" {
                let p: serde_json::Value =
                    serde_json::from_slice(&fs::read(run.join("presence.json")).unwrap()).unwrap();
                final_msg["hub_pid"] = p["hub_pid"].clone(); // perfectly formed claimed recorded PID
            }
            if mode == "early" {
                let _ = ws.send(Message::Text(final_msg.to_string()));
            }
            let hello = ws.read();
            if let Ok(Message::Text(raw)) = hello {
                let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
                assert_eq!(parsed["type"], "hello");
                assert_eq!(parsed["role"], "subscriber");
                // Record counts only, never the hello/token bytes.
                use std::io::Write;
                let mut file = fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(run.join("fixture-hello-count"))
                    .unwrap();
                file.write_all(b"1\n").unwrap();
                if mode == "d-auth-reject" {
                    let _ = ws.send(text(serde_json::json!({"type":"error","code":"unauthorized"})));
                    return;
                }
                if mode == "d-truncated" {
                    let _ = ws.send(presence("fixture")); return;
                }
                if mode == "d-live" {
                    final_msg["snapshot_count"] = serde_json::json!(1);
                    ws.write(presence("fixture")).unwrap();
                    ws.write(Message::Text(final_msg.to_string())).unwrap();
                    ws.write(text(serde_json::json!({"type":"presence","agent":"fixture","event":"busy","ts":"2026-09-27T12:00:00Z"}))).unwrap();
                    ws.flush().unwrap();
                    let _ = ws.read();
                    std::thread::sleep(Duration::from_secs(60));
                    return;
                }
                if mode == "wrongpid" {
                    final_msg["hub_pid"] = serde_json::json!(1);
                }
                if mode == "badcount" {
                    final_msg["snapshot_count"] = serde_json::json!(1);
                }
                if mode == "close" {
                    let _ = ws.close(None);
                    return;
                }
                if mode == "silent" {
                    std::thread::sleep(Duration::from_secs(4));
                    return;
                }
                if mode == "oversize" {
                    let _ = ws.send(Message::Text("x".repeat(SNAPSHOT_FRAME_BYTES + 1)));
                    return;
                }
                if mode == "presence-drift" {
                    crate::journal::write_private_json_atomic(&run.join("presence.json"), &serde_json::json!({"hub_pid":1,"updated_at":"2026-09-27T12:00:00Z","agents":{}}), true).unwrap();
                }
                if mode == "duplicate" {
                    ws.write(Message::Text(final_msg.to_string())).unwrap();
                    ws.write(Message::Text(final_msg.to_string())).unwrap();
                    let _ = ws.flush();
                } else {
                    let _ = ws.send(Message::Text(final_msg.to_string()));
                }
                if mode == "close-after" {
                    let _ = ws.close(None);
                    return;
                }
                // Remain established through both native checks; end only when
                // the consumer closes. No daemon exit on controller detach.
                let _ = ws.read();
                // Keep fixture FD stable during another caller's enumeration.
                std::thread::sleep(Duration::from_secs(60));
            }
        });
    }
}

#[test]
fn b2_kernel_abi_bounds_and_tuple_oracles() {
    crate::team_process::tcp_binding::abi_negative_oracles();
}

#[test]
fn b2_impostor_never_receives_hello_and_cannot_adopt_or_spawn() {
    let mut f = NativeFixture::new();
    let lease = ControllerLock::acquire(&f.home).unwrap();
    let expected = lease.identity().unwrap().clone(); // Same real process, not the listener.
    f.published(&lease, &expected);
    crate::journal::write_private_json_atomic(&lease.run_dir().unwrap().join("presence.json"), &serde_json::json!({"hub_pid":expected.pid,"updated_at":"2026-09-27T12:00:00Z","agents":{}}), false).unwrap();
    let impostor = f.launch("impostor");
    f.wait_file("fixture-ready");
    let before = fs::read_dir(lease.run_dir().unwrap().join("daemons/hub"))
        .unwrap()
        .count();
    assert!(Supervisor::new(&lease, f.spec("normal"))
        .unwrap()
        .reconcile()
        .is_err());
    assert!(!lease
        .run_dir()
        .unwrap()
        .join("fixture-hello-count")
        .exists());
    assert_eq!(
        Registry::open(&lease)
            .unwrap()
            .current("hub")
            .unwrap()
            .unwrap()
            .identity(),
        expected
    );
    assert_eq!(
        fs::read_dir(lease.run_dir().unwrap().join("daemons/hub"))
            .unwrap()
            .count(),
        before
    );
    assert_eq!(team_process::state(&impostor), ProcessState::Same); // signal0
    assert_eq!(team_process::state(&expected), ProcessState::Same);
}

#[test]
fn b2_protocol_negatives_preserve_record() {
    for mode in [
        "badcount",
        "wrongpid",
        "duplicate",
        "close",
        "close-after",
        "presence-drift",
        "oversize",
        "early",
        "silent",
    ] {
        let mut f = NativeFixture::new();
        let lease = ControllerLock::acquire(&f.home).unwrap();
        let expected = f.launch(mode);
        f.wait_file("fixture-ready");
        f.published(&lease, &expected);
        let result = Supervisor::new(&lease, f.spec("normal"))
            .unwrap()
            .reconcile();
        assert!(result.is_err(), "mode {mode}");
        assert_eq!(team_process::state(&expected), ProcessState::Same);
        assert_eq!(
            Registry::open(&lease)
                .unwrap()
                .current("hub")
                .unwrap()
                .unwrap()
                .identity(),
            expected
        );
        assert_eq!(
            fs::read_dir(lease.run_dir().unwrap().join("daemons/hub"))
                .unwrap()
                .count(),
            3
        );
    }
}

#[test]
fn b2_single_flight_spawns_once_and_detaches_without_signal() {
    let f = NativeFixture::new();
    let lease = ControllerLock::acquire(&f.home).unwrap();
    let results = std::thread::scope(|scope| {
        let a = scope.spawn(|| {
            Supervisor::new(&lease, f.spec("normal"))
                .unwrap()
                .reconcile()
        });
        let b = scope.spawn(|| {
            Supervisor::new(&lease, f.spec("normal"))
                .unwrap()
                .reconcile()
        });
        [a.join().unwrap().unwrap(), b.join().unwrap().unwrap()]
    });
    assert_eq!(results.iter().filter(|r| r.spawned).count(), 1);
    assert_eq!(results[0].identity, results[1].identity);
    assert_eq!(
        fs::read_dir(lease.run_dir().unwrap().join("daemons/hub"))
            .unwrap()
            .count(),
        3
    );
    let p = results[0].identity.clone();
    drop(lease);
    shutdown();
    assert_eq!(team_process::state(&p), ProcessState::Same);
    let lease = ControllerLock::acquire(&f.home).unwrap();
    assert!(
        !Supervisor::new(&lease, f.spec("normal"))
            .unwrap()
            .reconcile()
            .unwrap()
            .spawned
    );
}

#[test]
fn b2_crash_edges_never_retry_by_new_uuid_even_when_endpoint_free() {
    for point in ["reserved", "spawned", "recorded"] {
        let f = NativeFixture::new();
        let lease = ControllerLock::acquire(&f.home).unwrap();
        let mut spec = f.spec("normal");
        spec.fault = Some(point.into());
        assert!(Supervisor::new(&lease, spec).unwrap().reconcile().is_err());
        if point != "reserved" {
            f.wait_file("fixture-daemon.json");
        }
        let dir = lease.run_dir().unwrap().join("daemons/hub");
        let before = fs::read_dir(&dir).unwrap().count();
        for _ in 0..2 {
            assert!(Supervisor::new(&lease, f.spec("normal"))
                .unwrap()
                .reconcile()
                .is_err());
        }
        assert_eq!(fs::read_dir(dir).unwrap().count(), before);
    }
}

#[test]
fn b2_controller_term_and_kill_leave_daemon_alive_then_adopt() {
    for sig in [libc::SIGTERM, libc::SIGKILL] {
        let mut f = NativeFixture::new();
        let controller = f.launch("controller");
        let daemon: ProcessIdentity =
            serde_json::from_slice(&f.wait_file("fixture-controller-ready.json")).unwrap();
        assert!(ControllerLock::acquire(&f.home).is_err());
        assert_eq!(team_process::state(&controller), ProcessState::Same);
        assert_eq!(unsafe { libc::kill(controller.pid as i32, sig) }, 0);
        let (child, _) = f.children.last_mut().unwrap();
        child.wait().unwrap();
        assert_eq!(team_process::state(&controller), ProcessState::Gone);
        assert_eq!(team_process::state(&daemon), ProcessState::Same);
        let lease = ControllerLock::acquire(&f.home).unwrap();
        let r = Supervisor::new(&lease, f.spec("normal"))
            .unwrap()
            .reconcile()
            .unwrap();
        assert!(!r.spawned);
        assert_eq!(r.identity, daemon);
    }
}

#[test]
fn b2_unknown_listener_and_recycled_identity_never_reserve_or_signal() {
    let mut f = NativeFixture::new();
    let lease = ControllerLock::acquire(&f.home).unwrap();
    let listener = f.launch("normal");
    f.wait_file("fixture-ready");
    assert!(Supervisor::new(&lease, f.spec("normal"))
        .unwrap()
        .reconcile()
        .is_err());
    assert!(Registry::open(&lease)
        .unwrap()
        .current("hub")
        .unwrap()
        .is_none());
    assert_eq!(team_process::state(&listener), ProcessState::Same);
    let stream = TcpStream::connect(f.endpoint).unwrap();
    let mut recycled = listener.clone();
    recycled.start_time = "1.000001".into();
    assert!(team_process::verify_tcp_server_binding(&recycled, f.endpoint, &stream).is_err());
    let unreadable = ProcessIdentity {
        pid: 0,
        start_time: "1.000001".into(),
    };
    assert!(team_process::verify_tcp_server_binding(&unreadable, f.endpoint, &stream).is_err());
    assert!(team_process::verify_tcp_server_binding(
        &listener,
        SocketAddr::from((Ipv4Addr::LOCALHOST, f.endpoint.port() + 1)),
        &stream
    )
    .is_err());
}

#[test]
fn b2_gone_is_separate_single_spawn_transition_with_history_retained() {
    let mut f = NativeFixture::new();
    let lease = ControllerLock::acquire(&f.home).unwrap();
    let old = f.launch("normal");
    f.wait_file("fixture-ready");
    f.published(&lease, &old);
    assert_eq!(unsafe { libc::kill(old.pid as i32, libc::SIGKILL) }, 0);
    f.children.last_mut().unwrap().0.wait().unwrap();
    assert_eq!(team_process::state(&old), ProcessState::Gone);
    let unknown = TcpListener::bind(f.endpoint).unwrap();
    assert!(Supervisor::new(&lease, f.spec("normal"))
        .unwrap()
        .reconcile()
        .is_err());
    assert_eq!(
        fs::read_dir(lease.run_dir().unwrap().join("daemons/hub"))
            .unwrap()
            .count(),
        3
    );
    drop(unknown);

    let r = Supervisor::new(&lease, f.spec("normal"))
        .unwrap()
        .reconcile()
        .unwrap();
    assert!(r.spawned);
    assert_ne!(r.identity, old);
    assert_eq!(
        fs::read_dir(lease.run_dir().unwrap().join("daemons/hub"))
            .unwrap()
            .count(),
        5
    );
    assert!(
        !Supervisor::new(&lease, f.spec("normal"))
            .unwrap()
            .reconcile()
            .unwrap()
            .spawned
    );
}

#[test]
fn b2_real_owner_adoption_positive() {
    let mut f = NativeFixture::new();
    let lease = ControllerLock::acquire(&f.home).unwrap();
    let expected = f.launch("normal");
    f.wait_file("fixture-ready");
    f.published(&lease, &expected);
    let result = Supervisor::new(&lease, f.spec("normal"))
        .unwrap()
        .reconcile()
        .unwrap();
    assert!(!result.spawned);
    assert_eq!(result.identity, expected);
    assert_eq!(result.exit_observation, "NOT_OBSERVED");
    assert_eq!(team_process::state(&expected), ProcessState::Same);
    assert_eq!(
        fs::read_dir(lease.run_dir().unwrap().join("daemons/hub"))
            .unwrap()
            .count(),
        3
    );
    drop(lease);
    drop(f); // verified cleanup before PASS
}

#[test]
fn b2_header_adapter_fragmentation_oversize_early_bytes_and_deadline() {
    let response = b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: synthetic\r\n\r\n";
    let mut h = HeaderBuffer::default();
    for b in response {
        h.push(&[*b]).unwrap();
    }
    assert!(h.complete);
    let mut out = [0; 4096];
    let n = h.copy_to(&mut out);
    assert_eq!(&out[..n], response); // one normal-size delivery, no tiny reads
    assert!(HeaderBuffer::default()
        .push(&vec![b'a'; HEADER_BYTES])
        .is_err());
    assert!(HeaderBuffer::default()
        .push(&vec![b'a'; HEADER_BYTES + 1])
        .is_err());
    let mut combined = response.to_vec();
    combined.extend_from_slice(b"early-websocket-frame");
    assert!(HeaderBuffer::default().push(&combined).is_err());
    assert!(h.push(b"late-pre-hello-frame").is_err());
    let at = Instant::now();
    let deadline = at + Duration::from_secs(3);
    assert_eq!(
        remaining_time(deadline, at).unwrap(),
        Duration::from_secs(3)
    );
    assert_eq!(
        remaining_time(deadline, at + Duration::from_secs(2)).unwrap(),
        Duration::from_secs(1)
    );
    assert_eq!(
        remaining_time(deadline, deadline).unwrap_err().kind(),
        std::io::ErrorKind::TimedOut
    );
    assert!(remaining_time(deadline, deadline + Duration::from_millis(1)).is_err());
    // Kernel-queued pre-hello tail is refused too, not silently discarded.
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (mut server, _) = listener.accept().unwrap();
    server.write_all(b"early").unwrap();
    let mut byte = [0];
    client.peek(&mut byte).unwrap();
    assert!(no_pending_bytes(&client).is_err());
}

#[test]
fn b2_proxy_upgrade_never_receives_hello_for_recorded_other_listener() {
    let mut backend = NativeFixture::new();
    let expected = backend.launch("normal");
    backend.wait_file("fixture-ready");
    let mut front = NativeFixture::new();
    let lease = ControllerLock::acquire(&front.home).unwrap();
    front.published(&lease, &expected);
    crate::journal::write_private_json_atomic(&lease.run_dir().unwrap().join("presence.json"), &serde_json::json!({"hub_pid":expected.pid,"updated_at":"2026-09-27T12:00:00Z","agents":{}}), false).unwrap();
    let proxy = front.launch(&format!("proxy:{}", backend.endpoint.port()));
    front.wait_file("fixture-ready");
    assert!(Supervisor::new(&lease, front.spec("normal"))
        .unwrap()
        .reconcile()
        .is_err());
    assert!(!backend
        .home
        .join(".aperture/run/fixture-hello-count")
        .exists());
    assert_eq!(
        Registry::open(&lease)
            .unwrap()
            .current("hub")
            .unwrap()
            .unwrap()
            .identity(),
        expected
    );
    assert_eq!(team_process::state(&expected), ProcessState::Same);
    assert_eq!(team_process::state(&proxy), ProcessState::Same);
    assert_eq!(
        fs::read_dir(lease.run_dir().unwrap().join("daemons/hub"))
            .unwrap()
            .count(),
        3
    );
}

#[test]
fn b2_kernel_requires_accepted_fd_and_rechecks_identity_after_snapshot() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let endpoint = listener.local_addr().unwrap();
    let stream = TcpStream::connect(endpoint).unwrap();
    let identity = team_process::observe(std::process::id())
        .unwrap()
        .unwrap()
        .identity;
    // Listening PID alone is insufficient: accepted fd must also be enumerated.
    assert!(team_process::verify_tcp_server_binding(&identity, endpoint, &stream).is_err());
    let (server, _) = listener.accept().unwrap();
    team_process::verify_tcp_server_binding(&identity, endpoint, &stream).unwrap();
    for state in [
        ProcessState::Gone,
        ProcessState::Recycled,
        ProcessState::Unreadable,
    ] {
        assert!(crate::team_process::tcp_binding::post_identity_oracle(
            &identity, endpoint, &stream, state
        )
        .is_err());
    }
    server.shutdown(std::net::Shutdown::Both).unwrap();
    let mut b = [0];
    assert_eq!(stream.peek(&mut b).unwrap(), 0);
    assert!(team_process::verify_tcp_server_binding(&identity, endpoint, &stream).is_err());
}

#[test]
fn b2_direct_child_wait_handle_survives_supervisor_drop() {
    let f = NativeFixture::new();
    let lease = ControllerLock::acquire(&f.home).unwrap();
    let old = Supervisor::new(&lease, f.spec("normal"))
        .unwrap()
        .reconcile()
        .unwrap()
        .identity;
    assert_eq!(lease.hub_child().unwrap().as_ref().unwrap().id(), old.pid);
    assert_eq!(unsafe { libc::kill(old.pid as i32, libc::SIGKILL) }, 0);
    lease.hub_child().unwrap().as_mut().unwrap().wait().unwrap();
    assert_eq!(team_process::state(&old), ProcessState::Gone);
    let next = Supervisor::new(&lease, f.spec("normal"))
        .unwrap()
        .reconcile()
        .unwrap();
    assert!(next.spawned);
    assert_ne!(next.identity, old);
    assert_eq!(
        fs::read_dir(lease.run_dir().unwrap().join("daemons/hub"))
            .unwrap()
            .count(),
        5
    );
}

#[test]
fn c3_hub_reconcile_accepts_unrelated_v2_structure_not_readiness() {
    use crate::daemon_registry::{CodexEventV2 as E, Identity, NodePinV2, SocketPinsV2, TermResultV2, StopOutcomeV2, CleanupOutcomeV2};
    for stage in [0,1,2,4,6,8,9,10,11] {
        let mut f=NativeFixture::new();let lease=ControllerLock::acquire(&f.home).unwrap();
        let expected=f.launch("normal");f.wait_file("fixture-ready");f.published(&lease,&expected);
        let registry=Registry::open(&lease).unwrap();let slot=lease.codex_slot("unrelated").unwrap();let op=slot.enter().unwrap();
        let id=registry.begin_codex_v2(&op,Provenance::LegacyUnknown,1).unwrap();
        let append=|event|registry.append_codex_v2(&op,&id,event,2).unwrap();
        if stage>=1 {append(E::Spawned{process:Identity::from_native(lease.identity().unwrap()).unwrap()});}
        if stage>=2 {
            // Unrelated CODEX METADATA only; not claimed native socket/TERM proof.
            let parent=NodePinV2{dev:1,ino:2,uid:unsafe{libc::geteuid()},mode:libc::S_IFDIR as u32|0o700,links:0};
            let leaf=NodePinV2{dev:1,ino:3,uid:parent.uid,mode:libc::S_IFSOCK as u32|0o600,links:1};
            append(E::SocketReady{pins:SocketPinsV2{format_version:1,parents:vec![parent],entry:leaf,native_target:None}});
        }
        if stage>=3 {registry.publish_codex_v2(&op,&id).unwrap();}
        if stage>=4 {append(E::StopIntent{});}
        if stage>=5 {append(E::TermResult{result:if stage==6{TermResultV2::Esrch}else{TermResultV2::ReturnedZero}});}
        if stage>=7 {append(E::StopOutcome{outcome:StopOutcomeV2::TermSentThenDaemonGoneDescendantsUnverified});}
        if stage>=9 {append(E::CleanupIntent{});}
        if stage>=10 {append(E::CleanupOutcome{outcome:if stage==10{CleanupOutcomeV2::Unknown}else{CleanupOutcomeV2::FixedDirectEntryRemoved}});}
        assert!(registry.inspect().is_err());
        let before=crate::agents::lifecycle_tests::tree(&f.home.join(".aperture/run/daemons"));
        let adopted=Supervisor::new(&lease,f.spec("normal")).unwrap().reconcile().unwrap();
        assert!(!adopted.spawned);assert_eq!(adopted.identity,expected);
        assert_eq!(crate::agents::lifecycle_tests::tree(&f.home.join(".aperture/run/daemons")),before);
        assert!(registry.begin_codex_v2(&op,Provenance::LegacyUnknown,3).is_err());
        // Actual hub path must still deny malformed namespace without touching target.
        fs::write(f.home.join(".aperture/run/daemons/orphan"),b"unknown").unwrap();
        let malformed=crate::agents::lifecycle_tests::tree(&f.home.join(".aperture/run/daemons"));
        assert!(Supervisor::new(&lease,f.spec("normal")).unwrap().reconcile().is_err());
        assert_eq!(crate::agents::lifecycle_tests::tree(&f.home.join(".aperture/run/daemons")),malformed);
        assert_eq!(team_process::state(&expected),ProcessState::Same);
    }
}

#[test]
fn d_owned_subscriber_uses_same_native_bound_stream_initial_and_buffered_live() {
    for mode in ["normal","d-live"] {
        let mut f=NativeFixture::new(); let identity=f.launch(mode); f.wait_file("fixture-ready");
        let lease=ControllerLock::acquire(&f.home).unwrap(); f.published(&lease,&identity);
        let mut stream=registered_subscriber(&lease).unwrap();
        assert!(stream.next(&lease).is_err()); // initial snapshot must be consumed first
        let initial=stream.take_initial().unwrap();
        if mode=="d-live" {
            assert_eq!(initial.len(),1); assert_eq!(initial[0].agent,"fixture");
            let update=stream.next(&lease).unwrap().unwrap();
            assert_eq!(update.agent,"fixture"); assert_eq!(update.event,"busy");
        } else { assert!(initial.is_empty()); }
        assert!(stream.take_initial().is_err());
        assert_eq!(f.wait_file("fixture-hello-count"),b"1\n"); // exactly this connection
        let at=Instant::now(); assert!(stream.next(&lease).unwrap().is_none());
        assert!(at.elapsed()<Duration::from_secs(2));
        // Fresh post-frame/read verification, not the old PID claim.
        crate::journal::write_private_json_atomic(&lease.run_dir().unwrap().join("presence.json"),
            &serde_json::json!({"hub_pid":1,"updated_at":"2026-09-27T12:00:00Z","agents":{}}),true).unwrap();
        assert!(stream.next(&lease).is_err());
        drop(stream); drop(lease);
    }
}
#[test]
fn d_subscriber_auth_truncation_duplicate_timeout_and_impostor_never_trusted() {
    for mode in ["d-auth-reject","d-truncated","duplicate","silent","presence-drift","oversize","badcount"] {
        let mut f=NativeFixture::new(); let identity=f.launch(mode); f.wait_file("fixture-ready");
        let lease=ControllerLock::acquire(&f.home).unwrap(); f.published(&lease,&identity);
        let at=Instant::now(); assert!(registered_subscriber(&lease).is_err(),"{mode}");
        assert!(at.elapsed()<Duration::from_secs(4));
    }
    let mut f=NativeFixture::new(); let impostor=f.launch("impostor"); f.wait_file("fixture-ready");
    let lease=ControllerLock::acquire(&f.home).unwrap();
    let expected=team_process::observe(std::process::id()).unwrap().unwrap().identity;
    crate::journal::write_private_json_atomic(&lease.run_dir().unwrap().join("presence.json"),
        &serde_json::json!({"hub_pid":expected.pid,"updated_at":"2026-09-27T12:00:00Z","agents":{}}),true).unwrap();
    f.published(&lease,&expected);
    assert_eq!(team_process::state(&expected),ProcessState::Same);
    assert!(registered_subscriber(&lease).is_err());
    assert!(!f.home.join(".aperture/run/fixture-hello-count").exists());
    assert_eq!(team_process::state(&impostor),ProcessState::Same); // no signal/spawn
}

#[test]
fn local_composition_starts_registered_hub_and_drains_workers_without_killing_daemon() {
    let f=NativeFixture::new();
    let lease=ControllerLock::acquire(&f.home).unwrap();
    let tools=crate::daemons::LocalTools::fixture(&f.home,&std::env::current_exe().unwrap());
    let owner=crate::daemons::RuntimeOwner::local(lease,tools).unwrap();
    let state=crate::agents::lifecycle_tests::state("fixture","opus");
    owner.start_with_hub(state.clone(),f.spec("normal")).unwrap();
    let identity:ProcessIdentity=serde_json::from_slice(&f.wait_file("fixture-daemon.json")).unwrap();
    assert_eq!(team_process::state(&identity),ProcessState::Same);
    assert!(ControllerLock::acquire(&f.home).is_err());
    let token=fs::read(f.home.join(".aperture/run/hub-tokens/watchdog.token")).unwrap();
    owner.close().unwrap();assert!(owner.admit(None).is_err());
    assert_eq!(team_process::state(&identity),ProcessState::Same);
    drop(owner);
    let lease=ControllerLock::acquire(&f.home).unwrap();
    let tools=crate::daemons::LocalTools::fixture(&f.home,&std::env::current_exe().unwrap());
    let next=crate::daemons::RuntimeOwner::local(lease,tools).unwrap();
    next.start_with_hub(state,f.spec("normal")).unwrap();
    assert_eq!(fs::read(f.home.join(".aperture/run/hub-tokens/watchdog.token")).unwrap(),token,"adoption must not rotate token");
    next.close().unwrap();drop(next);drop(f); // fixture kills/reaps only its exact native child
}
#[test]
fn local_composition_unknown_hub_closes_without_token_rotation_or_spawn() {
    let mut f=NativeFixture::new();let expected=f.launch("normal");f.wait_file("fixture-ready");
    let token=fs::read(f.home.join(".aperture/run/hub-tokens/watchdog.token")).unwrap();
    let lease=ControllerLock::acquire(&f.home).unwrap();
    let tools=crate::daemons::LocalTools::fixture(&f.home,&std::env::current_exe().unwrap());
    let owner=crate::daemons::RuntimeOwner::local(lease,tools).unwrap();
    assert!(owner.start_with_hub(crate::agents::lifecycle_tests::state("fixture","opus"),f.spec("normal")).is_err());
    assert!(owner.admit(None).is_err());
    assert_eq!(team_process::state(&expected),ProcessState::Same);
    assert_eq!(fs::read(f.home.join(".aperture/run/hub-tokens/watchdog.token")).unwrap(),token);
    assert!(!f.home.join(".aperture/run/daemons/hub").exists());
    drop(owner);drop(f);
}
