use serde::{Deserialize, Serialize};
use std::fs;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Objective {
    pub id: String,
    pub title: String,
    pub description: String,
    pub spec: Option<String>,
    pub status: String, // draft, speccing, ready, approved, in_progress, done
    pub priority: u8,
    pub task_ids: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
}

fn objectives_path() -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    format!("{}/.aperture/objectives.json", home)
}

fn read_objectives() -> Vec<Objective> {
    match fs::read_to_string(objectives_path()) {
        Ok(data) => serde_json::from_str(&data).unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

fn write_objectives(objectives: &[Objective]) -> Result<(), String> {
    let json = serde_json::to_string_pretty(objectives).map_err(|e| e.to_string())?;
    fs::write(objectives_path(), json).map_err(|e| e.to_string())
}

fn now_iso() -> String {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    // Simple ISO-ish timestamp from millis
    format!("{}", ts)
}

fn gen_id() -> String {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    format!("obj-{}", &format!("{:x}", ts)[6..])
}

#[tauri::command]
pub fn list_objectives() -> Result<Vec<Objective>, String> {
    Ok(read_objectives())
}

#[tauri::command]
pub fn create_objective(title: String, description: String, priority: u8) -> Result<Objective, String> {
    let mut objectives = read_objectives();
    let now = now_iso();
    let obj = Objective {
        id: gen_id(),
        title,
        description,
        spec: None,
        status: "draft".into(),
        priority,
        task_ids: Vec::new(),
        created_at: now.clone(),
        updated_at: now,
    };
    objectives.push(obj.clone());
    write_objectives(&objectives)?;
    Ok(obj)
}

#[tauri::command]
pub fn update_objective(
    id: String,
    title: Option<String>,
    description: Option<String>,
    spec: Option<String>,
    status: Option<String>,
    priority: Option<u8>,
    task_ids: Option<Vec<String>>,
) -> Result<Objective, String> {
    let mut objectives = read_objectives();
    let obj = objectives
        .iter_mut()
        .find(|o| o.id == id)
        .ok_or(format!("Objective '{}' not found", id))?;

    if let Some(t) = title { obj.title = t; }
    if let Some(d) = description { obj.description = d; }
    if let Some(s) = spec { obj.spec = Some(s); }
    if let Some(st) = status { obj.status = st; }
    if let Some(p) = priority { obj.priority = p; }
    if let Some(ids) = task_ids { obj.task_ids = ids; }
    obj.updated_at = now_iso();

    let updated = obj.clone();
    write_objectives(&objectives)?;
    Ok(updated)
}

#[tauri::command]
pub fn delete_objective(id: String) -> Result<(), String> {
    let mut objectives = read_objectives();
    let len_before = objectives.len();
    objectives.retain(|o| o.id != id);
    if objectives.len() == len_before {
        return Err(format!("Objective '{}' not found", id));
    }
    write_objectives(&objectives)
}

#[tauri::command]
pub fn open_file(path: String) -> Result<(), String> {
    std::process::Command::new("open")
        .arg(&path)
        .output()
        .map_err(|e| format!("Failed to open file: {}", e))?;
    Ok(())
}
