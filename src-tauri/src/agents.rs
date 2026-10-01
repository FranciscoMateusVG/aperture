use crate::codex_appserver;
use crate::config;
use crate::hub_auth;
use crate::launcher;
use crate::state::AppState;
use crate::tmux;
use std::collections::HashMap;
use std::fs;
use std::process::Command;
use std::sync::{Arc, Mutex};

use crate::state::AgentDef;

#[path = "team_legacy_guard.rs"]
pub(crate) mod legacy_lifecycle_guard;
#[path = "coordinator_prompt.rs"]
mod coordinator_prompt;
#[path = "coordinator_lifecycle.rs"]
pub(crate) mod coordinator_lifecycle;

pub(crate) fn require_legacy_lifecycle_at(
    home: &std::path::Path,
    agents_root: &std::path::Path,
    name: &str,
) -> Result<(), String> {
    let membership = match crate::teams::classify_managed_seat(home, name) {
        Ok(None) => legacy_lifecycle_guard::Membership::Standing,
        Ok(Some(_)) => legacy_lifecycle_guard::Membership::Team,
        Err(_) => legacy_lifecycle_guard::Membership::Unknown,
    };
    legacy_lifecycle_guard::ensure_legacy(agents_root, name, membership)
}

/// Authoritative native classification, independent of UI visibility or cached
/// AppState. Only a proven standing seat may enter legacy lifecycle effects.
pub(crate) fn require_legacy_lifecycle(name: &str) -> Result<(), String> {
    let home = std::env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .map(std::path::PathBuf::from)
        .ok_or_else(|| legacy_lifecycle_guard::DENIED.to_string())?;
    let agents_root = std::env::var_os("APERTURE_AGENTS_DIR")
        .filter(|v| !v.is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| home.join(".claude/aperture"));
    require_legacy_lifecycle_at(&home, &agents_root, name)
}

// C3 is a fenced caller boundary, not permission to activate Codex.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LifecycleRefusal { InputsUnverified, PlanChanged, ContextMismatch }
impl LifecycleRefusal {
    pub(crate) fn code(self) -> &'static str { match self {
        Self::InputsUnverified => "E_CODEX_LAUNCH_INPUTS_UNVERIFIED",
        Self::PlanChanged => "E_LIFECYCLE_PLAN_CHANGED",
        Self::ContextMismatch => "E_LIFECYCLE_CONTEXT_MISMATCH",
    } }
}
pub(crate) struct LifecycleContext<'a> {
    lease: &'a crate::controller::ControllerLock,
    work: Option<&'a crate::daemons::RuntimeWork>,
    home: std::path::PathBuf,
    roots: std::path::PathBuf,
    #[cfg(test)] pub(crate) fixture: Option<&'a LifecycleFixture>,
}
impl<'a> LifecycleContext<'a> {
    pub(crate) fn new(lease: &'a crate::controller::ControllerLock) -> Result<Self, String> {
        let home = lease.run_dir()?.parent().and_then(std::path::Path::parent)
            .ok_or(LifecycleRefusal::ContextMismatch.code())?.to_path_buf();
        let roots = std::env::var_os("APERTURE_AGENTS_DIR").filter(|v| !v.is_empty())
            .map(std::path::PathBuf::from).unwrap_or_else(|| home.join(".claude/aperture"));
        Ok(Self { roots, home, lease, work: None,
            #[cfg(test)] fixture: None })
    }
    #[cfg(test)]
    pub(crate) fn fixture_context(lease: &'a crate::controller::ControllerLock) -> Result<Self, String> {
        let mut context = Self::new(lease)?;
        context.roots = context.home.join(".claude/aperture");
        Ok(context)
    }
    fn require_tools(&self) -> Result<(), String> {
        #[cfg(test)] if self.fixture.is_some() { return Ok(()); }
        self.work.ok_or("E_RUNTIME_TOOLS_UNVERIFIED")?.require_tools()
    }
    pub(crate) fn accounted(mut self, work: &'a crate::daemons::RuntimeWork) -> Self {
        self.work = Some(work); self
    }
    fn check_open(&self) -> Result<(), String> {
        self.lease.verify_live()?;
        if let Some(work) = self.work { work.check_open()?; }
        Ok(())
    }
    fn classify(&self, name: &str) -> Result<(), String> {
        self.check_open()?;
        if let Some(work) = self.work { work.check_seat(name)?; }
        require_legacy_lifecycle_at(&self.home, &self.roots, name)
    }
    fn has_codex_history(&self, name: &str) -> Result<bool, String> {
        if !crate::daemon_registry::valid_name(name) { return Err(LifecycleRefusal::ContextMismatch.code().into()); }
        // Read-only rejection: any leaf (including malformed/symlink) is history,
        // never enrollment or adoption. No Registry::open on a denial path.
        let root = self.lease.run_dir()?.join("daemons");
        match fs::symlink_metadata(&root) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(_) => return Err(LifecycleRefusal::InputsUnverified.code().into()),
            Ok(_) => crate::controller::private_dir_readonly(&root)?,
        }
        match fs::symlink_metadata(root.join(format!("codex-{name}"))) {
            Ok(_) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(_) => Err(LifecycleRefusal::InputsUnverified.code().into()),
        }
    }
}
pub(crate) fn detached_codex_denied_at(home: &std::path::Path, state: &Arc<Mutex<AppState>>, name: &str, is_codex: bool) -> Result<(), String> {
    require_legacy_lifecycle_at(home, &home.join(".claude/aperture"), name)?;
    let agent = state.lock().map_err(|_| LifecycleRefusal::PlanChanged.code())?.agents.get(name)
        .ok_or(LifecycleRefusal::PlanChanged.code())?.clone();
    if is_codex || agent.model.starts_with("codex/") {
        return Err(LifecycleRefusal::InputsUnverified.code().into());
    }
    let history = home.join(".aperture/run/daemons").join(format!("codex-{name}"));
    match fs::symlink_metadata(history) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        _ => Err(LifecycleRefusal::InputsUnverified.code().into()),
    }
}
fn same_lifecycle_plan(a: &AgentDef, b: &AgentDef) -> bool {
    a.name == b.name && a.model == b.model && a.role == b.role && a.prompt_file == b.prompt_file
        && a.status == b.status && a.tmux_window_id == b.tmux_window_id
}
fn verify_plan(state: &Arc<Mutex<AppState>>, expected: &AgentDef) -> Result<(), String> {
    let state = state.lock().map_err(|_| LifecycleRefusal::PlanChanged.code())?;
    if !state.agents.get(&expected.name).is_some_and(|v| same_lifecycle_plan(v, expected)) {
        return Err(LifecycleRefusal::PlanChanged.code().into());
    }
    Ok(())
}
#[derive(Clone)]
pub(crate) enum LifecycleAction { Start, Stop, Restart, Model(String) }
fn with_lifecycle_plan(
    name: &str, state: &Arc<Mutex<AppState>>, context: &LifecycleContext<'_>, action: LifecycleAction,
    execute: impl FnOnce(&AgentDef, &mut crate::controller::CodexOperation<'_>) -> Result<(), String>,
) -> Result<(), String> {
    context.classify(name)?;
    let advisory = state.lock().map_err(|e| e.to_string())?.agents.get(name)
        .ok_or_else(|| format!("Agent '{name}' not found"))?.clone();
    #[cfg(test)]
    if let Some(pause) = context.fixture.and_then(|f| f.pause.as_ref()) {
        pause.arrived.send(()).map_err(|_| "E_FIXTURE_CHANNEL")?;
        pause.resume.lock().unwrap().recv_timeout(std::time::Duration::from_secs(3)).map_err(|_| "E_FIXTURE_DEADLINE")?;
    }
    // NEVER wait on a seat while holding AppState.
    let slot = context.lease.codex_slot(name)?;
    let mut op = slot.enter()?;
    context.classify(name)?;
    verify_plan(state, &advisory)?;
    if let LifecycleAction::Model(ref requested) = action {
        if requested == &advisory.model { return Ok(()); } // verified effect-free no-op, not persistence
    }
    let codex = advisory.model.starts_with("codex/")
        || matches!(&action, LifecycleAction::Model(v) if v.starts_with("codex/"));
    let history = context.has_codex_history(name)?;
    if context.work.is_some_and(|w| w.tools().is_ok()) {
        coordinator_lifecycle::check_start(context.lease, name)?;
        if matches!(action,LifecycleAction::Stop|LifecycleAction::Restart) {
            return local_stop_or_restart(state,&advisory,context,&mut op,matches!(action,LifecycleAction::Restart));
        }
    }
    if codex || history {
        if matches!(action,LifecycleAction::Start) && advisory.model.starts_with("codex/")
            && context.work.is_some_and(|w|w.tools().is_ok()) {
            return start_codex_local(state,&advisory,context,&mut op);
        }
        #[cfg(test)]
        if matches!(action, LifecycleAction::Start) && !history {
            if let Some(fixture) = context.fixture.filter(|f| f.spec.is_some()) {
                let registry = crate::daemon_registry::Registry::open(context.lease)?;
                let plan = PreparedCaller { state, expected: &advisory, fixture };
                let spec = fixture.spec.as_ref().unwrap();
                let supervisor = crate::codex_appserver::NativeCodexSupervisor::new(context.lease, &registry, spec.copy_fixture())?;
                supervisor.prepared_fixture(&mut op, &plan)?;
                return Ok(());
            }
        }
        return Err(LifecycleRefusal::InputsUnverified.code().into());
    }
    verify_plan(state, &advisory)?;
    execute(&advisory, &mut op)
}

// Production preparation is borrowed by the SAME held supervisor operation.
// Existing login/home is an explicit prerequisite: never copy credentials or
// manufacture trust/auth in order to make a fresh installation look ready.
pub(crate) struct LocalCodexPreparation<'a> {
    context: &'a LifecycleContext<'a>, state:&'a Arc<Mutex<AppState>>, expected:&'a AgentDef,
    codex_home:std::path::PathBuf, bus:String, sentry:String, project:String,
    bus_pin:LocalInputPin, sentry_pin:LocalInputPin,
    #[cfg(test)] drift_point: Option<&'static str>,
}
impl LocalCodexPreparation<'_> {
    pub(crate) fn seat(&self)->&str{&self.expected.name}
    pub(crate) fn recheck(&self)->Result<(),String>{
        self.context.classify(&self.expected.name)?;self.context.require_tools()?;
        verify_plan(self.state,self.expected)?;
        crate::controller::private_dir_readonly(&self.codex_home)?;
        let state=self.state.lock().map_err(|_|"E_RUNTIME_STATE")?;
        if state.mcp_server_path!=self.bus || state.mcp_sentry_server_path!=self.sentry || state.project_dir!=self.project {
            return Err(LifecycleRefusal::PlanChanged.code().into());
        }
        self.bus_pin.recheck(std::path::Path::new(&self.bus))?;
        self.sentry_pin.recheck(std::path::Path::new(&self.sentry))?;
        Ok(())
    }
    fn configuration(&self)->Result<(toml::Value,LocalInputPin),String>{
        self.recheck()?;
        let path=self.codex_home.join("config.toml");
        let (bytes,pin)=local_pinned_bytes(&path,256*1024)?;
        use std::os::unix::fs::MetadataExt;
        if pin.mode&0o777!=0o600 {
            return Err("E_LOCAL_CODEX_CONFIG_PRIVATE_REQUIRED".into());
        }
        let config:toml::Value=std::str::from_utf8(&bytes).map_err(|_|"E_LOCAL_CODEX_CONFIG")?.parse().map_err(|_|"E_LOCAL_CODEX_CONFIG")?;
        let table=config.as_table().ok_or("E_LOCAL_CODEX_CONFIG")?;
        let servers=table.get("mcp_servers").and_then(toml::Value::as_table).ok_or("E_LOCAL_CODEX_CONFIG")?;
        for name in ["aperture-bus","sentry"] {
            let server=servers.get(name).and_then(toml::Value::as_table).ok_or("E_LOCAL_CODEX_CONFIG")?;
            if server.get("env").is_some_and(|v|!v.is_table()){return Err("E_LOCAL_CODEX_CONFIG".into());}
        }
        Ok((config,pin))
    }
    pub(crate) fn prepare(&self)->Result<(),String>{
        let (mut config,config_pin)=self.configuration()?;
        let config_path=self.codex_home.join("config.toml");
        let table=config.as_table_mut().ok_or("E_LOCAL_CODEX_CONFIG")?;
        // Preserve operator trust/approvals/provider configuration verbatim in
        // value; only the selected model and local MCP executable paths change.
        table.insert("model".into(),toml::Value::String(self.expected.model.trim_start_matches("codex/").into()));
        let tools=self.context.work.ok_or("E_RUNTIME_TOOLS_UNVERIFIED")?.tools()?;
        let servers=table.get_mut("mcp_servers").and_then(toml::Value::as_table_mut).ok_or("E_LOCAL_CODEX_CONFIG")?;
        for name in ["aperture-bus","sentry"]{
            let server=servers.get(name).and_then(toml::Value::as_table).ok_or("E_LOCAL_CODEX_CONFIG")?;
            if server.get("env").is_some_and(|v|!v.is_table()){return Err("E_LOCAL_CODEX_CONFIG".into());}
        }
        // All shape/path/plan checks precede token rotation and the config write.
        self.recheck()?;
        #[cfg(test)] lifecycle_tests::local_config_drift(self,"before-token");
        config_pin.recheck(&config_path)?;
        let token=hub_auth::provision_under_lease(self.context.lease,&self.expected.name)?;
        for (name,path) in [("aperture-bus",&self.bus),("sentry",&self.sentry)] {
            let server=servers.get_mut(name).and_then(toml::Value::as_table_mut).ok_or("E_LOCAL_CODEX_CONFIG")?;
            server.insert("command".into(),toml::Value::String(tools.node.path.to_string_lossy().into_owned()));
            server.insert("args".into(),toml::Value::Array(vec![toml::Value::String(path.clone())]));
            let env=server.entry("env").or_insert_with(||toml::Value::Table(Default::default())).as_table_mut().ok_or("E_LOCAL_CODEX_CONFIG")?;
            for (key,value) in tools.environment(){env.insert(key,toml::Value::String(value));}
            if name=="aperture-bus"{env.insert("APERTURE_HUB_TOKEN_FILE".into(),toml::Value::String(token.to_string_lossy().into_owned()));}
        }
        let output=toml::to_string(&config).map_err(|_|"E_LOCAL_CODEX_CONFIG")?;
        self.recheck()?;
        #[cfg(test)] lifecycle_tests::local_config_drift(self,"before-replace");
        config_pin.recheck(&config_path)?;
        crate::journal::write_private_bytes_atomic(&config_path,output.as_bytes(),true)
    }
}
#[derive(Clone,Debug,PartialEq,Eq)]
struct LocalInputPin {dev:u64,ino:u64,uid:u32,mode:u32,nlink:u64,len:u64,mtime:(i64,i64),ctime:(i64,i64)}
impl LocalInputPin {
    fn of(m:&fs::Metadata)->Self {use std::os::unix::fs::MetadataExt;Self{dev:m.dev(),ino:m.ino(),uid:m.uid(),mode:m.mode(),nlink:m.nlink(),len:m.len(),mtime:(m.mtime(),m.mtime_nsec()),ctime:(m.ctime(),m.ctime_nsec())}}
    fn recheck(&self,path:&std::path::Path)->Result<(),String>{
        let f=local_input_fd(path,self.len)?;
        if Self::of(&f.metadata().map_err(|_|"E_LOCAL_INPUT_UNVERIFIED")?)!=*self {return Err("E_LOCAL_INPUT_DRIFT".into());}Ok(())
    }
}
fn local_input_fd(path:&std::path::Path,cap:u64)->Result<fs::File,String>{
    use std::os::unix::fs::{OpenOptionsExt,MetadataExt};
    let file=fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW|libc::O_NONBLOCK|libc::O_CLOEXEC).open(path).map_err(|_|"E_LOCAL_INPUT_MISSING")?;
    let m=file.metadata().map_err(|_|"E_LOCAL_INPUT_UNVERIFIED")?;
    if !m.is_file() || m.nlink()!=1 || m.uid()!=unsafe{libc::geteuid()} || m.mode()&0o7022!=0 || m.mode()&0o400==0 || m.len()>cap {return Err("E_LOCAL_INPUT_UNVERIFIED".into());}Ok(file)
}
fn local_pinned_bytes(path:&std::path::Path,cap:u64)->Result<(Vec<u8>,LocalInputPin),String>{
    use std::io::Read;
    let mut file=local_input_fd(path,cap)?;
    let pin=LocalInputPin::of(&file.metadata().map_err(|_|"E_LOCAL_INPUT_UNVERIFIED")?);
    let mut bytes=Vec::new();(&mut file).take(cap+1).read_to_end(&mut bytes).map_err(|_|"E_LOCAL_INPUT_UNVERIFIED")?;
    if bytes.len() as u64!=pin.len || bytes.len() as u64>cap || LocalInputPin::of(&file.metadata().map_err(|_|"E_LOCAL_INPUT_UNVERIFIED")?)!=pin {return Err("E_LOCAL_INPUT_DRIFT".into());}
    pin.recheck(path)?;Ok((bytes,pin))
}
fn local_regular_bytes(path:&std::path::Path,cap:u64)->Result<Vec<u8>,String>{local_pinned_bytes(path,cap).map(|(bytes,_)|bytes)}
fn shell_quote(value:&str)->String{format!("'{}'",value.replace('\'',"'\\''"))}
fn wait_local_thread(path:&std::path::Path,work:&crate::daemons::RuntimeWork)->Result<String,String>{
    let until=std::time::Instant::now()+std::time::Duration::from_secs(60);
    loop {
        work.check_open()?;
        match fs::symlink_metadata(path){
            Err(e) if e.kind()==std::io::ErrorKind::NotFound=>{},
            Ok(_)=>{
                let bytes=local_regular_bytes(path,128)?;
                // The bridge publishes a canonical UUID followed by ONE LF.
                // Accept legacy bare UUIDs too, but never trim arbitrary input.
                let payload=bytes.strip_suffix(b"\n").unwrap_or(&bytes);
                let thread=std::str::from_utf8(payload).map_err(|_|"E_LOCAL_THREAD_UNVERIFIED")?;
                let id=uuid::Uuid::parse_str(thread).map_err(|_|"E_LOCAL_THREAD_UNVERIFIED")?;
                if id.to_string()!=thread{return Err("E_LOCAL_THREAD_UNVERIFIED".into());}
                return Ok(thread.into());
            },
            Err(_)=>return Err("E_LOCAL_THREAD_UNVERIFIED".into()),
        }
        if std::time::Instant::now()>=until{return Err("E_LOCAL_THREAD_UNVERIFIED".into());}
        work.wait_open(std::time::Duration::from_millis(50))?;
    }
}

