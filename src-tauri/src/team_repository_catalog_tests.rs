// Included inside teams::tests. All storage and capability fixtures are private
// temporary homes; no real repository, bearer, process, or provider is touched.
fn registry_fixture_repo(home: &Path, repo: &str) {
    let dir = home.join("projects").join(repo);
    ensure_private_dir(&dir).unwrap();
    ensure_private_dir(&dir.join(".git")).unwrap();
}

fn registry_fixture_input(hash: &str, enabled: bool) -> SaveRepositoryInput {
    SaveRepositoryInput {
        project: "project:incluir".into(), repo: "eunenem-engine".into(),
        display_name: "EuNeném Engine".into(), enabled, expected_sha256: hash.into(),
    }
}

fn registry_fixture_team(name: &str) -> CreateTeamInput {
    let mut input = fullstack_input(name);
    input.project = "project:incluir".into();
    input.repo = "eunenem-engine".into();
    input
}

fn registry_fixture_json() -> serde_json::Value {
    serde_json::json!({"schema_version":1,"repositories":[{
        "project":"project:aperture","repo":"aperture",
        "display_name":"Aperture","enabled":true
    }]})
}

#[test]
fn runtime_registry_first_save_keeps_seeds_and_is_immediately_usable() {
    let _guard = crate::team_auth::tests::ENV_LOCK.lock().unwrap();
    let home = temp_root("registry-first");
    let _env = EnvRestore::set(&home);
    prepare_glados(&home);
    registry_fixture_repo(&home, "eunenem-engine");
    let engine = TeamEngine::new(home.clone(), project_root());
    let path = home.join(".aperture/repositories.json");
    let initial = engine.list_repositories().unwrap();
    assert!(!path.exists(), "listing virtual seeds must not persist a registry");
    assert_eq!(initial.repositories.len(), REPOSITORY_SEEDS.len());
    let saved = engine.save_repository(&authenticate_glados_control().unwrap(), registry_fixture_input(&initial.sha256, true)).unwrap();
    assert_ne!(saved.sha256, initial.sha256);
    assert_eq!(saved.repositories.len(), REPOSITORY_SEEDS.len() + 1);
    let value = serde_json::to_value(&saved).unwrap();
    for (project, repo, _) in REPOSITORY_SEEDS {
        assert!(value["repositories"].as_array().unwrap().iter().any(|v| v["project"] == *project && v["repo"] == *repo && v["enabled"] == true));
    }
    assert!(engine.catalog().unwrap().repositories.iter().any(|v| v.repo == "eunenem-engine" && v.available));
    let created = engine.create_team(&AuthenticatedActor::operator_ui(), registry_fixture_team("runtime-new")).unwrap();
    assert_eq!(created.team.state.state, TeamLifecycle::Pending);
    assert_eq!(created.team.snapshot.repo, "eunenem-engine");
    assert_eq!(created.creation_request.repo, "eunenem-engine");
    assert_eq!(engine.list_repositories().unwrap(), saved);
    assert_eq!(TeamEngine::new(home.clone(), project_root()).list_repositories().unwrap(), saved);
    let meta = fs::symlink_metadata(&path).unwrap();
    assert_eq!(meta.mode() & 0o777, 0o600);
    assert_eq!(meta.nlink(), 1);
    fs::remove_dir_all(home).unwrap();
}

#[test]
fn runtime_registry_stale_cas_preserves_exact_file() {
    let _guard = crate::team_auth::tests::ENV_LOCK.lock().unwrap();
    let home = temp_root("registry-cas");
    let _env = EnvRestore::set(&home);
    prepare_glados(&home);
    registry_fixture_repo(&home, "eunenem-engine");
    let engine = TeamEngine::new(home.clone(), project_root());
    let actor = authenticate_glados_control().unwrap();
    let old = engine.list_repositories().unwrap().sha256;
    let saved = engine.save_repository(&actor, registry_fixture_input(&old, true)).unwrap();
    let path = home.join(".aperture/repositories.json");
    let before = fs::read(&path).unwrap();
    let inode = fs::metadata(&path).unwrap().ino();
    assert_eq!(engine.save_repository(&actor, registry_fixture_input(&old, false)).unwrap_err().code, "E_REPOSITORY_CONFLICT");
    assert_eq!(fs::read(&path).unwrap(), before);
    assert_eq!(fs::metadata(&path).unwrap().ino(), inode);
    assert_eq!(engine.list_repositories().unwrap(), saved);
    fs::remove_dir_all(home).unwrap();
}

