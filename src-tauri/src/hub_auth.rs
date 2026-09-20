use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use crate::agent_loader::is_valid_seat_name;

fn token_dir() -> PathBuf {
    std::env::var("APERTURE_HUB_TOKEN_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/tmp".into()))
                .join(".aperture/run/hub-tokens")
        })
}

fn validate_name(name: &str) -> Result<(), String> {
    if !is_valid_seat_name(name) {
        return Err("invalid hub token principal".into());
    }
    Ok(())
}

fn secure_dir(path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        let meta = fs::symlink_metadata(parent).map_err(|e| e.to_string())?;
        if !meta.is_dir() || meta.file_type().is_symlink() {
            return Err(format!(
                "hub token parent is not a real directory: {}",
                parent.display()
            ));
        }
        if meta.uid() != unsafe { libc::geteuid() } || (meta.mode() & 0o022) != 0 {
            return Err(format!(
                "hub token parent is not private: {}",
                parent.display()
            ));
        }
    }
    if path.exists() {
        let meta = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
        if !meta.is_dir() || meta.file_type().is_symlink() {
            return Err(format!(
                "hub token directory is not a real directory: {}",
                path.display()
            ));
        }
    } else {
        fs::create_dir_all(path).map_err(|e| e.to_string())?;
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
    let meta = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if meta.uid() != unsafe { libc::geteuid() } {
        return Err(format!(
            "hub token directory has wrong owner: {}",
            path.display()
        ));
    }
    Ok(())
}