fn start_codex_local(state:&Arc<Mutex<AppState>>,expected:&AgentDef,context:&LifecycleContext<'_>,op:&mut crate::controller::CodexOperation<'_>)->Result<(),String>{
    let work=context.work.ok_or("E_RUNTIME_TOOLS_UNVERIFIED")?;let tools=work.tools()?;
    let executable=tools.codex.as_ref().ok_or("E_LOCAL_CODEX_MISSING")?;executable.recheck()?;
    let home=coordinator_prompt::configured_home(&context.home,&expected.name)?;
    crate::controller::private_dir_readonly(&home).map_err(|_|"E_LOCAL_CODEX_HOME_REQUIRED: existing configured private per-agent home required")?;
    let (bus,sentry,project,session)={let s=state.lock().map_err(|_|"E_RUNTIME_STATE")?;(s.mcp_server_path.clone(),s.mcp_sentry_server_path.clone(),s.project_dir.clone(),s.tmux_session.clone())};
    let bus_pin=local_pinned_bytes(std::path::Path::new(&bus),8*1024*1024)?.1;
    let sentry_pin=local_pinned_bytes(std::path::Path::new(&sentry),8*1024*1024)?.1;
    let plan=LocalCodexPreparation{context,state,expected,codex_home:home.clone(),bus,sentry,project,bus_pin,sentry_pin,#[cfg(test)]drift_point:None};
    plan.configuration()?; // input shape/modes before registry or durable intent
    let registry=crate::daemon_registry::Registry::open(context.lease)?;
    if let Some(snapshot)=registry.codex_snapshot(&expected.name)? {
        if snapshot.identity.as_ref().is_some_and(|id| matches!(crate::team_process::state(id),crate::team_replacement::ProcessState::Gone|crate::team_replacement::ProcessState::Recycled)) {
            return local_stop_or_restart(state,expected,context,op,true);
        }
    }
    let fresh=registry.codex_snapshot(&expected.name)?.is_none();
    // A retained UUID is conversation continuity, not process authority. The
    // supervisor independently proves the endpoint pristine before spawning.
    let thread_path=context.lease.run_dir()?.join(format!("{}.thread-id",expected.name));
    if thread_path.exists(){wait_local_thread(&thread_path,work)?;}
    coordinator_prompt::ensure(&context.home,&context.roots,&expected.name,std::path::Path::new(&expected.prompt_file),&home,fresh)?;
    let spec=crate::codex_appserver::NativeCodexSpec{seat:expected.name.clone(),executable:executable.path.clone(),codex_home:home.clone(),provenance:crate::daemon_registry::Provenance::LegacyUnknown,
        #[cfg(test)]fixture:None,#[cfg(test)]fault:None};
    let supervisor=crate::codex_appserver::NativeCodexSupervisor::new(context.lease,&registry,spec)?;
    supervisor.prepared_local(op,&plan)?; // adoption skips prepare entirely
    plan.recheck()?;
    // Do not duplicate a live attached window or rewrite adopted configuration.
    let windows=tmux::list_windows_local(&session,work)?;
    let window=if let Some(existing)=find_running_window(&windows,&expected.name){existing.window_id.clone()}else{
        let thread_path=context.lease.run_dir()?.join(format!("{}.thread-id",expected.name));
        let thread=wait_local_thread(&thread_path,work)?;
        supervisor.recheck_held(op)?;
        let window=tmux::create_window_local(&session,&expected.name,work)?;
        let socket=context.lease.run_dir()?.join(format!("{}.sock",expected.name));
        let command=format!("/usr/bin/env -i HOME={} CODEX_HOME={} TERM=xterm-256color {} resume {} --remote {}",shell_quote(&context.home.to_string_lossy()),shell_quote(&home.to_string_lossy()),shell_quote(&executable.path.to_string_lossy()),shell_quote(&thread),shell_quote(&format!("unix://{}",socket.display())));
        executable.recheck()?;work.check_open()?;tmux::send_local(&window,&command,work)?;window
    };
    verify_plan(state,expected)?;
    let mut s=state.lock().map_err(|_|"E_RUNTIME_STATE")?;let agent=s.agents.get_mut(&expected.name).ok_or("E_RUNTIME_STATE")?;
    agent.tmux_window_id=Some(window);agent.status="running".into();Ok(())
}


#[derive(Clone,Debug,PartialEq,Eq)]
struct LocalPane {window:String,pane:String,pid:u32,dead:bool}
fn local_panes(session:&str,seat:&str,work:&crate::daemons::RuntimeWork)->Result<Vec<LocalPane>,String>{
    if !crate::daemon_registry::valid_name(session)||!crate::daemon_registry::valid_name(seat){return Err("E_RUNTIME_SELECTOR".into());}
    let bytes=tmux::local_output(work,vec!["list-panes".into(),"-s".into(),"-t".into(),format!("{session}:"),"-F".into(),"#{window_id}||#{window_name}||#{pane_id}||#{pane_pid}||#{pane_dead}".into()])?;
    let text=std::str::from_utf8(&bytes).map_err(|_|"E_TMUX_UNVERIFIED")?;
    let mut out=Vec::new();
    for (n,line) in text.lines().enumerate(){
        let f=line.split("||").collect::<Vec<_>>();
        if n>=512||f.len()!=5 {return Err("E_TMUX_UNVERIFIED".into());}
        if !f[1].eq_ignore_ascii_case(seat){continue;}
        if !f[0].starts_with('@')||!f[0][1..].bytes().all(|b|b.is_ascii_digit())
            ||!f[2].starts_with('%')||!f[2][1..].bytes().all(|b|b.is_ascii_digit())
            ||!matches!(f[4],"0"|"1"){return Err("E_TMUX_UNVERIFIED".into());}
        let pid=f[3].parse::<u32>().map_err(|_|"E_TMUX_UNVERIFIED")?;
        if pid<=1{return Err("E_TMUX_UNVERIFIED".into());}
        out.push(LocalPane{window:f[0].into(),pane:f[2].into(),pid,dead:f[4]=="1"});
    }
    Ok(out)
}
fn local_stop_or_restart(state:&Arc<Mutex<AppState>>,expected:&AgentDef,context:&LifecycleContext<'_>,
    op:&mut crate::controller::CodexOperation<'_>,restart:bool)->Result<(),String>
{
    let work=context.work.ok_or("E_RUNTIME_TOOLS_UNVERIFIED")?;
    context.require_tools()?;
    let session=state.lock().map_err(|_|"E_RUNTIME_STATE")?.tmux_session.clone();
    let panes=local_panes(&session,&expected.name,work)?;
    let registry=crate::daemon_registry::Registry::open(context.lease)?;
    let snapshot=registry.codex_snapshot(&expected.name)?;
    let mut roots=Vec::new();
    let mut recovered=None;
    if let Some(s)=&snapshot {
        if s.phase!=crate::daemon_registry::CodexPhaseV2::ReadyMetadataOnly{return Err("E_LIFECYCLE_PROCESS_UNKNOWN".into());}
        let id=s.identity.clone().ok_or("E_LIFECYCLE_PROCESS_UNKNOWN")?;
        if matches!(crate::team_process::state(&id),crate::team_replacement::ProcessState::Gone|crate::team_replacement::ProcessState::Recycled){
            let proof=coordinator_lifecycle::previous_closed(context.lease,&expected.name)?;
            proof.verifies(&expected.name,&id)?;recovered=Some(proof);
        }else{
            crate::team_terminal::codex_live_pins(&registry,op,s,&crate::team_terminal::CodexSocketResolver::production())?;
            roots.push(id);
        }
    } else if expected.model.starts_with("codex/") {
        crate::team_terminal::codex_pristine_endpoint(&registry,op)?;
    }
    for p in panes.iter().filter(|p|!p.dead && recovered.is_none()){
        let native=crate::team_process::observe(p.pid).map_err(|_|"E_LIFECYCLE_PROCESS_UNKNOWN")?.ok_or("E_LIFECYCLE_PROCESS_UNKNOWN")?;
        if native.uid!=unsafe{libc::geteuid()}{return Err("E_LIFECYCLE_PROCESS_UNKNOWN".into());}
        roots.push(native.identity);
    }
    // The second read binds roots to exact panes before the first signal.
    if local_panes(&session,&expected.name,work)?!=panes{return Err("E_LIFECYCLE_PROCESS_UNKNOWN".into());}
    let closed=if let Some(proof)=recovered {proof}else{coordinator_lifecycle::stop(context.lease,op,roots,||{
        context.classify(&expected.name)?;verify_plan(state,expected)
    })?};
    // Observed Gone is truth even if later socket/window cleanup refuses.
    crate::watchdog::on_agent_stopped(&expected.name);
    let next={let mut s=state.lock().map_err(|_|"E_RUNTIME_STATE")?;
        let a=s.agents.get_mut(&expected.name).ok_or("E_RUNTIME_STATE")?;
        a.status="stopped".into();a.tmux_window_id=None;a.clone()};
    if let Some(s)=&snapshot {
        crate::team_terminal::codex_cleanup_closed(&registry,op,s,&closed)?;
        registry.retire_closed_codex(op,s,&closed)?;
        op.release_exited()?;
    }
    // remain-on-exit panes can be removed, but never a replacement/live pane.
    let after=local_panes(&session,&expected.name,work)?;
    for p in &after {
        if !p.dead||!panes.iter().any(|old|old.window==p.window&&old.pane==p.pane&&old.pid==p.pid){return Err("E_LIFECYCLE_OUTCOME_UNKNOWN".into());}
    }
    let windows:std::collections::BTreeSet<_>=after.iter().map(|p|p.window.clone()).collect();
    for window in windows {tmux::local_output(work,vec!["kill-window".into(),"-t".into(),window])?;}
    closed.recheck()?;
    let kickoff=context.lease.run_dir()?.join(format!("{}.kickoff",expected.name));
    match fs::symlink_metadata(&kickoff){
        Err(e) if e.kind()==std::io::ErrorKind::NotFound=>{},
        Ok(_)=>{let (_,pin)=local_pinned_bytes(&kickoff,21)?;pin.recheck(&kickoff)?;fs::remove_file(&kickoff).map_err(|_|"E_LIFECYCLE_OUTCOME_UNKNOWN")?;},
        Err(_)=>return Err("E_LIFECYCLE_OUTCOME_UNKNOWN".into()),
    }
    if !restart{return Ok(());}
    if next.model.starts_with("codex/"){return start_codex_local(state,&next,context,op);}
    let (bus,sentry,project)={let s=state.lock().map_err(|_|"E_RUNTIME_STATE")?;(s.mcp_server_path.clone(),s.mcp_sentry_server_path.clone(),s.project_dir.clone())};
    let window=boot_agent_process_held(context,op,&next,session,bus,sentry,project)?;
    let mut s=state.lock().map_err(|_|"E_RUNTIME_STATE")?;
    let a=s.agents.get_mut(&expected.name).ok_or("E_RUNTIME_STATE")?;
    a.status="running".into();a.tmux_window_id=Some(window);Ok(())
}

#[cfg(test)]
pub(crate) struct LifecyclePause {
    pub arrived: std::sync::mpsc::SyncSender<()>,
    pub resume: Mutex<std::sync::mpsc::Receiver<()>>,
}
#[cfg(test)]
pub(crate) struct LifecycleFixture {
    pub spec: Option<crate::codex_appserver::NativeCodexSpec>,
    pub preparation_file: std::path::PathBuf,
    pub fail_preparation: bool,
    pub preparation_count: std::sync::atomic::AtomicUsize,
    pub effects: Mutex<Vec<&'static str>>,
    pub pause: Option<LifecyclePause>,
}
#[cfg(test)]
pub(crate) struct PreparedCaller<'a> {
    pub state: &'a Arc<Mutex<AppState>>,
    pub expected: &'a AgentDef,
    pub fixture: &'a LifecycleFixture,
}
#[cfg(test)]
impl PreparedCaller<'_> {
    pub(crate) fn recheck(&self) -> Result<(), String> { verify_plan(self.state, self.expected) }
    pub(crate) fn prepare(&self) -> Result<(), String> {
        self.recheck()?;
        self.fixture.preparation_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.fixture.effects.lock().unwrap().push("prepare");
        fs::write(&self.fixture.preparation_file, b"owned synthetic preparation").map_err(|_| "E_FIXTURE_PREPARE")?;
        if self.fixture.fail_preparation { Err("E_FIXTURE_PREPARE".into()) } else { Ok(()) }
    }
}

/// One assignee's resolved current-work summary (aperture-nr65b).
struct CurrentTask {
    id: String,
    title: String,
    extra_count: u32,
}

/// Resolve every agent's current-work summary from BEADS in a single `bd`
/// invocation — one process spawn per `list_agents` poll (every 3s), not one
/// per agent. Groups all `in_progress` beads by assignee, sorts each group by
/// `started_at` descending (ISO 8601 strings sort correctly as plain text),
/// and keeps the top one + a count of the rest.
///
/// Returns `None` if the query itself failed (bd not on PATH, bad JSON,
/// non-zero exit) — list_agents must never fail just because the
/// work-summary line couldn't be resolved this cycle, but it DOES need to
/// tell "query failed, no data" apart from "query succeeded, this assignee
/// simply has nothing in_progress" (the latter is a real, common state —
/// e.g. no one has anything claimed right now — and must render as "idle,"
/// not as "no data available," which is a materially different frontend
/// outcome). `Some(map)` with an assignee absent from the map means idle;
/// `None` means suppress the summary line entirely, same as before this
/// feature shipped.
fn resolve_current_tasks(work: Option<&crate::daemons::RuntimeWork>) -> Option<HashMap<String, CurrentTask>> {
    let work=work?;
    let tools=work.tools().ok()?;
    let input=work.client(&tools.bd,vec!["list".into(),"--status=in_progress".into(),"--json".into(),"--no-pager".into(),"--limit".into(),"512".into()]).ok()?;
    let output=crate::daemons::run_client(work,input,std::time::Duration::from_secs(3),1024*1024,64*1024).ok()?;
    if !output.accepted {return None;}

    let issues: Vec<serde_json::Value> = match serde_json::from_slice(&output.stdout) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[aperture] warn: failed to parse `bd list --json` output: {}", e);
            return None;
        }
    };

    // Group by assignee, tracking (started_at, id, title) so we can sort
    // each group without a second pass.
    let mut by_assignee: HashMap<String, Vec<(String, String, String)>> = HashMap::new();
    for issue in &issues {
        let assignee = issue.get("assignee").and_then(|v| v.as_str());
        let id = issue.get("id").and_then(|v| v.as_str());
        let title = issue.get("title").and_then(|v| v.as_str());
        let started_at = issue.get("started_at").and_then(|v| v.as_str()).unwrap_or("");
        if let (Some(assignee), Some(id), Some(title)) = (assignee, id, title) {
            by_assignee
                .entry(assignee.to_string())
                .or_default()
                .push((started_at.to_string(), id.to_string(), title.to_string()));
        }
    }

    Some(
        by_assignee
            .into_iter()
            .map(|(assignee, mut tasks)| {
                // Most-recently-claimed first.
                tasks.sort_by(|a, b| b.0.cmp(&a.0));
                let extra_count = (tasks.len() - 1) as u32;
                let (_, id, title) = tasks.into_iter().next().expect("group is never empty");
                (assignee, CurrentTask { id, title, extra_count })
            })
            .collect(),
    )
}

