use serde::Serialize;
use std::process::Command;

/// Create a Command with environment that works in production builds
/// where the .app bundle doesn't inherit the user's shell environment.
fn cmd(program: &str) -> Command {
    let mut c = Command::new(program);
    let current_path = std::env::var("PATH").unwrap_or_default();
    c.env("PATH", format!("/opt/homebrew/bin:/usr/local/bin:{}", current_path));
    c.env("TERM", "xterm-256color");
    c.env(
        "HOME",
        std::env::var("HOME").unwrap_or_else(|_| "/Users/<your-username>".into()),
    );
    c.env("LANG", "en_US.UTF-8");
    c
}

#[derive(Debug, Serialize, Clone)]
pub struct WindowInfo {
    pub window_id: String,
    pub name: String,
    pub command: String,
}

#[tauri::command]
pub fn tmux_create_session(session_name: String) -> Result<String, String> {
    let check = cmd("tmux")
        .args(["has-session", "-t", &session_name])
        .output()
        .map_err(|e| e.to_string())?;

    if check.status.success() {
        return Ok("already exists".into());
    }

    let output = cmd("tmux")
        .args(["new-session", "-d", "-s", &session_name])
        .output()
        .map_err(|e| e.to_string())?;

    if output.status.success() {
        // Enable mouse scrolling and increase scrollback history
        let _ = cmd("tmux")
            .args(["set-option", "-t", &session_name, "-g", "mouse", "on"])
            .output();
        let _ = cmd("tmux")
            .args(["set-option", "-t", &session_name, "-g", "history-limit", "50000"])
            .output();
        Ok("created".into())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).to_string())
    }
}

#[tauri::command]
pub fn tmux_list_windows(session_name: String) -> Result<Vec<WindowInfo>, String> {
    let output = cmd("tmux")
        .args([
            "list-windows",
            "-t",
            &session_name,
            "-F",
            "#{window_id}||#{window_name}||#{pane_current_command}",
        ])
        .output()
        .map_err(|e| e.to_string())?;

    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).to_string());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let windows = stdout
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let parts: Vec<&str> = line.splitn(3, "||").collect();
            WindowInfo {
                window_id: parts.first().unwrap_or(&"").to_string(),
                name: parts.get(1).unwrap_or(&"").to_string(),
                command: parts.get(2).unwrap_or(&"").to_string(),
            }
        })
        .collect();

    Ok(windows)
}

#[tauri::command]
pub fn tmux_create_window(session_name: String, window_name: String) -> Result<String, String> {
    let output = cmd("tmux")
        .args([
            "new-window",
            "-t",
            &session_name,
            "-n",
            &window_name,
            "-P",
            "-F",
            "#{window_id}",
        ])
        .output()
        .map_err(|e| e.to_string())?;

    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).to_string());
    }

    let window_id = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(window_id)
}

#[tauri::command]
pub fn tmux_kill_window(window_id: String) -> Result<(), String> {
    let output = cmd("tmux")
        .args(["kill-window", "-t", &window_id])
        .output()
        .map_err(|e| e.to_string())?;

    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).to_string())
    }
}

#[tauri::command]
pub fn tmux_select_window(window_id: String) -> Result<(), String> {
    let output = cmd("tmux")
        .args(["select-window", "-t", &window_id])
        .output()
        .map_err(|e| e.to_string())?;

    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).to_string())
    }
}

#[tauri::command]
pub fn tmux_rename_window(target: String, new_name: String) -> Result<(), String> {
    let output = cmd("tmux")
        .args(["rename-window", "-t", &target, &new_name])
        .output()
        .map_err(|e| e.to_string())?;

    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).to_string())
    }
}

pub fn tmux_capture_pane(window_id: &str) -> Result<String, String> {
    let output = cmd("tmux")
        .args(["capture-pane", "-t", window_id, "-p"])
        .output()
        .map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Send just the Enter key to a tmux pane. Used as a follow-up after
/// `tmux_send_keys` for Codex agents whose interactive loop sometimes
/// buffers the combined text+Enter and needs a second explicit press.
pub fn tmux_send_enter(target: &str) -> Result<(), String> {
    let output = cmd("tmux")
        .args(["send-keys", "-t", target, "Enter"])
        .output()
        .map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).to_string())
    }
}

#[tauri::command]
pub fn tmux_send_keys(target: String, keys: String) -> Result<(), String> {
    // Special keys like C-c should not be quoted or followed by Enter
    let is_special = keys.starts_with("C-") || keys.starts_with("M-");

    let output = if is_special {
        cmd("tmux")
            .args(["send-keys", "-t", &target, &keys])
            .output()
            .map_err(|e| e.to_string())?
    } else {
        cmd("tmux")
            .args(["send-keys", "-t", &target, "--", &keys, "Enter"])
            .output()
            .map_err(|e| e.to_string())?
    };

    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).to_string())
    }
}