#[test]
fn runtime_registry_disable_blocks_admission_not_existing_active_binding() {
    let _guard = crate::team_auth::tests::ENV_LOCK.lock().unwrap();
    let home = temp_root("registry-disable");
    let _env = EnvRestore::set(&home);
    prepare_glados(&home);
    registry_fixture_repo(&home, "eunenem-engine");
    let engine = TeamEngine::new(home.clone(), project_root());
    let actor = authenticate_glados_control().unwrap();
    let first = engine.list_repositories().unwrap();
    let enabled = engine.save_repository(&actor, registry_fixture_input(&first.sha256, true)).unwrap();
    let active = engine.create_team(&AuthenticatedActor::operator_ui(), registry_fixture_team("reg-active")).unwrap();
    let pending = engine.create_team(&AuthenticatedActor::operator_ui(), registry_fixture_team("reg-pending")).unwrap();
    engine.activate(&actor, ActivateTeamInput { team: "reg-active".into(), expected_generation: 0, creation_request_id: active.creation_request.request_id, epic_id: "aperture-fixture".into() }).unwrap();
    let snapshot = home.join(".aperture/teams/reg-active/team.json");
    let before = fs::read(&snapshot).unwrap();
    engine.save_repository(&actor, registry_fixture_input(&enabled.sha256, false)).unwrap();
    assert!(!engine.catalog().unwrap().repositories.iter().any(|v| v.repo == "eunenem-engine"));
    assert_eq!(engine.create_team(&AuthenticatedActor::operator_ui(), registry_fixture_team("reg-denied")).unwrap_err().code, "E_REPO_NOT_IN_CATALOG");
    assert!(!home.join(".aperture/teams/reg-denied").exists());
    assert_eq!(engine.activate(&actor, ActivateTeamInput { team: "reg-pending".into(), expected_generation: 0, creation_request_id: pending.creation_request.request_id, epic_id: "aperture-fixture".into() }).unwrap_err().code, "E_REPO_NOT_IN_CATALOG");
    assert_eq!(engine.read_team_view("reg-pending").unwrap().state.state, TeamLifecycle::Pending);
    for seat in &pending.team.snapshot.seats { assert!(!home.join(".claude/aperture").join(&seat.name).exists()); }
    let still_active = engine.read_team_view("reg-active").unwrap();
    assert_eq!(still_active.state.state, TeamLifecycle::Active);
    assert_eq!(fs::read(&snapshot).unwrap(), before);
    for seat in &still_active.snapshot.seats {
        assert_eq!(classify_managed_seat(&home, &seat.name).unwrap(), Some(ManagedSeatState::Active { team: "reg-active".into(), generation: 1 }));
    }
    assert_eq!(resolve_repository(&home, "project:incluir", "eunenem-engine").unwrap(), home.join("projects/eunenem-engine"));
    assert!(resolve_repository(&home, "project:incluir", "../eunenem-engine").is_err());
    fs::remove_dir_all(home).unwrap();
}