#[tauri::command]
pub fn start_agent(name: String, state: tauri::State<'_, Arc<Mutex<AppState>>>, runtime: tauri::State<'_, Arc<crate::daemons::RuntimeOwner>>) -> Result<(), String> {
    let work = runtime.admit(Some(&name))?;
    let _body = work.body()?;
    require_legacy_lifecycle(&name)?;
    start_agent_shared(name.clone(), state.inner(), &work.lifecycle(&name)?)
}

pub(crate) fn start_agent_shared(name: String, state: &Arc<Mutex<AppState>>, context: &LifecycleContext<'_>) -> Result<(), String> {
    with_lifecycle_plan(&name, state, context, LifecycleAction::Start, |expected, op| {
        verify_plan(state, expected)?;
        start_agent_held(name.clone(), state, context, op)
    })
}
fn start_agent_held(name: String, state: &Arc<Mutex<AppState>>, context: &LifecycleContext<'_>, op: &mut crate::controller::CodexOperation<'_>) -> Result<(), String> {
    context.classify(&name)?;
    // Extract all needed data while holding the lock briefly, then release it
    // before doing any expensive I/O (subprocess calls, file writes). This
    // prevents the global state mutex from blocking list_agents polling and
    // other commands for the full duration of agent startup.
    let (agent, tmux_session, mcp_server_path, mcp_sentry_server_path, project_dir) = {
        let app_state = state.lock().map_err(|e| e.to_string())?;
        let agent = app_state
            .agents
            .get(&name)
            .ok_or(format!("Agent '{}' not found", name))?
            .clone();

        if agent.status == "running" {
            return Err(format!("Agent '{}' is already running", name));
        }

        (
            agent,
            app_state.tmux_session.clone(),
            app_state.mcp_server_path.clone(),
            app_state.mcp_sentry_server_path.clone(),
            app_state.project_dir.clone(),
        )
    }; // ← mutex released here; all I/O below is lock-free

    let window_id = boot_agent_process_held(
        context, op,
        &agent,
        tmux_session,
        mcp_server_path,
        mcp_sentry_server_path,
        project_dir,
    )?;

    // Re-acquire lock only to write the final status
    {
        let mut app_state = state.lock().map_err(|e| e.to_string())?;
        let agent_mut = app_state.agents.get_mut(&name).unwrap();
        agent_mut.tmux_window_id = Some(window_id);
        agent_mut.status = "running".into();
    }

    Ok(())
}

