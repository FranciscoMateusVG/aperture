//! Native checkpoint persistence over the shared owner lock and secure IO.
//! This is an internal adapter, not a public command or an authentication seam.
//! The control caller must supply authenticated context and revalidate its
//! current generation capability under the lock immediately before mutation.
use super::{
    CheckpointContext, CheckpointEntry, CheckpointError, CheckpointPayload, CheckpointStore,
};
use crate::journal::{
    ensure_private_dir, read_private_json, validate_component_path, write_private_json_atomic,
};
use crate::owner::{try_lock, AdvisoryLock, OwnerRecord, OwnerStore};
use crate::state::OwnerState;
use crate::teams::{classify_managed_seat, ManagedSeatState};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

struct NativeStore<'a> {
    revalidate: &'a mut dyn FnMut() -> Result<(), CheckpointError>,
    // Drop seat before team: acquisition order is team, then seat.
    _seat_lock: AdvisoryLock,
    _team_lock: AdvisoryLock,
    dir: PathBuf,
    context: CheckpointContext,
}
impl<'a> NativeStore<'a> {
    fn open(
        home: &Path,
        ctx: &CheckpointContext,
        revalidate: &'a mut dyn FnMut() -> Result<(), CheckpointError>,
    ) -> Result<Self, CheckpointError> {
        if !super::identifier(&ctx.team, 16)
            || !super::identifier(&ctx.seat, 31)
            || ctx.generation == 0
            || ctx.generation != ctx.authenticated_generation
        {
            return Err(CheckpointError::Generation);
        }
        let team_lock = try_lock(&home.join(".aperture/run/team-locks"), &ctx.team)
            .map_err(|_| CheckpointError::Io)?;
        match classify_managed_seat(home, &ctx.seat).map_err(|_| CheckpointError::Corrupt)? {
            Some(ManagedSeatState::Active { team, .. }) if team == ctx.team => {}
            _ => return Err(CheckpointError::Generation),
        }
        let owners = OwnerStore::new(home.join(".aperture/run/owner"));
        let seat_lock = owners.lock(&ctx.seat).map_err(|_| CheckpointError::Io)?;
        // Read under the already-held owner lock; calling read_owner here would
        // recursively acquire that same OS lock. This adapter never writes it.
        let owner: OwnerRecord = read_private_json(&owners.record_path(&ctx.seat))
            .map_err(|_| CheckpointError::Corrupt)?;
        let actual = owner.incarnation.as_ref().ok_or(CheckpointError::Corrupt)?;
        if actual.harness != owner.requested.harness
            || actual.model != owner.requested.model
            || actual.reasoning != owner.requested.reasoning
        {
            return Err(CheckpointError::Corrupt);
        }
        let harness =
            serde_json::to_value(&owner.requested.harness).map_err(|_| CheckpointError::Corrupt)?;
        if owner.schema_version != 1
            || owner.seat != ctx.seat
            || owner.generation != ctx.generation
            || owner.state != OwnerState::Active
            || harness.as_str() != Some(&ctx.harness)
        {
            return Err(CheckpointError::Generation);
        }
        let root = home.join(".aperture/teams");
        let team_dir = validate_component_path(&root, &ctx.team, false)
            .map_err(|_| CheckpointError::Unsafe)?;
        let checkpoints = team_dir.join("checkpoints");
        ensure_private_dir(&checkpoints).map_err(|_| CheckpointError::Unsafe)?;
        let dir = checkpoints.join(&ctx.seat);
        ensure_private_dir(&dir).map_err(|_| CheckpointError::Unsafe)?;
        validate_component_path(
            &root,
            &format!("{}/checkpoints/{}", ctx.team, ctx.seat),
            false,
        )
        .map_err(|_| CheckpointError::Unsafe)?;
        Ok(Self {
            revalidate,
            _seat_lock: seat_lock,
            _team_lock: team_lock,
            dir,
            context: ctx.clone(),
        })
    }
}
impl CheckpointStore for NativeStore<'_> {
    fn entries(
        &mut self,
        team: &str,
        seat: &str,
        generation: u64,
    ) -> Result<Vec<CheckpointEntry>, CheckpointError> {
        if team != self.context.team
            || seat != self.context.seat
            || generation != self.context.generation
        {
            return Err(CheckpointError::Generation);
        }
        (self.revalidate)()?;
        let mut out = Vec::new();
        for entry in std::fs::read_dir(&self.dir).map_err(|_| CheckpointError::Io)? {
            let entry = entry.map_err(|_| CheckpointError::Io)?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| CheckpointError::Corrupt)?;
            // A temp file is never a committed checkpoint. Shared IO publishes
            // only by no-replace rename; crash residue cannot advance sequence.
            if name.starts_with('.') && name.ends_with(".tmp") {
                continue;
            }
            let stem = name.strip_suffix(".json").ok_or(CheckpointError::Corrupt)?;
            let (g, seq) = stem.split_once('-').ok_or(CheckpointError::Corrupt)?;
            let g: u64 = g.parse().map_err(|_| CheckpointError::Corrupt)?;
            let seq: u64 = seq.parse().map_err(|_| CheckpointError::Corrupt)?;
            if g == 0 || seq == 0 || name != format!("{g}-{seq}.json") {
                return Err(CheckpointError::Corrupt);
            }
            if g != generation {
                continue;
            }
            let path = validate_component_path(&self.dir, &name, false)
                .map_err(|_| CheckpointError::Unsafe)?;
            let value: CheckpointEntry =
                read_private_json(&path).map_err(|_| CheckpointError::Corrupt)?;
            if value.seq != seq || value.generation != g {
                return Err(CheckpointError::Corrupt);
            }
            out.push(value);
        }
        Ok(out)
    }
    fn digest(&self, bytes: &[u8]) -> Result<String, CheckpointError> {
        Ok(format!("{:x}", Sha256::digest(bytes)))
    }
    fn append(&mut self, value: &CheckpointEntry) -> Result<(), CheckpointError> {
        if value.team != self.context.team
            || value.seat != self.context.seat
            || value.generation != self.context.generation
            || value.seq == 0
        {
            return Err(CheckpointError::Generation);
        }
        let path = validate_component_path(
            &self.dir,
            &format!("{}-{}.json", value.generation, value.seq),
            true,
        )
        .map_err(|_| CheckpointError::Unsafe)?;
        (self.revalidate)()?;
        write_private_json_atomic(&path, value, false).map_err(|_| CheckpointError::Io)
    }
}

