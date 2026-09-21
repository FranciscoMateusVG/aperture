// Included inside teams::tests. All state lives under private temporary HOME.
// No live catalog, team, provider, process or repository is accessed.
fn project_runtime_offer(hash: &str, project: &str, enabled: bool) -> SaveRepositoryInput {
    let mut input = registry_fixture_input(hash, enabled);
    input.project = project.into();
    input
}

fn project_runtime_team(name: &str, project: &str) -> CreateTeamInput {
    let mut input = registry_fixture_team(name);
    input.project = project.into();
    input.preset_id = None;
    input
}

#[test]
fn project_runtime_key_boundaries_match_repository_keys() {
    for key in ["a".to_string(), "eunenem-engine".into(), "a.0_b-c".into(), "a".repeat(64)] {
        assert!(valid_repo_key(&key));
        assert!(valid_project_label(&format!("project:{key}")));
    }
    for key in ["".to_string(), "A".into(), "0a".into(), "_a".into(), "../a".into(),
        "a/b".into(), "a\\b".into(), "a:b".into(), "ação".into(), "a\n".into(), "a".repeat(65)] {
        assert!(!valid_repo_key(&key), "{key:?}");
        assert!(!valid_project_label(&format!("project:{key}")), "{key:?}");
    }
    for label in ["eunenem-engine", "Project:eunenem-engine", " project:a", "project:project:a"] {
        assert!(!valid_project_label(label));
    }
}

#[test]
fn project_runtime_new_project_requires_authenticated_offer_then_reloads_immediately() {
    let _guard = crate::team_auth::tests::ENV_LOCK.lock().unwrap();
    let home = temp_root("project-runtime-admit");
    let _env = EnvRestore::set(&home);
    prepare_glados(&home);
    registry_fixture_repo(&home, "eunenem-engine");
    let engine = TeamEngine::new(home.clone(), project_root());
    let initial = engine.list_repositories().unwrap();
    let registry = home.join(".aperture/repositories.json");
    let project = "project:eunenem-engine";
    assert_eq!(engine.create_team(&AuthenticatedActor::operator_ui(), project_runtime_team("project-new", project)).unwrap_err().code,
        "E_REPO_NOT_IN_CATALOG");
    assert!(!home.join(".aperture/teams/project-new").exists());
    for unauthorized in [AuthenticatedActor::operator_ui(), AuthenticatedActor::launcher()] {
        assert_eq!(engine.save_repository(&unauthorized, project_runtime_offer(&initial.sha256, project, true)).unwrap_err().code,
            "E_CONTROL_UNAUTHORIZED");
        assert!(!registry.exists());
    }
    let actor = authenticate_glados_control().unwrap();
    let saved = engine.save_repository(&actor, project_runtime_offer(&initial.sha256, project, true)).unwrap();
    assert!(engine.catalog().unwrap().repositories.iter().any(|r| r.project == project && r.repo == "eunenem-engine" && r.available));
    let created = engine.create_team(&actor, project_runtime_team("project-new", project)).unwrap();
    assert_eq!(created.team.state.state, TeamLifecycle::Pending);
    assert_eq!(created.team.snapshot.project, project);
    assert_eq!(created.creation_request.project, project);
    assert_eq!(created.team.snapshot.repo, "eunenem-engine");
    assert!(created.team.snapshot.preset.id.is_none());
    assert_eq!(TeamEngine::new(home.clone(), project_root()).list_repositories().unwrap(), saved);
    assert_eq!(engine.read_team_view("project-new").unwrap().snapshot.project, project);
    fs::remove_dir_all(home).unwrap();
}

#[test]
fn project_runtime_explicit_alias_preserves_legacy_incluir_and_does_not_infer_new_pair() {
    let _guard = crate::team_auth::tests::ENV_LOCK.lock().unwrap();
    let home = temp_root("project-runtime-alias");
    let _env = EnvRestore::set(&home);
    prepare_glados(&home);
    registry_fixture_repo(&home, "eunenem-engine");
    registry_fixture_repo(&home, "eunenem");
    let engine = TeamEngine::new(home.clone(), project_root());
    let actor = authenticate_glados_control().unwrap();
    let mut legacy = fullstack_input("project-legacy");
    legacy.project = "project:incluir".into();
    legacy.repo = "eunenem".into();
    engine.create_team(&actor, legacy).unwrap();
    let path = home.join(".aperture/teams/project-legacy/team.json");
    let original = fs::read(&path).unwrap();
    let initial = engine.list_repositories().unwrap();
    let saved = engine.save_repository(&actor, project_runtime_offer(&initial.sha256, "project:campaigns", true)).unwrap();
    let explicit = engine.create_team(&actor, project_runtime_team("project-alias", "project:campaigns")).unwrap();
    assert_eq!(explicit.team.snapshot.project, "project:campaigns");
    assert_eq!(explicit.team.snapshot.repo, "eunenem-engine");
    assert_eq!(engine.create_team(&actor, project_runtime_team("project-unlisted", "project:eunenem-engine")).unwrap_err().code,
        "E_REPO_NOT_IN_CATALOG");
    assert!(!home.join(".aperture/teams/project-unlisted").exists());
    assert_eq!(fs::read(&path).unwrap(), original);
    assert_eq!(engine.read_team_view("project-legacy").unwrap().snapshot.project, "project:incluir");
    let wire = serde_json::to_value(saved).unwrap();
    for repo in ["eunenem", "monorepo-incluir"] {
        assert!(wire["repositories"].as_array().unwrap().iter().any(|r|
            r["project"] == "project:incluir" && r["repo"] == repo && r["enabled"] == true));
    }
    fs::remove_dir_all(home).unwrap();
}

