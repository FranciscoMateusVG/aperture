// Included in publication_tests: uses the same real native publisher/owner
// fixture, not handwritten launch/settings wires. No Claude, provider or tmux.
impl Fixture {
    fn diagnostic_pending(&self) -> (PendingClaude, StartReservation) {
        ensure_private_dir(&self.home.join(".claude/projects")).unwrap();
        let mut binding = self.binding().unwrap();
        binding.mode = ClaudeLaunchMode::NormalPositional;
        let (res, token) = self.reservation();
        let published = binding.publish_with_password(&res, &token, &Deadline::new(), "").unwrap();
        let identity = crate::team_process::observe(std::process::id()).unwrap().unwrap().identity;
        let process = crate::team_process::native::capture_gated_identity(&identity).unwrap();
        OwnerStore::new(self.home.join(".aperture/run/owner")).record_start_candidate(
            &AuthenticatedActor::launcher(), &res, crate::owner::Incarnation {
                pid: process.pid, start_time: process.start_time, thread_id: String::new(),
                token_id: token.token_id().into(), harness: Harness::Claude,
                model: MODEL.into(), reasoning: None, observed: false, processes: vec![process.clone()],
            },
        ).unwrap();
        crate::team_claude_observation::record_attempt(&self.home, "t1", &res,
            published.session_id(), published.record.mode).unwrap();
        (PendingClaude { published, process, window_id: "@1".into(), pane_id: "%1".into() }, res)
    }
    fn diagnostic_released(&self) -> LaunchDiagnostics {
        let (pending, res) = self.diagnostic_pending();
        pending.release_at(&res, &Deadline::new(), &uuid::Uuid::new_v4().to_string(), &self.infra).unwrap()
    }
    fn diagnostic_gate(&self, d: &LaunchDiagnostics, exec: impl FnOnce(Command) -> Result<(), ClaudeError>) -> Result<(), ClaudeError> {
        preserve_gate_result(d, || gate_with_diagnostics(&self.home, "t1", "t1-worker", 1,
            Instant::now() + Duration::from_secs(3), d, &self.infra, exec))
    }
}

#[test]
fn diagnostic_native_success_preserves_pipeline_and_exec_callback_once() {
    let f = Fixture::new();
    let d = f.diagnostic_released();
    let mut calls = 0;
    f.diagnostic_gate(&d, |cmd| {
        calls += 1;
        assert_eq!(cmd.get_args().last().unwrap(), crate::launcher::KICKOFF_TEXT);
        assert_eq!(d.progress().unwrap(), (Some(LaunchStage::ExecBoundary), None));
        Ok(()) // Inert executor seam only; not a claimed model observation.
    }).unwrap();
    assert_eq!(calls, 1);
    assert_eq!(d.health().unwrap().process, crate::team_replacement::ProcessState::Same);
    assert_eq!(d.progress().unwrap(), (Some(LaunchStage::ExecBoundary), None));
    assert!(d.stage(LaunchStage::ExecBoundary).is_err());
}

#[test]
fn diagnostic_native_failures_preserve_exact_phase_without_private_text() {
    for expected in [LaunchStage::PinsSettings, LaunchStage::RepositoryCwd,
        LaunchStage::OwnerBinding, LaunchStage::SessionFreshness, LaunchStage::ExecBoundary] {
        let f = Fixture::new();
        let d = f.diagnostic_released();
        match expected {
            LaunchStage::PinsSettings => f.write(".aperture/run/managed/t1-worker/g1/claude-settings.json", b"{}"),
            LaunchStage::RepositoryCwd => {
                std::fs::rename(&f.cwd, f.home.join("old-repo")).unwrap();
                ensure_private_dir(&f.cwd).unwrap();
            }
            LaunchStage::OwnerBinding => f.write(".aperture/run/hub-tokens/t1-worker.token", b"changed synthetic token"),
            LaunchStage::SessionFreshness => {
                let project = f.home.join(".claude/projects/other-project");
                ensure_private_dir(&project).unwrap();
                write_private_bytes_atomic(&project.join(format!("{}.jsonl", d.context.session_id)), b"", false).unwrap();
            }
            LaunchStage::ExecBoundary => {},
            _ => unreachable!(),
        }
        let mut calls = 0;
        let started = Instant::now();
        let error = f.diagnostic_gate(&d, |_| { calls += 1; Err(ClaudeError::Io) }).unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(calls, usize::from(expected == LaunchStage::ExecBoundary));
        assert_eq!(d.progress().unwrap(), (Some(expected), Some(LaunchEnd::GateError { error })));
        assert_eq!(d.health().unwrap().end, Some(LaunchEnd::GateError { error }));
        let raw = std::fs::read_to_string(d.dir.join("claude-diagnostic-terminal.json")).unwrap();
        for forbidden in ["changed synthetic token", "fixture mission", "argv", "password", "stderr"] {
            assert!(!raw.contains(forbidden));
        }
        let before = std::fs::read(d.dir.join("claude-diagnostic-terminal.json")).unwrap();
        assert!(d.finish(LaunchEnd::ObservationTimeout).is_err());
        assert_eq!(before, std::fs::read(d.dir.join("claude-diagnostic-terminal.json")).unwrap());
    }
}

