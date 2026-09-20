use super::*;
use std::net::TcpListener;
use std::thread;
use tungstenite::protocol::{frame::coding::CloseCode, CloseFrame};
fn ack() -> serde_json::Value {
    serde_json::json!({
        "type":"ok","control":"revoke_generation","seat":"t1-worker","generation":1,
        "token_deleted":true,"token_absent_verified":true,"token_directory_synced":true,
        "sockets_close_requested":1,"sockets_closed_verified":1
    })
}
#[test]
fn factual_ack_accepts_absent_replay_but_not_unverified_or_cross_identity() {
    let mut replay = ack();
    replay["token_deleted"] = false.into();
    assert!(validate_ack(replay, "t1-worker", 1).is_ok());
    for variant in 0..8 {
        let mut v = ack();
        match variant {
            0 => v["seat"] = "other-worker".into(),
            1 => v["generation"] = 2.into(),
            2 => v["token_absent_verified"] = false.into(),
            3 => v["token_directory_synced"] = false.into(),
            4 => v["sockets_closed_verified"] = 0.into(),
            5 => {
                v.as_object_mut().unwrap().remove("token_absent_verified");
            }
            6 => v["unexpected_field"] = "fixture".into(),
            _ => v["control"] = "other".into(),
        }
        assert!(matches!(
            validate_ack(v, "t1-worker", 1),
            Err(ReplacementError::RevocationUnverified)
        ));
    }
}
fn frame(ws: &mut WebSocket<TcpStream>) -> serde_json::Value {
    match ws.read().unwrap() {
        Message::Text(s) => serde_json::from_str(&s).unwrap(),
        _ => panic!("expected synthetic text frame"),
    }
}
fn fixture(
    response: serde_json::Value,
    reconnect: Option<u16>,
) -> (SocketAddr, thread::JoinHandle<usize>) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        let (socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        socket
            .set_write_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut ws = tungstenite::accept(socket).unwrap();
        let hello = frame(&mut ws);
        assert_eq!(
            hello.get("role").and_then(|v| v.as_str()),
            Some("subscriber")
        );
        assert_eq!(
            hello.get("agent").and_then(|v| v.as_str()),
            Some("watchdog")
        );
        let request = frame(&mut ws);
        assert_eq!(
            request.get("type").and_then(|v| v.as_str()),
            Some("revoke_generation")
        );
        assert_eq!(
            request.get("seat").and_then(|v| v.as_str()),
            Some("t1-worker")
        );
        assert!(
            request.get("token").is_none(),
            "mutation does not expose bearer as request field"
        );
        ws.send(Message::Text(response.to_string())).unwrap();
        drop(ws);
        if let Some(code) = reconnect {
            let (socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut ws = tungstenite::accept(socket).unwrap();
            let negative = frame(&mut ws);
            assert_eq!(
                negative.get("role").and_then(|v| v.as_str()),
                Some("producer")
            );
            assert_eq!(negative.get("generation").and_then(|v| v.as_u64()), Some(1));
            ws.close(Some(CloseFrame {
                code: CloseCode::from(code),
                reason: "synthetic rejection".into(),
            }))
            .unwrap();
            // one control request, one negative hello; never a second revoke
            2
        } else {
            1
        }
    });
    (address, handle)
}
#[test]
fn native_socket_exchange_requires_factual_ack_then_original_identity_4003() {
    let (address, server) = fixture(ack(), Some(4003));
    let proof = exchange(
        address,
        &Bearer("a".repeat(64)),
        &Bearer("b".repeat(64)),
        "t1-worker",
        1,
        &"c".repeat(64),
    )
    .unwrap();
    assert!(proof.durable && proof.token_deleted && proof.sockets_closed);
    assert_eq!(proof.reconnect_code, 4003);
    assert!(proof.close_elapsed_ms <= 1000);
    assert_eq!(server.join().unwrap(), 2);
}
#[test]
fn wrong_reconnect_code_never_synthesizes_success_or_retries() {
    let (address, server) = fixture(ack(), Some(4001));
    let result = exchange(
        address,
        &Bearer("a".repeat(64)),
        &Bearer("b".repeat(64)),
        "t1-worker",
        1,
        &"c".repeat(64),
    );
    assert!(matches!(
        result,
        Err(ReplacementError::RevocationUnverified)
    ));
    assert_eq!(server.join().unwrap(), 2);
}
#[test]
fn cleanup_error_or_unverified_ack_fails_without_negative_reconnect_or_retry() {
    for response in [
        serde_json::json!({"type":"error","code":"E_REVOCATION_FAILED"}),
        {
            let mut v = ack();
            v["sockets_closed_verified"] = 0.into();
            v
        },
    ] {
        let (address, server) = fixture(response, None);
        let result = exchange(
            address,
            &Bearer("a".repeat(64)),
            &Bearer("b".repeat(64)),
            "t1-worker",
            1,
            &"c".repeat(64),
        );
        assert!(matches!(
            result,
            Err(ReplacementError::RevocationUnverified)
        ));
        assert_eq!(server.join().unwrap(), 1);
    }
}
#[test]
fn nonloopback_destination_is_rejected_before_connect() {
    let address = SocketAddr::from(([192, 0, 2, 1], 4517));
    assert!(matches!(
        connect_at(address, Instant::now() + DEADLINE),
        Err(ReplacementError::RevocationUnverified)
    ));
}