#[test]
fn project_runtime_disabled_offer_blocks_admission_but_preserves_active_snapshot() {
    let _guard = crate::team_auth::tests::ENV_LOCK.lock().unwrap();
    let home = temp_root("project-runtime-disable");
    let _env = EnvRestore::set(&home);
    prepare_glados(&home);
    registry_fixture_repo(&home, "eunenem-engine");
    let engine = TeamEngine::new(home.clone(), project_root());
    let actor = authenticate_glados_control().unwrap();
    let project = "project:eunenem-engine";
    let initial = engine.list_repositories().unwrap();
    let enabled = engine.save_repository(&actor, project_runtime_offer(&initial.sha256, project, true)).unwrap();
    let active = engine.create_team(&actor, project_runtime_team("proj-active", project)).unwrap();
    let pending = engine.create_team(&actor, project_runtime_team("proj-pending", project)).unwrap();
    engine.activate(&actor, ActivateTeamInput { team: "proj-active".into(), expected_generation: 0,
        creation_request_id: active.creation_request.request_id, epic_id: "aperture-fixture".into() }).unwrap();
    let path = home.join(".aperture/teams/proj-active/team.json");
    let original = fs::read(&path).unwrap();
    engine.save_repository(&actor, project_runtime_offer(&enabled.sha256, project, false)).unwrap();
    assert!(!engine.catalog().unwrap().repositories.iter().any(|r| r.project == project));
    assert_eq!(engine.create_team(&actor, project_runtime_team("proj-denied", project)).unwrap_err().code, "E_REPO_NOT_IN_CATALOG");
    assert!(!home.join(".aperture/teams/proj-denied").exists());
    assert_eq!(engine.activate(&actor, ActivateTeamInput { team: "proj-pending".into(), expected_generation: 0,
        creation_request_id: pending.creation_request.request_id, epic_id: "aperture-fixture".into() }).unwrap_err().code,
        "E_REPO_NOT_IN_CATALOG");
    assert_eq!(engine.read_team_view("proj-pending").unwrap().state.state, TeamLifecycle::Pending);
    for seat in &pending.team.snapshot.seats { assert!(!home.join(".claude/aperture").join(&seat.name).exists()); }
    let readback = engine.read_team_view("proj-active").unwrap();
    assert_eq!(readback.state.state, TeamLifecycle::Active);
    assert_eq!(readback.snapshot.project, project);
    assert_eq!(fs::read(&path).unwrap(), original);
    assert_eq!(resolve_repository(&home, &readback.snapshot.project, &readback.snapshot.repo).unwrap(), home.join("projects/eunenem-engine"));
    for seat in &readback.snapshot.seats {
        assert_eq!(classify_managed_seat(&home, &seat.name).unwrap(), Some(ManagedSeatState::Active { team: "proj-active".into(), generation: 1 }));
    }
    fs::remove_dir_all(home).unwrap();
}

#[test]
fn project_runtime_malformed_labels_never_publish_registry_or_team() {
    let _guard = crate::team_auth::tests::ENV_LOCK.lock().unwrap();
    let home = temp_root("project-runtime-invalid");
    let _env = EnvRestore::set(&home);
    prepare_glados(&home);
    registry_fixture_repo(&home, "eunenem-engine");
    let engine = TeamEngine::new(home.clone(), project_root());
    let actor = authenticate_glados_control().unwrap();
    let initial = engine.list_repositories().unwrap();
    let registry = home.join(".aperture/repositories.json");
    let malformed = ["project:../engine".to_string(), "project:Engine".into(), "project:".into(),
        "".into(), "engine".into(), "project:engine/sub".into(), "project:engine\n".into(),
        format!("project:{}", "a".repeat(65))];
    for (i, project) in malformed.iter().enumerate() {
        assert_eq!(engine.save_repository(&actor, project_runtime_offer(&initial.sha256, project, true)).unwrap_err().code,
            "E_REPOSITORY_REGISTRY_INVALID");
        assert!(!registry.exists());
        let name = format!("proj-bad-{i}");
        assert_eq!(engine.create_team(&actor, project_runtime_team(&name, project)).unwrap_err().code, "E_NAME_INVALID");
        assert!(!home.join(".aperture/teams").join(name).exists());
        assert_eq!(engine.list_repositories().unwrap(), initial);
    }
    assert_eq!(fs::read_dir(home.join(".aperture/teams/.staging")).unwrap().count(), 0);
    fs::remove_dir_all(home).unwrap();
}
