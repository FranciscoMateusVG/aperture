//! Native client of the existing launcher/watchdog control channel. No second
//! revocation writer. Bearers stay private in memory and never enter diagnostics.
use crate::owner::OwnerRecord;
use crate::team_process::PersistedProcessSnapshot;
use crate::team_replacement::{ProcessState, ReplacementError, RevocationProof};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::time::{Duration, Instant};
use tungstenite::{Message, WebSocket};

const HUB: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 4517);
const FRAME_CAP: usize = 16 * 1024;
const FRAME_COUNT_CAP: usize = 256;
const DEADLINE: Duration = Duration::from_millis(1000);
fn failure() -> ReplacementError {
    ReplacementError::RevocationUnverified
}

// Intentionally no Debug/Serialize. Only fixed canonical files are consumed.
struct Bearer(String);
impl Drop for Bearer {
    fn drop(&mut self) {
        // Best effort zeroing of this owned buffer, not a universal memory claim.
        // SAFETY: replacing every UTF-8 byte with ASCII zero preserves validity.
        unsafe {
            self.0.as_bytes_mut().fill(0);
        }
    }
}
fn read_bearer(home: &Path, seat: &str) -> Result<Bearer, ReplacementError> {
    if !crate::agent_loader::is_valid_seat_name(seat) {
        return Err(failure());
    }
    // Existing fixed runtime tree only: no creation/chmod while reading.
    for (path, forbidden) in [
        (home.to_path_buf(), 0o022),
        (home.join(".aperture"), 0o022),
        (home.join(".aperture/run"), 0o077),
        (home.join(".aperture/run/hub-tokens"), 0o077),
    ] {
        let meta = std::fs::symlink_metadata(path).map_err(|_| failure())?;
        if !meta.is_dir()
            || meta.file_type().is_symlink()
            || meta.uid() != unsafe { libc::geteuid() }
            || meta.mode() & forbidden != 0
        {
            return Err(failure());
        }
    }
    let path = home
        .join(".aperture/run/hub-tokens")
        .join(format!("{seat}.token"));
    let file = crate::journal::open_private_file_nofollow(&path).map_err(|_| failure())?;
    let mut value = String::new();
    file.take(257)
        .read_to_string(&mut value)
        .map_err(|_| failure())?;
    // Native provisioner emits exact lowercase hex. No trimming, fallback,
    // environment override, token creation or arbitrary caller-selected file.
    if value.len() != 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(failure());
    }
    Ok(Bearer(value))
}
fn remaining(deadline: Instant) -> Result<Duration, ReplacementError> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|d| !d.is_zero())
        .ok_or_else(failure)
}
fn would_block(error: &tungstenite::Error) -> bool {
    matches!(error, tungstenite::Error::Io(e) if e.kind()==std::io::ErrorKind::WouldBlock)
}
fn pause(deadline: Instant) -> Result<(), ReplacementError> {
    std::thread::sleep(remaining(deadline)?.min(Duration::from_millis(2)));
    remaining(deadline).map(|_| ())
}
fn connect_at(
    address: SocketAddr,
    deadline: Instant,
) -> Result<WebSocket<TcpStream>, ReplacementError> {
    if !address.ip().is_loopback() {
        return Err(failure());
    }
    let stream =
        TcpStream::connect_timeout(&address, remaining(deadline)?).map_err(|_| failure())?;
    stream.set_nonblocking(true).map_err(|_| failure())?;
    let config = tungstenite::protocol::WebSocketConfig {
        max_message_size: Some(FRAME_CAP),
        max_frame_size: Some(FRAME_CAP),
        ..Default::default()
    };
    let mut result =
        tungstenite::client::client_with_config(format!("ws://{address}"), stream, Some(config));
    loop {
        remaining(deadline)?;
        match result {
            Ok((ws, _)) => return Ok(ws),
            Err(tungstenite::HandshakeError::Interrupted(handshake)) => {
                pause(deadline)?;
                result = handshake.handshake();
            }
            Err(_) => return Err(failure()),
        }
    }
}
fn send(
    ws: &mut WebSocket<TcpStream>,
    value: serde_json::Value,
    deadline: Instant,
) -> Result<(), ReplacementError> {
    let text = serde_json::to_string(&value).map_err(|_| failure())?;
    if text.len() > FRAME_CAP {
        return Err(failure());
    }
    remaining(deadline)?;
    // write queues this frame once. WouldBlock retains its queued remainder;
    // only flush is retried, never the revoke/hello frame itself.
    match ws.write(Message::Text(text)) {
        Ok(()) => {}
        Err(e) if would_block(&e) => {}
        Err(_) => return Err(failure()),
    }
    loop {
        remaining(deadline)?;
        match ws.flush() {
            Ok(()) => return Ok(()),
            Err(e) if would_block(&e) => pause(deadline)?,
            Err(_) => return Err(failure()),
        }
    }
}
fn read_before(
    ws: &mut WebSocket<TcpStream>,
    deadline: Instant,
) -> Result<Message, ReplacementError> {
    loop {
        remaining(deadline)?;
        match ws.read() {
            Ok(message) => return Ok(message),
            Err(e) if would_block(&e) => pause(deadline)?,
            Err(_) => return Err(failure()),
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Ack {
    #[serde(rename = "type")]
    kind: String,
    control: String,
    seat: String,
    generation: u64,
    token_deleted: bool,
    token_absent_verified: bool,
    token_directory_synced: bool,
    sockets_close_requested: u64,
    sockets_closed_verified: u64,
}
fn validate_ack(
    value: serde_json::Value,
    seat: &str,
    generation: u64,
) -> Result<Ack, ReplacementError> {
    let ack: Ack = serde_json::from_value(value).map_err(|_| failure())?;
    if ack.kind != "ok"
        || ack.control != "revoke_generation"
        || ack.seat != seat
        || ack.generation != generation
        || !ack.token_absent_verified
        || !ack.token_directory_synced
        || ack.sockets_close_requested != ack.sockets_closed_verified
        || ack.sockets_close_requested > 65536
    {
        return Err(failure());
    }
    Ok(ack)
}
fn exchange(
    address: SocketAddr,
    watchdog: &Bearer,
    old: &Bearer,
    seat: &str,
    generation: u64,
    token_id: &str,
) -> Result<RevocationProof, ReplacementError> {
    exchange_optional(address, watchdog, Some(old), seat, generation, token_id)
}
fn exchange_optional(
    address: SocketAddr,
    watchdog: &Bearer,
    old: Option<&Bearer>,
    seat: &str,
    generation: u64,
    token_id: &str,
) -> Result<RevocationProof, ReplacementError> {
    exchange_optional_until(
        address,
        watchdog,
        old,
        seat,
        generation,
        token_id,
        Instant::now() + Duration::from_secs(3),
    )
}
fn exchange_optional_until(
    address: SocketAddr,
    watchdog: &Bearer,
    old: Option<&Bearer>,
    seat: &str,
    generation: u64,
    token_id: &str,
    until: Instant,
) -> Result<RevocationProof, ReplacementError> {
    // One total bound across both handshakes/reads/writes. Per-read socket
    // timeouts alone could be renewed forever by a fragmented handshake/frame.
    let total_deadline = until.min(Instant::now() + Duration::from_secs(3));
    let mut control = connect_at(address, total_deadline)?;
    send(
        &mut control,
        serde_json::json!({"type":"hello","role":"subscriber","agent":"watchdog","token":watchdog.0}),
        total_deadline,
    )?;
    let started = Instant::now();
    let deadline = (started + DEADLINE).min(total_deadline);
    // Exactly one mutation request; uncertain outcome never causes a retry.
    send(
        &mut control,
        serde_json::json!({"type":"revoke_generation","seat":seat,"generation":generation,"token_id":token_id}),
        deadline,
    )?;
    let mut proof = None;
    for _ in 0..FRAME_COUNT_CAP {
        match read_before(&mut control, deadline)? {
            Message::Text(text) => {
                let value: serde_json::Value =
                    serde_json::from_str(&text).map_err(|_| failure())?;
                match value.get("type").and_then(|v| v.as_str()) {
                    Some("presence") => continue,
                    Some("ok") => {
                        proof = Some(validate_ack(value, seat, generation)?);
                        break;
                    }
                    _ => return Err(failure()),
                }
            }
            Message::Ping(_) | Message::Pong(_) => continue,
            _ => return Err(failure()),
        }
    }
    let ack = proof.ok_or_else(failure)?;
    let elapsed = u64::try_from(started.elapsed().as_millis()).map_err(|_| failure())?;
    if elapsed > 1000 {
        return Err(failure());
    }
    let _ = control.close(None);
    // One negative reconnect with the original in-memory bearer. Never rewrite
    // the deleted file and never substitute a newer generation/token.
    if let Some(old) = old {
        let mut denied = connect_at(address, total_deadline)?;
        send(
            &mut denied,
            serde_json::json!({"type":"hello","role":"producer","agent":seat,
        "generation":generation,"token_id":token_id,"token":old.0}),
            total_deadline,
        )?;
        let reconnect_deadline = (Instant::now() + DEADLINE).min(total_deadline);
        let mut rejected = false;
        for _ in 0..FRAME_COUNT_CAP {
            match read_before(&mut denied, reconnect_deadline)? {
                Message::Close(Some(frame)) if u16::from(frame.code) == 4003 => {
                    rejected = true;
                    break;
                }
                Message::Ping(_) | Message::Pong(_) => continue,
                _ => return Err(failure()),
            }
        }
        if !rejected {
            return Err(failure());
        }
    }
    // token_deleted is per-call mutation history; confirmed absence+fsync is
    // the safety fact. An idempotent already-absent ACK is valid evidence.
    let _deleted_this_call = ack.token_deleted;
    Ok(RevocationProof {
        generation,
        durable: true,
        sockets_closed: true,
        close_code: 4001,
        close_elapsed_ms: elapsed,
        reconnect_code: if old.is_some() { 4003 } else { 0 },
        reconnect_is_historical: false,
        token_deleted: ack.token_absent_verified,
    })
}

/// Called after exact stop verification while the durable snapshot guard holds
/// team+owner locks. It cannot revoke a caller-supplied free-form identity.
pub(crate) fn revoke_stopped(
    home: &Path,
    recorded: &PersistedProcessSnapshot,
) -> Result<RevocationProof, ReplacementError> {
    revoke_stopped_before(home, recorded, Instant::now() + Duration::from_secs(3))
}
pub(crate) fn revoke_stopped_before(
    home: &Path,
    recorded: &PersistedProcessSnapshot,
    until: Instant,
) -> Result<RevocationProof, ReplacementError> {
    remaining(until)?;
    let snapshot = recorded.snapshot();
    if snapshot
        .processes
        .iter()
        .any(|p| crate::team_process::state(&p.identity) != ProcessState::Gone)
    {
        return Err(failure());
    }
    let path = home
        .join(".aperture/run/owner")
        .join(format!("{}.json", snapshot.seat));
    // Do not reacquire the lock held by recorded.
    let owner: OwnerRecord = crate::journal::read_private_json(&path).map_err(|_| failure())?;
    let incarnation = owner.incarnation.as_ref().ok_or_else(failure)?;
    if owner.seat != snapshot.seat
        || owner.generation != snapshot.generation
        || incarnation.thread_id != snapshot.thread_id
        || !matches!(
            owner.state,
            crate::state::OwnerState::Active | crate::state::OwnerState::Starting
        )
    {
        return Err(failure());
    }
    let watchdog = read_bearer(home, "watchdog")?;
    let old = read_bearer(home, &snapshot.seat)?;
    let digest = format!("{:x}", Sha256::digest(old.0.as_bytes()));
    if digest != incarnation.token_id {
        return Err(failure());
    }
    exchange_optional_until(
        HUB,
        &watchdog,
        Some(&old),
        &snapshot.seat,
        snapshot.generation,
        &digest,
        until,
    )
}

/// Fresh no-token ACK plus exact historical Ready negative-reconnect evidence.
/// This does NOT claim to repeat a 4003 request with an erased bearer.
pub(crate) fn reaffirm_stopped(
    home: &Path,
    recorded: &PersistedProcessSnapshot,
    prior: &crate::team_replacement::deadline::PriorReady,
    until: Instant,
) -> Result<RevocationProof, ReplacementError> {
    reaffirm_stopped_at(home, recorded, prior, HUB, until)
}
fn reaffirm_stopped_at(
    home: &Path,
    recorded: &PersistedProcessSnapshot,
    prior: &crate::team_replacement::deadline::PriorReady,
    address: SocketAddr,
    until: Instant,
) -> Result<RevocationProof, ReplacementError> {
    remaining(until)?;
    let snapshot = recorded.snapshot();
    if snapshot
        .processes
        .iter()
        .any(|p| crate::team_process::state(&p.identity) != ProcessState::Gone)
        || !snapshot.complete
        || !snapshot.unowned_matches.is_empty()
    {
        return Err(failure());
    }
    let owner: OwnerRecord = crate::journal::read_private_json(
        &home
            .join(".aperture/run/owner")
            .join(format!("{}.json", snapshot.seat)),
    )
    .map_err(|_| failure())?;
    prior.verify_owner(&owner)?;
    let i = owner.incarnation.as_ref().ok_or_else(failure)?;
    if owner.seat != snapshot.seat
        || owner.generation != snapshot.generation
        || i.thread_id != snapshot.thread_id
    {
        return Err(failure());
    }
    let token = home
        .join(".aperture/run/hub-tokens")
        .join(format!("{}.token", snapshot.seat));
    if !matches!(std::fs::symlink_metadata(&token),Err(e) if e.kind()==std::io::ErrorKind::NotFound)
    {
        return Err(failure());
    }
    verify_floor(home, &snapshot.seat, snapshot.generation, &i.token_id)?;
    let watchdog = read_bearer(home, "watchdog")?;
    let mut proof = exchange_optional_until(
        address,
        &watchdog,
        None,
        &snapshot.seat,
        snapshot.generation,
        &i.token_id,
        until,
    )?;
    verify_floor(home, &snapshot.seat, snapshot.generation, &i.token_id)?;
    // Historical 4003 belongs to this exact immutable owner identity. Current
    // durable floor and exact socket-close/absent-token ACK were freshly checked.
    proof.reconnect_code = 4003;
    proof.reconnect_is_historical = true;
    Ok(proof)
}

#[cfg(test)]
#[path = "team_revoke_native_tests.rs"]
mod tests;

/// Failed publication before any candidate was attached. No synthetic PID,
/// thread or bearer is minted to obtain an ACK. If publication never produced
/// a readable bearer, durable floor+factual ACK is checked but negative reconnect
/// is deliberately NOT claimed. The same owner lock binds this unlaunched case.
pub(crate) fn revoke_unlaunched(
    home: &Path,
    team: &str,
    res: &crate::owner::StartReservation,
) -> Result<(), ReplacementError> {
    revoke_unlaunched_before(home, team, res, Instant::now() + Duration::from_secs(3))
}
pub(crate) fn revoke_unlaunched_before(
    home: &Path,
    team: &str,
    res: &crate::owner::StartReservation,
    until: Instant,
) -> Result<(), ReplacementError> {
    remaining(until)?;
    let _team = crate::owner::try_lock(&home.join(".aperture/run/team-locks"), team)
        .map_err(|_| failure())?;
    match crate::teams::classify_managed_seat(home, &res.seat).map_err(|_| failure())? {
        Some(crate::teams::ManagedSeatState::Active { team: actual, .. }) if actual == team => {}
        _ => return Err(failure()),
    }
    let store = crate::owner::OwnerStore::new(home.join(".aperture/run/owner"));
    let _seat = store.lock(&res.seat).map_err(|_| failure())?;
    let owner: OwnerRecord =
        crate::journal::read_private_json(&store.record_path(&res.seat)).map_err(|_| failure())?;
    if owner.schema_version != 1
        || owner.seat != res.seat
        || owner.generation != res.generation
        || owner.state != crate::state::OwnerState::Starting
        || owner.incarnation.is_some()
        || owner.reservation_nonce_sha256.as_deref()
            != Some(format!("{:x}", Sha256::digest(res.nonce().as_bytes())).as_str())
    {
        return Err(failure());
    }
    let Some(token_id) = owner.provisional_token_id.as_ref() else {
        return Ok(());
    };
    if token_id.len() != 64
        || !token_id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(failure());
    }
    let path = home
        .join(".aperture/run/hub-tokens")
        .join(format!("{}.token", res.seat));
    let old = match std::fs::symlink_metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Ok(_) => Some(read_bearer(home, &res.seat)?),
        _ => return Err(failure()),
    };
    if old
        .as_ref()
        .is_some_and(|b| format!("{:x}", Sha256::digest(b.0.as_bytes())) != *token_id)
    {
        return Err(failure());
    }
    let watchdog = read_bearer(home, "watchdog")?;
    exchange_optional_until(
        HUB,
        &watchdog,
        old.as_ref(),
        &res.seat,
        res.generation,
        token_id,
        until,
    )?;
    if !matches!(std::fs::symlink_metadata(&path),Err(e) if e.kind()==std::io::ErrorKind::NotFound)
    {
        return Err(failure());
    }
    verify_floor(home, &res.seat, res.generation, token_id)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NativeRevocationFloor {
    schema_version: u32,
    seat: String,
    revoked_through_generation: u64,
    revoked_token_ids: Vec<String>,
}
pub(crate) fn verify_floor(
    home: &Path,
    seat: &str,
    generation: u64,
    token_id: &str,
) -> Result<(), ReplacementError> {
    let state: NativeRevocationFloor = crate::journal::read_private_json(
        &home
            .join(".aperture/run/revocations")
            .join(format!("{seat}.json")),
    )
    .map_err(|_| failure())?;
    let mut sorted = state.revoked_token_ids.clone();
    sorted.sort();
    sorted.dedup();
    let valid = |s: &str| {
        s.len() == 64
            && s.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    };
    if state.schema_version != 1
        || state.seat != seat
        || state.revoked_through_generation != generation
        || state.revoked_token_ids.is_empty()
        || state.revoked_token_ids.len() > 10000
        || sorted != state.revoked_token_ids
        || !state.revoked_token_ids.iter().all(|s| valid(s))
        || !valid(token_id)
        || !state.revoked_token_ids.iter().any(|s| s == token_id)
    {
        return Err(failure());
    }
    Ok(())
}