/// GUI-free spawn core (aperture-syepg). Creates the tmux window, writes the
/// per-agent MCP config + launcher script, fires the launcher (baking the
/// static Claude kickoff / the Codex resume-gate), and returns the tmux window
/// id. Shared by the Tauri `start_agent` command and the headless
/// `aperture-boot` bin (aperture-xt16e L3 harness / watchdog aperture-wul6m).
/// Plain data in — no AppState / mutex — which is what makes it callable
/// headlessly. The env knobs (APERTURE_CLAUDE_BIN / APERTURE_CODEX_BIN /
/// APERTURE_LAUNCHER_PATH_PREFIX) are read here so the headless path honors
/// them identically to the GUI path.
pub fn boot_agent_process(
    agent: &AgentDef,
    tmux_session: String,
    mcp_server_path: String,
    mcp_sentry_server_path: String,
    project_dir: String,
) -> Result<String, String> {
    require_legacy_lifecycle(&agent.name)?;
    if agent.model.starts_with("codex/") { return Err(LifecycleRefusal::InputsUnverified.code().into()); }
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from).ok_or("home unavailable")?;
    let lease = crate::controller::ControllerLock::acquire(&home)?;
    let context = LifecycleContext::new(&lease)?;
    let slot = lease.codex_slot(&agent.name)?;
    let mut op = slot.enter()?;
    boot_agent_process_held(&context, &mut op, agent, tmux_session, mcp_server_path, mcp_sentry_server_path, project_dir)
}
struct LocalClaudeStaging {root:std::path::PathBuf, directory:fs::File, identity:(u64,u64,u32,u32), written:std::cell::RefCell<Vec<(String,LocalInputPin)>>}
impl LocalClaudeStaging {
    fn prepare(context:&LifecycleContext<'_>,op:&crate::controller::CodexOperation<'_>,seat:&str)->Result<Self,String>{
        use std::os::unix::fs::MetadataExt;
        context.classify(seat)?;op.verify_for(context.lease)?;
        if op.seat()!=seat{return Err("E_LIFECYCLE_CONTEXT_MISMATCH".into());}
        use std::os::{fd::{AsRawFd,FromRawFd},unix::fs::OpenOptionsExt};
        fn child(parent:&fs::File,name:&str)->Result<fs::File,String>{
            let name=std::ffi::CString::new(name).map_err(|_|"E_LOCAL_STAGING")?;
            let flags=libc::O_RDONLY|libc::O_DIRECTORY|libc::O_NOFOLLOW|libc::O_CLOEXEC;
            let mut fd=unsafe{libc::openat(parent.as_raw_fd(),name.as_ptr(),flags)};
            if fd<0 && std::io::Error::last_os_error().raw_os_error()==Some(libc::ENOENT){
                if unsafe{libc::mkdirat(parent.as_raw_fd(),name.as_ptr(),0o700)}!=0{return Err("E_LOCAL_STAGING".into());}
                fd=unsafe{libc::openat(parent.as_raw_fd(),name.as_ptr(),flags)};
            }
            if fd<0{return Err("E_LOCAL_STAGING".into());}let f=unsafe{fs::File::from_raw_fd(fd)};
            let m=f.metadata().map_err(|_|"E_LOCAL_STAGING")?;
            if !m.is_dir()||m.uid()!=unsafe{libc::geteuid()}||m.mode()&0o7777!=0o700{return Err("E_LOCAL_STAGING".into());}Ok(f)
        }
        let run=context.lease.run_dir()?;
        let run_fd=fs::OpenOptions::new().read(true).custom_flags(libc::O_DIRECTORY|libc::O_NOFOLLOW|libc::O_CLOEXEC).open(&run).map_err(|_|"E_LOCAL_STAGING")?;
        let parent=child(&run_fd,"launch")?;let directory=child(&parent,seat)?;
        let root=run.join("launch").join(seat);let m=directory.metadata().map_err(|_|"E_LOCAL_STAGING")?;
        let out=Self{root,directory,identity:(m.dev(),m.ino(),m.uid(),m.mode()),written:Default::default()};
        out.recheck_empty(context,op)?;Ok(out)
    }
    fn recheck(&self,context:&LifecycleContext<'_>,op:&crate::controller::CodexOperation<'_>)->Result<(),String>{
        use std::os::unix::fs::MetadataExt;
        context.classify(op.seat())?;op.verify_for(context.lease)?;
        crate::controller::private_dir_readonly(&self.root)?;
        for m in [self.directory.metadata().map_err(|_|"E_LOCAL_STAGING")?,fs::symlink_metadata(&self.root).map_err(|_|"E_LOCAL_STAGING")?]{
            if (m.dev(),m.ino(),m.uid(),m.mode())!=self.identity{return Err("E_LOCAL_STAGING_DRIFT".into());}
        }Ok(())
    }
    fn recheck_empty(&self,context:&LifecycleContext<'_>,op:&crate::controller::CodexOperation<'_>)->Result<(),String>{
        self.recheck(context,op)?;
        for name in ["mcp.json","prompt.md","launch.sh"]{match fs::symlink_metadata(self.root.join(name)){
            Err(e) if e.kind()==std::io::ErrorKind::NotFound=>{},_=>return Err("E_LOCAL_STAGING_EXISTS".into())
        }}Ok(())
    }
    fn write_new(&self,context:&LifecycleContext<'_>,op:&crate::controller::CodexOperation<'_>,name:&str,bytes:&[u8],mode:u32)->Result<(),String>{
        use std::os::fd::{AsRawFd,FromRawFd};use std::io::Write;
        if !matches!((name,mode),("mcp.json",0o600)|("prompt.md",0o600)|("launch.sh",0o700)){return Err("E_LOCAL_STAGING".into());}
        self.recheck(context,op)?;let leaf=std::ffi::CString::new(name).map_err(|_|"E_LOCAL_STAGING")?;
        let fd=unsafe{libc::openat(self.directory.as_raw_fd(),leaf.as_ptr(),libc::O_WRONLY|libc::O_CREAT|libc::O_EXCL|libc::O_NOFOLLOW|libc::O_NONBLOCK|libc::O_CLOEXEC,mode)};
        if fd<0{return Err("E_LOCAL_STAGING_EXISTS".into());}let mut f=unsafe{fs::File::from_raw_fd(fd)};
        let initial=LocalInputPin::of(&f.metadata().map_err(|_|"E_LOCAL_STAGING")?);
        if initial.uid!=unsafe{libc::geteuid()} || initial.nlink!=1 || initial.mode&0o7777!=mode || initial.mode&libc::S_IFMT as u32!=libc::S_IFREG as u32{return Err("E_LOCAL_STAGING".into());}
        f.write_all(bytes).and_then(|_|f.sync_all()).map_err(|_|"E_LOCAL_STAGING")?;
        let pin=LocalInputPin::of(&f.metadata().map_err(|_|"E_LOCAL_STAGING")?);pin.recheck(&self.root.join(name))?;
        self.recheck(context,op)?;self.written.borrow_mut().push((name.into(),pin));Ok(())
    }
    fn recheck_written(&self,context:&LifecycleContext<'_>,op:&crate::controller::CodexOperation<'_>)->Result<(),String>{
        self.recheck(context,op)?;
        if self.written.borrow().len()!=3{return Err("E_LOCAL_STAGING_INCOMPLETE".into());}
        for (name,pin) in self.written.borrow().iter(){pin.recheck(&self.root.join(name))?;}Ok(())
    }
}
fn boot_agent_process_held(
    context: &LifecycleContext<'_>, op: &mut crate::controller::CodexOperation<'_>,
    agent: &AgentDef, tmux_session: String, mcp_server_path: String,
    mcp_sentry_server_path: String, project_dir: String,
) -> Result<String, String> {
    context.classify(&agent.name)?;
    if op.seat() != agent.name { return Err(LifecycleRefusal::ContextMismatch.code().into()); }
    op.verify_for(context.lease)?;
    if agent.model.starts_with("codex/") || context.has_codex_history(&agent.name)? {
        return Err(LifecycleRefusal::InputsUnverified.code().into());
    }
    #[cfg(test)] if let Some(f) = context.fixture {
        f.effects.lock().unwrap().push("legacy-boot");
        return Ok("fixture-pane".into());
    }
    context.require_tools()?;
    let name = agent.name.clone();

    // Create a dedicated tmux window for this agent
    let work=context.work.ok_or("E_RUNTIME_TOOLS_UNVERIFIED")?;
    let tools=work.tools()?;
    let claude=tools.claude.as_ref().ok_or("E_LOCAL_CLAUDE_MISSING")?;claude.recheck()?;
    // Launch exclusion is not UI liveness: a retained shell also owns this
    // exact seat name. Never recreate/reuse it after an uncertain attempt.
    if tmux::list_windows_local(&tmux_session,work)?.iter().any(|window|window.name==name){
        return Err("E_LOCAL_PANE_ALREADY_PRESENT".into());
    }
    let staging=LocalClaudeStaging::prepare(context,op,&name)?;
    let bus_pin=local_pinned_bytes(std::path::Path::new(&mcp_server_path),8*1024*1024)?.1;
    let sentry_pin=local_pinned_bytes(std::path::Path::new(&mcp_sentry_server_path),8*1024*1024)?.1;
    let window_id = tmux::create_window_local(&tmux_session,&name,work)?;
    // Every subsequent failure retains the pane; the next Start consults the
    // real window list above, including shells. External removal/rename is not
    // durable reconciliation and is deliberately outside this bounded guard.
    let boot_result = (|| -> Result<String, String> {

    // Ensure agent's mailbox directory exists
    let mailbox_dir = format!("{}/.aperture/mailbox", std::env::var("HOME").unwrap_or_else(|_| "/tmp".into()));
    let _ = fs::create_dir_all(format!("{}/{}", mailbox_dir, name));

    let home_dir = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let palace_path = format!("{}/.aperture/mempalace", home_dir);

    // aperture-ktwoy — forward the Dolt sql-server password to the MCP's `bd`
    // calls. Needed only when ~/.aperture/.beads is configured for server mode
    // (post-migration); harmless empty default while still embedded. Sourced
    // from the Tauri process env (operator exports BEADS_DOLT_PASSWORD before
    // launching Aperture). host/port/user/database live in the per-machine
    // .beads/config.yaml, not here.
    let beads_dolt_password = std::env::var("BEADS_DOLT_PASSWORD").unwrap_or_default();

    // aperture-xt16e — test/ops knobs for the generated launcher scripts.
    // Unset in normal operation, which keeps the generated output
    // byte-identical to the historical inline templates. The env reads live
    // here (side-effecting layer); the pure builders in launcher.rs only see
    // plain values.
    let claude_bin = claude.path.to_string_lossy().into_owned();
    let pane_codex_bin = std::env::var("APERTURE_CODEX_BIN").unwrap_or_else(|_| "codex".into());
    let launcher_path_prefix = std::env::var("APERTURE_LAUNCHER_PATH_PREFIX").ok();
    staging.recheck_empty(context,op)?;
    bus_pin.recheck(std::path::Path::new(&mcp_server_path))?;
    sentry_pin.recheck(std::path::Path::new(&mcp_sentry_server_path))?;
    let hub_token_path = hub_auth::provision_under_lease(context.lease,&name)?;
    let hub_token_path = hub_token_path.to_string_lossy().into_owned();

    let mcp_config = serde_json::json!({
        "mcpServers": {
            "aperture-bus": {
                "type": "stdio",
                "command": tools.node.path,
                "args": [&mcp_server_path],
                "env": {
                    "PATH": tools.environment().into_iter().find(|(k,_)|k=="PATH").map(|(_,v)|v).unwrap_or_default(),
                    "HOME": &home_dir,
                    "AGENT_NAME": &name,
                    "AGENT_ROLE": &agent.role,
                    "AGENT_MODEL": &agent.model,
                    "APERTURE_MAILBOX": &mailbox_dir,
                    "BEADS_DIR": format!("{}/.aperture/.beads", home_dir),
                    "BD_ACTOR": &name,
                    "BEADS_DOLT_PASSWORD": &beads_dolt_password,
                    "APERTURE_HUB_TOKEN_FILE": &hub_token_path
                }
            },
            // Sentry MCP wrap layer — enforces Cipher's 9 constraints from
            // aperture-ttzz (allowlist, mutation/attachment approval, audit
            // emission, token redaction). If mcp-server-sentry/dist is not
            // built yet, the agent's MCP client will fail to start `sentry`
            // and other tools (aperture-bus, mempalace) still work.
            "sentry": {
                "type": "stdio",
                "command": tools.node.path,
                "args": [&mcp_sentry_server_path],
                "env": {
                    "AGENT_NAME": &name,
                    "AGENT_ROLE": &agent.role,
                    "AGENT_MODEL": &agent.model,
                    "HOME": &home_dir,
                    "BEADS_DIR": format!("{}/.aperture/.beads", home_dir),
                    "BD_ACTOR": &name
                }
            },
            "mempalace": {
                "type": "stdio",
                "command": "/usr/bin/python3",
                "args": ["-m", "mempalace.mcp_server", "--palace", &palace_path],
                "env": {
                    "MEMPALACE_WING": &name
                }
            }
        }
    });

    let launcher_path = staging.root.join("launch.sh").to_string_lossy().into_owned();
    let launcher_script = if agent.model.starts_with("codex/") {
        let bare_model = agent.model.trim_start_matches("codex/");
        let codex_home = format!("/tmp/aperture-codex-{}", name);
        let config_toml_path = format!("{}/config.toml", codex_home);

        let beads_dir = format!("{}/.aperture/.beads", std::env::var("HOME").unwrap_or_else(|_| "/tmp".into()));
        fs::create_dir_all(&codex_home).map_err(|e| e.to_string())?;

        // Codex discovers skills from $CODEX_HOME/skills. Mirror the same
        // manifest-selected runtime links that Claude Code receives under
        // ~/.claude/aperture/<agent>/skills; CODEX_HOME is per-agent and /tmp
        // is recreated after reboot, so this must happen at every launch.
        let codex_skill_count = crate::agent_loader::populate_codex_skill_home(&name, &codex_home)?;
        eprintln!(
            "[aperture] linked {} native Codex skills for '{}'",
            codex_skill_count, name
        );

        // aperture-kc7lb: Codex reads its ChatGPT login from $CODEX_HOME/auth.json.
        // The per-agent CODEX_HOME lives in /tmp (wiped on reboot) and is created
        // fresh above, so without seeding it from the operator's canonical
        // ~/.codex/auth.json every codex agent boots to the sign-in screen — and
        // a pane-side login only writes to /tmp, so it recurs after every reboot.
        // Seed when the source exists and the dest is missing or older: a fresh
        // operator login propagates on next launch, while a newer in-agent token
        // refresh isn't clobbered by a stale central file. Non-fatal on failure —
        // the agent still launches, it just shows the sign-in screen.
        let central_auth = format!("{}/.codex/auth.json", home_dir);
        let agent_auth = format!("{}/auth.json", codex_home);
        let seed_auth = match (fs::metadata(&central_auth), fs::metadata(&agent_auth)) {
            (Ok(src), Ok(dst)) => matches!(
                (src.modified(), dst.modified()),
                (Ok(s), Ok(d)) if s > d
            ),
            (Ok(_), Err(_)) => true,
            (Err(_), _) => false,
        };
        if seed_auth {
            if let Err(e) = fs::copy(&central_auth, &agent_auth) {
                eprintln!(
                    "[aperture] warning: failed to seed codex auth.json for {}: {}",
                    name, e
                );
            }
        }

        // Copy prompt into codex_home so the path is always correct.
        let prompt_content = fs::read_to_string(&agent.prompt_file)
            .map_err(|e| format!("Failed to read prompt file '{}': {}", agent.prompt_file, e))?;
        // Resident/lazy split (aperture-i7bg0): only resident.txt skills get
        // full bodies in prompt.md; the rest ride Codex's native catalog
        // populated from $CODEX_HOME/skills above.
        let prompt_content = inject_codex_skills(prompt_content, &name);
        // Codex has no SessionStart/PreCompact hook system (unlike Claude
        // Code — see .claude/settings.json), so the same boot seam the
        // SessionStart hook runs (scripts/aperture-prime.sh boot: workflow
        // preamble + memory INDEX, never the full bank) is mirrored into the
        // static prompt manually here.
        let prompt_content = inject_bd_memory(prompt_content, &project_dir, &beads_dir, &name, Some(&hub_token_path));
        // Comms Layer v2 (docs/superpowers/specs/2026-07-19-comms-layer-v2-design.md):
        // unread messages are replayed by the aperture-bus codex-bridge over
        // the app-server socket; nothing is prepended to the prompt here.
        // (Historical: the pre-v2 codex_harness prompt-injection path was
        // deleted in Phase 3.)
        let prompt_dest = format!("{}/prompt.md", codex_home);
        fs::write(&prompt_dest, &prompt_content).map_err(|e| e.to_string())?;

        // Comms Layer v2, Phase 2: aperture-bus is launched through
        // mcp-server/start.sh (resolved from project_dir, same as the other
        // mcp paths in config.rs). start.sh requires AGENT_NAME in the env
        // and exec's dist/index.js, which also reads AGENT_ROLE/AGENT_MODEL/
        // APERTURE_MAILBOX plus the BEADS vars — so those are kept.
        let bus_start_sh = format!("{}/mcp-server/start.sh", project_dir);
        let config_toml = launcher::build_codex_config_toml(&launcher::CodexConfigParams {
            bare_model,
            prompt_dest: &prompt_dest,
            project_dir: &project_dir,
            bus_start_sh: &bus_start_sh,
            mcp_sentry_server_path: &mcp_sentry_server_path,
            name: &name,
            role: &agent.role,
            model: &agent.model,
            mailbox_dir: &mailbox_dir,
            beads_dir: &beads_dir,
            beads_dolt_password: &beads_dolt_password,
            home_dir: &home_dir,
            hub_token_path: &hub_token_path,
        });
        fs::write(&config_toml_path, &config_toml).map_err(|e| e.to_string())?;

        // Comms Layer v2, Phase 2 (spec §Protocol 2): spawn the supervised
        // `codex app-server --listen unix://~/.aperture/run/<name>.sock`
        // BEFORE the pane launches, so `codex --remote` has a live socket to
        // attach to. The app-server carries CODEX_HOME (model + MCP wiring
        // live in config.toml above); the pane is just the interactive TUI.
        // aperture-syepg: clear any stale exact-id handoff file from a previous
        // session before (re)spawning, so the launcher's thread-id wait gate
        // can't read a dead UUID. The codex-bridge (Rex, PR #34) re-writes it
        // (mode 0600) after it binds the fresh kickoff thread. Path mirrors the
        // socket: <run>/<name>.thread-id.
        let _ = fs::remove_file(format!("{}/.aperture/run/{}.thread-id", home_dir, name));

        let sock_path = codex_appserver::spawn_app_server(&name, &codex_home)?;

        launcher::build_codex_launcher(
            &pane_codex_bin,
            launcher_path_prefix.as_deref(),
            &codex_home,
            &sock_path,
        )
    } else {
        let config_path = staging.root.join("mcp.json").to_string_lossy().into_owned();
        staging.write_new(context,op,"mcp.json",serde_json::to_string_pretty(&mcp_config).map_err(|_|"E_LOCAL_MCP_CONFIG")?.as_bytes(),0o600)?;

        // Read prompt and inject agent-specific skills. Resident/lazy split
        // parity with the Codex path (aperture-g4hku): when resident.txt
        // exists only the listed skills' bodies are force-injected; without
        // it every skills.txt body is injected (aperture-auane behavior).
        // Everything else stays lazily invocable through Claude Code's
        // native .claude/skills discovery.
        let prompt_content = fs::read_to_string(&agent.prompt_file)
            .map_err(|e| format!("Failed to read prompt file '{}': {}", agent.prompt_file, e))?;
        let prompt_content = inject_skills(prompt_content, &name);
        let prompt_path = staging.root.join("prompt.md").to_string_lossy().into_owned();
        staging.write_new(context,op,"prompt.md",prompt_content.as_bytes(),0o600)?;

        let mut env=tools.environment();
        env.push(("APERTURE_HUB_TOKEN_FILE".into(),hub_token_path.clone()));
        env.push(("APERTURE_PROJECT_DIR".into(),project_dir.clone()));
        let assignments=env.iter().map(|(k,v)|shell_quote(&format!("{k}={v}"))).collect::<Vec<_>>().join(" ");
        format!("#!/bin/sh\nset -eu\ncd {}\nPROMPT=$(/bin/cat {})\nexec /usr/bin/env -i {} {} --dangerously-skip-permissions --model {} --system-prompt \"$PROMPT\" --mcp-config {} --name {} {}\n",
            shell_quote(&project_dir),shell_quote(&prompt_path),assignments,shell_quote(&claude_bin),shell_quote(&agent.model),shell_quote(&config_path),shell_quote(&name),shell_quote(launcher::KICKOFF_TEXT))
    };
    staging.write_new(context,op,"launch.sh",launcher_script.as_bytes(),0o700)?;
    staging.recheck_written(context,op)?;
    bus_pin.recheck(std::path::Path::new(&mcp_server_path))?;
    sentry_pin.recheck(std::path::Path::new(&mcp_sentry_server_path))?;
    claude.recheck()?;work.check_open()?;
    tmux::send_local(&window_id,&shell_quote(&launcher_path),work)?;

    // aperture-syepg: record the kickoff-fired timestamp for Claude — the
    // kickoff positional is baked into the launcher we just fired, so this
    // instant is turn-1 fire. Feeds the presence dots (aperture-8gypy) + the
    // watchdog re-kick (aperture-wul6m). Codex writes its OWN .kickoff stamp
    // from the codex-bridge at bind time (aperture-3x136) — an earlier version
    // of this comment claimed the bridge did so via PR #34, but it never
    // actually wrote the file, which left every codex dot permanently grey.
    // Shared file: ~/.aperture/run/<name>.kickoff = unix-epoch millis (ASCII).
    if !agent.model.starts_with("codex/") {
        let millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let run_dir = format!("{}/.aperture/run", home_dir);
        let _ = fs::create_dir_all(&run_dir);
        let _ = fs::write(format!("{}/{}.kickoff", run_dir, name), millis.to_string());
    }

    // Comms Layer v2 (docs/superpowers/specs/2026-07-19-comms-layer-v2-design.md):
    // outbound Codex comms flow through the aperture-bus MCP server (wired
    // into config.toml above); inbound delivery is injected by the bus
    // codex-bridge via the app-server socket. (Historical: the pre-v2
    // codex_harness pane-scraping monitor was deleted in Phase 3.)

    // D: no detached capture/Enter worker. Trust approval is not automatic
    // lifecycle authority; it requires separately approved operator interaction.

    Ok(window_id.clone())
    })();

    if boot_result.is_err() {
        // Partial native launch is Unknown. Never destroy the pane/descendants
        // to manufacture rollback or permit an automatic second attempt.
        eprintln!("[aperture] partial local launch; retained pane {}",window_id);
        // Codex never entered this legacy attempt; no daemon rollback authority.
    }

    boot_result
}

