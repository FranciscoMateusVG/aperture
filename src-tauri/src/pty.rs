use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use std::io::{Read, Write};
use std::sync::Mutex;
use tauri::{AppHandle, Emitter};

pub struct PtyState {
    pub writer: Option<Box<dyn Write + Send>>,
    pub master: Option<Box<dyn MasterPty + Send>>,
}

#[tauri::command]
pub fn start_pty(
    session_name: String,
    app: AppHandle,
    pty_state: tauri::State<'_, Mutex<PtyState>>,
) -> Result<(), String> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| e.to_string())?;

    let mut cmd = CommandBuilder::new("/opt/homebrew/bin/tmux");
    cmd.arg("attach-session");
    cmd.arg("-t");
    cmd.arg(&session_name);

    // Production Tauri .app bundles inherit almost no environment.
    // We must explicitly set the essentials for tmux and the shell to work.
    let current_path = std::env::var("PATH").unwrap_or_default();
    cmd.env("PATH", format!("/opt/homebrew/bin:/usr/local/bin:{}", current_path));
    cmd.env("TERM", "xterm-256color");
    cmd.env(
        "HOME",
        std::env::var("HOME").unwrap_or_else(|_| "/Users/<your-username>".into()),
    );
    cmd.env(
        "SHELL",
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into()),
    );
    cmd.env("LANG", "en_US.UTF-8");

    let mut child = pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
    let writer = pair.master.take_writer().map_err(|e| e.to_string())?;

    {
        let mut state = pty_state.lock().map_err(|e| e.to_string())?;
        state.writer = Some(writer);
        state.master = Some(pair.master);
    }

    let app_clone = app.clone();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let data = String::from_utf8_lossy(&buf[..n]).to_string();
                    let _ = app_clone.emit("pty-output", &data);
                }
                Err(_) => break,
            }
        }
    });

    std::thread::spawn(move || {
        let _ = child.wait();
    });

    Ok(())
}

#[tauri::command]
pub fn write_pty(input: String, pty_state: tauri::State<'_, Mutex<PtyState>>) -> Result<(), String> {
    let mut state = pty_state.lock().map_err(|e| e.to_string())?;
    if let Some(ref mut writer) = state.writer {
        writer
            .write_all(input.as_bytes())
            .map_err(|e| e.to_string())?;
        writer.flush().map_err(|e| e.to_string())?;
        Ok(())
    } else {
        Err("PTY not started".into())
    }
}

#[tauri::command]
pub fn resize_pty(
    rows: u16,
    cols: u16,
    pty_state: tauri::State<'_, Mutex<PtyState>>,
) -> Result<(), String> {
    let state = pty_state.lock().map_err(|e| e.to_string())?;
    if let Some(ref master) = state.master {
        master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| e.to_string())?;
        Ok(())
    } else {
        Err("PTY not started".into())
    }
}