fn random_token() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .map_err(|e| format!("failed to obtain OS randomness: {e}"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Atomically provision a fresh mode-0600 bearer token for one principal.
/// Only the file path is exported to child processes; the token never enters
/// argv, launcher scripts, logs, or pane scrollback.
fn provision_token_in(dir: &Path, name: &str) -> Result<PathBuf, String> {
    validate_name(name)?;
    secure_dir(dir)?;
    let target = dir.join(format!("{name}.token"));
    let temp = dir.join(format!(".{name}.token.{}.tmp", std::process::id()));
    let token = random_token()?;
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp)
            .map_err(|e| e.to_string())?;
        file.write_all(token.as_bytes())
            .map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        fs::rename(&temp, &target).map_err(|e| e.to_string())?;
        fs::set_permissions(&target, fs::Permissions::from_mode(0o600))
            .map_err(|e| e.to_string())?;
        Ok(target)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

pub fn provision_token(name: &str) -> Result<PathBuf, String> {
    provision_token_in(&token_dir(), name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn private_parent() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "aperture-hub-auth-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        path
    }

    #[test]
    fn provisions_private_fresh_tokens() {
        let parent = private_parent();
        let dir = parent.join("tokens");
        let path = provision_token_in(&dir, "cipher").unwrap();
        let first = fs::read_to_string(&path).unwrap();
        assert_eq!(first.len(), 64);
        assert_eq!(fs::symlink_metadata(&dir).unwrap().mode() & 0o777, 0o700);
        assert_eq!(fs::symlink_metadata(&path).unwrap().mode() & 0o777, 0o600);
        provision_token_in(&dir, "cipher").unwrap();
        assert_ne!(first, fs::read_to_string(&path).unwrap());
        fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn rejects_invalid_principals() {
        let parent = private_parent();
        for invalid in [
            "../cipher",
            "-lead",
            "_lead",
            "Upper",
            "with.dot",
            "a1234567890123456789012345678901",
        ] {
            assert!(provision_token_in(&parent.join("tokens"), invalid).is_err(), "{invalid}");
        }
        assert!(provision_token_in(
            &parent.join("tokens"),
            "a123456789012345678901234567890"
        )
        .is_ok());
        fs::remove_dir_all(parent).unwrap();
    }
}

pub fn token_path(name: &str) -> Result<PathBuf, String> {
    validate_name(name)?;
    Ok(token_dir().join(format!("{name}.token")))
}

/// Managed-only provisioning. Standing `provision_token` above is unchanged.
/// The current start reservation is native authority; no replacement request
/// supplies a token value/path or controls the token directory.
pub(crate) mod managed {
    use crate::journal::{
        ensure_private_dir, open_private_file_nofollow, read_private_json, validate_component_path,
        write_private_bytes_atomic,
    };
    use crate::owner::{try_lock, OwnerRecord, OwnerStore, StartReservation};
    use crate::state::OwnerState;
    use crate::team_auth::AuthenticatedActor;
    use crate::teams::{classify_managed_seat, ManagedSeatState};
    use serde::Deserialize;
    use sha2::{Digest, Sha256};
    use std::io::Read;
    use std::path::{Path, PathBuf};

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(crate) enum TokenError {
        Authority,
        Generation,
        Unsafe,
        Collision,
        Revocation,
        Io,
    }
    /// Only opaque digest and canonical path leave provisioning. No bearer,
    /// Debug/Serialize implementation or environment override is exposed.
    pub(crate) struct ManagedToken {
        path: PathBuf,
        token_id: String,
        generation: u64,
    }
    impl ManagedToken {
        pub(crate) fn path(&self) -> &Path {
            &self.path
        }
        pub(crate) fn token_id(&self) -> &str {
            &self.token_id
        }
        pub(crate) fn generation(&self) -> u64 {
            self.generation
        }
    }
    struct PrivateToken(Vec<u8>);
    impl Drop for PrivateToken {
        fn drop(&mut self) {
            self.0.fill(0);
        }
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Revocations {
        schema_version: u32,
        seat: String,
        revoked_through_generation: u64,
        revoked_token_ids: Vec<String>,
    }
    fn digest(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }
    fn hash_valid(s: &str) -> bool {
        s.len() == 64
            && s.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }
    fn current(home: &Path, reservation: &StartReservation) -> Result<OwnerRecord, TokenError> {
        let record: OwnerRecord = read_private_json(
            &home
                .join(".aperture/run/owner")
                .join(format!("{}.json", reservation.seat)),
        )
        .map_err(|_| TokenError::Generation)?;
        if record.schema_version != 1
            || record.seat != reservation.seat
            || record.generation != reservation.generation
            || record.state != OwnerState::Starting
            || record.incarnation.is_some()
            || record.reservation_nonce_sha256.as_deref()
                != Some(digest(reservation.nonce().as_bytes()).as_str())
        {
            return Err(TokenError::Generation);
        }
        Ok(record)
    }
    fn prior_revocations(
        home: &Path,
        reservation: &StartReservation,
    ) -> Result<Vec<String>, TokenError> {
        let run = home.join(".aperture/run");
        let root =
            validate_component_path(&run, "revocations", true).map_err(|_| TokenError::Unsafe)?;
        let value = match std::fs::symlink_metadata(&root) {
            Ok(m) if m.is_dir() => {
                ensure_private_dir(&root).map_err(|_| TokenError::Unsafe)?;
                let path =
                    validate_component_path(&root, &format!("{}.json", reservation.seat), true)
                        .map_err(|_| TokenError::Unsafe)?;
                match std::fs::symlink_metadata(&path) {
                    Ok(_) => Some(
                        read_private_json::<Revocations>(&path)
                            .map_err(|_| TokenError::Revocation)?,
                    ),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                    Err(_) => return Err(TokenError::Revocation),
                }
            }
            Ok(_) => return Err(TokenError::Unsafe),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => return Err(TokenError::Revocation),
        };
        let Some(value) = value else {
            return if reservation.generation == 1 {
                Ok(vec![])
            } else {
                Err(TokenError::Revocation)
            };
        };
        let mut sorted = value.revoked_token_ids.clone();
        sorted.sort();
        sorted.dedup();
        if value.schema_version != 1
            || value.seat != reservation.seat
            || value.revoked_through_generation != reservation.generation - 1
            || value.revoked_token_ids.len() > 4096
            || sorted != value.revoked_token_ids
            || sorted.iter().any(|s| !hash_valid(s))
        {
            return Err(TokenError::Revocation);
        }
        Ok(value.revoked_token_ids)
    }
    fn entropy() -> Result<PrivateToken, TokenError> {
        let mut random = [0u8; 32];
        if std::fs::File::open("/dev/urandom")
            .and_then(|mut f| f.read_exact(&mut random))
            .is_err()
        {
            random.fill(0);
            return Err(TokenError::Io);
        }
        let mut encoded = Vec::with_capacity(64);
        const HEX: &[u8; 16] = b"0123456789abcdef";
        for byte in &random {
            encoded.push(HEX[(byte >> 4) as usize]);
            encoded.push(HEX[(byte & 15) as usize]);
        }
        random.fill(0);
        Ok(PrivateToken(encoded))
    }
    /// A fresh generation must have a durable Starting reservation and no
    /// candidate yet. Publication is one no-replace write using shared dir-fd
    /// secure IO + file/parent fsync. An existing/partial token is never reused,
    /// overwritten or deleted here. Any failure leaves evidence for recovery.
    /// The shared owner store binds the digest durably before the publication
    /// callback; failure retains that identity for exact native revoke cleanup.
    pub(crate) fn provision(
        home: &Path,
        team: &str,
        actor: &AuthenticatedActor,
        reservation: &StartReservation,
    ) -> Result<ManagedToken, TokenError> {
        provision_checked(home, team, actor, reservation, entropy, |_| Ok(()))
    }
    fn provision_checked<R, F>(
        home: &Path,
        team: &str,
        actor: &AuthenticatedActor,
        reservation: &StartReservation,
        random: R,
        before_publish: F,
    ) -> Result<ManagedToken, TokenError>
    where
        R: FnOnce() -> Result<PrivateToken, TokenError>,
        F: FnOnce(&Path) -> Result<(), TokenError>,
    {
        if !actor.is_launcher() {
            return Err(TokenError::Authority);
        }
        if !crate::agent_loader::is_valid_seat_name(team)
            || team.len() > 16
            || !crate::agent_loader::is_valid_seat_name(&reservation.seat)
            || reservation.generation == 0
        {
            return Err(TokenError::Generation);
        }
        let _team = try_lock(&home.join(".aperture/run/team-locks"), team)
            .map_err(|_| TokenError::Unsafe)?;
        match classify_managed_seat(home, &reservation.seat).map_err(|_| TokenError::Generation)? {
            Some(ManagedSeatState::Active { team: actual, .. }) if actual == team => {}
            _ => return Err(TokenError::Authority),
        }
        let store = OwnerStore::new(home.join(".aperture/run/owner"));
        let before = {
            let _seat = store
                .lock(&reservation.seat)
                .map_err(|_| TokenError::Generation)?;
            current(home, reservation)?
        };
        let revoked = prior_revocations(home, reservation)?;
        let root = home.join(".aperture/run/hub-tokens");
        ensure_private_dir(&root).map_err(|_| TokenError::Unsafe)?;
        let path = validate_component_path(&root, &format!("{}.token", reservation.seat), true)
            .map_err(|_| TokenError::Unsafe)?;
        match std::fs::symlink_metadata(&path) {
            Ok(_) => return Err(TokenError::Collision),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(TokenError::Unsafe),
        }
        let token = random()?;
        if token.0.len() != 64
            || !token
                .0
                .iter()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b))
        {
            return Err(TokenError::Io);
        }
        let token_id = digest(&token.0);
        if revoked.contains(&token_id) {
            return Err(TokenError::Revocation);
        }
        if before.provisional_token_id.is_some() {
            return Err(TokenError::Generation);
        }
        let mut expected = before;
        expected.provisional_token_id = Some(token_id.clone());
        store
            .bind_and_publish_token(actor, reservation, token_id.clone(), || {
                // This callback executes only after durable binding and while the
                // SAME shared owner lock is held. No nested owner writer/lock.
                let publish = || -> Result<(), TokenError> {
                    before_publish(&path)?; // private fixture-only failure/race seam
                    if current(home, reservation)? != expected
                        || prior_revocations(home, reservation)? != revoked
                    {
                        return Err(TokenError::Generation);
                    }
                    write_private_bytes_atomic(&path, &token.0, false)
                        .map_err(|_| TokenError::Io)?;
                    let mut file =
                        open_private_file_nofollow(&path).map_err(|_| TokenError::Unsafe)?;
                    let mut readback = PrivateToken(vec![]);
                    file.by_ref()
                        .take(65)
                        .read_to_end(&mut readback.0)
                        .map_err(|_| TokenError::Io)?;
                    if readback.0.len() != 64 || digest(&readback.0) != token_id {
                        return Err(TokenError::Unsafe);
                    }
                    Ok(())
                };
                publish().map_err(|_| {
                    "E_TOKEN_PUBLICATION: exact managed token publication failed".to_string()
                })
            })
            .map_err(|_| TokenError::Io)?;
        Ok(ManagedToken {
            path,
            token_id,
            generation: reservation.generation,
        })
    }
    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::journal::{write_private_bytes_atomic, write_private_json_atomic};
        use crate::state::{ExecutionTuple, Harness, ReasoningEffort};
        use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
        struct Fixture {
            home: PathBuf,
            reservation: StartReservation,
        }
        impl Fixture {
            fn new() -> Self {
                let home = std::env::temp_dir().join(format!(
                    "aperture-managed-token-fixture-{}",
                    uuid::Uuid::new_v4()
                ));
                let dir = home.join(".aperture/teams/t1");
                ensure_private_dir(&dir).unwrap();
                write_private_json_atomic(&dir.join("team.json"),&serde_json::json!({"schema_version":1,"team":"t1","project":"project:aperture","mission":"fixture","acceptance":"fixture","preset":{"id":null,"sha256":null},"lead":"t1-worker","seats":[{"name":"t1-worker","role":"backend","harness":"codex","model":"gpt-6-astra","reasoning":"high"}],"fallbacks":[],"grants":[],"created_at":"2026-09-20T00:00:00Z","creation_request_id":uuid::Uuid::new_v4().to_string(),"staging_uuid":uuid::Uuid::new_v4().to_string()}),false).unwrap();
                write_private_json_atomic(&dir.join("state.json"),&serde_json::json!({"schema_version":1,"state":"active","generation":1,"epic_id":"aperture-fixture","failure":null,"updated_at":"2026-09-20T00:00:00Z"}),false).unwrap();
                let seat = home.join(".claude/aperture/t1-worker");
                ensure_private_dir(&seat).unwrap();
                for name in ["TEAM", ".complete"] {
                    write_private_bytes_atomic(&seat.join(name), b"", false).unwrap();
                }
                let store = OwnerStore::new(home.join(".aperture/run/owner"));
                let actor = AuthenticatedActor::launcher();
                let tuple = ExecutionTuple {
                    harness: Harness::Codex,
                    model: "gpt-6-astra".into(),
                    reasoning: Some(ReasoningEffort::High),
                };
                store
                    .initialize_owner(&actor, "t1-worker", tuple.clone())
                    .unwrap();
                let reservation = store.reserve_start(&actor, "t1-worker", 0, tuple).unwrap();
                Self { home, reservation }
            }
            fn path(&self) -> PathBuf {
                self.home.join(".aperture/run/hub-tokens/t1-worker.token")
            }
            fn call(&self) -> Result<ManagedToken, TokenError> {
                provision(
                    &self.home,
                    "t1",
                    &AuthenticatedActor::launcher(),
                    &self.reservation,
                )
            }
            fn owner_path(&self) -> PathBuf {
                self.home.join(".aperture/run/owner/t1-worker.json")
            }
            fn revoked(&self, floor: u64, ids: Vec<String>) {
                let dir = self.home.join(".aperture/run/revocations");
                ensure_private_dir(&dir).unwrap();
                write_private_json_atomic(&dir.join("t1-worker.json"),&serde_json::json!({"schema_version":1,"seat":"t1-worker","revoked_through_generation":floor,"revoked_token_ids":ids}),true).unwrap();
            }
        }
        impl Drop for Fixture {
            fn drop(&mut self) {
                std::fs::remove_dir_all(&self.home).unwrap();
            }
        }
        #[test]
        fn fresh_managed_token_is_private_and_cannot_be_reused_or_overwritten() {
            let f = Fixture::new();
            let token = f.call().unwrap();
            assert_eq!(token.path(), f.path());
            assert_eq!(token.generation(), 1);
            assert!(hash_valid(token.token_id()));
            let meta = std::fs::symlink_metadata(token.path()).unwrap();
            assert_eq!(meta.mode() & 0o777, 0o600);
            assert_eq!(meta.nlink(), 1);
            assert_eq!(meta.uid(), unsafe { libc::geteuid() });
            let before = std::fs::read(f.path()).unwrap();
            assert!(matches!(f.call(), Err(TokenError::Collision)));
            assert_eq!(before, std::fs::read(f.path()).unwrap());
        }
        #[test]
        fn actual_two_publishers_have_one_winner() {
            let f = Fixture::new();
            let barrier = std::sync::Barrier::new(2);
            let results = std::thread::scope(|s| {
                let one = s.spawn(|| {
                    barrier.wait();
                    f.call()
                });
                let two = s.spawn(|| {
                    barrier.wait();
                    f.call()
                });
                [one.join().unwrap(), two.join().unwrap()]
            });
            assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
            assert_eq!(std::fs::read(f.path()).unwrap().len(), 64);
        }
        #[test]
        fn destination_appearing_before_publication_is_not_replaced() {
            let f = Fixture::new();
            let result = provision_checked(
                &f.home,
                "t1",
                &AuthenticatedActor::launcher(),
                &f.reservation,
                || Ok(PrivateToken(vec![b'a'; 64])),
                |path| {
                    write_private_bytes_atomic(path, &[b'b'; 64], false).unwrap();
                    Ok(())
                },
            );
            assert!(result.is_err());
            assert_eq!(std::fs::read(f.path()).unwrap(), vec![b'b'; 64]);
        }
        #[test]
        fn unsafe_parent_leaf_hardlink_and_permissions_fail_before_token_use() {
            let f = Fixture::new();
            let outside = f.home.join("outside");
            ensure_private_dir(&outside).unwrap();
            symlink(&outside, f.home.join(".aperture/run/hub-tokens")).unwrap();
            assert!(f.call().is_err());
            assert_eq!(std::fs::read_dir(outside).unwrap().count(), 0);
            let f = Fixture::new();
            ensure_private_dir(f.path().parent().unwrap()).unwrap();
            symlink("missing", f.path()).unwrap();
            assert!(f.call().is_err());
            let f = Fixture::new();
            f.call().unwrap();
            std::fs::hard_link(f.path(), f.home.join("extra-link")).unwrap();
            assert!(f.call().is_err());
            let f = Fixture::new();
            ensure_private_dir(f.path().parent().unwrap()).unwrap();
            std::fs::set_permissions(
                f.path().parent().unwrap(),
                std::fs::Permissions::from_mode(0o755),
            )
            .unwrap();
            assert!(matches!(f.call(), Err(TokenError::Unsafe)));
        }
        #[test]
        fn wrong_actor_generation_or_nonce_cannot_provision() {
            let f = Fixture::new();
            assert!(matches!(
                provision(
                    &f.home,
                    "t1",
                    &AuthenticatedActor::operator_ui(),
                    &f.reservation
                ),
                Err(TokenError::Authority)
            ));
            assert!(!f.path().exists());
            let mut row: OwnerRecord = read_private_json(&f.owner_path()).unwrap();
            row.reservation_nonce_sha256 = Some("b".repeat(64));
            write_private_json_atomic(&f.owner_path(), &row, true).unwrap();
            assert!(matches!(f.call(), Err(TokenError::Generation)));
            assert!(!f.path().exists());
            let f = Fixture::new();
            let mut row: OwnerRecord = read_private_json(&f.owner_path()).unwrap();
            row.generation = 2;
            write_private_json_atomic(&f.owner_path(), &row, true).unwrap();
            assert!(matches!(f.call(), Err(TokenError::Generation)));
        }
        #[test]
        fn generation_two_needs_exact_prior_durable_revocation_and_new_id() {
            let mut f = Fixture::new();
            let mut row: OwnerRecord = read_private_json(&f.owner_path()).unwrap();
            row.generation = 2;
            write_private_json_atomic(&f.owner_path(), &row, true).unwrap();
            f.reservation.generation = 2;
            assert!(matches!(f.call(), Err(TokenError::Revocation)));
            f.revoked(0, vec![]);
            assert!(matches!(f.call(), Err(TokenError::Revocation)));
            let reused = digest(&[b'a'; 64]);
            f.revoked(1, vec![reused.clone()]);
            assert!(matches!(
                provision_checked(
                    &f.home,
                    "t1",
                    &AuthenticatedActor::launcher(),
                    &f.reservation,
                    || Ok(PrivateToken(vec![b'a'; 64])),
                    |_| Ok(())
                ),
                Err(TokenError::Revocation)
            ));
            assert!(!f.path().exists());
            let token = f.call().unwrap();
            assert_eq!(token.generation(), 2);
            assert_ne!(token.token_id(), reused);
        }
        #[test]
        fn corrupt_revocation_and_failure_before_publish_leave_no_usable_token() {
            let f = Fixture::new();
            f.revoked(0, vec!["invalid".into()]);
            assert!(matches!(f.call(), Err(TokenError::Revocation)));
            assert!(!f.path().exists());
            let f = Fixture::new();
            assert!(matches!(
                provision_checked(
                    &f.home,
                    "t1",
                    &AuthenticatedActor::launcher(),
                    &f.reservation,
                    || Err(TokenError::Io),
                    |_| Ok(())
                ),
                Err(TokenError::Io)
            ));
            assert!(!f.path().exists());
            let f = Fixture::new();
            assert!(matches!(
                provision_checked(
                    &f.home,
                    "t1",
                    &AuthenticatedActor::launcher(),
                    &f.reservation,
                    || Ok(PrivateToken(vec![b'a'; 64])),
                    |_| Err(TokenError::Io)
                ),
                Err(TokenError::Io)
            ));
            assert!(!f.path().exists());
        }
        #[test]
        fn digest_is_durable_under_owner_lock_before_publish_and_failure_is_not_retried() {
            let f = Fixture::new();
            let expected = digest(&[b'a'; 64]);
            let reached = std::cell::Cell::new(false);
            let result = provision_checked(&f.home, "t1", &AuthenticatedActor::launcher(), &f.reservation,
                || Ok(PrivateToken(vec![b'a'; 64])), |path| {
                    reached.set(true);
                    let owner: OwnerRecord = read_private_json(&f.owner_path()).unwrap();
                    assert_eq!(owner.provisional_token_id.as_deref(), Some(expected.as_str()));
                    assert!(owner.incarnation.is_none());
                    assert!(!path.exists());
                    let store = OwnerStore::new(f.home.join(".aperture/run/owner"));
                    assert!(store.lock(&f.reservation.seat).is_err());
                    Err(TokenError::Io)
                });
            assert!(result.is_err());
            assert!(reached.get());
            assert!(!f.path().exists());
            let after: OwnerRecord = read_private_json(&f.owner_path()).unwrap();
            assert_eq!(after.provisional_token_id, Some(expected));
            assert!(f.call().is_err());
            assert_eq!(read_private_json::<OwnerRecord>(&f.owner_path()).unwrap(), after);
            assert!(!f.path().exists());
        }
        #[test]
        fn owner_change_after_binding_blocks_publication_without_repairing_state() {
            let f = Fixture::new();
            let result = provision_checked(&f.home, "t1", &AuthenticatedActor::launcher(), &f.reservation,
                || Ok(PrivateToken(vec![b'a'; 64])), |_| {
                    let mut row: OwnerRecord = read_private_json(&f.owner_path()).unwrap();
                    row.generation += 1;
                    write_private_json_atomic(&f.owner_path(), &row, true).unwrap();
                    Ok(())
                });
            assert!(result.is_err());
            assert!(!f.path().exists());
            let row: OwnerRecord = read_private_json(&f.owner_path()).unwrap();
            assert_eq!(row.generation, 2);
            assert_eq!(row.provisional_token_id, Some(digest(&[b'a'; 64])));
        }
        #[test]
        fn post_publication_crash_residue_is_preserved_not_retried() {
            let f = Fixture::new();
            let first = f.call().unwrap();
            drop(first);
            let bytes = std::fs::read(f.path()).unwrap();
            assert!(f.call().is_err());
            assert_eq!(bytes, std::fs::read(f.path()).unwrap());
        }
    }
}
