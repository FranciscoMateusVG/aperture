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

fn main() -> std::process::ExitCode {
    if std::env::args().nth(1).as_deref() == Some("--attach-managed") {
        let args: Vec<_> = std::env::args().skip(2).collect();
        if args.len() != 4 { return std::process::ExitCode::from(2); }
        let Ok(generation) = args[2].parse() else { return std::process::ExitCode::from(2); };
        return match aperture_lib::attach_managed_terminal(args[0].clone(), args[1].clone(), generation, args[3].clone()) {
            Ok(()) => std::process::ExitCode::SUCCESS,
            Err(e) => { eprintln!("{e}"); std::process::ExitCode::FAILURE }
        };
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
