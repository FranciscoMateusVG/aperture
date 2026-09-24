//! Authenticated archive finalizer. Evidence is native-derived before this
//! seam; only the durable journal may resume an interrupted mutation.
use crate::journal::{
    apply_or_recover_journal, ensure_private_dir, read_journal, read_private_json, remove_journal,
    replace_journal, sync_dir, write_journal, write_private_bytes_atomic,
    write_private_json_atomic, ArchiveJournalApproval, ArchiveMutableFile, ArchivePreimageEntry,
    Journal, JournalMove, JournalObjectKind, JournalOperation, JournalRoot, JournalRoots,
};
use crate::owner::{try_lock, OwnerStore};
use crate::state::OwnerState;
use crate::team_auth::AuthenticatedActor;
use crate::teams::{TeamLifecycle, TeamSnapshot, TeamStateFile};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::os::unix::fs::MetadataExt;
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

fn validate_owner_binding(
    owner: &crate::owner::OwnerRecord,
    expected_state: Option<&str>,
    pre_sha256: Option<&String>,
    post_sha256: Option<&String>,
    recovering: bool,
) -> Result<(), String> {
    let actual = owner_hash(owner)?;
    match (recovering, expected_state, &owner.state) {
        (false, Some("active"), OwnerState::Active) | (false, Some("stale"), OwnerState::Stale)
        | (false, Some("quarantined"), OwnerState::Quarantined) => {
            if pre_sha256 != Some(&actual) {
                return Err("E_ARCHIVE_APPROVAL_STALE: owner changed".into());
            }
        }
        (true, Some("active"), OwnerState::Active) => {
            if pre_sha256 != Some(&actual) {
                return Err("E_ARCHIVE_APPROVAL_STALE: owner changed".into());
            }
        }
        (true, Some("active"), OwnerState::Stale) => {
            if post_sha256 != Some(&actual) {
                return Err("E_ARCHIVE_APPROVAL_STALE: owner postimage changed".into());
            }
        }
        (true, Some("stale"), OwnerState::Stale) | (true, Some("quarantined"), OwnerState::Quarantined) => {
            if pre_sha256 != Some(&actual) || post_sha256 != Some(&actual) {
                return Err("E_ARCHIVE_APPROVAL_STALE: stale owner changed".into());
            }
        }
        _ => return Err("E_ARCHIVE_OWNER_INVALID: owner state changed".into()),
    }
    Ok(())
}

fn journal_path(home: &Path, team: &str) -> PathBuf {
    home.join(".aperture/run/team-journals")
        .join(format!("{team}.archive.json"))
}

fn rollback_plan_path(home: &Path, team: &str) -> PathBuf {
    home.join(".aperture/run/archive-manifests")
        .join(format!("{team}.json"))
}

