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
