use std::process::Command;

fn bd_cmd() -> Command {
    let mut c = Command::new("bd");
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let current_path = std::env::var("PATH").unwrap_or_default();
    c.env(
        "PATH",
        format!("/opt/homebrew/bin:/usr/local/bin:{}", current_path),
    );
    c.env("BEADS_DIR", format!("{}/.aperture/.beads", home));
    c
}

#[tauri::command]
pub fn list_beads_tasks() -> Result<serde_json::Value, String> {
    let output = bd_cmd()
        .args(["list", "--json", "--all"])
        .output()
        .map_err(|e| format!("Failed to run bd: {}", e))?;

    if !output.status.success() {
        return Ok(serde_json::json!([]));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    match serde_json::from_str(&stdout) {
        Ok(v) => Ok(v),
        Err(_) => Ok(serde_json::json!([])),
    }
}

#[tauri::command]
pub fn update_beads_task_status(task_id: String, status: String) -> Result<(), String> {
    let output = bd_cmd()
        .args(["update", &task_id, "--status", &status, "--quiet"])
        .output()
        .map_err(|e| format!("Failed to run bd: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("bd update failed: {}", stderr));
    }

    Ok(())
}