#[test]
fn runtime_registry_corrupt_schema_and_duplicate_never_fall_back_or_overwrite() {
    let _guard = crate::team_auth::tests::ENV_LOCK.lock().unwrap();
    let mut schema = registry_fixture_json(); schema["schema_version"] = serde_json::json!(2);
    let mut duplicate = registry_fixture_json(); let item = duplicate["repositories"][0].clone(); duplicate["repositories"].as_array_mut().unwrap().push(item);
    let mut unknown = registry_fixture_json(); unknown["approved"] = serde_json::json!(true);
    let mut entry_unknown = registry_fixture_json(); entry_unknown["repositories"][0]["path"] = serde_json::json!("/tmp/untrusted");
    let mut traversal = registry_fixture_json(); traversal["repositories"][0]["repo"] = serde_json::json!("../aperture");
    for raw in [b"not-json".to_vec(), serde_json::to_vec(&schema).unwrap(), serde_json::to_vec(&duplicate).unwrap(), serde_json::to_vec(&unknown).unwrap(), serde_json::to_vec(&entry_unknown).unwrap(), serde_json::to_vec(&traversal).unwrap()] {
        let home = temp_root("registry-corrupt");
        let _env = EnvRestore::set(&home);
        prepare_glados(&home);
        let engine = TeamEngine::new(home.clone(), project_root());
        let hash = engine.list_repositories().unwrap().sha256;
        let path = home.join(".aperture/repositories.json");
        write_private_bytes_atomic(&path, &raw, false).unwrap();
        assert!(engine.list_repositories().is_err());
        assert!(engine.catalog().is_err());
        assert!(engine.create_team(&AuthenticatedActor::operator_ui(), fullstack_input("reg-corrupt")).is_err());
        assert!(engine.save_repository(&authenticate_glados_control().unwrap(), registry_fixture_input(&hash, false)).is_err());
        assert_eq!(fs::read(&path).unwrap(), raw);
        assert!(!home.join(".aperture/teams/reg-corrupt").exists());
        fs::remove_dir_all(home).unwrap();
    }
}

#[test]
fn runtime_registry_symlink_hardlink_and_permissions_fail_closed() {
    let _guard = crate::team_auth::tests::ENV_LOCK.lock().unwrap();
    for kind in ["symlink", "dangling", "hardlink", "public-file", "public-parent"] {
        let home = temp_root("registry-path");
        let _env = EnvRestore::set(&home);
        prepare_glados(&home);
        let engine = TeamEngine::new(home.clone(), project_root());
        let hash = engine.list_repositories().unwrap().sha256;
        let path = home.join(".aperture/repositories.json");
        let target = home.join("canary.json");
        let raw = serde_json::to_vec(&registry_fixture_json()).unwrap();
        write_private_bytes_atomic(&target, &raw, false).unwrap();
        match kind {
            "symlink" => std::os::unix::fs::symlink(&target, &path).unwrap(),
            "dangling" => std::os::unix::fs::symlink(home.join("absent.json"), &path).unwrap(),
            "hardlink" => fs::hard_link(&target, &path).unwrap(),
            _ => { write_private_bytes_atomic(&path, &raw, false).unwrap(); }
        }
        if kind == "public-file" { fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap(); }
        if kind == "public-parent" { fs::set_permissions(home.join(".aperture"), fs::Permissions::from_mode(0o755)).unwrap(); }
        assert!(engine.list_repositories().is_err(), "{kind}");
        assert!(engine.catalog().is_err(), "{kind}");
        // Authenticate before the mutated parent is needed only for the file
        // cases; unsafe parent itself must reject even authentication.
        if let Ok(actor) = authenticate_glados_control() {
            assert!(engine.save_repository(&actor, registry_fixture_input(&hash, false)).is_err(), "{kind}");
        }
        assert_eq!(fs::read(&target).unwrap(), raw, "canary modified for {kind}");
        let after = fs::symlink_metadata(&path).unwrap();
        if kind == "symlink" || kind == "dangling" { assert!(after.file_type().is_symlink()); }
        if kind == "hardlink" { assert_eq!(after.nlink(), 2); }
        fs::remove_dir_all(home).unwrap();
    }
}

