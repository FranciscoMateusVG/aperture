//! Shared startup under an already-held controller lease.
//! Local composition uses the registered supervisor and tracked worker owner.
//! No legacy kill-by-port or detached startup path is reachable here.
use crate::{controller::ControllerLock, state::AppState};
use std::sync::{Arc, Mutex};
// The actual production composition and inert counted-effects tests pass
// through precisely this preflight, not a separate mock registry algorithm.
pub(crate) fn start_checked(
    lease: &ControllerLock,
    downstream: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    let registry = crate::daemon_registry::Registry::open(lease)?;
    registry.validate_namespace()?;
    downstream()
}

// Local personal-installation inputs: resolved once, rechecked at each dispatch.
// Pins detect replacement; they do not retain Homebrew/npm dependencies forever.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ToolPin {
    pub(crate) path: std::path::PathBuf,
    dev: u64, ino: u64, uid: u32, mode: u32, len: u64, modified: (i64, i64), changed: (i64, i64),
}
impl ToolPin {
    pub(crate) fn capture(path: &std::path::Path) -> Result<Self, String> {
        use std::os::unix::fs::MetadataExt;
        if !path.is_absolute() { return Err("E_LOCAL_TOOL_PATH".into()); }
        let m=std::fs::symlink_metadata(path).map_err(|_| "E_LOCAL_TOOL_MISSING")?;
        if !m.is_file() || m.file_type().is_symlink() || m.nlink()!=1
            || (m.uid()!=unsafe{libc::geteuid()} && m.uid()!=0)
            || m.mode() & 0o6022 != 0 || m.mode() & 0o111 == 0 {
            return Err("E_LOCAL_TOOL_UNSAFE".into());
        }
        Ok(Self { path:path.to_path_buf(),dev:m.dev(),ino:m.ino(),uid:m.uid(),mode:m.mode(),len:m.len(),modified:(m.mtime(),m.mtime_nsec()),changed:(m.ctime(),m.ctime_nsec()) })
    }
    pub(crate) fn fingerprint(&self) -> Result<String, String> {
        use sha2::{Digest, Sha256};
        self.recheck()?;
        let bytes = serde_json::to_vec(&(&self.path, self.dev, self.ino, self.uid, self.mode,
            self.len, self.modified, self.changed)).map_err(|_| "E_LOCAL_TOOL_PIN")?;
        Ok(format!("{:x}", Sha256::digest(bytes)))
    }
    pub(crate) fn recheck(&self) -> Result<(), String> {
        if &Self::capture(&self.path)?!=self { return Err("E_LOCAL_TOOL_CHANGED".into()); }
        Ok(())
    }
}
#[derive(Clone)]
pub(crate) struct LocalTools {
    pub(crate) node: ToolPin,
    pub(crate) tmux: ToolPin,
    pub(crate) bd: ToolPin,
    pub(crate) claude: Option<ToolPin>,
    pub(crate) codex: Option<ToolPin>,
    home: std::path::PathBuf,
    path: String,
}
impl LocalTools {
    pub(crate) fn resolve(home: &std::path::Path) -> Result<Self, String> {
        fn find(home:&std::path::Path, key:&str, name:&str) -> Result<ToolPin,String> {
            let candidates = if let Some(path)=std::env::var_os(key).filter(|v|!v.is_empty()) {
                vec![std::path::PathBuf::from(path)]
            } else { vec![std::path::PathBuf::from("/opt/homebrew/bin").join(name),std::path::PathBuf::from("/usr/local/bin").join(name),home.join(".local/bin").join(name)] };
            for candidate in candidates {
                match std::fs::canonicalize(&candidate) {
                    Ok(path)=>return ToolPin::capture(&path),
                    Err(e) if e.kind()==std::io::ErrorKind::NotFound=>continue,
                    Err(_)=>return Err("E_LOCAL_TOOL_PATH".into()),
                }
            }
            Err(format!("E_LOCAL_TOOL_MISSING: {name}"))
        }
        let node=find(home,"APERTURE_NODE_BIN","node")?;
        let tmux=find(home,"APERTURE_TMUX_BIN","tmux")?;
        let bd=find(home,"APERTURE_BD_BIN","bd")?;
        let claude=find(home,"APERTURE_CLAUDE_BIN","claude").ok();
        let codex=find(home,"APERTURE_CODEX_BIN","codex").ok();
        Self::from_pins(home,node,tmux,bd,claude,codex)
    }
    fn from_pins(home:&std::path::Path,node:ToolPin,tmux:ToolPin,bd:ToolPin,claude:Option<ToolPin>,codex:Option<ToolPin>)->Result<Self,String>{
        if !home.is_absolute() { return Err("E_LOCAL_HOME".into()); }
        let mut dirs=Vec::new();
        for pin in [&node,&tmux,&bd].into_iter().chain(claude.iter()).chain(codex.iter()) {
            let dir=pin.path.parent().ok_or("E_LOCAL_TOOL_PATH")?.to_str().ok_or("E_LOCAL_TOOL_PATH")?;
            if dir.contains(':') { return Err("E_LOCAL_TOOL_PATH".into()); }
            if !dirs.contains(&dir) {dirs.push(dir);}
        }
        dirs.extend(["/usr/bin","/bin"]);
        let path=dirs.join(":");
        Ok(Self{node,tmux,bd,claude,codex,home:home.to_path_buf(),path})
    }
    #[cfg(test)]
    pub(crate) fn fixture(home:&std::path::Path,tool:&std::path::Path)->Self{
        let pin=ToolPin::capture(tool).unwrap();
        Self::from_pins(home,pin.clone(),pin.clone(),pin.clone(),Some(pin.clone()),Some(pin)).unwrap()
    }
    pub(crate) fn recheck(&self)->Result<(),String>{
        self.node.recheck()?;self.tmux.recheck()?;self.bd.recheck()
    }
    pub(crate) fn environment(&self)->Vec<(String,String)>{
        vec![("HOME".into(),self.home.to_string_lossy().into_owned()),("PATH".into(),self.path.clone()),
            ("LANG".into(),"en_US.UTF-8".into()),("TERM".into(),"xterm-256color".into()),
            ("BEADS_DIR".into(),self.home.join(".aperture/.beads").to_string_lossy().into_owned())]
    }
    pub(crate) fn client(&self, tool:&ToolPin,args:Vec<String>)->Result<ClientInput,String>{
        tool.recheck()?;
        Ok(ClientInput{executable:tool.path.clone(),args,env:self.environment(),pin:Some(tool.clone()),
            #[cfg(test)] audit:None, #[cfg(test)] unreadable:false})
    }
}

// D: the core, not a request future, owns the lease. Tokens/workers retain it
// until their real bodies end; close proves drain by counts AND joined handles.
use std::collections::HashSet;
use std::sync::Condvar;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RuntimePhase { Open, Closing, Drained }
struct Admission {
    phase: RuntimePhase,
    bodies: usize,
    seats: HashSet<String>,
}
struct RuntimeCore {
    lease: ControllerLock,
    admission: Mutex<Admission>,
    wake: Condvar,
    workers: Mutex<Vec<(&'static str, std::thread::JoinHandle<()>)>>,
    close_serial: Mutex<()>,
    tools: Option<LocalTools>,
    #[cfg(test)] pause: Mutex<Option<Arc<BodyPause>>>,
    #[cfg(test)] fail_worker_at: Mutex<Option<usize>>,
}
pub(crate) struct RuntimeOwner { core: Arc<RuntimeCore> }
pub(crate) struct RuntimeWork {
    core: Arc<RuntimeCore>,
    seat: Option<String>,
}
// Thread marking is NOT authority. It only prohibits self-close while a body
// is active; admission and the lease remain in the private core.
thread_local! {
    static ACTIVE_RUNTIME: std::cell::RefCell<Vec<usize>> = const { std::cell::RefCell::new(Vec::new()) };
}
pub(crate) struct RuntimeBody<'a> {
    work: &'a RuntimeWork,
    _not_send: std::marker::PhantomData<std::rc::Rc<()>>,
}
impl RuntimeOwner {
    pub(crate) fn new(lease: ControllerLock) -> Self { Self::compose(lease,None) }
    pub(crate) fn local(lease: ControllerLock, tools: LocalTools) -> Result<Self,String> {
        tools.recheck()?;
        if tools.home.join(".aperture/run")!=lease.run_dir()? {return Err("E_LOCAL_HOME".into());}
        Ok(Self::compose(lease,Some(tools)))
    }
    fn compose(lease: ControllerLock, tools: Option<LocalTools>) -> Self {
        Self { core: Arc::new(RuntimeCore {
            lease, tools, admission: Mutex::new(Admission { phase: RuntimePhase::Open, bodies: 0, seats: HashSet::new() }),
            wake: Condvar::new(), workers: Mutex::new(Vec::new()), close_serial: Mutex::new(()),
            #[cfg(test)] pause: Mutex::new(None),
            #[cfg(test)] fail_worker_at: Mutex::new(None),
        }) }
    }
    pub(crate) fn admit(&self, seat: Option<&str>) -> Result<RuntimeWork, String> {
        admit_core(&self.core, seat)
    }
    pub(crate) fn start(&self, state: Arc<Mutex<AppState>>) -> Result<(), String> {
        let tools=self.core.tools.as_ref().ok_or("E_RUNTIME_TOOLS_UNVERIFIED")?;
        tools.recheck()?;
        let hub_script=std::path::PathBuf::from(&state.lock().map_err(|_|"E_RUNTIME_STATE")?.mcp_server_path)
            .parent().ok_or("E_LOCAL_MCP_PATH")?.join("ws-hub.js");
        let spec=crate::ws_hub::local_spec(&self.core.lease,tools,&hub_script)?;
        self.start_with_hub(state,spec)
    }
    pub(crate) fn start_with_hub(&self,state:Arc<Mutex<AppState>>,spec:crate::ws_hub::HubSpec)->Result<(),String>{
        self.core.tools.as_ref().ok_or("E_RUNTIME_TOOLS_UNVERIFIED")?.recheck()?;
        let result=start_checked(&self.core.lease,||{
            crate::ws_hub::Supervisor::new(&self.core.lease,spec)?.reconcile()?;
            self.start_workers(state)
        });
        if result.is_err(){self.close()?;}
        result
    }
    pub(crate) fn close(&self) -> Result<(), String> { self.close_until(Instant::now() + Duration::from_secs(15)) }
    fn close_until(&self, until: Instant) -> Result<(), String> {
        let key = Arc::as_ptr(&self.core) as usize;
        if ACTIVE_RUNTIME.with(|v| v.borrow().contains(&key)) { return Err("E_RUNTIME_SELF_CLOSE".into()); }
        // All concurrent closes close admission immediately, then one collector
        // joins. No admission/state/worker mutex is held during a join.
        {
            let mut a = self.core.admission.lock().map_err(|_| "E_RUNTIME_UNAVAILABLE")?;
            if a.phase == RuntimePhase::Drained { return Ok(()); }
            a.phase = RuntimePhase::Closing;
            self.core.wake.notify_all();
        }
        let serial = loop {
            match self.core.close_serial.try_lock() {
                Ok(v) => break v,
                Err(std::sync::TryLockError::Poisoned(_)) => return Err("E_RUNTIME_UNAVAILABLE".into()),
                Err(std::sync::TryLockError::WouldBlock) => {
                    if Instant::now() >= until { return Err("E_RUNTIME_DRAIN_INCOMPLETE".into()); }
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
        };
        loop {
            let finished = {
                let mut workers = self.core.workers.lock().map_err(|_| "E_RUNTIME_UNAVAILABLE")?;
                let mut done = Vec::new();
                let mut i = 0;
                while i < workers.len() {
                    if workers[i].1.is_finished() { done.push(workers.remove(i)); } else { i += 1; }
                }
                done
            };
            for (_, handle) in finished { let _ = handle.join(); } // unwind is an exit, not an effect outcome
            let workers_empty = self.core.workers.lock().map_err(|_| "E_RUNTIME_UNAVAILABLE")?.is_empty();
            let mut a = self.core.admission.lock().map_err(|_| "E_RUNTIME_UNAVAILABLE")?;
            if a.bodies == 0 && workers_empty {
                a.phase = RuntimePhase::Drained;
                self.core.wake.notify_all();
                drop(serial);
                return Ok(());
            }
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() { return Err("E_RUNTIME_DRAIN_INCOMPLETE".into()); }
            let (_a, _) = self.core.wake.wait_timeout(a, left.min(Duration::from_millis(20))).map_err(|_| "E_RUNTIME_UNAVAILABLE")?;
        }
    }
}
fn admit_core(core: &Arc<RuntimeCore>, seat: Option<&str>) -> Result<RuntimeWork, String> {
    if seat.is_some_and(|s| !crate::daemon_registry::valid_name(s)) { return Err("E_RUNTIME_SELECTOR".into()); }
    core.lease.verify_live()?;
    let mut a = core.admission.lock().map_err(|_| "E_RUNTIME_UNAVAILABLE")?;
    if a.phase != RuntimePhase::Open { return Err("E_RUNTIME_CLOSING".into()); }
    if a.bodies >= 32 || seat.is_some_and(|s| a.seats.contains(s)) { return Err("E_RUNTIME_CAPACITY".into()); }
    a.bodies += 1;
    if let Some(s) = seat { a.seats.insert(s.to_string()); }
    Ok(RuntimeWork { core: core.clone(), seat: seat.map(str::to_owned) })
}
impl Drop for RuntimeOwner {
    fn drop(&mut self) {
        // Explicit callers report close errors. Drop is a last-resort close on
        // early return/unwind, never a lease-release bypass. Live tokens/workers
        // still retain core if the finite drain cannot complete.
        let _ = self.close();
    }
}
impl RuntimeWork {
    pub(crate) fn body(&self) -> Result<RuntimeBody<'_>, String> {
        self.check_open()?;
        ACTIVE_RUNTIME.with(|v| v.borrow_mut().push(Arc::as_ptr(&self.core) as usize));
        let body = RuntimeBody { work: self, _not_send: std::marker::PhantomData };
        #[cfg(test)] {
            let pause = self.core.pause.lock().unwrap().take();
            if let Some(pause) = pause {
                pause.arrived.send(()).map_err(|_| "E_FIXTURE_CHANNEL")?;
                pause.resume.lock().unwrap().recv_timeout(Duration::from_secs(3)).map_err(|_| "E_FIXTURE_DEADLINE")?;
                self.check_open()?;
            }
        }
        Ok(body)
    }
    pub(crate) fn check_open(&self) -> Result<(), String> {
        self.core.lease.verify_live()?;
        if self.core.admission.lock().map_err(|_| "E_RUNTIME_UNAVAILABLE")?.phase != RuntimePhase::Open {
            return Err("E_RUNTIME_CLOSING".into());
        }
        Ok(())
    }
    pub(crate) fn check_seat(&self, seat: &str) -> Result<(), String> {
        self.check_open()?;
        if self.seat.as_deref() != Some(seat) { return Err("E_RUNTIME_SELECTOR".into()); }
        Ok(())
    }
    pub(crate) fn lifecycle(&self, seat: &str) -> Result<crate::agents::LifecycleContext<'_>, String> {
        self.check_seat(seat)?;
        #[cfg(test)] { return crate::agents::LifecycleContext::fixture_context(&self.core.lease).map(|c| c.accounted(self)); }
        #[cfg(not(test))] { crate::agents::LifecycleContext::new(&self.core.lease).map(|c| c.accounted(self)) }
    }
}
impl Drop for RuntimeBody<'_> {
    fn drop(&mut self) {
        let key = Arc::as_ptr(&self.work.core) as usize;
        ACTIVE_RUNTIME.with(|v| {
            let mut v = v.borrow_mut();
            if let Some(i) = v.iter().rposition(|k| *k == key) { v.remove(i); }
        });
    }
}
impl Drop for RuntimeWork {
    fn drop(&mut self) {
        if let Ok(mut a) = self.core.admission.lock() {
            a.bodies -= 1;
            if let Some(s) = &self.seat { a.seats.remove(s); }
            self.core.wake.notify_all();
        }
    }
}