/// Blocking teardown shared by `stop_agent` and `restart_agent`. Call with NO
/// AppState lock held (sleeps ~1s). With a window id: interrupt, `/exit`, kill
/// the window. Always: stop any supervised codex app-server, remove the
/// kickoff file, and clear the watchdog's in-memory state — see the comments
/// inline. Does NOT touch AppState; callers write `status`/`tmux_window_id`
/// themselves under a fresh lock.
fn teardown_agent(name: &str, window_id: Option<String>, context: &LifecycleContext<'_>) {
    #[cfg(test)] if let Some(f) = context.fixture { f.effects.lock().unwrap().push("legacy-teardown"); return; }
    if let Some(window_id) = window_id {
        let _ = tmux::tmux_send_keys(window_id.clone(), "C-c".into());
        std::thread::sleep(std::time::Duration::from_millis(500));
        let _ = tmux::tmux_send_keys(window_id.clone(), "/exit".into());
        std::thread::sleep(std::time::Duration::from_millis(500));
        let _ = tmux::tmux_kill_window(window_id);
    }

    // Comms Layer v2, Phase 2: kill this agent's supervised codex app-server
    // (no-op for Claude agents, which never register one).
    // Codex teardown was denied at ingress. Do not infer daemon stop from a void call.

    // aperture-wul6m: clear watchdog eligibility for a DELIBERATE stop so it is
    // never fought. Removing the kickoff file drops the agent below the
    // "expected-present" gate (eligibility = running window + kickoff file);
    // on_agent_stopped also clears any in-memory presence/attempt state so a
    // later restart begins from a clean slate.
    {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        let _ = fs::remove_file(format!("{}/.aperture/run/{}.kickoff", home, name));
        crate::watchdog::on_agent_stopped(name);
    }
}

#[tauri::command]
pub fn stop_agent(name: String, state: tauri::State<'_, Arc<Mutex<AppState>>>, runtime: tauri::State<'_, Arc<crate::daemons::RuntimeOwner>>) -> Result<(), String> {
    let work = runtime.admit(Some(&name))?;
    let _body = work.body()?;
    require_legacy_lifecycle(&name)?;
    stop_agent_shared(name.clone(), state.inner(), &work.lifecycle(&name)?)
}

pub(crate) fn stop_agent_shared(name: String, state: &Arc<Mutex<AppState>>, context: &LifecycleContext<'_>) -> Result<(), String> {
    with_lifecycle_plan(&name, state, context, LifecycleAction::Stop, |expected, op| {
        verify_plan(state, expected)?;
        stop_agent_held(name.clone(), state, context, op)
    })
}
fn stop_agent_held(name: String, state: &Arc<Mutex<AppState>>, context: &LifecycleContext<'_>, op: &mut crate::controller::CodexOperation<'_>) -> Result<(), String> {
    context.classify(&name)?;
    // Extract needed data and release the lock before the blocking sleep calls
    let (window_id_opt, is_running) = {
        let app_state = state.lock().map_err(|e| e.to_string())?;
        let agent = app_state
            .agents
            .get(&name)
            .ok_or(format!("Agent '{}' not found", name))?;

        (agent.tmux_window_id.clone(), agent.status == "running")
    }; // ← mutex released here

    if !is_running {
        return Err(format!("Agent '{}' is not running", name));
    }

    context.require_tools()?;
    teardown_agent(&name, window_id_opt, context);

    // Re-acquire to update status
    {
        let mut app_state = state.lock().map_err(|e| e.to_string())?;
        let agent_mut = app_state.agents.get_mut(&name).unwrap();
        agent_mut.tmux_window_id = None;
        agent_mut.status = "stopped".into();
    }

    Ok(())
}

/// Restart an agent regardless of whether it is currently alive
/// (aperture-ull4y). The gap this closes: `stop_agent` errors with "not
/// running" on a crashed/exited agent and `start_agent` errors with "already
/// running" on a live one, so the launcher had no single action for "bring
/// this agent back." Liveness is decided by the SAME tmux probe `list_agents`
/// uses (a real window running claude/codex/node), not the cached `status`,
/// which can lag a crash by up to one poll cycle.
///
/// Running → full stop sequence (C-c, /exit, kill window) then boot.
/// Not running → skip the tmux teardown WITHOUT erroring, still run the
/// idempotent cleanup (codex app-server, kickoff file, watchdog state) so a
/// crash's leftovers can't leak into the fresh boot, then boot.
///
/// Lock discipline mirrors `start_agent`: snapshot under the lock, release for
/// every blocking step (tmux probe, teardown sleeps, boot), re-lock only to
/// write the outcome.
#[tauri::command]
pub fn restart_agent(name: String, state: tauri::State<'_, Arc<Mutex<AppState>>>, runtime: tauri::State<'_, Arc<crate::daemons::RuntimeOwner>>) -> Result<(), String> {
    let work = runtime.admit(Some(&name))?;
    let _body = work.body()?;
    require_legacy_lifecycle(&name)?;
    restart_agent_shared(name.clone(), state.inner(), &work.lifecycle(&name)?)
}