#[test]
fn runtime_registry_authority_and_forged_fields_reject_before_write() {
    let _guard = crate::team_auth::tests::ENV_LOCK.lock().unwrap();
    let home = temp_root("registry-auth");
    let _env = EnvRestore::set(&home);
    prepare_glados(&home);
    registry_fixture_repo(&home, "eunenem-engine");
    let engine = TeamEngine::new(home.clone(), project_root());
    let hash = engine.list_repositories().unwrap().sha256;
    for actor in [AuthenticatedActor::operator_ui(), AuthenticatedActor::launcher()] {
        assert_eq!(engine.save_repository(&actor, registry_fixture_input(&hash, true)).unwrap_err().code, "E_CONTROL_UNAUTHORIZED");
    }
    for field in ["actor", "principal", "approved", "source", "path", "generation"] {
        let mut request = serde_json::json!({"action":"save_repository","input":registry_fixture_input(&hash, true)});
        request["input"][field] = serde_json::json!("glados");
        assert!(team_control_headless(&request.to_string()).is_err(), "forged {field}");
        assert!(!home.join(".aperture/repositories.json").exists());
    }
    let mut request = serde_json::json!({"action":"save_repository","input":registry_fixture_input(&hash, true)});
    request["actor"] = serde_json::json!("glados");
    assert!(team_control_headless(&request.to_string()).is_err());
    fs::remove_file(home.join(".aperture/run/hub-tokens/glados.token")).unwrap();
    let request = serde_json::json!({"action":"save_repository","input":registry_fixture_input(&hash, true)});
    assert!(team_control_headless(&request.to_string()).is_err());
    assert!(!home.join(".aperture/repositories.json").exists());
    fs::remove_dir_all(home).unwrap();
}

#[test]
fn runtime_registry_replaced_capability_cannot_edit_existing_registry() {
    let _guard = crate::team_auth::tests::ENV_LOCK.lock().unwrap();
    let home = temp_root("registry-replaced");
    let _env = EnvRestore::set(&home);
    prepare_glados(&home);
    registry_fixture_repo(&home, "eunenem-engine");
    let engine = TeamEngine::new(home.clone(), project_root());
    let actor = authenticate_glados_control().unwrap();
    let hash = engine.list_repositories().unwrap().sha256;
    let saved = engine.save_repository(&actor, registry_fixture_input(&hash, true)).unwrap();
    let path = home.join(".aperture/repositories.json"); let before = fs::read(&path).unwrap();
    let token = home.join(".aperture/run/hub-tokens/glados.token");
    fs::rename(&token, token.with_extension("old")).unwrap();
    write_private_bytes_atomic(&token, b"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", false).unwrap();
    assert_eq!(engine.save_repository(&actor, registry_fixture_input(&saved.sha256, false)).unwrap_err().code, "E_CONTROL_UNAUTHORIZED");
    assert_eq!(fs::read(&path).unwrap(), before);
    fs::remove_dir_all(home).unwrap();
}

#[test]
fn runtime_registry_invalid_request_or_unavailable_repo_never_publishes() {
    let _guard = crate::team_auth::tests::ENV_LOCK.lock().unwrap();
    let home = temp_root("registry-invalid-input");
    let _env = EnvRestore::set(&home);
    prepare_glados(&home);
    let engine = TeamEngine::new(home.clone(), project_root());
    let actor = authenticate_glados_control().unwrap();
    let hash = engine.list_repositories().unwrap().sha256;
    assert_eq!(engine.save_repository(&actor, registry_fixture_input(&hash, true)).unwrap_err().code, "E_REPO_UNAVAILABLE");
    registry_fixture_repo(&home, "eunenem-engine");
    for field in ["repo", "project", "display", "hash"] {
        let mut input = registry_fixture_input(&hash, true);
        match field {
            "repo" => input.repo = "../eunenem-engine".into(),
            "project" => input.project = "project:unknown".into(),
            "display" => input.display_name = "unsafe\ntext".into(),
            _ => input.expected_sha256 = "not-a-hash".into(),
        }
        assert!(engine.save_repository(&actor, input).is_err(), "{field}");
        assert!(!home.join(".aperture/repositories.json").exists());
    }
    fs::remove_dir_all(home).unwrap();
}