fn preimage_hash(entries: &[ArchivePreimageEntry]) -> Result<String, String> {
    let bytes = serde_json::to_vec(entries)
        .map_err(|_| "E_ARCHIVE_PREIMAGE_INVALID: manifest encoding failed".to_string())?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn walk_preimage(
    root: &Path,
    root_kind: JournalRoot,
    relative: &Path,
    out: &mut Vec<ArchivePreimageEntry>,
    total_bytes: &mut u64,
) -> Result<(), String> {
    if out.len() >= 2048 {
        return Err("E_ARCHIVE_PREIMAGE_INVALID: path count exceeds limit".into());
    }
    let path = root.join(relative);
    let meta = fs::symlink_metadata(&path)
        .map_err(|_| "E_ARCHIVE_PREIMAGE_INVALID: path is unavailable".to_string())?;
    if meta.file_type().is_symlink()
        || meta.uid() != unsafe { libc::geteuid() }
        || meta.mode() & 0o077 != 0
    {
        return Err("E_ARCHIVE_PREIMAGE_INVALID: unsafe path metadata".into());
    }
    let rel = relative
        .to_str()
        .ok_or_else(|| "E_ARCHIVE_PREIMAGE_INVALID: non-UTF8 path".to_string())?
        .to_string();
    if meta.is_dir() {
        out.push(ArchivePreimageEntry {
            root: root_kind.clone(),
            relative: rel,
            kind: JournalObjectKind::Directory,
            mode: meta.mode() & 0o777,
            sha256: None,
        });
        let mut children = fs::read_dir(&path)
            .map_err(|_| "E_ARCHIVE_PREIMAGE_INVALID: directory unreadable".to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| "E_ARCHIVE_PREIMAGE_INVALID: directory unreadable".to_string())?;
        children.sort_by_key(|entry| entry.file_name());
        for child in children {
            walk_preimage(
                root,
                root_kind.clone(),
                &relative.join(child.file_name()),
                out,
                total_bytes,
            )?;
        }
    } else if meta.is_file() && meta.nlink() == 1 {
        *total_bytes = total_bytes
            .checked_add(meta.len())
            .ok_or_else(|| "E_ARCHIVE_PREIMAGE_INVALID: byte count overflow".to_string())?;
        if *total_bytes > 32 * 1024 * 1024 {
            return Err("E_ARCHIVE_PREIMAGE_INVALID: byte count exceeds limit".into());
        }
        let mut file = crate::journal::open_private_file_nofollow(&path)?;
        let mut hasher = Sha256::new();
        let mut buffer = [0u8; 16 * 1024];
        loop {
            let count = file
                .read(&mut buffer)
                .map_err(|_| "E_ARCHIVE_PREIMAGE_INVALID: file unreadable".to_string())?;
            if count == 0 {
                break;
            }
            hasher.update(&buffer[..count]);
        }
        out.push(ArchivePreimageEntry {
            root: root_kind,
            relative: rel,
            kind: JournalObjectKind::File,
            mode: meta.mode() & 0o777,
            sha256: Some(format!("{:x}", hasher.finalize())),
        });
    } else {
        return Err("E_ARCHIVE_PREIMAGE_INVALID: unsupported path kind".into());
    }
    Ok(())
}

fn read_mutable(
    root: JournalRoot,
    base: &Path,
    relative: &str,
) -> Result<ArchiveMutableFile, String> {
    let path = base.join(relative);
    let mut file = crate::journal::open_private_file_nofollow(&path)?;
    let mut bytes = Vec::new();
    file.by_ref()
        .take(16 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "E_ARCHIVE_PREIMAGE_INVALID: mutable file unreadable".to_string())?;
    if bytes.len() > 16 * 1024 {
        return Err("E_ARCHIVE_PREIMAGE_INVALID: mutable file exceeds limit".into());
    }
    Ok(ArchiveMutableFile {
        root,
        relative: relative.into(),
        sha256: format!("{:x}", Sha256::digest(&bytes)),
        bytes,
    })
}

fn collect_preimage(
    teams: &Path,
    agents: &Path,
    team: &str,
    seats: &[String],
) -> Result<(Vec<ArchivePreimageEntry>, Vec<ArchiveMutableFile>), String> {
    let mut entries = Vec::new();
    let mut total = 0;
    walk_preimage(
        teams,
        JournalRoot::Teams,
        Path::new(team),
        &mut entries,
        &mut total,
    )?;
    for seat in seats {
        walk_preimage(
            agents,
            JournalRoot::Agents,
            Path::new(seat),
            &mut entries,
            &mut total,
        )?;
    }
    entries.sort_by(|a, b| {
        (format!("{:?}", a.root), &a.relative).cmp(&(format!("{:?}", b.root), &b.relative))
    });
    let encoded_manifest = serde_json::to_vec(&entries)
        .map_err(|_| "E_ARCHIVE_PREIMAGE_INVALID: manifest encoding failed".to_string())?;
    if encoded_manifest.len() > 256 * 1024 {
        return Err("E_ARCHIVE_PREIMAGE_INVALID: manifest exceeds limit".into());
    }
    let mut mutable = vec![read_mutable(
        JournalRoot::Teams,
        teams,
        &format!("{team}/state.json"),
    )?];
    for seat in seats {
        mutable.push(read_mutable(
            JournalRoot::Agents,
            agents,
            &format!("{seat}/manifest.json"),
        )?);
    }
    let mutable_total = mutable
        .iter()
        .try_fold(0usize, |total, entry| total.checked_add(entry.bytes.len()))
        .ok_or_else(|| "E_ARCHIVE_PREIMAGE_INVALID: mutable byte count overflow".to_string())?;
    if mutable_total > 96 * 1024 {
        return Err("E_ARCHIVE_PREIMAGE_INVALID: mutable byte count exceeds limit".into());
    }
    Ok((entries, mutable))
}

fn revalidate_evidence_locked(
    home: &Path,
    snapshot: &TeamSnapshot,
    state: &TeamStateFile,
    approval: &ArchiveJournalApproval,
    team_lock: &crate::owner::AdvisoryLock,
    seat_locks: &[crate::owner::AdvisoryLock],
) -> Result<(), String> {
    if approval.category == crate::journal::ArchiveCategory::DiagnosticRetirement {
        let fresh = crate::team_archive::diagnostic::inspect_locked(home, snapshot, state, team_lock, seat_locks)?;
        if fresh.generation != approval.generation || fresh.epic_id != approval.epic_id
            || fresh.owner_sha256 != approval.owner_sha256 || fresh.owner_states != approval.owner_states {
            return Err("E_ARCHIVE_APPROVAL_STALE: diagnostic bindings changed".into());
        }
        return validate_evidence_projection(approval, &fresh.record_sha256, &fresh.inventory_sha256, &fresh.native_sha256, true);
    }
    let epic = state
        .epic_id
        .as_deref()
        .ok_or_else(|| "E_RECONCILIATION_INCOMPLETE: archive epic is unavailable".to_string())?;
    let seats: Vec<_> = snapshot
        .seats
        .iter()
        .map(|seat| seat.name.clone())
        .collect();
    // BEADS has no team/owner lock of its own. Holding the lifecycle locks
    // prevents a concurrent local lifecycle mutation while both complete
    // external projections are collected and compared immediately before the
    // archive journal is published.
    let beads = crate::team_archive::beads::collect_native(
        home,
        &snapshot.team,
        state.generation,
        epic,
        &[],
    )
    .map_err(|_| "E_ARCHIVE_APPROVAL_STALE: reconciliation recheck failed".to_string())?;
    let native = crate::team_archive::native::inspect_native_locked(
        home,
        &snapshot.team,
        state.generation,
        &[],
        team_lock,
        seat_locks,
    )
    .map_err(|_| "E_ARCHIVE_APPROVAL_STALE: native recheck failed".to_string())?;
    let mut blockers = beads.structural_blockers(&seats);
    for seat in &native.seats {
        blockers.extend(seat.blockers.clone());
    }
    validate_evidence_projection(
        approval,
        &beads.record_sha256,
        &beads.inventory_sha256,
        &native.sha256,
        blockers.is_empty(),
    )
}

fn validate_evidence_projection(
    approval: &ArchiveJournalApproval,
    record_sha256: &str,
    inventory_sha256: &str,
    native_sha256: &str,
    blockers_empty: bool,
) -> Result<(), String> {
    if !blockers_empty
        || record_sha256 != approval.record_sha256
        || inventory_sha256 != approval.inventory_sha256
        || native_sha256 != approval.native_sha256
    {
        return Err("E_ARCHIVE_APPROVAL_STALE: archive evidence changed".into());
    }
    Ok(())
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
        || journal.preimage_sha256 != preimage_hash(&journal.archive_preimage)?
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
    finalize_inner(home, actor, team, generation, fresh, false)
}

pub(crate) fn rollback(
    home: &Path,
    actor: &AuthenticatedActor,
    team: &str,
    generation: u64,
) -> Result<(), String> {
    if !crate::agent_loader::is_valid_seat_name(team) || team.len() > 16 || generation == 0 {
        return Err("E_ARCHIVE_BINDING: invalid rollback selector".into());
    }
    actor.revalidate_before_mutation()?;
    let teams = home.join(".aperture/teams");
    let agents = home.join(".claude/aperture");
    let owners = OwnerStore::new(home.join(".aperture/run/owner"));
    let journal_path = journal_path(home, team);
    let plan_path = rollback_plan_path(home, team);
    let source = if fs::symlink_metadata(&journal_path).is_ok() {
        read_journal(&journal_path)?
    } else {
        read_journal(&plan_path)?
    };
    let approval = source
        .archive_approval
        .as_ref()
        .ok_or_else(|| "E_JOURNAL_INCONSISTENT: rollback approval missing".to_string())?;
    if source.team != team
        || approval.generation != generation
        || source.preimage_sha256 != preimage_hash(&source.archive_preimage)?
        || !matches!(
            source.operation,
            JournalOperation::Archive | JournalOperation::RollbackArchive
        )
    {
        return Err("E_JOURNAL_INCONSISTENT: rollback binding mismatch".into());
    }
    let mut seats: Vec<_> = approval
        .owner_sha256
        .iter()
        .map(|(seat, _)| seat.clone())
        .collect();
    seats.sort();
    seats.dedup();
    if seats.is_empty() || seats.len() != approval.owner_sha256.len() {
        return Err("E_JOURNAL_INCONSISTENT: rollback seat coverage mismatch".into());
    }
    let _team_lock = try_lock(&home.join(".aperture/run/team-locks"), team)?;
    let mut _seat_locks = Vec::new();
    for seat in &seats {
        _seat_locks.push(owners.lock(seat)?);
    }
    actor.revalidate_before_mutation()?;
    let owner_pre: std::collections::BTreeMap<_, _> =
        approval.owner_sha256.iter().cloned().collect();
    let owner_states: std::collections::BTreeMap<_, _> =
        approval.owner_states.iter().cloned().collect();
    let owner_post: std::collections::BTreeMap<_, _> =
        approval.owner_post_sha256.iter().cloned().collect();
    if owner_pre.len() != seats.len()
        || owner_states.len() != seats.len()
        || owner_post.len() != seats.len()
    {
        return Err("E_JOURNAL_INCONSISTENT: rollback owner coverage mismatch".into());
    }
    for seat in &seats {
        let owner = owners.read_owner_locked(seat)?;
        validate_owner_binding(
            &owner,
            owner_states.get(seat).map(String::as_str),
            owner_pre.get(seat),
            owner_post.get(seat),
            true,
        )?;
        let expected = match approval.category {
            crate::journal::ArchiveCategory::Mission => OwnerState::Stale,
            crate::journal::ArchiveCategory::DiagnosticRetirement => OwnerState::Quarantined,
        };
        if owner.state != expected {
            return Err("E_ARCHIVE_OWNER_INVALID: rollback owner category changed".into());
        }
    }

    let rollback_journal = if source.operation == JournalOperation::RollbackArchive {
        source
    } else {
        let mut inverse = source.clone();
        inverse.operation = JournalOperation::RollbackArchive;
        inverse.uuid = Uuid::new_v4().to_string();
        inverse.step = 0;
        inverse.moves = source
            .moves
            .iter()
            .rev()
            .map(|mv| JournalMove {
                from_root: mv.to_root.clone(),
                from_rel: mv.to_rel.clone(),
                to_root: mv.from_root.clone(),
                to_rel: mv.from_rel.clone(),
                kind: mv.kind.clone(),
            })
            .collect();
        if fs::symlink_metadata(&journal_path).is_ok() {
            replace_journal(&journal_path, &inverse)?;
        } else {
            ensure_private_dir(journal_path.parent().unwrap())?;
            write_journal(&journal_path, &inverse)?;
        }
        inverse
    };
    apply_or_recover_journal(
        &journal_path,
        &JournalRoots {
            teams: teams.clone(),
            staging: teams.join(".staging"),
            agents: agents.clone(),
            owner: owners.root.clone(),
        },
    )?;
    for mutable in &rollback_journal.archive_mutable_files {
        let root = match mutable.root {
            JournalRoot::Teams => &teams,
            JournalRoot::Agents => &agents,
            _ => return Err("E_JOURNAL_INCONSISTENT: invalid mutable root".into()),
        };
        if format!("{:x}", Sha256::digest(&mutable.bytes)) != mutable.sha256 {
            return Err("E_JOURNAL_INCONSISTENT: mutable preimage hash mismatch".into());
        }
        write_private_bytes_atomic(&root.join(&mutable.relative), &mutable.bytes, true)?;
    }
    let (actual, _) = collect_preimage(&teams, &agents, team, &seats)?;
    if actual != rollback_journal.archive_preimage {
        return Err("E_JOURNAL_INCONSISTENT: rollback byte manifest mismatch".into());
    }
    remove_journal(&journal_path)?;
    if fs::symlink_metadata(&plan_path).is_ok() {
        remove_journal(&plan_path)?;
    }
    Ok(())
}

fn finalize_inner(
    home: &Path,
    actor: &AuthenticatedActor,
    team: &str,
    generation: u64,
    fresh: Option<FreshArchive<'_>>,
    #[cfg_attr(not(test), allow(unused_variables))] skip_external_recheck_for_fixture: bool,
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
    let mut approval = if recovering {
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
    let approved_post: std::collections::BTreeMap<_, _> =
        approval.owner_post_sha256.iter().cloned().collect();
    let category_states_valid = approved_states.values().all(|s| match approval.category {
        crate::journal::ArchiveCategory::Mission => matches!(s.as_str(), "active" | "stale"),
        crate::journal::ArchiveCategory::DiagnosticRetirement => s == "quarantined",
    });
    if approved.len() != seats.len() || approved_states.len() != seats.len() || !category_states_valid {
        return Err("E_ARCHIVE_APPROVAL_INVALID: owner coverage mismatch".into());
    }
    let mut current_owners = std::collections::BTreeMap::new();
    for seat in &seats {
        let owner = owners.read_owner_locked(seat)?;
        validate_owner_binding(
            &owner,
            approved_states.get(seat).map(String::as_str),
            approved.get(seat),
            approved_post.get(seat),
            recovering,
        )?;
        current_owners.insert(seat.clone(), owner);
    }
    if !recovering {
        approval.transition_at = chrono::Utc::now().to_rfc3339();
        approval.owner_post_sha256 = current_owners
            .iter()
            .map(|(seat, owner)| {
                let mut post = owner.clone();
                if post.state == OwnerState::Active {
                    post.state = OwnerState::Stale;
                    post.since = approval.transition_at.clone();
                    post.writer = actor.principal().into();
                }
                owner_hash(&post).map(|hash| (seat.clone(), hash))
            })
            .collect::<Result<Vec<_>, _>>()?;
    } else if approved_post.len() != seats.len() {
        return Err("E_ARCHIVE_APPROVAL_INVALID: owner postimage coverage mismatch".into());
    }
    let (archive_preimage, archive_mutable_files) = if recovering {
        let journal = read_journal(&path)?;
        (journal.archive_preimage, journal.archive_mutable_files)
    } else {
        collect_preimage(&teams, &agents, team, &seats)?
    };
    if !recovering {
        // Last read before any archive directory or journal is created. The
        // held lifecycle locks freeze local facts; both complete external
        // projections are compared again here so known pre-mutation drift
        // fails closed.
        if !skip_external_recheck_for_fixture {
            revalidate_evidence_locked(
                home,
                &snapshot,
                &state,
                &approval,
                &_team_lock,
                &seat_locks,
            )?;
        }
        ensure_private_dir(&teams.join("archive"))?;
        ensure_private_dir(&home.join(".aperture/run/archive-manifests"))?;
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
                preimage_sha256: preimage_hash(&archive_preimage)?,
                archive_approval: Some(approval.clone()),
                archive_preimage: archive_preimage.clone(),
                archive_mutable_files: archive_mutable_files.clone(),
            },
        )?;
    }
    for seat in &seats {
        let owner = owners.read_owner_locked(seat)?;
        if owner.state == OwnerState::Active {
            owners.mark_stale_locked_at(actor, seat, owner.generation, &approval.transition_at)?;
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
    for seat in &seats {
        let manifest_path = agents
            .join("_archived")
            .join(team)
            .join(seat)
            .join("manifest.json");
        let mut manifest: serde_json::Value = read_private_json(&manifest_path)?;
        let object = manifest
            .as_object_mut()
            .ok_or_else(|| "E_ARCHIVE_MANIFEST_INVALID: manifest is not an object".to_string())?;
        match object.get("enabled") {
            Some(serde_json::Value::Bool(_)) => {
                object.insert("enabled".into(), serde_json::Value::Bool(false));
            }
            _ => return Err("E_ARCHIVE_MANIFEST_INVALID: enabled flag is unavailable".into()),
        }
        write_private_json_atomic(&manifest_path, &manifest, true)?;
    }
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
    let completed_journal = read_journal(&path)?;
    let plan_path = rollback_plan_path(home, team);
    if fs::symlink_metadata(&plan_path).is_ok() {
        if read_journal(&plan_path)? != completed_journal {
            return Err("E_JOURNAL_INCONSISTENT: rollback plan conflicts".into());
        }
    } else {
        write_private_json_atomic(&plan_path, &completed_journal, false)?;
    }
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
        let manifest: serde_json::Value = read_private_json(
            &agents
                .join("_archived")
                .join(team)
                .join(seat)
                .join("manifest.json"),
        )?;
        if manifest.get("enabled") != Some(&serde_json::Value::Bool(false)) {
            return Err("E_JOURNAL_INCONSISTENT: archived seat remains enabled".into());
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

    #[test]
    fn archive_evidence_requires_all_exact_hashes_and_zero_blockers() {
        let approval = ArchiveJournalApproval {
            category: crate::journal::ArchiveCategory::Mission,
            generation: 1,
            epic_id: "aperture-epic".into(),
            record_sha256: "a".repeat(64),
            inventory_sha256: "b".repeat(64),
            native_sha256: "c".repeat(64),
            owner_sha256: vec![("t1-worker".into(), "d".repeat(64))],
            owner_states: vec![("t1-worker".into(), "stale".into())],
            owner_post_sha256: vec![("t1-worker".into(), "d".repeat(64))],
            transition_at: "2026-09-20T00:00:00Z".into(),
            approved_by: "glados".into(),
        };
        assert!(validate_evidence_projection(
            &approval,
            &"a".repeat(64),
            &"b".repeat(64),
            &"c".repeat(64),
            true,
        )
        .is_ok());
        for (record, inventory, native, blockers_empty) in [
            ("x".repeat(64), "b".repeat(64), "c".repeat(64), true),
            ("a".repeat(64), "x".repeat(64), "c".repeat(64), true),
            ("a".repeat(64), "b".repeat(64), "x".repeat(64), true),
            ("a".repeat(64), "b".repeat(64), "c".repeat(64), false),
        ] {
            assert!(validate_evidence_projection(
                &approval,
                &record,
                &inventory,
                &native,
                blockers_empty,
            )
            .is_err());
        }
    }

    #[test]
    fn recovery_owner_binding_rejects_generation_tuple_and_incarnation_drift() {
        let mut owner = OwnerStore::initial_record(
            &crate::team_auth::AuthenticatedActor::launcher(),
            "t1-worker",
            ExecutionTuple {
                harness: Harness::Codex,
                model: "gpt-6-astra".into(),
                reasoning: Some(ReasoningEffort::High),
            },
        )
        .unwrap();
        owner.state = OwnerState::Active;
        owner.generation = 1;
        owner.incarnation = Some(crate::owner::Incarnation {
            pid: 900001,
            start_time: 42,
            thread_id: "thread".into(),
            token_id: "a".repeat(64),
            harness: Harness::Codex,
            model: "gpt-6-astra".into(),
            reasoning: Some(ReasoningEffort::High),
            observed: true,
            processes: vec![crate::owner::ProcessIdentity {
                pid: 900001,
                start_time: 42,
                ppid: 1,
                pgid: 900001,
                cmdline_sha256: "b".repeat(64),
                cwd: "/private".into(),
            }],
        });
        let pre = owner_hash(&owner).unwrap();
        let mut post = owner.clone();
        post.state = OwnerState::Stale;
        post.since = "2026-09-20T00:00:00Z".into();
        post.writer = "glados".into();
        let post_hash = owner_hash(&post).unwrap();
        validate_owner_binding(&owner, Some("active"), Some(&pre), Some(&post_hash), true).unwrap();
        validate_owner_binding(&post, Some("active"), Some(&pre), Some(&post_hash), true).unwrap();
        for mode in 0..3 {
            let mut changed = post.clone();
            match mode {
                0 => changed.generation = 2,
                1 => changed.requested.model = "other".into(),
                _ => changed.incarnation.as_mut().unwrap().token_id = "c".repeat(64),
            }
            assert!(validate_owner_binding(
                &changed,
                Some("active"),
                Some(&pre),
                Some(&post_hash),
                true
            )
            .is_err());
        }
    }

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
        write_private_json_atomic(
            &home.join(".claude/aperture/t1-worker/manifest.json"),
            &serde_json::json!({"name":"t1-worker","model":"gpt-6-astra","window":"t1-worker","role":"backend","enabled":true}),
            false,
        ).unwrap();
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
        let (preimage_before, _) = collect_preimage(
            &home.join(".aperture/teams"),
            &home.join(".claude/aperture"),
            "t1",
            &["t1-worker".into()],
        )
        .unwrap();
        let approval = ArchiveJournalApproval {
            category: crate::journal::ArchiveCategory::Mission,
            generation: 1,
            epic_id: "aperture-epic".into(),
            record_sha256: "a".repeat(64),
            inventory_sha256: "b".repeat(64),
            native_sha256: "c".repeat(64),
            owner_sha256: vec![("t1-worker".into(), owner_hash(&owner).unwrap())],
            owner_states: vec![("t1-worker".into(), "stale".into())],
            owner_post_sha256: vec![],
            transition_at: String::new(),
            approved_by: "glados".into(),
        };
        finalize_inner(
            &home,
            &actor,
            "t1",
            1,
            Some(FreshArchive {
                snapshot: &snapshot,
                state: &state,
                approval: &approval,
            }),
            true,
        )
        .unwrap();
        assert!(!home.join(".aperture/teams/t1").exists());
        assert!(home.join(".aperture/teams/archive/t1").is_dir());
        assert!(home
            .join(".claude/aperture/_archived/t1/t1-worker/.complete")
            .is_file());
        let manifest: serde_json::Value =
            read_private_json(&home.join(".claude/aperture/_archived/t1/t1-worker/manifest.json"))
                .unwrap();
        assert_eq!(
            manifest.get("enabled"),
            Some(&serde_json::Value::Bool(false))
        );
        assert!(!has_journal(&home, "t1"));
        // Simulate a killed manual inverse after its first no-replace move.
        // Recovery must reconcile physical state, not trust the step hint.
        let source = read_journal(&rollback_plan_path(&home, "t1")).unwrap();
        let mut inverse = source.clone();
        inverse.operation = JournalOperation::RollbackArchive;
        inverse.uuid = Uuid::new_v4().to_string();
        inverse.step = 0;
        inverse.moves = source
            .moves
            .iter()
            .rev()
            .map(|mv| JournalMove {
                from_root: mv.to_root.clone(),
                from_rel: mv.to_rel.clone(),
                to_root: mv.from_root.clone(),
                to_rel: mv.from_rel.clone(),
                kind: mv.kind.clone(),
            })
            .collect();
        write_journal(&journal_path(&home, "t1"), &inverse).unwrap();
        crate::journal::rename_no_replace(
            &home.join(".claude/aperture/_archived/t1/t1-worker"),
            &home.join(".claude/aperture/t1-worker"),
        )
        .unwrap();
        rollback(&home, &actor, "t1", 1).unwrap();
        let (preimage_after, _) = collect_preimage(
            &home.join(".aperture/teams"),
            &home.join(".claude/aperture"),
            "t1",
            &["t1-worker".into()],
        )
        .unwrap();
        assert_eq!(preimage_after, preimage_before);
        let restored_manifest: serde_json::Value =
            read_private_json(&home.join(".claude/aperture/t1-worker/manifest.json")).unwrap();
        assert_eq!(
            restored_manifest.get("enabled"),
            Some(&serde_json::Value::Bool(true))
        );
        assert!(!rollback_plan_path(&home, "t1").exists());
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
            category: crate::journal::ArchiveCategory::Mission,
            generation: 1,
            epic_id: "aperture-epic".into(),
            record_sha256: "a".repeat(64),
            inventory_sha256: "b".repeat(64),
            native_sha256: "c".repeat(64),
            owner_sha256: vec![("t1-worker".into(), owner_hash(&owner).unwrap())],
            owner_states: vec![("t1-worker".into(), "stale".into())],
            owner_post_sha256: vec![("t1-worker".into(), owner_hash(&owner).unwrap())],
            transition_at: "2026-09-20T00:00:00Z".into(),
            approved_by: "glados".into(),
        };
        let teams = home.join(".aperture/teams");
        let agents = home.join(".claude/aperture");
        ensure_private_dir(&teams.join("archive")).unwrap();
        ensure_private_dir(&agents.join("_archived/t1")).unwrap();
        ensure_private_dir(&home.join(".aperture/run/team-journals")).unwrap();
        let path = journal_path(&home, "t1");
        let (archive_preimage, archive_mutable_files) =
            collect_preimage(&teams, &agents, "t1", &["t1-worker".into()]).unwrap();
        write_journal(
            &path,
            &Journal {
                schema_version: 1,
                operation: JournalOperation::Archive,
                team: "t1".into(),
                uuid: Uuid::new_v4().to_string(),
                step: 0,
                preimage_sha256: preimage_hash(&archive_preimage).unwrap(),
                archive_approval: Some(approval),
                archive_preimage,
                archive_mutable_files,
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

        finalize_inner(&home, &actor, "t1", 1, None, true).unwrap();
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

    #[test]
    fn rollback_missing_source_fails_closed_and_retains_recovery_evidence() {
        let _guard = crate::team_auth::tests::ENV_LOCK.lock().unwrap();
        let (home, snapshot, state, owner) = setup();
        let old_home = std::env::var_os("HOME");
        let old_agents = std::env::var_os("APERTURE_AGENTS_DIR");
        std::env::set_var("HOME", &home);
        std::env::set_var("APERTURE_AGENTS_DIR", home.join(".claude/aperture"));
        let actor = crate::team_auth::authenticate_glados_control().unwrap();
        let approval = ArchiveJournalApproval {
            category: crate::journal::ArchiveCategory::Mission,
            generation: 1,
            epic_id: "aperture-epic".into(),
            record_sha256: "a".repeat(64),
            inventory_sha256: "b".repeat(64),
            native_sha256: "c".repeat(64),
            owner_sha256: vec![("t1-worker".into(), owner_hash(&owner).unwrap())],
            owner_states: vec![("t1-worker".into(), "stale".into())],
            owner_post_sha256: vec![],
            transition_at: String::new(),
            approved_by: "glados".into(),
        };
        finalize_inner(
            &home,
            &actor,
            "t1",
            1,
            Some(FreshArchive {
                snapshot: &snapshot,
                state: &state,
                approval: &approval,
            }),
            true,
        )
        .unwrap();
        let archived_seat = home.join(".claude/aperture/_archived/t1/t1-worker");
        let displaced = home.join("displaced-seat-evidence");
        crate::journal::rename_no_replace(&archived_seat, &displaced).unwrap();
        let error = rollback(&home, &actor, "t1", 1).unwrap_err();
        assert!(error.contains("E_JOURNAL_INCONSISTENT"));
        assert!(journal_path(&home, "t1").is_file());
        assert!(rollback_plan_path(&home, "t1").is_file());
        assert!(displaced.is_dir());
        crate::journal::rename_no_replace(&displaced, &archived_seat).unwrap();
        rollback(&home, &actor, "t1", 1).unwrap();
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
    fn rollback_corrupt_preimage_fails_closed_and_preserves_plan() {
        let _guard = crate::team_auth::tests::ENV_LOCK.lock().unwrap();
        let (home, snapshot, state, owner) = setup();
        let old_home = std::env::var_os("HOME");
        let old_agents = std::env::var_os("APERTURE_AGENTS_DIR");
        std::env::set_var("HOME", &home);
        std::env::set_var("APERTURE_AGENTS_DIR", home.join(".claude/aperture"));
        let actor = crate::team_auth::authenticate_glados_control().unwrap();
        let approval = ArchiveJournalApproval {
            category: crate::journal::ArchiveCategory::Mission,
            generation: 1,
            epic_id: "aperture-epic".into(),
            record_sha256: "a".repeat(64),
            inventory_sha256: "b".repeat(64),
            native_sha256: "c".repeat(64),
            owner_sha256: vec![("t1-worker".into(), owner_hash(&owner).unwrap())],
            owner_states: vec![("t1-worker".into(), "stale".into())],
            owner_post_sha256: vec![],
            transition_at: String::new(),
            approved_by: "glados".into(),
        };
        finalize_inner(
            &home,
            &actor,
            "t1",
            1,
            Some(FreshArchive {
                snapshot: &snapshot,
                state: &state,
                approval: &approval,
            }),
            true,
        )
        .unwrap();
        let plan = rollback_plan_path(&home, "t1");
        let mut corrupted: serde_json::Value = read_private_json(&plan).unwrap();
        corrupted["archive_mutable_files"][0]["sha256"] = serde_json::Value::String("0".repeat(64));
        write_private_json_atomic(&plan, &corrupted, true).unwrap();
        assert!(rollback(&home, &actor, "t1", 1)
            .unwrap_err()
            .contains("E_JOURNAL_INCONSISTENT"));
        assert!(plan.is_file());
        assert!(home.join(".aperture/teams/archive/t1").is_dir());
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