pub(crate) fn restart_agent_shared(name: String, state: &Arc<Mutex<AppState>>, context: &LifecycleContext<'_>) -> Result<(), String> {
    with_lifecycle_plan(&name, state, context, LifecycleAction::Restart, |expected, op| {
        verify_plan(state, expected)?;
        restart_agent_held(name.clone(), state, context, op)
    })
}
fn restart_agent_held(name: String, state: &Arc<Mutex<AppState>>, context: &LifecycleContext<'_>, op: &mut crate::controller::CodexOperation<'_>) -> Result<(), String> {
    context.classify(&name)?;
    context.require_tools()?;
    let (agent, tmux_session, mcp_server_path, mcp_sentry_server_path, project_dir) = {
        let app_state = state.lock().map_err(|e| e.to_string())?;
        let agent = app_state
            .agents
            .get(&name)
            .ok_or(format!("Agent '{}' not found", name))?
            .clone();
        (
            agent,
            app_state.tmux_session.clone(),
            app_state.mcp_server_path.clone(),
            app_state.mcp_sentry_server_path.clone(),
            app_state.project_dir.clone(),
        )
    }; // ← mutex released here; all I/O below is lock-free

    // Same liveness probe as list_agents. If tmux itself can't be listed,
    // fall back to the cached status/window id rather than refusing to act.
    let live_window: Option<String> = match tmux::tmux_list_windows(tmux_session.clone()) {
        Ok(windows) => find_running_window(&windows, &name).map(|w| w.window_id.clone()),
        Err(_) => agent.tmux_window_id.clone().filter(|_| agent.status == "running"),
    };

    if live_window.is_some() {
        teardown_agent(&name, live_window, context);
    } else {
        eprintln!("[aperture] restart_agent: '{}' is not running — skipping stop, booting fresh", name);
        teardown_agent(&name, None, context);
    }

    // Reflect the stopped state before the (possibly failing) boot so a boot
    // failure never leaves a phantom "running" with a dead window id.
    {
        let mut app_state = state.lock().map_err(|e| e.to_string())?;
        if let Some(agent_mut) = app_state.agents.get_mut(&name) {
            agent_mut.tmux_window_id = None;
            agent_mut.status = "stopped".into();
        }
    }

    let window_id = boot_agent_process_held(
        context, op,
        &agent,
        tmux_session,
        mcp_server_path,
        mcp_sentry_server_path,
        project_dir,
    )?;

    {
        let mut app_state = state.lock().map_err(|e| e.to_string())?;
        let agent_mut = app_state
            .agents
            .get_mut(&name)
            .ok_or(format!("Agent '{}' not found", name))?;
        agent_mut.tmux_window_id = Some(window_id);
        agent_mut.status = "running".into();
    }

    Ok(())
}

/// The one liveness probe: an agent is running iff its tmux session has a
/// window named after it whose foreground command is claude/codex/node.
/// Shared by `list_agents` (every 3s poll) and `restart_agent`.
fn find_running_window<'a>(windows: &'a [tmux::WindowInfo], agent_name: &str) -> Option<&'a tmux::WindowInfo> {
    windows.iter().find(|window| {
        window.name == agent_name
            && (window.command == "claude"
                || window.command.contains("claude")
                || window.command == "codex"
                || window.command.contains("codex")
                || window.command == "node")
    })
}

/// Replace the cached registry membership with a fresh authoritative load,
/// while retaining only launcher-owned runtime state for principals that are
/// still enabled.  Keeping this merge pure makes the activate/archive
/// boundary testable without a running Tauri process or tmux session.
fn merge_fresh_registry(
    mut fresh: HashMap<String, AgentDef>,
    previous: &HashMap<String, AgentDef>,
    overrides: &HashMap<String, String>,
) -> HashMap<String, AgentDef> {
    for (name, agent) in fresh.iter_mut() {
        if let Some(model) = overrides.get(name) {
            agent.model = model.clone();
        }
        if let Some(previous) = previous.get(name) {
            agent.tmux_window_id = previous.tmux_window_id.clone();
            agent.status = previous.status.clone();
            agent.attention = previous.attention;
            agent.attention_reason = previous.attention_reason.clone();
            agent.turn_state = previous.turn_state.clone();
            agent.current_task_id = previous.current_task_id.clone();
            agent.current_task_title = previous.current_task_title.clone();
            agent.current_task_extra_count = previous.current_task_extra_count;
            agent.dot_state = previous.dot_state.clone();
            agent.dot_state_since = previous.dot_state_since.clone();
            agent.kickoff_fired_at = previous.kickoff_fired_at.clone();
        }
    }
    fresh
}

#[tauri::command]
pub fn list_agents(state: tauri::State<'_, Arc<Mutex<AppState>>>, runtime: tauri::State<'_, Arc<crate::daemons::RuntimeOwner>>) -> Result<Vec<AgentDef>, String> {
    let work = runtime.admit(None)?;
    let _body = work.body()?;
    work.require_tools()?;
    list_agents_local(state.inner(), &work)
}

pub(crate) fn list_agents_shared(state: &Arc<Mutex<AppState>>) -> Result<Vec<AgentDef>, String> {
    list_agents_impl(state,None)
}
pub(crate) fn list_agents_local(state:&Arc<Mutex<AppState>>,work:&crate::daemons::RuntimeWork)->Result<Vec<AgentDef>,String>{
    work.require_tools()?;list_agents_impl(state,Some(work))
}
fn list_agents_impl(state:&Arc<Mutex<AppState>>,work:Option<&crate::daemons::RuntimeWork>)->Result<Vec<AgentDef>,String>{
    let mut app_state = state.lock().map_err(|e| e.to_string())?;

    // V4 P0: the filesystem registry is authoritative and activation/archive
    // must become visible without restarting the Tauri process. Reload on the
    // existing 3s list poll, while retaining launcher-owned ephemeral state
    // for principals that remain enabled. An invalid/incoherent snapshot is
    // absent (fail closed), never merged with stale AppState membership.
    let fresh = crate::agent_loader::load_agents_from_disk();
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let overrides = crate::config::load_agent_overrides(&home);
    app_state.agents = merge_fresh_registry(fresh, &app_state.agents, &overrides);

    // Cross-reference with actual tmux windows to detect agents started outside the UI
    if let Some(Ok(windows)) = work.map(|w| tmux::list_windows_local(&app_state.tmux_session,w)) {
        for agent in app_state.agents.values_mut() {
            let running_window = find_running_window(&windows, &agent.name);

            if let Some(window) = running_window {
                agent.status = "running".into();
                agent.tmux_window_id = Some(window.window_id.clone());
            } else {
                agent.status = "stopped".into();
                agent.tmux_window_id = None;
            }
        }
    }

    // Current-work summary line (aperture-nr65b). One `bd` spawn per poll
    // cycle, not one per agent — see resolve_current_tasks. Only meaningful
    // for a running agent; a stopped agent's fields are cleared rather than
    // left showing stale work from before it stopped.
    //
    // resolve_current_tasks distinguishes "query failed" (None — suppress
    // the summary line entirely, same as before this feature shipped) from
    // "query succeeded, this assignee just has nothing in_progress" (a real,
    // common state that must render as "idle," not as missing data). The
    // sentinel for idle is current_task_id = Some("") with no title — see
    // the doc comment on AgentDef::current_task_id in state.rs.
    let current_tasks = resolve_current_tasks(work);
    for agent in app_state.agents.values_mut() {
        if agent.status != "running" {
            agent.current_task_id = None;
            agent.current_task_title = None;
            agent.current_task_extra_count = None;
            continue;
        }
        match &current_tasks {
            None => {
                // bd query failed this cycle — no data, render nothing extra.
                agent.current_task_id = None;
                agent.current_task_title = None;
                agent.current_task_extra_count = None;
            }
            Some(tasks) => match tasks.get(&agent.name) {
                Some(task) => {
                    agent.current_task_id = Some(task.id.clone());
                    agent.current_task_title = Some(task.title.clone());
                    agent.current_task_extra_count = Some(task.extra_count);
                }
                // Query succeeded; this agent has no in_progress bead — idle.
                None => {
                    agent.current_task_id = Some(String::new());
                    agent.current_task_title = None;
                    agent.current_task_extra_count = Some(0);
                }
            },
        }
    }

    Ok(app_state.agents.values().cloned().collect())
}

/// Why an attention badge is being lit (aperture-ull4y). Serialized onto
/// `AgentDef.attention_reason` as `"message"` / `"crash"`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum AttentionReason {
    /// The agent rang the operator doorbell (`send_message(to: "operator")`).
    Message,
    /// The watchdog latched red after exhausting its re-kick budget.
    Crash,
}

impl AttentionReason {
    fn as_str(self) -> &'static str {
        match self {
            AttentionReason::Message => "message",
            AttentionReason::Crash => "crash",
        }
    }
}

/// Light the attention badge with a reason. Precedence rule: `crash` always
/// wins — a crash latch overwrites a lit `message` badge, but a later message
/// never downgrades a standing `crash` (the operator must still see that the
/// agent is dead, and the doorbell text lives in scrollback regardless).
/// Callers: poller.rs (message), watchdog.rs (crash). `clear_attention` is the
/// only thing that resets it.
pub fn light_attention(agent: &mut AgentDef, reason: AttentionReason) {
    agent.attention = true;
    let already_crash = agent.attention_reason.as_deref() == Some(AttentionReason::Crash.as_str());
    if reason == AttentionReason::Crash || !already_crash {
        agent.attention_reason = Some(reason.as_str().to_string());
    }
}

#[tauri::command]
pub fn clear_attention(name: String, state: tauri::State<'_, Arc<Mutex<AppState>>>, runtime: tauri::State<'_, Arc<crate::daemons::RuntimeOwner>>) -> Result<(), String> {
    let work = runtime.admit(None)?;
    let _body = work.body()?;
    clear_attention_shared(name, state.inner(), &work)
}

pub(crate) fn clear_attention_shared(name: String, state: &Arc<Mutex<AppState>>, work: &crate::daemons::RuntimeWork) -> Result<(), String> {
    let mut app_state = state.lock().map_err(|e| e.to_string())?;
    work.check_open()?;
    if let Some(agent) = app_state.agents.get_mut(&name) {
        agent.attention = false;
        agent.attention_reason = None;
    }
    Ok(())
}

/// Claude aliases the launcher's model picker offers. The frontend catalog
/// (src/components/AgentConfigModal.ts `CLAUDE_MODELS`) is the source of
/// truth for what the UI shows; this array must match it exactly, and the
/// `picker_and_validator_agree` test below parses that file to enforce it.
/// Codex models are accepted by prefix (`codex/<anything non-empty>`) — the
/// picker lists a curated few, the validator deliberately allows the whole
/// family. A bare `codex/` is rejected: it would boot `codex --model ""`.
const CLAUDE_MODEL_ALIASES: [&str; 4] = ["opus", "sonnet", "haiku", "fable"];

pub fn is_valid_model(model: &str) -> bool {
    CLAUDE_MODEL_ALIASES.contains(&model)
        || model.strip_prefix("codex/").is_some_and(|m| !m.is_empty())
}

#[tauri::command]
pub fn update_agent_model(name: String, model: String, state: tauri::State<'_, Arc<Mutex<AppState>>>, runtime: tauri::State<'_, Arc<crate::daemons::RuntimeOwner>>) -> Result<(), String> {
    let work = runtime.admit(Some(&name))?;
    let _body = work.body()?;
    require_legacy_lifecycle(&name)?;
    update_agent_model_shared(name.clone(), model, state.inner(), &work.lifecycle(&name)?)
}

pub(crate) fn update_agent_model_shared(name: String, model: String, state: &Arc<Mutex<AppState>>, context: &LifecycleContext<'_>) -> Result<(), String> {
    context.classify(&name)?;
    if !is_valid_model(&model) {
        return Err(format!("Invalid model '{}'. Must be one of {} or codex/<model>", model, CLAUDE_MODEL_ALIASES.join("/")));
    }
    with_lifecycle_plan(&name, state, context, LifecycleAction::Model(model.clone()), |expected, _op| {
        verify_plan(state, expected)?;
        update_agent_model_held(name.clone(), model.clone(), state, context)
    })
}
fn update_agent_model_held(name: String, model: String, state: &Arc<Mutex<AppState>>, context: &LifecycleContext<'_>) -> Result<(), String> {
    context.classify(&name)?;
    if !is_valid_model(&model) {
        return Err(format!(
            "Invalid model '{}'. Must be one of {} or codex/<model>",
            model,
            CLAUDE_MODEL_ALIASES.join("/")
        ));
    }

    let mut app_state = state.lock().map_err(|e| e.to_string())?;
    let agent = app_state
        .agents
        .get_mut(&name)
        .ok_or(format!("Agent '{}' not found", name))?;

    if agent.model.starts_with("codex/") || model.starts_with("codex/") {
        return Err(LifecycleRefusal::InputsUnverified.code().into());
    }
    #[cfg(test)] if let Some(f) = context.fixture {
        f.effects.lock().unwrap().push("legacy-model");
        return Ok(());
    }
    context.check_open()?;
    agent.model = model.clone();

    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    config::save_agent_override(&home, &name, &model);

    Ok(())
}

/// Claude variant of skill injection, honoring the optional resident/lazy
/// split (aperture-g4hku — parity with [`inject_codex_skills`]). When
/// `~/.claude/aperture/<agent>/resident.txt` exists, only the listed skills'
/// bodies are appended to the prompt; the rest of skills.txt stays lazily
/// invocable through Claude Code's own `.claude/skills` discovery. When the
/// file is absent, every skill under `<agent>/skills/` is injected — the
/// pre-parity behavior (aperture-auane), so agents without a resident.txt are
/// unaffected. Resident names with no matching skill dir are warned about
/// and skipped, exactly as on the Codex path.
///
/// Skills are loaded in deterministic alphabetical order (see
/// `agent_loader::load_agent_skills`). The on-disk layout is built by
/// `just setup`; the canonical sources live in the repo at
/// `agents/<name>/{skills.txt,resident.txt}` and `.claude/skills/<name>/`.
pub fn inject_skills(prompt: String, agent_name: &str) -> String {
    match crate::agent_loader::load_agent_resident_list(agent_name) {
        Some(resident) => inject_resident_skills(
            prompt,
            agent_name,
            &resident,
            "claude",
            "native .claude/skills discovery",
        ),
        None => inject_all_skills(prompt, agent_name),
    }
}

