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
/// `new-window` argv with a SESSION-ONLY target (`<session>:`). A bare
/// `<session>` is resolved by tmux as a window target with name-prefix
/// matching, so any window whose name starts with the session name (e.g.
/// `aperture-web-backend-g1` in session `aperture`) makes tmux try that
/// window's index and fail with "index N in use" (aperture-fr859, live
/// failure 2026-09-29). The trailing colon names the session and lets tmux
/// allocate the next free index; no `-a`, so existing windows are never
/// shifted or renumbered.
pub(crate) fn new_window_args(session:&str,name:&str)->Vec<String>{
    vec!["new-window".into(),"-t".into(),format!("{session}:"),"-n".into(),name.into(),"-P".into(),"-F".into(),"#{window_id}".into()]
}
/// Validate a `new-window -P -F '#{window_id}'` outcome into a window id.
/// tmux stderr (which carries indices, names and paths) is never reflected.
pub(crate) fn new_window_result(out:std::process::Output)->Result<String,String>{
    if !out.status.success(){return Err("E_TMUX_UNVERIFIED".into());}
    window_id_from_stdout(&out.stdout)
}
fn window_id_from_stdout(bytes:&[u8])->Result<String,String>{
    let window=std::str::from_utf8(bytes).map_err(|_|"E_TMUX_UNVERIFIED")?.trim_end_matches('\n');
    if !window_selector(window){return Err("E_TMUX_UNVERIFIED".into());}Ok(window.into())
}
pub(crate) fn create_window_local(session:&str,name:&str,work:&crate::daemons::RuntimeWork)->Result<String,String>{
    if !crate::daemon_registry::valid_name(session)||!crate::daemon_registry::valid_name(name){return Err("E_RUNTIME_SELECTOR".into());}
    let bytes=local_output(work,new_window_args(session,name))?;
    window_id_from_stdout(&bytes)
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
    if !crate::daemon_registry::valid_name(&session_name) || !crate::daemon_registry::valid_name(&window_name) {
        return Err("E_RUNTIME_SELECTOR".into());
    }
    // Same session-only target and stderr-free outcome as create_window_local
    // (aperture-fr859): the raw tmux message ("index 6 in use", paths) used
    // to be returned verbatim and surfaced in the UI.
    let output = cmd("tmux")
        .args(new_window_args(&session_name, &window_name))
        .output()
        .map_err(|_| "E_TMUX_UNVERIFIED".to_string())?;
    new_window_result(output)
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

#[cfg(test)]
mod window_target_tests {
    use super::*;
    use std::process::{Command, Output};
    use std::sync::{Arc, Mutex};

    /// Pure argv contract: a session-only target (`<session>:`) lets tmux pick
    /// the next free index; a bare `<session>` is resolved as a WINDOW target
    /// with name-prefix matching and collides with any window whose name
    /// starts with the session name ("index N in use"). No `-a`: never
    /// renumber or shift existing windows.
    #[test]
    fn new_window_args_use_session_only_target_without_renumbering() {
        let args = new_window_args("aperture", "glados");
        assert_eq!(
            args,
            vec!["new-window", "-t", "aperture:", "-n", "glados", "-P", "-F", "#{window_id}"]
        );
        assert!(!args.iter().any(|a| a == "-a"));
    }

    #[test]
    fn new_window_result_never_reflects_tmux_stderr() {
        let failed = Output {
            status: exit_status(1),
            stdout: Vec::new(),
            stderr: b"create window failed: index 6 in use\n".to_vec(),
        };
        let err = new_window_result(failed).unwrap_err();
        assert_eq!(err, "E_TMUX_UNVERIFIED");
        assert!(!err.contains("index"));
    }

    #[test]
    fn new_window_result_returns_validated_window_id() {
        let ok = Output { status: exit_status(0), stdout: b"@12\n".to_vec(), stderr: Vec::new() };
        assert_eq!(new_window_result(ok).unwrap(), "@12");
        let garbage = Output { status: exit_status(0), stdout: b"12\n".to_vec(), stderr: Vec::new() };
        assert_eq!(new_window_result(garbage).unwrap_err(), "E_TMUX_UNVERIFIED");
    }

    fn exit_status(code: i32) -> std::process::ExitStatus {
        Command::new("sh").arg("-c").arg(format!("exit {code}")).status().unwrap()
    }

    /// Private tmux server (own `-L` socket, never the shared `aperture`
    /// daemon). Killed on drop AND by a deadline thread so a hung tmux can
    /// never outlive the test.
    struct PrivateTmux {
        socket: String,
        /// Synthetic private HOME so the server never sees the operator's
        /// real ~/.tmux.conf or state; removed on drop.
        home: std::path::PathBuf,
        killed: Arc<Mutex<bool>>,
    }
    /// Every invocation: `-f /dev/null` (no config file at all) + `-L` private
    /// socket + synthetic HOME. The shared `aperture` server is never addressed.
    fn private_tmux(socket: &str, home: &std::path::Path) -> Command {
        let mut c = Command::new("tmux");
        c.args(["-f", "/dev/null", "-L", socket]).env("HOME", home).env_remove("TMUX");
        c
    }
    impl PrivateTmux {
        fn start() -> Option<Self> {
            Command::new("tmux").arg("-V").output().ok().filter(|o| o.status.success())?;
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0);
            let socket = format!("fr859-{}-{nanos}", std::process::id());
            let home = std::env::temp_dir().join(format!("aperture-{socket}-home"));
            {
                use std::os::unix::fs::DirBuilderExt;
                std::fs::DirBuilder::new().mode(0o700).create(&home).ok()?;
            }
            let killed = Arc::new(Mutex::new(false));
            let (deadline_socket, deadline_home, deadline_flag) = (socket.clone(), home.clone(), Arc::clone(&killed));
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(20));
                if !*deadline_flag.lock().unwrap() {
                    let _ = private_tmux(&deadline_socket, &deadline_home).arg("kill-server").output();
                }
            });
            Some(Self { socket, home, killed })
        }
        fn run(&self, args: &[&str]) -> Output {
            private_tmux(&self.socket, &self.home).args(args).output().expect("spawn tmux")
        }
        fn ok(&self, args: &[&str]) -> String {
            let out = self.run(args);
            assert!(out.status.success(), "tmux {args:?} failed: {}", String::from_utf8_lossy(&out.stderr));
            String::from_utf8(out.stdout).unwrap()
        }
        fn windows(&self) -> Vec<(String, String, String)> {
            self.ok(&["list-windows", "-t", "aperture", "-F", "#{window_index}||#{window_id}||#{window_name}"])
                .lines()
                .map(|l| {
                    let f: Vec<&str> = l.split("||").collect();
                    (f[0].into(), f[1].into(), f[2].into())
                })
                .collect()
        }
    }
    impl Drop for PrivateTmux {
        fn drop(&mut self) {
            *self.killed.lock().unwrap() = true;
            let _ = self.run(&["kill-server"]);
            let _ = std::fs::remove_dir_all(&self.home);
        }
    }

    /// Isolated native regression for the live failure (operator 2026-09-29):
    /// session `aperture` already had window 6 named `aperture-web-backend-g1`;
    /// `new-window -t aperture` matched that window by name prefix and died
    /// with "index 6 in use". The session-only target must allocate a fresh
    /// index and leave every existing window's index/id/name untouched.
    #[test]
    fn new_window_survives_window_named_with_session_prefix() {
        let Some(t) = PrivateTmux::start() else {
            eprintln!("skip: tmux not available");
            return;
        };
        t.ok(&["new-session", "-d", "-s", "aperture", "-n", "w0"]);
        t.ok(&["new-window", "-t", "aperture:", "-n", "aperture-web-backend-g1"]);
        t.ok(&["select-window", "-t", "aperture:0"]);
        let before = t.windows();
        assert_eq!(before.len(), 2);

        // The bare-session form is the bug: it must fail on this layout, so the
        // test is proven to exercise the collision and not a trivially empty session.
        let bare = t.run(&["new-window", "-t", "aperture", "-n", "glados", "-P", "-F", "#{window_id}"]);
        assert!(!bare.status.success(), "bare -t <session> unexpectedly succeeded");
        assert_eq!(t.windows(), before, "failed bare form must not mutate the session");

        let args = new_window_args("aperture", "glados");
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        let window = new_window_result(t.run(&refs)).expect("session-only target creates the window");

        let after = t.windows();
        assert_eq!(after.len(), before.len() + 1);
        for row in &before {
            assert!(after.contains(row), "existing window changed: {row:?}");
        }
        let created = after.iter().find(|(_, id, _)| *id == window).expect("new window listed");
        assert_eq!(created.2, "glados");
        assert!(!before.iter().any(|(idx, _, _)| *idx == created.0), "new index must be free");
    }
}
