//! Authenticated archive finalizer. Evidence is native-derived before this
//! seam; only the durable journal may resume an interrupted mutation.
use crate::journal::{
    apply_or_recover_journal, ensure_private_dir, read_journal, read_private_json, remove_journal,
    sync_dir, write_journal, write_private_bytes_atomic, write_private_json_atomic,
    ArchiveJournalApproval, Journal, JournalMove, JournalObjectKind, JournalOperation, JournalRoot,
    JournalRoots,
};
use crate::owner::{try_lock, OwnerStore};
use crate::state::OwnerState;
use crate::team_auth::AuthenticatedActor;
use crate::teams::{TeamLifecycle, TeamSnapshot, TeamStateFile};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

pub(crate) struct FreshArchive<'a> {
    pub snapshot: &'a TeamSnapshot,
    pub state: &'a TeamStateFile,
    pub approval: &'a ArchiveJournalApproval,
}

fn owner_hash(value: &crate::owner::OwnerRecord) -> Result<String, String> {
    let bytes = serde_json::to_vec(value)
        .map_err(|_| "E_ARCHIVE_OWNER_INVALID: owner hash unavailable".to_string())?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn journal_path(home: &Path, team: &str) -> PathBuf {
    home.join(".aperture/run/team-journals")
        .join(format!("{team}.archive.json"))
}

fn preimage(approval: &ArchiveJournalApproval) -> Result<String, String> {
    let bytes = serde_json::to_vec(approval)
        .map_err(|_| "E_ARCHIVE_APPROVAL_INVALID: approval encoding failed".to_string())?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn read_snapshot(home: &Path, team: &str) -> Result<(TeamSnapshot, TeamStateFile), String> {
    let active = home.join(".aperture/teams").join(team);
    let archived = home.join(".aperture/teams/archive").join(team);
    let root = if active.exists() { active } else { archived };
    Ok((
        read_private_json(&root.join("team.json"))?,
        read_private_json(&root.join("state.json"))?,
    ))
}

pub(crate) fn has_journal(home: &Path, team: &str) -> bool {
    fs::symlink_metadata(journal_path(home, team)).is_ok()
}

pub(crate) fn recover_approval(
    home: &Path,
    team: &str,
    generation: u64,
) -> Result<ArchiveJournalApproval, String> {
    let journal = read_journal(&journal_path(home, team))?;
    let approval = journal
        .archive_approval
        .ok_or_else(|| "E_JOURNAL_INCONSISTENT: archive approval missing".to_string())?;
    if journal.operation != JournalOperation::Archive
        || journal.team != team
        || approval.generation != generation
        || journal.preimage_sha256 != preimage(&approval)?
    {
        return Err("E_JOURNAL_INCONSISTENT: archive recovery binding mismatch".into());
    }
    Ok(approval)
}

pub(crate) fn finalize(
    home: &Path,
    actor: &AuthenticatedActor,
    team: &str,
    generation: u64,
    fresh: Option<FreshArchive<'_>>,
) -> Result<(), String> {
    if !crate::agent_loader::is_valid_seat_name(team) || team.len() > 16 || generation == 0 {
        return Err("E_ARCHIVE_BINDING: invalid archive selector".into());
    }
    // This helper is crate-visible for composition, so it enforces the same
    // GLaDOS capability boundary as the headless command before creating even
    // a lock or journal directory. It is revalidated again under the locks.
    actor.revalidate_before_mutation()?;
    let teams = home.join(".aperture/teams");
    let agents = home.join(".claude/aperture");
    let owners = OwnerStore::new(home.join(".aperture/run/owner"));
    let team_locks = home.join(".aperture/run/team-locks");
    let journals = home.join(".aperture/run/team-journals");
    ensure_private_dir(&journals)?;
    let path = journal_path(home, team);
    let recovering = fs::symlink_metadata(&path).is_ok();
    let approval = if recovering {
        recover_approval(home, team, generation)?
    } else {
        fresh
            .as_ref()
            .ok_or_else(|| "E_ARCHIVE_APPROVAL_INVALID: fresh evidence required".to_string())?
            .approval
            .clone()
    };
    let (snapshot, state) = read_snapshot(home, team)?;
    if snapshot.team != team
        || state.generation != generation
        || approval.generation != generation
        || state.epic_id.as_deref() != Some(&approval.epic_id)
        || !matches!(state.state, TeamLifecycle::Active | TeamLifecycle::Archived)
    {
        return Err("E_GENERATION_MISMATCH: archive snapshot changed".into());
    }
    if !recovering {
        let evidence = fresh.as_ref().unwrap();
        if evidence.snapshot != &snapshot
            || evidence.state != &state
            || evidence.approval != &approval
        {
            return Err("E_ARCHIVE_APPROVAL_STALE: archive evidence changed".into());
        }
    }
    let mut seats: Vec<_> = snapshot
        .seats
        .iter()
        .map(|seat| seat.name.clone())
        .collect();
    seats.sort();
    if seats.is_empty() || seats.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err("E_ARCHIVE_BINDING: invalid archive seats".into());
    }
    let _team_lock = try_lock(&team_locks, team)?;
    let mut seat_locks = Vec::with_capacity(seats.len());
    for seat in &seats {
        seat_locks.push(owners.lock(seat)?);
    }
    actor.revalidate_before_mutation()?;
    let (locked_snapshot, locked_state) = read_snapshot(home, team)?;
    if locked_snapshot != snapshot || locked_state != state {
        return Err("E_ARCHIVE_APPROVAL_STALE: team changed before archive mutation".into());
    }
    let approved: std::collections::BTreeMap<_, _> =
        approval.owner_sha256.iter().cloned().collect();
    let approved_states: std::collections::BTreeMap<_, _> =
        approval.owner_states.iter().cloned().collect();
    if approved.len() != seats.len() || approved_states.len() != seats.len() {
        return Err("E_ARCHIVE_APPROVAL_INVALID: owner coverage mismatch".into());
    }
    for seat in &seats {
        let owner = owners.read_owner_locked(seat)?;
        match (approved_states.get(seat).map(String::as_str), &owner.state) {
            (Some("active"), OwnerState::Active) | (Some("stale"), OwnerState::Stale) => {
                if approved.get(seat) != Some(&owner_hash(&owner)?) {
                    return Err("E_ARCHIVE_APPROVAL_STALE: owner changed".into());
                }
            }
            // A durable archive journal is written before this exact transition.
            // Recovery may therefore observe the owner already fenced stale.
            (Some("active"), OwnerState::Stale) if recovering => {}
            _ => return Err("E_ARCHIVE_OWNER_INVALID: owner state changed".into()),
        }
    }
    if !recovering {
        ensure_private_dir(&teams.join("archive"))?;
        ensure_private_dir(&agents.join("_archived"))?;
        ensure_private_dir(&agents.join("_archived").join(team))?;
        let mut moves = vec![JournalMove {
            from_root: JournalRoot::Teams,
            from_rel: team.into(),
            to_root: JournalRoot::Teams,
            to_rel: format!("archive/{team}"),
            kind: JournalObjectKind::Directory,
        }];
        moves.extend(seats.iter().map(|seat| JournalMove {
            from_root: JournalRoot::Agents,
            from_rel: seat.clone(),
            to_root: JournalRoot::Agents,
            to_rel: format!("_archived/{team}/{seat}"),
            kind: JournalObjectKind::Directory,
        }));
        write_journal(
            &path,
            &Journal {
                schema_version: 1,
                operation: JournalOperation::Archive,
                team: team.into(),
                uuid: Uuid::new_v4().to_string(),
                moves,
                step: 0,
                preimage_sha256: preimage(&approval)?,
                archive_approval: Some(approval.clone()),
            },
        )?;
    }
    for seat in &seats {
        let owner = owners.read_owner_locked(seat)?;
        if owner.state == OwnerState::Active {
            owners.mark_stale_locked(actor, seat, owner.generation)?;
        }
    }
    let roots = JournalRoots {
        teams: teams.clone(),
        staging: teams.join(".staging"),
        agents: agents.clone(),
        owner: owners.root.clone(),
    };
    apply_or_recover_journal(&path, &roots)?;
    let archived = teams.join("archive").join(team);
    let mut archived_state: TeamStateFile = read_private_json(&archived.join("state.json"))?;
    archived_state.state = TeamLifecycle::Archived;
    archived_state.failure = None;
    archived_state.updated_at = chrono::Utc::now().to_rfc3339();
    write_private_json_atomic(&archived.join("state.json"), &archived_state, true)?;
    for seat in &seats {
        write_private_bytes_atomic(
            &agents
                .join("_archived")
                .join(team)
                .join(seat)
                .join(".complete"),
            b"complete\n",
            true,
        )?;
    }
    sync_dir(&archived)?;
    sync_dir(&agents.join("_archived").join(team))?;
    remove_journal(&path)?;
    drop(seat_locks);
    if teams.join(team).exists() || !archived.is_dir() || path.exists() {
        return Err("E_JOURNAL_INCONSISTENT: canonical archive readback failed".into());
    }
    let final_state: TeamStateFile = read_private_json(&archived.join("state.json"))?;
    if final_state.state != TeamLifecycle::Archived || final_state.generation != generation {
        return Err("E_JOURNAL_INCONSISTENT: archived state readback failed".into());
    }
    for seat in &seats {
        if agents.join(seat).exists()
            || !agents
                .join("_archived")
                .join(team)
                .join(seat)
                .join(".complete")
                .is_file()
        {
            return Err("E_JOURNAL_INCONSISTENT: archived seat readback failed".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::{ensure_private_dir, write_private_json_atomic};
    use crate::state::{ExecutionTuple, Harness, ReasoningEffort};
    use std::os::unix::fs::PermissionsExt;

    fn setup() -> (
        PathBuf,
        TeamSnapshot,
        TeamStateFile,
        crate::owner::OwnerRecord,
    ) {
        let home =
            std::env::temp_dir().join(format!("aperture-archive-finalize-{}", Uuid::new_v4()));
        ensure_private_dir(&home).unwrap();
        for dir in [
            ".aperture/teams/t1",
            ".aperture/run/owner",
            ".aperture/run/team-locks",
            ".aperture/run/hub-tokens",
            ".claude/aperture/glados",
            ".claude/aperture/t1-worker",
        ] {
            ensure_private_dir(&home.join(dir)).unwrap();
        }
        fs::write(home.join(".claude/aperture/glados/prompt.md"), "test").unwrap();
        fs::write(home.join(".claude/aperture/glados/manifest.json"), r#"{"name":"GLaDOS","model":"sonnet","window":"glados","role":"orchestrator","enabled":true}"#).unwrap();
        fs::write(
            home.join(".aperture/run/hub-tokens/glados.token"),
            b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        )
        .unwrap();
        fs::set_permissions(
            home.join(".aperture/run/hub-tokens/glados.token"),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        fs::write(
            home.join(".claude/aperture/t1-worker/.complete"),
            b"complete\n",
        )
        .unwrap();
        fs::set_permissions(
            home.join(".claude/aperture/t1-worker/.complete"),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        let snapshot: TeamSnapshot = serde_json::from_value(serde_json::json!({
            "schema_version":1,"team":"t1","project":"project:aperture","repo":"aperture","mission":"fixture","acceptance":"fixture","preset":{"id":null,"sha256":null},
            "lead":"t1-worker","seats":[{"name":"t1-worker","role":"backend","harness":"codex","model":"gpt-6-astra","reasoning":"high"}],"fallbacks":[],"grants":[],
            "created_at":"2026-09-20T00:00:00Z","creation_request_id":Uuid::new_v4().to_string(),"staging_uuid":Uuid::new_v4().to_string()
        })).unwrap();
        let state = TeamStateFile {
            schema_version: 1,
            state: TeamLifecycle::Active,
            generation: 1,
            epic_id: Some("aperture-epic".into()),
            failure: None,
            updated_at: "2026-09-20T00:00:00Z".into(),
        };
        let owner = OwnerStore::initial_record(
            &crate::team_auth::AuthenticatedActor::launcher(),
            "t1-worker",
            ExecutionTuple {
                harness: Harness::Codex,
                model: "gpt-6-astra".into(),
                reasoning: Some(ReasoningEffort::High),
            },
        )
        .unwrap();
        write_private_json_atomic(&home.join(".aperture/teams/t1/team.json"), &snapshot, false)
            .unwrap();
        write_private_json_atomic(&home.join(".aperture/teams/t1/state.json"), &state, false)
            .unwrap();
        write_private_json_atomic(
            &home.join(".aperture/run/owner/t1-worker.json"),
            &owner,
            false,
        )
        .unwrap();
        (home, snapshot, state, owner)
    }

    #[test]
    fn authenticated_archive_moves_once_and_reads_back_canonical_state() {
        let _guard = crate::team_auth::tests::ENV_LOCK.lock().unwrap();
        let (home, snapshot, state, owner) = setup();
        let old_home = std::env::var_os("HOME");
        let old_agents = std::env::var_os("APERTURE_AGENTS_DIR");
        std::env::set_var("HOME", &home);
        std::env::set_var("APERTURE_AGENTS_DIR", home.join(".claude/aperture"));
        let actor = crate::team_auth::authenticate_glados_control().unwrap();
        let approval = ArchiveJournalApproval {
            generation: 1,
            epic_id: "aperture-epic".into(),
            record_sha256: "a".repeat(64),
            inventory_sha256: "b".repeat(64),
            native_sha256: "c".repeat(64),
            owner_sha256: vec![("t1-worker".into(), owner_hash(&owner).unwrap())],
            owner_states: vec![("t1-worker".into(), "stale".into())],
            approved_by: "glados".into(),
        };
        finalize(
            &home,
            &actor,
            "t1",
            1,
            Some(FreshArchive {
                snapshot: &snapshot,
                state: &state,
                approval: &approval,
            }),
        )
        .unwrap();
        assert!(!home.join(".aperture/teams/t1").exists());
        assert!(home.join(".aperture/teams/archive/t1").is_dir());
        assert!(home
            .join(".claude/aperture/_archived/t1/t1-worker/.complete")
            .is_file());
        assert!(!has_journal(&home, "t1"));
        match old_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        match old_agents {
            Some(v) => std::env::set_var("APERTURE_AGENTS_DIR", v),
            None => std::env::remove_var("APERTURE_AGENTS_DIR"),
        }
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn journal_recovery_rolls_forward_a_partial_archive_without_fresh_evidence() {
        let _guard = crate::team_auth::tests::ENV_LOCK.lock().unwrap();
        let (home, _snapshot, _state, owner) = setup();
        let old_home = std::env::var_os("HOME");
        let old_agents = std::env::var_os("APERTURE_AGENTS_DIR");
        std::env::set_var("HOME", &home);
        std::env::set_var("APERTURE_AGENTS_DIR", home.join(".claude/aperture"));
        let actor = crate::team_auth::authenticate_glados_control().unwrap();
        let approval = ArchiveJournalApproval {
            generation: 1,
            epic_id: "aperture-epic".into(),
            record_sha256: "a".repeat(64),
            inventory_sha256: "b".repeat(64),
            native_sha256: "c".repeat(64),
            owner_sha256: vec![("t1-worker".into(), owner_hash(&owner).unwrap())],
            owner_states: vec![("t1-worker".into(), "stale".into())],
            approved_by: "glados".into(),
        };
        let teams = home.join(".aperture/teams");
        let agents = home.join(".claude/aperture");
        ensure_private_dir(&teams.join("archive")).unwrap();
        ensure_private_dir(&agents.join("_archived/t1")).unwrap();
        ensure_private_dir(&home.join(".aperture/run/team-journals")).unwrap();
        let path = journal_path(&home, "t1");
        write_journal(
            &path,
            &Journal {
                schema_version: 1,
                operation: JournalOperation::Archive,
                team: "t1".into(),
                uuid: Uuid::new_v4().to_string(),
                step: 0,
                preimage_sha256: preimage(&approval).unwrap(),
                archive_approval: Some(approval),
                moves: vec![
                    JournalMove {
                        from_root: JournalRoot::Teams,
                        from_rel: "t1".into(),
                        to_root: JournalRoot::Teams,
                        to_rel: "archive/t1".into(),
                        kind: JournalObjectKind::Directory,
                    },
                    JournalMove {
                        from_root: JournalRoot::Agents,
                        from_rel: "t1-worker".into(),
                        to_root: JournalRoot::Agents,
                        to_rel: "_archived/t1/t1-worker".into(),
                        kind: JournalObjectKind::Directory,
                    },
                ],
            },
        )
        .unwrap();
        crate::journal::rename_no_replace(&teams.join("t1"), &teams.join("archive/t1")).unwrap();

        finalize(&home, &actor, "t1", 1, None).unwrap();
        assert!(!has_journal(&home, "t1"));
        assert!(teams.join("archive/t1/state.json").is_file());
        assert!(agents.join("_archived/t1/t1-worker/.complete").is_file());

        match old_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        match old_agents {
            Some(v) => std::env::set_var("APERTURE_AGENTS_DIR", v),
            None => std::env::remove_var("APERTURE_AGENTS_DIR"),
        }
        fs::remove_dir_all(home).unwrap();
    }
}