pub(crate) fn write_native<F>(
    home: &Path,
    ctx: &CheckpointContext,
    schema: u32,
    payload: CheckpointPayload,
    now: u64,
    sentinels: &[String],
    mut revalidate: F,
) -> Result<CheckpointEntry, CheckpointError>
where
    F: FnMut() -> Result<(), CheckpointError>,
{
    // Reject unsafe payload before any directory/lock publication.
    super::validate_payload(&payload, sentinels)?;
    revalidate()?;
    let mut store = NativeStore::open(home, ctx, &mut revalidate)?;
    super::write(&mut store, ctx, schema, payload, now, sentinels)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::team_checkpoint::{CheckpointValidation, CheckpointWriter};
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let home = std::env::temp_dir().join(format!(
                "aperture-k310b-checkpoint-{}",
                uuid::Uuid::new_v4()
            ));
            let team = home.join(".aperture/teams/t1");
            ensure_private_dir(&team).unwrap();
            write_private_json_atomic(&team.join("team.json"),&serde_json::json!({"schema_version":1,"team":"t1","project":"project:aperture","mission":"Fixture","acceptance":"Fixture","preset":{"id":null,"sha256":null},"lead":"t1-backend","seats":[{"name":"t1-backend","role":"backend","harness":"codex","model":"gpt-6-astra","reasoning":"high"}],"fallbacks":[],"grants":[],"created_at":"2026-09-20T00:00:00Z","creation_request_id":uuid::Uuid::new_v4().to_string(),"staging_uuid":uuid::Uuid::new_v4().to_string()}),false).unwrap();
            write_private_json_atomic(&team.join("state.json"),&serde_json::json!({"schema_version":1,"state":"active","generation":1,"epic_id":"aperture-fixture","failure":null,"updated_at":"2026-09-20T00:00:00Z"}),false).unwrap();
            let seat = home.join(".claude/aperture/t1-backend");
            ensure_private_dir(&seat).unwrap();
            for name in ["TEAM", ".complete"] {
                crate::journal::write_private_bytes_atomic(&seat.join(name), b"", false).unwrap();
            }
            let owners = OwnerStore::new(home.join(".aperture/run/owner"));
            let actor = crate::team_auth::AuthenticatedActor::launcher();
            owners
                .initialize_owner(
                    &actor,
                    "t1-backend",
                    crate::state::ExecutionTuple {
                        harness: crate::state::Harness::Codex,
                        model: "gpt-6-astra".into(),
                        reasoning: Some(crate::state::ReasoningEffort::High),
                    },
                )
                .unwrap();
            let reservation = owners
                .reserve_start(
                    &actor,
                    "t1-backend",
                    0,
                    crate::state::ExecutionTuple {
                        harness: crate::state::Harness::Codex,
                        model: "gpt-6-astra".into(),
                        reasoning: Some(crate::state::ReasoningEffort::High),
                    },
                )
                .unwrap();
            owners
                .record_start_candidate(
                    &actor,
                    &reservation,
                    crate::owner::Incarnation {
                        pid: 999999,
                        start_time: 1,
                        thread_id: "fixture-thread".into(),
                        token_id: "fixture-token-id".into(),
                        harness: crate::state::Harness::Codex,
                        model: "gpt-6-astra".into(),
                        reasoning: Some(crate::state::ReasoningEffort::High),
                        processes: vec![crate::owner::ProcessIdentity {
                            pid: 999999,
                            start_time: 1,
                            ppid: 1,
                            pgid: 999999,
                            cmdline_sha256: "a".repeat(64),
                            cwd: "/fixture".into(),
                        }],
                    },
                )
                .unwrap();
            owners.commit_start(&actor, &reservation).unwrap();
            Self(home)
        }
        fn dir(&self) -> PathBuf {
            self.0.join(".aperture/teams/t1/checkpoints/t1-backend")
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap()
        }
    }
    fn ctx() -> CheckpointContext {
        CheckpointContext {
            team: "t1".into(),
            seat: "t1-backend".into(),
            generation: 1,
            authenticated_generation: 1,
            harness: "codex".into(),
            writer: CheckpointWriter::Explicit,
        }
    }
    fn payload() -> CheckpointPayload {
        CheckpointPayload {
            task_id: "aperture-fixture".into(),
            worktree: "fixture-worktree".into(),
            branch: "fixture".into(),
            head_sha: "a".repeat(40),
            dirty_files: vec![],
            open_pr: None,
            running_procs: vec![],
            decisions: vec![],
            next_step: "Run bounded fixture.".into(),
            remote_effects: vec![],
        }
    }
    #[test]
    fn shared_locks_private_noreplace_files_and_real_sha_dedupe() {
        let f = Fixture::new();
        let first = write_native(&f.0, &ctx(), 1, payload(), 0, &[], || Ok(())).unwrap();
        assert_eq!(first.content_hash.len(), 64);
        assert_eq!(first.validation, CheckpointValidation::Pending);
        let bytes = std::fs::read(f.dir().join("1-1.json")).unwrap();
        assert_eq!(
            write_native(&f.0, &ctx(), 1, payload(), 5000, &[], || Ok(())).unwrap(),
            first
        );
        assert_eq!(
            write_native(&f.0, &ctx(), 1, payload(), 5001, &[], || Ok(()))
                .unwrap()
                .seq,
            2
        );
        assert_eq!(std::fs::read(f.dir().join("1-1.json")).unwrap(), bytes);
        let meta = std::fs::symlink_metadata(f.dir().join("1-1.json")).unwrap();
        assert_eq!(meta.mode() & 0o777, 0o600);
        assert_eq!(meta.nlink(), 1);
        assert_eq!(
            std::fs::symlink_metadata(f.dir()).unwrap().mode() & 0o777,
            0o700
        );
    }
    #[test]
    fn revalidation_failure_after_lock_publishes_no_checkpoint() {
        let f = Fixture::new();
        let mut checks = 0;
        assert_eq!(
            write_native(&f.0, &ctx(), 1, payload(), 0, &[], || {
                checks += 1;
                if checks == 2 {
                    Err(CheckpointError::Generation)
                } else {
                    Ok(())
                }
            }),
            Err(CheckpointError::Generation)
        );
        assert_eq!(std::fs::read_dir(f.dir()).unwrap().count(), 0);
    }
    #[test]
    fn stale_owner_or_harness_mismatch_cannot_write() {
        let f = Fixture::new();
        let mut c = ctx();
        c.harness = "claude".into();
        assert!(write_native(&f.0, &c, 1, payload(), 0, &[], || Ok(())).is_err());
        let owners = OwnerStore::new(f.0.join(".aperture/run/owner"));
        owners
            .mark_stale(
                &crate::team_auth::AuthenticatedActor::launcher(),
                "t1-backend",
                1,
            )
            .unwrap();
        assert!(write_native(&f.0, &ctx(), 1, payload(), 0, &[], || Ok(())).is_err());
        assert!(!f.dir().exists());
    }
    #[test]
    fn hardlink_or_unsafe_mode_prevents_sequence_advance() {
        for hardlink in [true, false] {
            let f = Fixture::new();
            write_native(&f.0, &ctx(), 1, payload(), 0, &[], || Ok(())).unwrap();
            if hardlink {
                std::fs::hard_link(f.dir().join("1-1.json"), f.0.join("link.json")).unwrap()
            } else {
                std::fs::set_permissions(
                    f.dir().join("1-1.json"),
                    std::fs::Permissions::from_mode(0o644),
                )
                .unwrap()
            }
            assert!(write_native(&f.0, &ctx(), 1, payload(), 6000, &[], || Ok(())).is_err());
            assert!(!f.dir().join("1-2.json").exists());
        }
    }
    #[test]
    fn capability_revoked_immediately_before_append_has_zero_records() {
        let f = Fixture::new();
        let mut checks = 0;
        assert_eq!(
            write_native(&f.0, &ctx(), 1, payload(), 0, &[], || {
                checks += 1;
                if checks == 3 {
                    Err(CheckpointError::Generation)
                } else {
                    Ok(())
                }
            }),
            Err(CheckpointError::Generation)
        );
        assert_eq!(checks, 3);
        assert_eq!(std::fs::read_dir(f.dir()).unwrap().count(), 0);
    }
    #[test]
    fn conflicting_owner_lock_cannot_publish_checkpoint() {
        let f = Fixture::new();
        let owners = OwnerStore::new(f.0.join(".aperture/run/owner"));
        let _lock = owners.lock("t1-backend").unwrap();
        assert_eq!(
            write_native(&f.0, &ctx(), 1, payload(), 0, &[], || Ok(())),
            Err(CheckpointError::Io)
        );
        assert!(!f.dir().exists());
    }
    #[test]
    fn active_owner_without_observed_tuple_fails_closed() {
        let f = Fixture::new();
        let path = f.0.join(".aperture/run/owner/t1-backend.json");
        let mut owner: OwnerRecord = read_private_json(&path).unwrap();
        owner.incarnation = None;
        write_private_json_atomic(&path, &owner, true).unwrap();
        assert_eq!(
            write_native(&f.0, &ctx(), 1, payload(), 0, &[], || Ok(())),
            Err(CheckpointError::Corrupt)
        );
        assert!(!f.dir().exists());
    }
}
