//! Private headless entrypoint for authenticated GLaDOS team activation.
//!
//! The caller sends one bounded JSON request on stdin. Actor fields, argv and
//! environment identity are never accepted as authority; the library opens
//! and revalidates the canonical GLaDOS capability immediately before the
//! first team mutation.

use std::io::{Read, Write};

const MAX_INPUT: u64 = 16 * 1024;

fn main() -> std::process::ExitCode {
    if std::env::args_os().len() != 1 {
        eprintln!("aperture-team-control: request must be provided on stdin");
        return std::process::ExitCode::from(2);
    }
    let mut bytes = Vec::new();
    if std::io::stdin().take(MAX_INPUT + 1).read_to_end(&mut bytes).is_err() || bytes.len() as u64 > MAX_INPUT {
        eprintln!("aperture-team-control: invalid bounded request");
        return std::process::ExitCode::from(2);
    }
    let Ok(request) = std::str::from_utf8(&bytes) else {
        eprintln!("aperture-team-control: request must be UTF-8 JSON");
        return std::process::ExitCode::from(2);
    };
    match aperture_lib::team_control_json(request) {
        Ok(response) => {
            let _ = std::io::stdout().write_all(response.as_bytes());
            let _ = std::io::stdout().write_all(b"\n");
            std::process::ExitCode::SUCCESS
        }
        Err(error) => {
            let _ = std::io::stdout().write_all(error.as_bytes());
            let _ = std::io::stdout().write_all(b"\n");
            std::process::ExitCode::FAILURE
        }
    }
}