#[cfg(test)]
#[path = "daemons_tests.rs"]
pub(crate) mod tests;

// Bounded short-lived client mechanics. Production tool descriptors are E's
// prerequisite; this module never selects PATH or imports ambient environment.
#[derive(Clone)]
pub(crate) struct ClientInput {
    executable: std::path::PathBuf,
    args: Vec<String>,
    env: Vec<(String, String)>,
    pin: Option<ToolPin>,
    #[cfg(test)] pub(crate) audit: Option<Arc<Mutex<ClientAudit>>>,
    #[cfg(test)] pub(crate) unreadable: bool,
}
impl ClientInput {
    pub(crate) fn production_unverified() -> Result<Self, String> {
        Err("E_RUNTIME_TOOLS_UNVERIFIED".into())
    }
    #[cfg(test)]
    pub(crate) fn fixture(executable: std::path::PathBuf, args: Vec<String>, env: Vec<(String, String)>) -> Self {
        Self { executable, args, env, pin:None, audit: None, unreadable: false }
    }
}
#[cfg(test)]
#[derive(Default)]
pub(crate) struct ClientAudit { pub pid: u32, pub kills: usize, pub gone: bool }
#[derive(Debug)]
pub(crate) struct ClientResult {
    pub spawned: bool,
    pub accepted: bool,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub unknown: bool,
}
struct RetainedClient {
    child: std::process::Child,
}
impl Drop for RetainedClient {
    fn drop(&mut self) {
        // Never abandon a live client during unwind. No signal in Drop and no
        // detached waiter. The borrowing work/token stays on this body's stack.
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => break,
                _ => std::thread::sleep(Duration::from_millis(20)),
            }
        }
    }
}
pub(crate) fn run_client(
    work: &RuntimeWork, input: ClientInput, budget: Duration, out_cap: usize, err_cap: usize,
) -> Result<ClientResult, String> {
    use std::io::Read;
    use std::os::fd::AsRawFd;
    use std::process::{Command, Stdio};
    work.check_open()?;
    if !input.executable.is_absolute() { return Err("E_RUNTIME_TOOLS_UNVERIFIED".into()); }
    let mut command = Command::new(&input.executable);
    command.args(&input.args).env_clear().envs(input.env)
        .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let until = Instant::now() + budget;
    work.check_open()?;
    if let Some(pin)=&input.pin {pin.recheck()?;}
    let child = match command.spawn() {
        Ok(child) => child,
        Err(_) => return Ok(ClientResult { spawned: false, accepted: false, stdout: vec![], stderr: vec![], unknown: false }),
    };
    // Install retention before any fallible observation or pipe setup.
    let mut held = RetainedClient { child };
    let identity = crate::team_process::observe(held.child.id()).ok().flatten();
    #[cfg(test)] if let Some(audit) = &input.audit { audit.lock().unwrap().pid = held.child.id(); }
    // Fault injection is observation failure, not simulated OS PID reuse.
    #[cfg(test)] let identity = if input.unreadable { None } else { identity };
    let mut stdout = held.child.stdout.take().ok_or("E_RUNTIME_CLIENT_UNKNOWN")?;
    let mut stderr = held.child.stderr.take().ok_or("E_RUNTIME_CLIENT_UNKNOWN")?;
    for fd in [stdout.as_raw_fd(), stderr.as_raw_fd()] {
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            return Err("E_RUNTIME_CLIENT_UNKNOWN".into());
        }
    }
    fn drain(r: &mut impl Read, bytes: &mut Vec<u8>, cap: usize) -> Result<bool, ()> {
        let mut chunk = [0u8; 4096];
        loop {
            match r.read(&mut chunk) {
                Ok(0) => return Ok(true),
                Ok(n) => {
                    if bytes.len().checked_add(n).is_none_or(|v| v > cap) { return Err(()); }
                    bytes.extend_from_slice(&chunk[..n]);
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return Ok(false),
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => return Err(()),
            }
        }
    }
    let mut out = Vec::new();
    let mut err = Vec::new();
    let mut exit = None;
    loop {
        let out_eof = drain(&mut stdout, &mut out, out_cap);
        let err_eof = drain(&mut stderr, &mut err, err_cap);
        match held.child.try_wait() {
            Ok(status) => { if status.is_some() { exit = status; } }
            Err(_) => break,
        }
        if let Some(status) = exit {
            if out_eof == Ok(true) && err_eof == Ok(true) {
                return Ok(ClientResult { spawned: true, accepted: status.success(), stdout: out, stderr: err, unknown: !status.success() });
            }
        }
        if out_eof.is_err() || err_eof.is_err() || Instant::now() >= until || work.check_open().is_err() {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    // At most one exact direct-child kill. UID/PPID/birth must match afresh;
    // an unreadable/recycled child gets observation-only retention.
    if exit.is_none() {
        if let Some(expected) = identity {
            if expected.ppid == std::process::id() && expected.uid == unsafe { libc::geteuid() }
                && expected.identity.pid == held.child.id()
                && crate::team_process::observe(held.child.id()).ok().flatten().is_some_and(|fresh|
                    fresh.identity == expected.identity && fresh.ppid == std::process::id()
                    && fresh.uid == unsafe { libc::geteuid() }) {
                #[cfg(test)] if let Some(audit) = &input.audit { audit.lock().unwrap().kills += 1; }
                let _ = held.child.kill();
            }
        }
        let reap_until = Instant::now() + Duration::from_secs(1);
        while Instant::now() < reap_until {
            if matches!(held.child.try_wait(), Ok(Some(_))) { break; }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    // Drop retains the original body/token in observation-only polling if still
    // unresolved. close may time out but cannot release its lease.
    #[cfg(test)] let pid = held.child.id();
    drop(held);
    #[cfg(test)] if let Some(audit) = &input.audit {
        audit.lock().unwrap().gone = crate::team_process::observe(pid).ok().flatten().is_none();
    }
    Ok(ClientResult { spawned: true, accepted: false, stdout: out, stderr: err, unknown: true })
}

pub(crate) struct WorkerContext { core: Arc<RuntimeCore> }
impl WorkerContext {
    pub(crate) fn stopped(&self) -> bool {
        self.core.admission.lock().map_or(true, |a| a.phase != RuntimePhase::Open)
    }
    pub(crate) fn wait(&self, duration: Duration) -> bool {
        let until = Instant::now() + duration;
        let Ok(mut a) = self.core.admission.lock() else { return true; };
        while a.phase == RuntimePhase::Open && Instant::now() < until {
            let left = until.saturating_duration_since(Instant::now()).min(Duration::from_millis(100));
            let Ok((next, _)) = self.core.wake.wait_timeout(a, left) else { return true; };
            a = next;
        }
        a.phase != RuntimePhase::Open
    }
    pub(crate) fn admit(&self, seat: Option<&str>) -> Result<RuntimeWork, String> {
        admit_core(&self.core, seat)
    }
    pub(crate) fn home(&self) -> Result<std::path::PathBuf, String> {
        let run = self.core.lease.run_dir()?;
        run.parent().and_then(std::path::Path::parent).map(std::path::Path::to_path_buf)
            .ok_or_else(|| "E_RUNTIME_HOME".into())
    }
}
impl RuntimeWork {
    pub(crate) fn verified_run_dir(&self) -> Result<&std::path::Path, String> {
        self.core.lease.run_dir()
    }
    pub(crate) fn wait_open(&self, duration: Duration) -> Result<(), String> {
        if (WorkerContext { core: self.core.clone() }).wait(duration) { Err("E_RUNTIME_CLOSING".into()) }
        else { self.check_open() }
    }
}
impl WorkerContext {
    pub(crate) fn subscriber(&self) -> Result<crate::ws_hub::BoundSubscriber, String> {
        if self.stopped() { return Err("E_RUNTIME_CLOSING".into()); }
        crate::ws_hub::registered_subscriber(&self.core.lease)
    }
    pub(crate) fn subscriber_next(&self, stream: &mut crate::ws_hub::BoundSubscriber) -> Result<Option<crate::ws_hub::PresenceUpdate>, String> {
        if self.stopped() { return Err("E_RUNTIME_CLOSING".into()); }
        stream.next(&self.core.lease)
    }
}
impl RuntimeOwner {
    fn start_workers(&self, state: Arc<Mutex<AppState>>) -> Result<(), String> {
        let shared = crate::watchdog::WatchdogState::new();
        for (_index, name) in ["subscriber", "decision", "unread", "mailbox"].into_iter().enumerate() {
            #[cfg(test)] if *self.core.fail_worker_at.lock().unwrap() == Some(_index) {
                self.close()?; return Err("E_RUNTIME_WORKER_START".into());
            }
            let mut handles = self.core.workers.lock().map_err(|_| "E_RUNTIME_UNAVAILABLE")?;
            let a = self.core.admission.lock().map_err(|_| "E_RUNTIME_UNAVAILABLE")?;
            if a.phase != RuntimePhase::Open || handles.iter().any(|(n, _)| *n == name) || handles.len() >= 4 {
                drop(a); drop(handles);
                self.close()?;
                return Err("E_RUNTIME_WORKERS".into());
            }
            let context = WorkerContext { core: self.core.clone() };
            let shared = shared.clone();
            let state = state.clone();
            let handle = std::thread::Builder::new().name(format!("aperture-{name}")).spawn(move || {
                let key = Arc::as_ptr(&context.core) as usize;
                ACTIVE_RUNTIME.with(|v| v.borrow_mut().push(key));
                if name == "mailbox" { crate::poller::run_message_poller(state, context); }
                else { crate::watchdog::run_owned_worker(name, shared, state, context); }
                ACTIVE_RUNTIME.with(|v| { let mut v=v.borrow_mut(); if let Some(i)=v.iter().rposition(|k| *k==key) { v.remove(i); } });
            });
            match handle {
                Ok(handle) => handles.push((name, handle)),
                Err(_) => {
                    drop(a); drop(handles);
                    let _ = self.close();
                    return Err("E_RUNTIME_WORKER_START".into());
                }
            }
        }
        Ok(())
    }
}

impl RuntimeWork {
    pub(crate) fn nudge(&self, state: &Arc<Mutex<AppState>>, seat: &str, producer: crate::watchdog::NudgeProducer, inputs: crate::watchdog::NudgeInputs) -> Result<crate::watchdog::DispatchOutcome, String> {
        if self.seat.as_deref() != Some(seat) { return Err("E_RUNTIME_SELECTOR".into()); }
        crate::watchdog::guarded_nudge(&self.core.lease, self, state, seat, producer, inputs)
    }
}

#[cfg(test)]
struct BodyPause {
    arrived: std::sync::mpsc::SyncSender<()>,
    resume: Mutex<std::sync::mpsc::Receiver<()>>,
}
#[cfg(test)]
impl RuntimeOwner {
    pub(crate) fn fixture_pause(&self, arrived: std::sync::mpsc::SyncSender<()>, resume: std::sync::mpsc::Receiver<()>) {
        *self.core.pause.lock().unwrap() = Some(Arc::new(BodyPause { arrived, resume: Mutex::new(resume) }));
    }
    pub(crate) fn fixture_worker(&self) -> WorkerContext { WorkerContext { core: self.core.clone() } }
    pub(crate) fn fixture_close_short(&self) -> Result<(), String> {
        self.close_until(Instant::now() + Duration::from_millis(20))
    }
}
impl RuntimeWork {
    pub(crate) fn require_tools(&self) -> Result<(), String> {
        self.check_open()?;
        self.tools().map(|_| ())
    }
    pub(crate) fn tools(&self)->Result<&LocalTools,String>{
        self.check_open()?;
        let tools=self.core.tools.as_ref().ok_or("E_RUNTIME_TOOLS_UNVERIFIED")?;
        tools.recheck()?;Ok(tools)
    }
    pub(crate) fn client(&self,tool:&ToolPin,args:Vec<String>)->Result<ClientInput,String>{self.tools()?.client(tool,args)}
}