#[test]
fn diagnostic_helper_waits_for_context_and_release_without_resetting_clock() {
    let f = Fixture::new();
    let (pending, res) = f.diagnostic_pending();
    let until = Instant::now() + Duration::from_secs(3);
    std::thread::scope(|scope| {
        let waiter = scope.spawn(|| {
            let d = LaunchDiagnostics::for_gate(&f.home, "t1", "t1-worker", 1, until).unwrap();
            let mut calls = 0;
            preserve_gate_result(&d, || gate_with_diagnostics(&f.home, "t1", "t1-worker", 1,
                until, &d, &f.infra, |_| { calls += 1; Ok(()) })).unwrap();
            assert_eq!(calls, 1);
        });
        std::thread::sleep(Duration::from_millis(50));
        let d = pending.release_at(&res, &Deadline::new(), &uuid::Uuid::new_v4().to_string(), &f.infra).unwrap();
        waiter.join().unwrap();
        assert_eq!(d.progress().unwrap(), (Some(LaunchStage::ExecBoundary), None));
    });
    let empty = Fixture::new();
    let start = Instant::now();
    assert!(matches!(LaunchDiagnostics::for_gate(&empty.home, "t1", "t1-worker", 1,
        start + Duration::from_millis(30)), Err(ClaudeError::Closed)));
    assert!(start.elapsed() < Duration::from_secs(1));
}

#[test]
fn diagnostic_partial_write_or_corrupt_binding_never_executes_or_replaces() {
    let f = Fixture::new();
    let d = f.diagnostic_released();
    // Occupied private destination models publication failure before success;
    // no-replace must retain it, and the executor must never run.
    ensure_private_dir(&d.dir.join("claude-stage-0.json")).unwrap();
    let mut calls = 0;
    assert!(f.diagnostic_gate(&d, |_| { calls += 1; Ok(()) }).is_err());
    assert_eq!(calls, 0);
    assert!(d.dir.join("claude-stage-0.json").is_dir());
    assert!(!d.dir.join("claude-diagnostic-terminal.json").exists());
    let f = Fixture::new();
    let d = f.diagnostic_released();
    let mut changed = d.context.clone();
    changed.session_id = uuid::Uuid::new_v4().to_string();
    write_private_json_atomic(&d.dir.join("claude-diagnostic-context.json"), &changed, true).unwrap();
    assert!(d.health().is_err());
    assert!(f.diagnostic_gate(&d, |_| { calls += 1; Ok(()) }).is_err());
    assert_eq!(calls, 0);
}

