//! Headless boot entry point (aperture-syepg).
//!
//! Boots one or more registered agents by name through the REAL spawn path —
//! tmux window, launcher script, and (Claude) the baked-in kickoff positional /
//! (Codex) the bridge-thread resume gate — with no Tauri GUI. This is the
//! hard-required headless hook behind the boot-verification harness
//! (aperture-xt16e L3) and the watchdog re-kick (aperture-wul6m).
//!
//! Usage:
//!   aperture-boot --agent <name> [--agent <name> ...]
//!
//! Honors the launcher env knobs (APERTURE_CLAUDE_BIN / APERTURE_CODEX_BIN /
//! APERTURE_LAUNCHER_PATH_PREFIX), the registry override (APERTURE_AGENTS_DIR),
//! and the tmux session override (APERTURE_TMUX_SESSION). The target tmux
//! session must already exist. Exit: 0 = all booted, 1 = one or more failed,
//! 2 = usage error.

fn usage() {
    eprintln!("usage: aperture-boot --agent <name> [--agent <name> ...]");
}

#[derive(Debug, PartialEq, Eq)]
enum ManagedClaudeRequest {
    Observe { team: String, seat: String },
    Gate { team: String, seat: String, generation: u64 },
}

fn managed_claude_request(args: &[String]) -> Result<Option<ManagedClaudeRequest>, &'static str> {
    const INVALID: &str = "E_CLAUDE_HELPER_ARGUMENTS";
    if !args.iter().any(|arg| arg.starts_with("--managed-claude-")) {
        return Ok(None);
    }
    let Some(mode) = args.first() else { return Err(INVALID) };
    let observe = mode == "--managed-claude-observe";
    let gate = mode == "--managed-claude-gate";
    if (!observe && !gate) || args.len() != if observe { 5 } else { 7 }
        || args[1] != "--team" || args[3] != "--seat" {
        return Err(INVALID);
    }
    let key = |value: &str, max: usize| !value.is_empty() && value.len() <= max
        && value.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if !key(&args[2], 16) || !key(&args[4], 31) { return Err(INVALID); }
    if observe {
        Ok(Some(ManagedClaudeRequest::Observe { team: args[2].clone(), seat: args[4].clone() }))
    } else {
        if args[5] != "--generation" || args[6].is_empty()
            || !args[6].bytes().all(|b| b.is_ascii_digit()) { return Err(INVALID); }
        let generation: u64 = args[6].parse().map_err(|_| INVALID)?;
        if generation == 0 { return Err(INVALID); }
        Ok(Some(ManagedClaudeRequest::Gate { team: args[2].clone(), seat: args[4].clone(), generation }))
    }
}

fn main() -> std::process::ExitCode {
    match managed_claude_request(&std::env::args().skip(1).collect::<Vec<_>>()) {
        Err(code) => { eprintln!("{code}"); return std::process::ExitCode::from(2); }
        Ok(Some(request)) => {
            let result = match request {
                ManagedClaudeRequest::Observe { team, seat } => aperture_lib::managed_claude_observe(&team, &seat),
                ManagedClaudeRequest::Gate { team, seat, generation } => aperture_lib::managed_claude_gate(&team, &seat, generation),
            };
            return match result {
                Ok(()) => std::process::ExitCode::SUCCESS,
                Err(code) => { eprintln!("{code}"); std::process::ExitCode::FAILURE }
            };
        }
        Ok(None) => {}
    }
    let mut agents: Vec<String> = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--agent" => match args.next() {
                Some(name) => agents.push(name),
                None => {
                    eprintln!("aperture-boot: --agent requires a value");
                    return std::process::ExitCode::from(2);
                }
            },
            "-h" | "--help" => {
                usage();
                return std::process::ExitCode::SUCCESS;
            }
            other => {
                eprintln!("aperture-boot: unexpected argument '{}'", other);
                usage();
                return std::process::ExitCode::from(2);
            }
        }
    }

    if agents.is_empty() {
        usage();
        return std::process::ExitCode::from(2);
    }

    let mut failed = false;
    for name in &agents {
        match aperture_lib::boot_agent_headless(name) {
            Ok(window_id) => println!("booted agent={} window={}", name, window_id),
            Err(e) => {
                eprintln!("aperture-boot: boot failed for '{}': {}", name, e);
                failed = true;
            }
        }
    }

    if failed {
        std::process::ExitCode::FAILURE
    } else {
        std::process::ExitCode::SUCCESS
    }
}

#[cfg(test)]
mod managed_helper_tests {
    use super::*;
    fn args(values: &[&str]) -> Vec<String> { values.iter().map(|v| (*v).into()).collect() }
    #[test]
    fn parses_selectors_only_and_preserves_legacy_path() {
        assert_eq!(managed_claude_request(&args(&["--agent","wheatley"])), Ok(None));
        assert_eq!(managed_claude_request(&args(&["--managed-claude-observe","--team","test","--seat","test-qa"])),
            Ok(Some(ManagedClaudeRequest::Observe { team:"test".into(), seat:"test-qa".into() })));
        assert_eq!(managed_claude_request(&args(&["--managed-claude-gate","--team","test","--seat","test-qa","--generation","1"])),
            Ok(Some(ManagedClaudeRequest::Gate { team:"test".into(), seat:"test-qa".into(), generation:1 })));
    }
    #[test]
    fn malformed_mixed_or_forged_requests_never_dispatch() {
        for input in [
            vec!["--managed-claude-observe"],
            vec!["--managed-claude-unknown"],
            vec!["--agent","wheatley","--managed-claude-observe"],
            vec!["--managed-claude-observe","--team","test","--seat","test-qa","--actor","glados"],
            vec!["--managed-claude-observe","--team","test","--team","test-qa"],
            vec!["--managed-claude-observe","--team","../test","--seat","test-qa"],
            vec!["--managed-claude-gate","--team","test","--seat","test-qa","--generation","0"],
            vec!["--managed-claude-gate","--team","test","--seat","test-qa","--generation","+1"],
            vec!["--managed-claude-gate","--team","test","--seat","test-qa","--generation","18446744073709551616"],
            vec!["--managed-claude-gate","--team","test","--seat","test-qa","--model","claude-sonnet-5"],
        ] { assert_eq!(managed_claude_request(&args(&input)), Err("E_CLAUDE_HELPER_ARGUMENTS")); }
    }
}
