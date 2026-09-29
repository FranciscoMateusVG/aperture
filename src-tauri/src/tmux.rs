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
pub fn tmux_create_session(session_name: String, runtime: tauri::State<'_, std::sync::Arc<crate::daemons::RuntimeOwner>>) -> Result<String, String> {
    let work = runtime.admit(None)?;
    let _body = work.body()?;
    tmux_create_session_shared(session_name, &work)
}
pub(crate) fn tmux_create_session_shared(session_name: String, work: &crate::daemons::RuntimeWork) -> Result<String, String> {
    work.check_open()?;
    if !crate::daemon_registry::valid_name(&session_name) {return Err("E_RUNTIME_SELECTOR".into());}
    let rows=local_output(work, vec!["list-sessions".into(),"-F".into(),"#{session_name}".into()])?;
    let text=std::str::from_utf8(&rows).map_err(|_|"E_TMUX_UNVERIFIED")?;
    if text.lines().any(|s|!crate::daemon_registry::valid_name(s)) {return Err("E_TMUX_UNVERIFIED".into());}
    if text.lines().any(|s|s==session_name) {return Ok("already exists".into());}
    // Only a successful complete list from the existing server proves absence.
    // Nonzero/timeout/no-server is never a request to create a new server.
    local_output(work,vec!["new-session".into(),"-d".into(),"-s".into(),session_name.clone()])?;
    local_output(work,vec!["set-option".into(),"-t".into(),session_name.clone(),"mouse".into(),"on".into()])?;
    local_output(work,vec!["set-option".into(),"-t".into(),session_name,"history-limit".into(),"50000".into()])?;
    Ok("created".into())
}
pub(crate) fn local_output(work:&crate::daemons::RuntimeWork,args:Vec<String>)->Result<Vec<u8>,String>{
    let tool=&work.tools()?.tmux;
    let input=work.client(tool,args)?;
    let out=crate::daemons::run_client(work,input,std::time::Duration::from_secs(3),1024*1024,64*1024)?;
    if !out.accepted {return Err("E_TMUX_OUTCOME_UNKNOWN".into());}
    Ok(out.stdout)
}
pub(crate) fn list_windows_local(session:&str,work:&crate::daemons::RuntimeWork)->Result<Vec<WindowInfo>,String>{
    if !crate::daemon_registry::valid_name(session){return Err("E_RUNTIME_SELECTOR".into());}
    let bytes=local_output(work,vec!["list-windows".into(),"-t".into(),session.into(),"-F".into(),"#{window_id}||#{window_name}||#{pane_current_command}".into()])?;
    let text=std::str::from_utf8(&bytes).map_err(|_|"E_TMUX_UNVERIFIED")?;
    let mut windows=Vec::new();
    for line in text.lines(){
        let fields=line.split("||").collect::<Vec<_>>();
        if fields.len()!=3 || !window_selector(fields[0]) || windows.len()>=512 {return Err("E_TMUX_UNVERIFIED".into());}
        windows.push(WindowInfo{window_id:fields[0].into(),name:fields[1].into(),command:fields[2].into()});
    }
    Ok(windows)
}
fn window_selector(s:&str)->bool{s.len()>1 && s.len()<=16 && s.starts_with('@') && s[1..].bytes().all(|v|v.is_ascii_digit())}
pub(crate) fn create_window_local(session:&str,name:&str,work:&crate::daemons::RuntimeWork)->Result<String,String>{
    if !crate::daemon_registry::valid_name(session)||!crate::daemon_registry::valid_name(name){return Err("E_RUNTIME_SELECTOR".into());}
    let bytes=local_output(work,vec!["new-window".into(),"-t".into(),session.into(),"-n".into(),name.into(),"-P".into(),"-F".into(),"#{window_id}".into()])?;
    let window=std::str::from_utf8(&bytes).map_err(|_|"E_TMUX_UNVERIFIED")?.trim_end_matches('\n');
    if !window_selector(window){return Err("E_TMUX_UNVERIFIED".into());}Ok(window.into())
}
pub(crate) fn send_local(window:&str,text:&str,work:&crate::daemons::RuntimeWork)->Result<(),String>{
    if !window_selector(window)||text.len()>65536{return Err("E_RUNTIME_SELECTOR".into());}
    local_output(work,vec!["send-keys".into(),"-t".into(),window.into(),"-l".into(),text.into()])?;
    local_output(work,vec!["send-keys".into(),"-t".into(),window.into(),"Enter".into()])?;Ok(())
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
pub fn tmux_select_window(window_id: String, runtime: tauri::State<'_, std::sync::Arc<crate::daemons::RuntimeOwner>>) -> Result<(), String> {
    let work = runtime.admit(None)?;
    let _body = work.body()?;
    tmux_select_window_shared(window_id, &work)
}
pub(crate) fn tmux_select_window_shared(window_id: String, work: &crate::daemons::RuntimeWork) -> Result<(), String> {
    work.check_open()?;
    if !window_selector(&window_id){return Err("E_RUNTIME_SELECTOR".into());}
    local_output(work,vec!["select-window".into(),"-t".into(),window_id])?;Ok(())
}

pub fn tmux_capture_pane(window_id: &str) -> Result<String, String> {
    let output = cmd("tmux")
        .args(["capture-pane", "-t", window_id, "-p"])
        .output()
        .map_err(|e| e.to_string())?;
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
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
        // Use -l (literal) to paste text verbatim — without it, Claude Code's
        // TUI input handler may not receive the characters correctly.
        // Then send Enter as a separate send-keys call so tmux interprets it
        // as the Enter key rather than literal text.
        let text_output = cmd("tmux")
            .args(["send-keys", "-t", &target, "-l", &keys])
            .output()
            .map_err(|e| e.to_string())?;

        if !text_output.status.success() {
            return Err(String::from_utf8_lossy(&text_output.stderr).to_string());
        }

        cmd("tmux")
            .args(["send-keys", "-t", &target, "Enter"])
            .output()
            .map_err(|e| e.to_string())?
    };

    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).to_string())
    }
}