/// Codex variant of skill injection, honoring the optional resident/lazy
/// split (aperture-i7bg0). Codex natively surfaces a lazy `## Skills`
/// catalog from `$CODEX_HOME/skills` — the same directory
/// `populate_codex_skill_home` links on every launch — and reads a skill's
/// full SKILL.md on demand when a task matches its description. So full-body
/// prompt injection is only needed for the small "resident" subset of
/// always-active behavioral norms listed in
/// `~/.claude/aperture/<agent>/resident.txt`.
///
/// When resident.txt is absent, ALL skill bodies are injected exactly as
/// before — rollout is opt-in per agent, zero behavior change without the
/// file. Resident names with no matching skill dir are warned about and
/// skipped (warn-don't-fail). Since aperture-g4hku the Claude path
/// ([`inject_skills`]) applies the same rule; only the lazy pool differs.
pub fn inject_codex_skills(prompt: String, agent_name: &str) -> String {
    match crate::agent_loader::load_agent_resident_list(agent_name) {
        Some(resident) => {
            inject_resident_skills(prompt, agent_name, &resident, "codex", "native catalog")
        }
        // No resident.txt — inject every skill body, today's behavior.
        None => inject_all_skills(prompt, agent_name),
    }
}

/// Append every skill body under `~/.claude/aperture/<agent>/skills/` — the
/// no-resident.txt fallback shared by both backends.
fn inject_all_skills(prompt: String, agent_name: &str) -> String {
    let skills = crate::agent_loader::load_agent_skills(agent_name);
    if skills.is_empty() {
        eprintln!(
            "[aperture] warn: no skills found for agent '{}' under \
             ~/.claude/aperture/{}/skills/ — did you run `just setup`?",
            agent_name, agent_name
        );
        return prompt;
    }
    let names: Vec<&str> = skills.iter().map(|(n, _)| n.as_str()).collect();
    eprintln!(
        "[aperture] loading {} skills for '{}': {:?}",
        skills.len(),
        agent_name,
        names
    );
    append_skill_bodies(prompt, skills)
}

/// Append only the bodies of the skills named in `resident` (the parsed
/// resident.txt), shared by both backends. Unknown names are warned about and
/// skipped; `backend` / `lazy_via` only label the log line.
fn inject_resident_skills(
    prompt: String,
    agent_name: &str,
    resident: &[String],
    backend: &str,
    lazy_via: &str,
) -> String {
    let skills = crate::agent_loader::load_agent_skills(agent_name);
    for name in resident {
        if !skills.iter().any(|(n, _)| n == name) {
            eprintln!(
                "[aperture] warn: resident.txt for '{}' names unknown skill \
                 '{}' (not under ~/.claude/aperture/{}/skills/) — skipped",
                agent_name, name, agent_name
            );
        }
    }
    let total = skills.len();
    let resident_skills: Vec<(String, String)> = skills
        .into_iter()
        .filter(|(name, _)| resident.iter().any(|r| r == name))
        .collect();
    eprintln!(
        "[aperture] {} skills for '{}': {} resident injected, {} lazy ({})",
        backend,
        agent_name,
        resident_skills.len(),
        total - resident_skills.len(),
        lazy_via
    );
    append_skill_bodies(prompt, resident_skills)
}

fn append_skill_bodies(mut prompt: String, skills: Vec<(String, String)>) -> String {
    for (skill_name, content) in skills {
        prompt.push_str(&format!("\n\n---\n# Skill: {}\n\n{}", skill_name, content));
    }
    prompt
}

/// Section header appended to a Codex prompt.md by [`inject_bd_memory`].
/// Names the seam so an agent reading its own prompt knows the full bank is
/// intentionally absent and where to get it (aperture-bus `recall` tools).
pub const BD_MEMORY_INDEX_HEADER: &str =
    "# Beads Memory Index (aperture-prime.sh boot — full bank is never injected; use recall/recall_full)";