#[test]
fn fragmented_handshake_cannot_renew_absolute_deadline() {
    use std::io::Write;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        for byte in b"HTTP/1.1 101 Switching Protocols\r\n" {
            if socket.write_all(&[*byte]).is_err() {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
    });
    let started = Instant::now();
    assert!(connect_at(address, started + Duration::from_millis(25)).is_err());
    assert!(started.elapsed() < Duration::from_millis(500));
    server.join().unwrap();
}

#[test]
fn fragmented_frame_cannot_renew_absolute_deadline() {
    use std::io::Write;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (socket, _) = listener.accept().unwrap();
        let mut ws = tungstenite::accept(socket).unwrap();
        ws.get_mut().write_all(&[0x81, 64]).unwrap();
        for _ in 0..64 {
            if ws.get_mut().write_all(b"x").is_err() {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
    });
    let mut ws = connect_at(address, Instant::now() + DEADLINE).unwrap();
    let started = Instant::now();
    assert!(read_before(&mut ws, started + Duration::from_millis(25)).is_err());
    assert!(started.elapsed() < Duration::from_millis(500));
    drop(ws);
    server.join().unwrap();
}
#[test]
fn private_reader_is_bounded_and_rejects_symlink_or_unsafe_mode() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let home = std::env::temp_dir().join(format!("aperture-k310b-revoke-{}", uuid::Uuid::new_v4()));
    let dir = home.join(".aperture/run/hub-tokens");
    crate::journal::ensure_private_dir(&dir).unwrap();
    let path = dir.join("watchdog.token");
    crate::journal::write_private_bytes_atomic(&path, "a".repeat(64).as_bytes(), false).unwrap();
    assert!(read_bearer(&home, "watchdog").is_ok());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(read_bearer(&home, "watchdog").is_err());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    crate::journal::write_private_bytes_atomic(&path, "a".repeat(257).as_bytes(), true).unwrap();
    assert!(read_bearer(&home, "watchdog").is_err());
    std::fs::remove_file(&path).unwrap();
    symlink("missing-fixture", &path).unwrap();
    assert!(read_bearer(&home, "watchdog").is_err());
    assert!(read_bearer(&home, "../other").is_err());
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn never_published_bearer_does_not_fabricate_reconnect_witness() {
    let (address, server) = fixture(ack(), None);
    let proof = exchange_optional(
        address,
        &Bearer("a".repeat(64)),
        None,
        "t1-worker",
        1,
        &"c".repeat(64),
    )
    .unwrap();
    assert!(proof.durable && proof.sockets_closed && proof.token_deleted);
    assert_eq!(proof.reconnect_code, 0);
    assert_eq!(server.join().unwrap(), 1);
}
#[test]
fn unlaunched_floor_rejects_corruption_ambiguity_and_noncanonical_digests() {
    use crate::journal::{ensure_private_dir, write_private_json_atomic};
    let home =
        std::env::temp_dir().join(format!("aperture-floor-fixture-{}", uuid::Uuid::new_v4()));
    let root = home.join(".aperture/run/revocations");
    ensure_private_dir(&root).unwrap();
    let path = root.join("t1-worker.json");
    let valid = serde_json::json!({"schema_version":1,"seat":"t1-worker","revoked_through_generation":1,"revoked_token_ids":["a".repeat(64)]});
    write_private_json_atomic(&path, &valid, false).unwrap();
    verify_floor(&home, "t1-worker", 1, &"a".repeat(64)).unwrap();
    for i in 0..8 {
        let mut v = valid.clone();
        match i {
            0 => v["schema_version"] = 2.into(),
            1 => v["seat"] = "other".into(),
            2 => v["revoked_through_generation"] = 2.into(),
            3 => v["revoked_token_ids"] = serde_json::json!(["a".repeat(64), "a".repeat(64)]),
            4 => v["revoked_token_ids"] = serde_json::json!(["a".repeat(64), "not-a-digest"]),
            5 => v["revoked_token_ids"] = serde_json::json!(["b".repeat(64), "a".repeat(64)]),
            6 => v["revoked_token_ids"] = serde_json::json!([]),
            _ => v["unexpected"] = true.into(),
        }
        write_private_json_atomic(&path, &v, true).unwrap();
        assert!(verify_floor(&home, "t1-worker", 1, &"a".repeat(64)).is_err());
    }
    std::fs::remove_dir_all(home).unwrap();
}

