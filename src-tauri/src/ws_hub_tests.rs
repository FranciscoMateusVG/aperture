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