/// Claude Code agents get BEADS context for free via the `SessionStart` /
/// `PreCompact` hooks in `.claude/settings.json`, which run
/// `scripts/aperture-prime.sh boot|precompact` (context diet, aperture-trgpo:
/// `bd prime` workflow preamble + a ~25 KiB memory INDEX — never the ~373 KiB
/// bank). Codex has no equivalent hook system — it only reads a static
/// `model_instructions_file` written once at boot — so without this, every
/// Codex-backed agent (Rex/Scout/Cipher as of 2026-07) boots with zero
/// memory context even though `bd` itself is fully wired for them (same
/// BEADS_DIR/BD_ACTOR env as Claude agents get). Shell out to the SAME boot
/// seam and append its output, so both backends see identical context.
///
/// Failure here must not fail agent boot. Unlike the pre-diet version, a
/// failure appends a visible `[memory index unavailable: <reason>]` line
/// under the header instead of nothing, so a Codex agent can tell "no index"
/// from "no memories" and fall back to `recall` explicitly.
///
/// `hub_token_path` is the already-provisioned per-agent hub token FILE PATH
/// (never its contents). The boot seam keys "agent session vs operator
/// session" on `APERTURE_HUB_TOKEN_FILE` — the launcher exports it for
/// Claude agents (launcher.rs) and this is the Codex-side equivalent. Without
/// it the seam treats the assembly as an operator session and prepends the
/// bd workflow preamble, which is what pushed the Codex boot prompt over the
/// 40 KiB hook budget (aperture-3kavd HOLD #3). `None` = explicit operator /
/// no-token path; the env var is then left unset, never set to "".
pub fn inject_bd_memory(
    mut prompt: String,
    project_dir: &str,
    beads_dir: &str,
    agent_name: &str,
    hub_token_path: Option<&str>,
) -> String {
    let script = format!("{}/scripts/aperture-prime.sh", project_dir);
    let mut cmd = std::process::Command::new(&script);
    cmd.arg("boot").env("BEADS_DIR", beads_dir).env("BD_ACTOR", agent_name);
    match hub_token_path {
        Some(p) if !p.is_empty() => {
            cmd.env("APERTURE_HUB_TOKEN_FILE", p);
        }
        _ => {
            cmd.env_remove("APERTURE_HUB_TOKEN_FILE");
        }
    }
    let output = cmd.output();
    let body = match output {
        Ok(out) if out.status.success() => {
            let text = String::from_utf8_lossy(&out.stdout).into_owned();
            if text.trim().is_empty() {
                eprintln!(
                    "[aperture] warning: `aperture-prime.sh boot` returned empty output for '{}'",
                    agent_name
                );
                "[memory index unavailable: aperture-prime.sh boot returned empty output]".to_string()
            } else {
                eprintln!(
                    "[aperture] injected aperture-prime boot block ({} bytes) for '{}'",
                    text.len(),
                    agent_name
                );
                text
            }
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let stderr = stderr.trim();
            eprintln!(
                "[aperture] warning: `aperture-prime.sh boot` exited non-zero for '{}': {} {}",
                agent_name, out.status, stderr
            );
            format!(
                "[memory index unavailable: aperture-prime.sh boot exited {}{}]",
                out.status,
                if stderr.is_empty() { String::new() } else { format!(" — {}", stderr) }
            )
        }
        Err(e) => {
            eprintln!(
                "[aperture] warning: failed to run `{}` for '{}': {}",
                script, agent_name, e
            );
            format!("[memory index unavailable: failed to run {}: {}]", script, e)
        }
    };
    prompt.push_str(&format!("\n\n---\n{}\n\n{}", BD_MEMORY_INDEX_HEADER, body));
    prompt
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent() -> AgentDef {
        AgentDef {
            name: "vance".into(),
            model: "fable".into(),
            role: "builder".into(),
            prompt_file: String::new(),
            tmux_window_id: None,
            status: "running".into(),
            emoji: None,
            attention: false,
            attention_reason: None,
            turn_state: None,
            current_task_id: None,
            current_task_title: None,
            current_task_extra_count: None,
            dot_state: None,
            dot_state_since: None,
            kickoff_fired_at: None,
        }
    }

    #[test]
    fn fresh_registry_drops_archived_adds_activated_and_preserves_runtime_only() {
        let mut retained = agent();
        retained.name = "retained".into();
        retained.role = "old-role".into();
        retained.prompt_file = "old-prompt".into();
        retained.tmux_window_id = Some("@7".into());
        retained.attention = true;
        retained.turn_state = Some("busy".into());
        retained.current_task_id = Some("aperture-test".into());

        let mut archived = agent();
        archived.name = "archived".into();

        let previous = HashMap::from([
            (retained.name.clone(), retained.clone()),
            (archived.name.clone(), archived),
        ]);

        let mut fresh_retained = agent();
        fresh_retained.name = "retained".into();
        fresh_retained.role = "new-role".into();
        fresh_retained.prompt_file = "new-prompt".into();
        fresh_retained.status = "stopped".into();
        let mut activated = agent();
        activated.name = "activated".into();
        activated.status = "stopped".into();

        let fresh = HashMap::from([
            (fresh_retained.name.clone(), fresh_retained),
            (activated.name.clone(), activated),
        ]);
        let overrides = HashMap::from([("retained".into(), "codex/test".into())]);
        let merged = merge_fresh_registry(fresh, &previous, &overrides);

        assert!(!merged.contains_key("archived"), "archived seats must disappear on refresh");
        assert!(merged.contains_key("activated"), "newly activated seats must appear on refresh");
        let retained = merged.get("retained").unwrap();
        assert_eq!(retained.role, "new-role", "registry-owned fields come from the fresh snapshot");
        assert_eq!(retained.prompt_file, "new-prompt");
        assert_eq!(retained.model, "codex/test", "model override applies to the fresh definition");
        assert_eq!(retained.tmux_window_id.as_deref(), Some("@7"));
        assert!(retained.attention);
        assert_eq!(retained.turn_state.as_deref(), Some("busy"));
        assert_eq!(retained.current_task_id.as_deref(), Some("aperture-test"));
    }

    // ---- aperture-84bby: picker <-> validator alignment ----

    /// The Claude aliases the picker offers, parsed from the frontend source
    /// at test time: every `value: "<alias>"` inside the `CLAUDE_MODELS`
    /// literal (stops at the first `]`). Codex entries live in a separate
    /// literal and are covered by the prefix rule, not by this list.
    fn picker_claude_aliases() -> Vec<String> {
        let src = include_str!("../../src/components/AgentConfigModal.ts");
        let start = src
            .find("const CLAUDE_MODELS = [")
            .expect("AgentConfigModal.ts: CLAUDE_MODELS literal not found");
        let body = &src[start..];
        let end = body.find(']').expect("CLAUDE_MODELS literal not closed");
        body[..end]
            .split("value: \"")
            .skip(1)
            .map(|rest| rest.split('"').next().unwrap().to_string())
            .collect()
    }

    #[test]
    fn picker_and_validator_agree() {
        let mut picker = picker_claude_aliases();
        assert!(!picker.is_empty(), "picker parse returned nothing");
        let mut validator: Vec<String> = CLAUDE_MODEL_ALIASES.iter().map(|s| s.to_string()).collect();
        picker.sort();
        validator.sort();
        assert_eq!(
            picker, validator,
            "AgentConfigModal.ts CLAUDE_MODELS and agents.rs CLAUDE_MODEL_ALIASES drifted"
        );
    }

    #[test]
    fn validator_accepts_every_picker_alias_and_codex_prefix() {
        for alias in picker_claude_aliases() {
            assert!(is_valid_model(&alias), "picker offers {alias} but validator rejects it");
        }
        assert!(is_valid_model("codex/gpt-5.6-sol"));
        assert!(is_valid_model("codex/anything-new"));
        assert!(!is_valid_model("codex/"));
        assert!(!is_valid_model("gpt-5.6-sol"));
        assert!(!is_valid_model("claude-opus-4"));
        assert!(!is_valid_model(""));
    }

    // ---- aperture-ull4y: attention_reason precedence ----

    #[test]
    fn attention_reason_message_lights_badge() {
        let mut a = agent();
        light_attention(&mut a, AttentionReason::Message);
        assert!(a.attention);
        assert_eq!(a.attention_reason.as_deref(), Some("message"));
    }

    #[test]
    fn attention_reason_crash_overwrites_message() {
        let mut a = agent();
        light_attention(&mut a, AttentionReason::Message);
        light_attention(&mut a, AttentionReason::Crash);
        assert!(a.attention);
        assert_eq!(a.attention_reason.as_deref(), Some("crash"));
    }

    #[test]
    fn attention_reason_message_never_downgrades_crash() {
        let mut a = agent();
        light_attention(&mut a, AttentionReason::Crash);
        light_attention(&mut a, AttentionReason::Message);
        assert!(a.attention);
        assert_eq!(a.attention_reason.as_deref(), Some("crash"));
    }

    #[test]
    fn attention_clear_resets_both_fields() {
        // Mirrors clear_attention's body (the command itself needs a Tauri
        // State handle); after a clear, a fresh message lights "message"
        // again — no stale crash precedence survives the clear.
        let mut a = agent();
        light_attention(&mut a, AttentionReason::Crash);
        a.attention = false;
        a.attention_reason = None;
        light_attention(&mut a, AttentionReason::Message);
        assert_eq!(a.attention_reason.as_deref(), Some("message"));
    }

    #[test]
    fn find_running_window_matches_agent_shell_commands_only() {
        let w = |name: &str, command: &str| tmux::WindowInfo {
            window_id: format!("@{}", name),
            name: name.into(),
            command: command.into(),
        };
        let windows = vec![w("vance", "zsh"), w("rex", "codex"), w("izzy", "node"), w("scout", "claude")];
        assert!(find_running_window(&windows, "vance").is_none(), "a bare shell is a dead agent");
        assert_eq!(find_running_window(&windows, "rex").unwrap().window_id, "@rex");
        assert_eq!(find_running_window(&windows, "izzy").unwrap().window_id, "@izzy");
        assert_eq!(find_running_window(&windows, "scout").unwrap().window_id, "@scout");
        assert!(find_running_window(&windows, "ghost").is_none());
    }

    // ---- aperture-trgpo: inject_bd_memory runs the boot seam, never bd prime ----

    /// A throwaway project_dir with a fake `scripts/aperture-prime.sh` whose
    /// body is `script` (a `#!/bin/sh` shebang is prepended). Cleaned up on drop.
    struct FakeProject {
        dir: std::path::PathBuf,
    }
    impl FakeProject {
        fn with_script(tag: &str, script: &str) -> Self {
            use std::os::unix::fs::PermissionsExt;
            let dir = std::env::temp_dir().join(format!(
                "aperture-inject-bd-memory-{}-{}",
                tag,
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(dir.join("scripts")).unwrap();
            let path = dir.join("scripts/aperture-prime.sh");
            fs::write(&path, format!("#!/bin/sh\n{}\n", script)).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            FakeProject { dir }
        }
        fn empty(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "aperture-inject-bd-memory-{}-{}",
                tag,
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            FakeProject { dir }
        }
        fn path(&self) -> &str {
            self.dir.to_str().unwrap()
        }
    }
    impl Drop for FakeProject {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn inject_bd_memory_appends_boot_seam_output_under_new_header() {
        // The stub echoes its mode and the env the seam relies on, so the
        // test proves (a) `boot` is the argument, (b) BEADS_DIR/BD_ACTOR
        // still flow through, (c) the captured stdout lands verbatim.
        let p = FakeProject::with_script(
            "ok",
            "echo \"MARKER mode=$1 actor=$BD_ACTOR beads=$BEADS_DIR\"",
        );
        let out = inject_bd_memory("PROMPT".into(), p.path(), "/tmp/fake-beads", "rex", None);
        assert!(out.starts_with("PROMPT\n\n---\n"), "prompt body must be preserved: {out}");
        assert!(out.contains(BD_MEMORY_INDEX_HEADER), "new header missing: {out}");
        assert!(
            out.contains("MARKER mode=boot actor=rex beads=/tmp/fake-beads"),
            "stub output not appended / wrong mode or env: {out}"
        );
        assert!(!out.contains("Beads Memory Bank (bd prime"), "old bd-prime header must be gone");
        assert!(!out.contains("[memory index unavailable"), "success path must not print the fallback");
    }

    #[test]
    fn inject_bd_memory_passes_provisioned_hub_token_path_never_contents() {
        // aperture-3kavd HOLD #3: the seam decides agent-vs-operator on this
        // env var. The launched-agent assembly must carry the provisioned
        // token FILE PATH (the stub prints the var, proving the path — not a
        // token value — crossed the boundary).
        let p = FakeProject::with_script("tokenpath", "echo \"TOKENFILE=${APERTURE_HUB_TOKEN_FILE:-UNSET}\"");
        let out = inject_bd_memory(
            "PROMPT".into(),
            p.path(),
            "/tmp/fake-beads",
            "rex",
            Some("/Users/x/.aperture/run/hub-tokens/rex.token"),
        );
        assert!(
            out.contains("TOKENFILE=/Users/x/.aperture/run/hub-tokens/rex.token"),
            "provisioned token path must reach the boot seam: {out}"
        );
    }

    #[test]
    fn inject_bd_memory_operator_path_leaves_hub_token_env_unset() {
        // Explicit no-token path: the var must be ABSENT (not empty), so the
        // seam takes the operator branch deliberately, and an inherited value
        // from the launcher's own environment can never leak into a `None` call.
        let p = FakeProject::with_script("operatorpath", "echo \"TOKENFILE=${APERTURE_HUB_TOKEN_FILE-ABSENT}\"");
        std::env::set_var("APERTURE_HUB_TOKEN_FILE", "/should/not/leak.token");
        let out = inject_bd_memory("PROMPT".into(), p.path(), "/tmp/fake-beads", "rex", None);
        std::env::remove_var("APERTURE_HUB_TOKEN_FILE");
        assert!(out.contains("TOKENFILE=ABSENT"), "operator path must not set the var: {out}");
        let out2 = inject_bd_memory("PROMPT".into(), p.path(), "/tmp/fake-beads", "rex", Some(""));
        assert!(out2.contains("TOKENFILE=ABSENT"), "empty path must behave as None: {out2}");
    }

    #[test]
    fn inject_bd_memory_header_is_the_index_header_not_bd_prime() {
        assert_eq!(
            BD_MEMORY_INDEX_HEADER,
            "# Beads Memory Index (aperture-prime.sh boot — full bank is never injected; use recall/recall_full)"
        );
    }

    #[test]
    fn inject_bd_memory_missing_script_appends_visible_fallback() {
        let p = FakeProject::empty("missing");
        let out = inject_bd_memory("PROMPT".into(), p.path(), "/tmp/fake-beads", "rex", None);
        assert!(out.starts_with("PROMPT"));
        assert!(out.contains(BD_MEMORY_INDEX_HEADER));
        assert!(
            out.contains("[memory index unavailable: failed to run "),
            "missing script must leave a visible marker, not silence: {out}"
        );
        assert!(out.contains("scripts/aperture-prime.sh"), "reason must name the script: {out}");
    }

    #[test]
    fn inject_bd_memory_nonzero_exit_appends_fallback_with_stderr_reason() {
        let p = FakeProject::with_script("fail", "echo 'boom' >&2; exit 3");
        let out = inject_bd_memory("PROMPT".into(), p.path(), "/tmp/fake-beads", "rex", None);
        assert!(out.contains(BD_MEMORY_INDEX_HEADER));
        assert!(out.contains("[memory index unavailable: aperture-prime.sh boot exited"), "{out}");
        assert!(out.contains("boom"), "stderr reason must be surfaced: {out}");
    }

    #[test]
    fn inject_bd_memory_empty_output_appends_fallback() {
        let p = FakeProject::with_script("empty", "exit 0");
        let out = inject_bd_memory("PROMPT".into(), p.path(), "/tmp/fake-beads", "rex", None);
        assert!(out.contains(BD_MEMORY_INDEX_HEADER));
        assert!(out.contains("[memory index unavailable: aperture-prime.sh boot returned empty output]"), "{out}");
    }
    // ---- aperture-g4hku: resident.txt parity between the Claude and Codex paths ----

    /// Serializes tests that point APERTURE_AGENTS_DIR at a throwaway
    /// registry: the loaders read the env var on every call, so parallel
    /// tests must not race on it.
    static REGISTRY_ENV_LOCK: Mutex<()> = Mutex::new(());

    /// A throwaway `~/.claude/aperture`-shaped registry holding one agent
    /// with real (non-symlinked) `skills/<name>/SKILL.md` dirs and an
    /// optional resident.txt. Cleaned up on drop.
    struct FakeRegistry {
        dir: std::path::PathBuf,
        agent: String,
    }
    impl FakeRegistry {
        fn new(tag: &str, skills: &[(&str, &str)], resident: Option<&str>) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "aperture-resident-parity-{}-{}",
                tag,
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&dir);
            let agent = format!("agent-{}", tag);
            for (name, body) in skills {
                let skill_dir = dir.join(&agent).join("skills").join(name);
                fs::create_dir_all(&skill_dir).unwrap();
                fs::write(skill_dir.join("SKILL.md"), body).unwrap();
            }
            fs::create_dir_all(dir.join(&agent)).unwrap();
            if let Some(text) = resident {
                fs::write(dir.join(&agent).join("resident.txt"), text).unwrap();
            }
            FakeRegistry { dir, agent }
        }
        /// Run `f` with APERTURE_AGENTS_DIR pointing at this registry,
        /// restoring the previous value afterwards.
        fn with<T>(&self, f: impl FnOnce(&str) -> T) -> T {
            let _guard = REGISTRY_ENV_LOCK
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let prev = std::env::var_os("APERTURE_AGENTS_DIR");
            std::env::set_var("APERTURE_AGENTS_DIR", &self.dir);
            let out = f(&self.agent);
            match prev {
                Some(v) => std::env::set_var("APERTURE_AGENTS_DIR", v),
                None => std::env::remove_var("APERTURE_AGENTS_DIR"),
            }
            out
        }
    }
    impl Drop for FakeRegistry {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    const THREE_SKILLS: &[(&str, &str)] = &[
        ("alpha", "ALPHA-BODY"),
        ("beta", "BETA-BODY"),
        ("gamma", "GAMMA-BODY"),
    ];

    #[test]
    fn claude_inject_skills_with_resident_txt_injects_only_resident_bodies() {
        let reg = FakeRegistry::new("claude-resident", THREE_SKILLS, Some("beta\n"));
        let out = reg.with(|agent| inject_skills("PROMPT".into(), agent));
        assert!(out.starts_with("PROMPT"));
        assert!(out.contains("# Skill: beta\n\nBETA-BODY"), "{out}");
        assert!(!out.contains("ALPHA-BODY"), "alpha is lazy, must not be injected: {out}");
        assert!(!out.contains("GAMMA-BODY"), "gamma is lazy, must not be injected: {out}");
        assert!(!out.contains("# Skill: alpha") && !out.contains("# Skill: gamma"), "{out}");
    }

    #[test]
    fn claude_inject_skills_without_resident_txt_injects_every_skill() {
        let reg = FakeRegistry::new("claude-all", THREE_SKILLS, None);
        let out = reg.with(|agent| inject_skills("PROMPT".into(), agent));
        for (name, body) in THREE_SKILLS {
            assert!(out.contains(&format!("# Skill: {}\n\n{}", name, body)), "{out}");
        }
        // Deterministic alphabetical order is preserved.
        let a = out.find("# Skill: alpha").unwrap();
        let b = out.find("# Skill: beta").unwrap();
        let g = out.find("# Skill: gamma").unwrap();
        assert!(a < b && b < g, "{out}");
    }

    #[test]
    fn claude_inject_skills_skips_unknown_resident_name_and_keeps_the_rest() {
        // "ghost" has no skills/<ghost>/SKILL.md → warned (stderr) and
        // skipped; the other resident entries still inject.
        let reg = FakeRegistry::new(
            "claude-unknown",
            THREE_SKILLS,
            Some("# resident core\nbeta\nghost\ngamma  # inline comment\n"),
        );
        let out = reg.with(|agent| inject_skills("PROMPT".into(), agent));
        assert!(out.contains("# Skill: beta\n\nBETA-BODY"), "{out}");
        assert!(out.contains("# Skill: gamma\n\nGAMMA-BODY"), "{out}");
        assert!(!out.contains("ghost"), "{out}");
        assert!(!out.contains("ALPHA-BODY"), "{out}");
    }

    /// A present-but-empty resident.txt means "inject no bodies" on both
    /// backends (Some(vec![]) is distinct from None — see agent_loader).
    #[test]
    fn claude_inject_skills_comments_only_resident_txt_injects_nothing() {
        let reg = FakeRegistry::new("claude-empty", THREE_SKILLS, Some("# nothing resident yet\n\n"));
        let out = reg.with(|agent| inject_skills("PROMPT".into(), agent));
        assert_eq!(out, "PROMPT", "{out}");
    }

    #[test]
    fn codex_and_claude_paths_produce_identical_prompts_for_the_same_registry() {
        // With resident.txt: both trim to the resident subset.
        let reg = FakeRegistry::new("parity-resident", THREE_SKILLS, Some("alpha\ngamma\n"));
        let (claude, codex) = reg.with(|agent| {
            (
                inject_skills("PROMPT".into(), agent),
                inject_codex_skills("PROMPT".into(), agent),
            )
        });
        assert_eq!(claude, codex);
        assert!(codex.contains("ALPHA-BODY") && codex.contains("GAMMA-BODY"), "{codex}");
        assert!(!codex.contains("BETA-BODY"), "{codex}");

        // Without resident.txt: both inject everything (Codex path unchanged).
        let reg = FakeRegistry::new("parity-all", THREE_SKILLS, None);
        let (claude, codex) = reg.with(|agent| {
            (
                inject_skills("PROMPT".into(), agent),
                inject_codex_skills("PROMPT".into(), agent),
            )
        });
        assert_eq!(claude, codex);
        for (_, body) in THREE_SKILLS {
            assert!(codex.contains(body), "{codex}");
        }
    }
}

#[cfg(test)]
#[path = "agents_team_tests.rs"]
mod team_lifecycle_guard_tests;

#[cfg(test)]
#[path = "agents_lifecycle_tests.rs"]
pub(crate) mod lifecycle_tests;