struct ReadyFixture(std::path::PathBuf);
impl Drop for ReadyFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
impl ReadyFixture {
    fn new() -> Self {
        use crate::journal::{
            ensure_private_dir, write_private_bytes_atomic, write_private_json_atomic,
        };
        let f = Self(
            std::env::temp_dir().join(format!("aperture-ready-revoke-{}", uuid::Uuid::new_v4())),
        );
        let put = |p: &str, v: serde_json::Value| {
            let p = f.0.join(p);
            ensure_private_dir(p.parent().unwrap()).unwrap();
            write_private_json_atomic(&p, &v, false).unwrap();
        };
        put(
            ".aperture/teams/t1/team.json",
            serde_json::json!({"schema_version":1,"team":"t1","project":"project:aperture","repo":"aperture","mission":"fixture","acceptance":"fixture","preset":{"id":null,"sha256":null},"lead":"t1-worker","seats":[{"name":"t1-worker","role":"lead","harness":"codex","model":"gpt-6-astra","reasoning":"high"}],"fallbacks":[],"grants":[],"created_at":"2026-09-20T00:00:00Z","creation_request_id":uuid::Uuid::new_v4().to_string(),"staging_uuid":uuid::Uuid::new_v4().to_string()}),
        );
        put(
            ".aperture/teams/t1/state.json",
            serde_json::json!({"schema_version":1,"state":"active","generation":1,"epic_id":"aperture-fixture","failure":null,"updated_at":"2026-09-20T00:00:00Z"}),
        );
        put(
            ".aperture/run/owner/t1-worker.json",
            serde_json::json!({"schema_version":1,"seat":"t1-worker","generation":1,"state":"active","reservation_nonce_sha256":null,"provisional_token_id":null,"requested":{"harness":"codex","model":"gpt-6-astra","reasoning":"high"},"incarnation":{"pid":900001,"start_time":42,"thread_id":"fixture-thread","token_id":format!("{:x}",Sha256::digest("b".repeat(64).as_bytes())),"harness":"codex","model":"gpt-6-astra","reasoning":"high","observed":true,"processes":[]},"since":"2026-09-20T00:00:00Z","writer":"launcher"}),
        );
        put(
            ".aperture/run/revocations/t1-worker.json",
            serde_json::json!({"schema_version":1,"seat":"t1-worker","revoked_through_generation":1,"revoked_token_ids":[format!("{:x}",Sha256::digest("b".repeat(64).as_bytes()))]}),
        );
        for (path, bytes) in [
            (".claude/aperture/t1-worker/TEAM", b"".as_slice()),
            (".claude/aperture/t1-worker/.complete", b"".as_slice()),
            (
                ".aperture/run/hub-tokens/watchdog.token",
                "a".repeat(64).as_bytes(),
            ),
        ] {
            let p = f.0.join(path);
            ensure_private_dir(p.parent().unwrap()).unwrap();
            write_private_bytes_atomic(&p, bytes, false).unwrap();
        }
        f
    }
    fn prior(&self) -> crate::team_replacement::deadline::PriorReady {
        // Obtain the historical proof through the real native socket exchange,
        // not a test-created boolean. The server/bearers are synthetic only.
        let (address, server) = fixture(ack(), Some(4003));
        let proof = exchange(
            address,
            &Bearer("a".repeat(64)),
            &Bearer("b".repeat(64)),
            "t1-worker",
            1,
            &format!("{:x}", Sha256::digest("b".repeat(64).as_bytes())),
        )
        .unwrap();
        assert_eq!(server.join().unwrap(), 2);
        let mut attempt = crate::team_replacement::deadline::RuntimeAttempt::begin(
            &self.0,
            &crate::team_auth::AuthenticatedActor::launcher(),
            "t1",
            "t1-worker",
            1,
            crate::team_replacement::deadline::Deadline::new(),
        )
        .unwrap();
        attempt.admit_effects().unwrap();
        let ready = attempt.finish_ready(&proof).unwrap();
        drop(ready);
        crate::journal::read_private_json(&self.prepared()).unwrap()
    }
    fn prepared(&self) -> std::path::PathBuf {
        self.0
            .join(".aperture/teams/t1/runtime-attempts/t1-worker/g1/prepared.json")
    }
    fn guard(&self) -> PersistedProcessSnapshot {
        let owner: OwnerRecord =
            crate::journal::read_private_json(&self.0.join(".aperture/run/owner/t1-worker.json"))
                .unwrap();
        let i = owner.incarnation.unwrap();
        let identity = crate::team_process::identity_from_owner(i.pid, i.start_time).unwrap();
        assert_eq!(crate::team_process::state(&identity), ProcessState::Gone);
        crate::team_process::persist_for_stop(
            &self.0,
            "t1",
            &crate::team_auth::AuthenticatedActor::launcher(),
            crate::team_replacement::OwnershipSnapshot {
                seat: "t1-worker".into(),
                generation: 1,
                thread_id: i.thread_id,
                complete: true,
                unowned_matches: vec![],
                processes: vec![crate::team_replacement::OwnedProcess {
                    identity,
                    parent_pid: 1,
                    process_group: 900001,
                    depth: 0,
                    cmdline_sha256: "d".repeat(64),
                    cwd: self.0.to_string_lossy().into_owned(),
                }],
            },
        )
        .unwrap()
    }
}
#[test]
fn ready_reaffirm_uses_native_bound_history_floor_and_one_fresh_ack_without_bearer() {
    let f = ReadyFixture::new();
    let prior = f.prior();
    let before = std::fs::read(f.prepared()).unwrap();
    let guard = f.guard();
    let (address, server) = fixture(ack(), None);
    let proof = reaffirm_stopped_at(
        &f.0,
        &guard,
        &prior,
        address,
        Instant::now() + Duration::from_secs(3),
    )
    .unwrap();
    assert_eq!(server.join().unwrap(), 1);
    assert!(proof.reconnect_is_historical);
    assert_eq!(proof.reconnect_code, 4003);
    assert!(proof.durable && proof.sockets_closed && proof.token_deleted);
    assert!(!f
        .0
        .join(".aperture/run/hub-tokens/t1-worker.token")
        .exists());
    assert_eq!(std::fs::read(f.prepared()).unwrap(), before);
}
#[test]
fn ready_reaffirm_owner_proof_floor_drift_or_token_presence_prevents_connect() {
    use crate::journal::{
        read_private_json, write_private_bytes_atomic, write_private_json_atomic,
    };
    for variant in 0..4 {
        let f = ReadyFixture::new();
        let mut prior = f.prior();
        match variant {
            0 => {
                let p = f.0.join(".aperture/run/owner/t1-worker.json");
                let mut o: OwnerRecord = read_private_json(&p).unwrap();
                o.incarnation.as_mut().unwrap().thread_id = "changed".into();
                write_private_json_atomic(&p, &o, true).unwrap();
            }
            1 => {
                let mut v: serde_json::Value = read_private_json(&f.prepared()).unwrap();
                v["owner_identity_sha256"] = "0".repeat(64).into();
                write_private_json_atomic(&f.prepared(), &v, true).unwrap();
                prior = read_private_json(&f.prepared()).unwrap();
            }
            2 => write_private_bytes_atomic(
                &f.0.join(".aperture/run/hub-tokens/t1-worker.token"),
                b"unexpected",
                false,
            )
            .unwrap(),
            _ => write_private_bytes_atomic(
                &f.0.join(".aperture/run/revocations/t1-worker.json"),
                b"{}",
                true,
            )
            .unwrap(),
        }
        let guard = f.guard();
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        assert!(reaffirm_stopped_at(
            &f.0,
            &guard,
            &prior,
            listener.local_addr().unwrap(),
            Instant::now() + Duration::from_millis(200)
        )
        .is_err());
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
}
#[test]
fn ready_reaffirm_bad_fresh_ack_cannot_be_replaced_by_historical_success() {
    let f = ReadyFixture::new();
    let prior = f.prior();
    let guard = f.guard();
    let mut bad = ack();
    bad["token_directory_synced"] = false.into();
    let (address, server) = fixture(bad, None);
    assert!(reaffirm_stopped_at(
        &f.0,
        &guard,
        &prior,
        address,
        Instant::now() + Duration::from_secs(3)
    )
    .is_err());
    assert_eq!(server.join().unwrap(), 1);
    assert!(!f
        .0
        .join(".aperture/run/hub-tokens/t1-worker.token")
        .exists());
}