/// Exact native pane locator for V4 ownership capture. A window containing
/// multiple panes is not silently reduced to whichever pane happens to be
/// active. Process birth identity is obtained separately from the OS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneProcess {
    pub window_id: String,
    pub pane_id: String,
    pub pid: u32,
}
fn exact_tmux_id(value: &str, prefix: char) -> bool {
    value.starts_with(prefix)
        && value.len() > 1
        && value.len() <= 21
        && value[1..].bytes().all(|b| b.is_ascii_digit())
}
pub(crate) fn parse_pane_process(window_id: &str, output: &[u8]) -> Result<PaneProcess, String> {
    let deny = || "E_STOP_UNVERIFIED".to_string();
    if !exact_tmux_id(window_id, '@') || output.len() > 1024 {
        return Err(deny());
    }
    let text = std::str::from_utf8(output).map_err(|_| deny())?;
    let rows: Vec<_> = text.lines().collect();
    if rows.len() != 1 {
        return Err(deny());
    }
    let fields: Vec<_> = rows[0].split('\t').collect();
    if fields.len() != 3 || fields[0] != window_id || !exact_tmux_id(fields[1], '%') {
        return Err(deny());
    }
    let pid: u32 = fields[2].parse().map_err(|_| deny())?;
    if pid <= 1 || pid > i32::MAX as u32 || fields[2] != pid.to_string() {
        return Err(deny());
    }
    Ok(PaneProcess {
        window_id: window_id.into(),
        pane_id: fields[1].into(),
        pid,
    })
}
/// Read-only native command. Never falls back to a name, active pane, command,
/// cwd, cached AgentDef or another window after an unavailable/ambiguous result.
pub fn tmux_pane_process(window_id: &str) -> Result<PaneProcess, String> {
    if !exact_tmux_id(window_id, '@') {
        return Err("E_STOP_UNVERIFIED".into());
    }
    let out = cmd("tmux")
        .args([
            "list-panes",
            "-t",
            window_id,
            "-F",
            "#{window_id}\t#{pane_id}\t#{pane_pid}",
        ])
        .output()
        .map_err(|_| "E_STOP_UNVERIFIED".to_string())?;
    if !out.status.success() {
        return Err("E_STOP_UNVERIFIED".into());
    }
    parse_pane_process(window_id, &out.stdout)
}