#[test]
fn diagnostic_reader_denies_gaps_symlinks_and_attempt_or_process_drift() {
    let f = Fixture::new();
    let mut d = f.diagnostic_released();
    d.stage(LaunchStage::ReleaseWait).unwrap();
    let step = d.dir.join("claude-stage-0.json");
    let saved = std::fs::read(&step).unwrap();
    std::fs::remove_file(&step).unwrap();
    std::os::unix::fs::symlink(d.dir.join("claude-launch.json"), &step).unwrap();
    assert!(d.health().is_err());
    std::fs::remove_file(&step).unwrap();
    write_private_bytes_atomic(&step, &saved, false).unwrap();
    d.stage(LaunchStage::RecordStructure).unwrap();
    std::fs::remove_file(&step).unwrap();
    assert!(d.progress().is_err());
    write_private_bytes_atomic(&step, &saved, false).unwrap();
    let attempt = f.home.join(".aperture/run/t1-worker.g1.claude-attempt.json");
    let bytes = std::fs::read(&attempt).unwrap();
    f.write(".aperture/run/t1-worker.g1.claude-attempt.json", b"{}");
    assert!(d.health().is_err());
    write_private_bytes_atomic(&attempt, &bytes, true).unwrap();
    d.context.root_start_time_us += 1;
    write_private_json_atomic(&d.dir.join("claude-diagnostic-context.json"), &d.context, true).unwrap();
    assert!(d.health().is_err(), "changed context invalidates all earlier step bindings");
}

// This subprocess test executes ONLY /usr/bin/true in place, not Claude/tmux.
// Its parent can still read the exact diagnostic evidence after this PID exits.
#[test]
fn diagnostic_inert_exec_child() {
    let Some(output) = std::env::var_os("APERTURE_DIAGNOSTIC_TEST_OUTPUT") else { return; };
    let f = Fixture::new();
    let d = f.diagnostic_released();
    write_private_json_atomic(Path::new(&output), &json!({"home":f.home}), false).unwrap();
    f.diagnostic_gate(&d, |_| {
        let error = Command::new("/usr/bin/true").env_clear().exec();
        panic!("inert exec failed: {:?}", error.kind());
    }).unwrap();
    unreachable!("successful exec never returns");
}
#[test]
fn diagnostic_post_exec_exit_is_gone_not_timeout_or_invented_cli_exit_code() {
    let transfer = std::env::temp_dir().join(format!("aperture-diag-transfer-{}", uuid::Uuid::new_v4()));
    ensure_private_dir(&transfer).unwrap();
    let out = transfer.join("fixture.json");
    let status = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "team_claude_launch::publication_tests::diagnostic_inert_exec_child"])
        .env("APERTURE_DIAGNOSTIC_TEST_OUTPUT", &out).output().unwrap();
    assert!(status.status.success(), "{}", String::from_utf8_lossy(&status.stderr));
    let value: serde_json::Value = read_private_json(&out).unwrap();
    let home = PathBuf::from(value["home"].as_str().unwrap());
    let dir = generation_dir(&home, "t1-worker", 1);
    let context = diagnostic_json(&dir.join("claude-diagnostic-context.json")).unwrap();
    let d = LaunchDiagnostics { home: home.clone(), dir, context };
    let health = d.health().unwrap();
    assert_eq!(health.process, crate::team_replacement::ProcessState::Gone);
    assert!(health.exec_boundary);
    assert!(health.end.is_none());
    d.finish(LaunchEnd::RootExitedWithoutObservation).unwrap();
    let raw = std::fs::read_to_string(d.dir.join("claude-diagnostic-terminal.json")).unwrap();
    assert!(raw.contains("root_exited_without_observation"));
    assert!(!raw.contains("exit_code") && !raw.contains("signal"));
    std::fs::remove_dir_all(home).unwrap();
    std::fs::remove_dir_all(transfer).unwrap();
}

#[test]
fn diagnostic_exec_intent_write_or_fsync_error_and_expired_budget_never_exec() {
    let mut calls = 0;
    let until = Instant::now() + Duration::from_secs(1);
    // Model the exact atomic writer failure return, including post-rename
    // fsync failure: no success may be inferred just from a visible file.
    assert_eq!(exec_after_durable_intent(until, || Err(ClaudeError::Io),
        || { calls += 1; Ok(()) }), Err(ClaudeError::Io));
    assert_eq!(calls, 0);
    let expired = Instant::now();
    assert_eq!(exec_after_durable_intent(expired, || panic!("must not write after deadline"),
        || { calls += 1; Ok(()) }), Err(ClaudeError::Closed));
    assert_eq!(calls, 0);
    exec_after_durable_intent(until, || Ok(()), || { calls += 1; Ok(()) }).unwrap();
    assert_eq!(calls, 1);
}
