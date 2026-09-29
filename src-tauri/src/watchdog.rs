//! D overlay: Respawn and trust auto-confirm are denied. Both Nudge producers
//! use one durable intent/outcome actuator; DispatchAccepted is not recovery.
//! Production tool selection and startup remain fenced pending E. The older
//! design description below is historical context, not authorization.
//! Liveness watchdog (aperture-wul6m) — the agent-side half of comms-v2
//! reliability. Companion to the hub-side 256ru Volta-shim fix (#41).
//!
//! WHY: comms-v2 push delivery assumes a *booted* agent whose inbox monitor is
//! connected to the hub. The canonical inbox client (`mcp-server/dist/hub-client.js`)
//! is EXIT-ON-DROP by design (deafness must be loud, never silently retried into
//! noise). So any socket drop — a hub bounce, a network blip, a crash — kills the
//! monitor and the agent goes deaf. A *responsive* agent self-heals: the Monitor
//! tool fires a wake event on the monitor's exit and the agent re-runs its boot
//! routine (Path 1, ~seconds). This watchdog is Path 2 — the irreplaceable
//! recovery for the agent that CANNOT self-heal: mid-long-turn, wedged, or asleep
//! when the drop lands, plus the fleet-wide bounce where every agent needs a
//! re-kick at once.
//!
//! DESIGN (see the aperture-wul6m bead notes for the full spec + review trail):
//!   * PLACEMENT — launcher/Tauri backend, not the hub. The hub is deliberately
//!     roster-agnostic (it knows who DID hello, never who SHOULD). The launcher
//!     owns the expected roster (it spawned the agents), the actuator (tmux /
//!     boot_agent_headless), and — via this module's subscriber — actual presence.
//!     Split: the hub DETECTS the drop (its `leave` broadcast); the launcher
//!     DECIDES + acts.
//!   * ONE TRUTH — a single 60s deadline (`SILENCE_DEADLINE`) and a single
//!     clock (the `~/.aperture/run/<name>.kickoff` file) feed BOTH the re-kick
//!     decision AND the presence dot the frontend polls. The watchdog computes
//!     `dot_state` once and writes it onto `AgentDef`; `list_agents` returns it
//!     verbatim (poll, not push — a missed event can never freeze the dot).
//!   * TIERED ACTUATOR — attempt 1 is a gentle nudge (send the boot-routine turn
//!     into the live pane; preserves the agent's context, cures the dominant
//!     hub-bounce case at zero cost). Attempts 2-3 escalate to a respawn (a
//!     wedged agent's context is already forfeit). After 3 failures: latch red +
//!     ring the operator. Never hammer a genuinely-broken session forever.
//!   * SUBSCRIBER-DOWN PAUSE — if THIS watchdog's own hub subscriber is down,
//!     presence is untrustworthy, so re-kicks are suppressed until it reconnects
//!     + a grace window. Prevents a hub restart from triggering a fleet-wide
//!     false re-kick storm (agents' own monitors reconnect in ~seconds).

use crate::state::AppState;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};


/// The one silence deadline — MUST equal the syepg boot SLA and Izzy's harness
/// assertion (GLaDOS ruling): the deadline must match the boot budget or a
/// slow-but-legal thundering-herd boot false-positives into red/re-kick.
const SILENCE_DEADLINE: Duration = Duration::from_secs(60);

/// C3 (reconnect-storm-as-deafness): "online" (green) must mean a STABLE hub
/// presence, not a momentary join. A flapping monitor (join → drop → join …)
/// would read intermittently-healthy while functionally deaf. Green requires the
/// hub presence be held continuously for at least this long.
const ONLINE_DEBOUNCE: Duration = Duration::from_secs(4);

/// Re-kick gaps after attempts 1, 2, 3 (§4 backoff cascade). GLaDOS: the cascade
/// doubles as the nudge/respawn response-window — the next attempt fires only
/// after the prior one got no hub-join within its window.
const BACKOFF_GAPS_SECS: [u64; 3] = [60, 120, 240];
const MAX_ATTEMPTS: u8 = 3;

/// C1b jitter ceiling: a per-agent deterministic offset (0..this) added to each
/// re-kick gap so the fleet never re-hellos in lockstep and re-crashes a fresh
/// hub. Deterministic-per-agent (name hash), so no rand dependency.
const JITTER_CEILING_SECS: u64 = 8;

/// How often the decision loop wakes to recompute dots + evaluate re-kicks.
const TICK_INTERVAL: Duration = Duration::from_secs(2);

/// Grace window after the subscriber (re)connects before re-kicks resume — lets
/// agents' own monitors reconnect + re-hello so we don't act on a stale/empty
/// presence picture (§5).
const RECONNECT_GRACE: Duration = Duration::from_secs(10);

/// Reconnect backoff ceiling for this watchdog's own subscriber socket (C1:
/// quiet, bounded — never a tight reconnect loop).
const SUBSCRIBER_RECONNECT_MAX: Duration = Duration::from_secs(30);

/// Hub turn-state (aperture-ull4y): the hub broadcasts `busy` when an agent
/// starts a turn and `idle` when it finishes. Previously folded into
/// `Presence.online` and discarded; now carried through to `AgentDef.turn_state`
/// so the launcher can tell "working" from "waiting" on a green dot.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Turn {
    Busy,
    Idle,
}

impl Turn {
    fn as_str(self) -> &'static str {
        match self {
            Turn::Busy => "busy",
            Turn::Idle => "idle",
        }
    }
}

/// One agent's hub-presence facts, as learned from the subscriber stream.
#[derive(Default)]
struct Presence {
    /// True while the last presence event for this agent was join/busy/idle
    /// (a positive presence signal); false after a `leave`.
    online: bool,
    /// When the agent became *continuously* online (reset on any leave). Used
    /// for the ONLINE_DEBOUNCE stability check.
    online_since: Option<SystemTime>,
    /// Last `busy`/`idle` frame seen since the agent joined. `None` on a fresh
    /// entry or after `join` alone (no turn frame yet), and cleared on `leave`
    /// / subscriber disconnect — an absent agent has no trustworthy turn state.
    turn: Option<Turn>,
}

/// One agent's re-kick bookkeeping.
#[derive(Default)]
struct Watch {
    /// The kickoff timestamp (epoch millis) we are currently tracking. When the
    /// launcher writes a NEWER value (a fresh boot or a re-kick), we reset the
    /// attempt counter — a new kickoff is a clean slate.
    tracked_kickoff_millis: Option<u64>,
    attempts: u8,
    /// When the most recent re-kick attempt fired (start of its response window).
    last_attempt_at: Option<SystemTime>,
    /// Set once the 3-attempt budget is spent — latches red + rings the operator
    /// exactly once, then stops hammering.
    latched: bool,
}

struct Shared {
    presence: HashMap<String, Presence>,
    watch: HashMap<String, Watch>,
    /// Whether THIS watchdog's subscriber socket is currently connected.
    subscriber_connected: bool,
    /// When the subscriber last (re)connected — start of the RECONNECT_GRACE.
    connected_since: Option<SystemTime>,
}

impl Shared {
    fn new() -> Self {
        Shared {
            presence: HashMap::new(),
            watch: HashMap::new(),
            subscriber_connected: false,
            connected_since: None,
        }
    }
}

/// Deterministic per-agent jitter (0..JITTER_CEILING_SECS) — de-synchronises the
/// fleet without a rand dependency.
fn agent_jitter_secs(name: &str) -> u64 {
    let h = name
        .bytes()
        .fold(0u64, |a, b| a.wrapping_mul(31).wrapping_add(b as u64));
    if JITTER_CEILING_SECS == 0 {
        0
    } else {
        h % JITTER_CEILING_SECS
    }
}

fn now() -> SystemTime {
    SystemTime::now()
}

fn millis_to_systemtime(millis: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(millis)
}

fn iso8601(t: SystemTime) -> String {
    let millis = t
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    chrono::DateTime::from_timestamp_millis(millis)
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_default()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KickoffRead {
    Absent,
    Millis(u64),
    Unverified,
}

fn read_kickoff_millis(home: &std::path::Path, name: &str) -> KickoffRead {
    use std::io::Read;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    // Both existing writers emit raw decimal digits, but do not guarantee0600.
    // Reading0640/0644 under run0700 is compatible; writable-to-others is not.
    fn leaf_safe(m: &std::fs::Metadata) -> bool {
        m.is_file() && m.uid() == unsafe { libc::geteuid() } && m.nlink() == 1
            && m.mode() & 0o400 != 0 && m.mode() & 0o7022 == 0
            && (1..=20).contains(&m.len())
    }
    fn parent_pin(m: &std::fs::Metadata) -> Option<(u64, u64, u32, u32)> {
        (m.is_dir() && m.uid() == unsafe { libc::geteuid() }
            && m.mode() & 0o7777 == 0o700 && m.nlink() > 0)
            .then_some((m.dev(), m.ino(), m.uid(), m.mode()))
    }
    fn leaf_pin(m: &std::fs::Metadata) -> (u64,u64,u32,u32,u64,u64,i64,i64,i64,i64) {
        (m.dev(),m.ino(),m.uid(),m.mode(),m.nlink(),m.len(),m.mtime(),m.mtime_nsec(),m.ctime(),m.ctime_nsec())
    }
    let read = || -> Result<Option<u64>, ()> {
        if !crate::daemon_registry::valid_name(name) { return Err(()); }
        let root = home.join(".aperture");
        let run = root.join("run");
        crate::controller::private_dir_readonly(&root).map_err(|_| ())?;
        let before_parent = std::fs::symlink_metadata(&run).map_err(|_| ())?;
        let pin = parent_pin(&before_parent).ok_or(())?;
        let parent = std::fs::OpenOptions::new().read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(&run).map_err(|_| ())?;
        let verify_parent = || -> Result<(), ()> {
            crate::controller::private_dir_readonly(&root).map_err(|_| ())?;
            if parent_pin(&parent.metadata().map_err(|_| ())?) != Some(pin)
                || parent_pin(&std::fs::symlink_metadata(&run).map_err(|_| ())?) != Some(pin) {
                return Err(());
            }
            Ok(())
        };
        verify_parent()?;
        let basename = format!("{name}.kickoff");
        let c_name = std::ffi::CString::new(basename.as_bytes()).map_err(|_| ())?;
        // Only the validated fixed basename is resolved under this owned FD.
        // NONBLOCK prevents FIFO open from hanging before fstat can reject it.
        let raw = unsafe { libc::openat(parent.as_raw_fd(), c_name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC) };
        if raw < 0 {
            let error = std::io::Error::last_os_error();
            verify_parent()?;
            return if error.raw_os_error() == Some(libc::ENOENT) { Ok(None) } else { Err(()) };
        }
        let mut file = unsafe { std::fs::File::from_raw_fd(raw) };
        let before = file.metadata().map_err(|_| ())?;
        if !leaf_safe(&before) { return Err(()); }
        let mut bytes = [0u8; 21];
        let mut count = 0;
        while count < bytes.len() {
            let got = file.read(&mut bytes[count..]).map_err(|_| ())?;
            if got == 0 { break; }
            count += got;
        }
        let after = file.metadata().map_err(|_| ())?;
        verify_parent()?;
        let named = std::fs::symlink_metadata(run.join(&basename)).map_err(|_| ())?;
        if !leaf_safe(&after) || !leaf_safe(&named)
            || leaf_pin(&before) != leaf_pin(&after) || leaf_pin(&after) != leaf_pin(&named)
            || count == 0 || count > 20 || count as u64 != before.len()
            || !bytes[..count].iter().all(u8::is_ascii_digit) { return Err(()); }
        let value = bytes[..count].iter().try_fold(0u64, |n, b|
            n.checked_mul(10).and_then(|n| n.checked_add(u64::from(b - b'0')))).ok_or(())?;
        Ok(Some(value))
    };
    match read() {
        Ok(None) => KickoffRead::Absent,
        Ok(Some(value)) => KickoffRead::Millis(value),
        Err(()) => KickoffRead::Unverified,
    }
}

/// The four presence-dot states (docs/presence-dots-spec.md). `stuck`/`online`
/// are watchdog-only — the frontend never guesses them.
#[derive(Clone, Copy, PartialEq)]
enum Dot {
    Spawned,
    Booting,
    Online,
    Stuck,
}

impl Dot {
    fn as_str(self) -> &'static str {
        match self {
            Dot::Spawned => "spawned",
            Dot::Booting => "booting",
            Dot::Online => "online",
            Dot::Stuck => "stuck",
        }
    }
}

/// Compute the authoritative dot state for one agent. This is the ONE place the
/// state machine lives — both the frontend dot (via the field we write) and the
/// re-kick decision below read from the same logic.
///
/// Rules (spec): `online` ALWAYS wins over booting/stuck regardless of elapsed
/// time — the moment the hub confirms a STABLE presence, green, even at 59.9s.
/// Silence past the 60s deadline is the ONLY thing that paints red.
fn compute_dot(kickoff_millis: Option<u64>, presence: Option<&Presence>, at: SystemTime) -> Dot {
    // No kickoff recorded → the turn hasn't fired; grey.
    let Some(k_millis) = kickoff_millis else {
        return Dot::Spawned;
    };

    // Stable-online wins outright (debounced per C3).
    if let Some(p) = presence {
        if p.online {
            if let Some(since) = p.online_since {
                if at.duration_since(since).unwrap_or_default() >= ONLINE_DEBOUNCE {
                    return Dot::Online;
                }
            }
        }
    }

    // Not (stably) online: booting until the deadline, stuck after it.
    let kickoff_at = millis_to_systemtime(k_millis);
    if at.duration_since(kickoff_at).unwrap_or_default() >= SILENCE_DEADLINE {
        Dot::Stuck
    } else {
        Dot::Booting
    }
}

/// When the CURRENT dot state began (aperture-ull4y) — the value behind
/// `AgentDef.dot_state_since`, which the frontend renders as a live "{N}s ago"
/// counter. Derived from the two source clocks rather than stamped with the
/// tick time (the previous bug: every tick re-stamped `now`, so the counter
/// never left ~0s):
///
/// - `online`  → `Presence.online_since` (the join that has held stable).
/// - `spawned` / `booting` / `stuck` → the kickoff timestamp. `stuck` deliberately
///   keeps the kickoff clock rather than the deadline crossing: the spec's
///   tooltip is "kickoff sent {N}s ago, still not connected", so the operator
///   sees the whole silence, not silence-minus-60s.
///
/// Same inputs → same output across ticks, so a steady state yields a steady
/// `since`; only a real transition (a new kickoff, a fresh join) moves it.
/// `None` only when the source clock is missing (an online dot always has an
/// `online_since`; spawned has no kickoff) — the caller falls back to the tick.
fn dot_since(dot: Dot, kickoff_millis: Option<u64>, presence: Option<&Presence>) -> Option<SystemTime> {
    match dot {
        Dot::Online => presence.and_then(|p| p.online_since),
        Dot::Spawned | Dot::Booting | Dot::Stuck => kickoff_millis.map(millis_to_systemtime),
    }
}

/// The hub turn-state to publish for a given dot (aperture-ull4y). Rule:
/// `turn_state` is `None` whenever the dot is not `online` — an unstable,
/// booting, or stuck agent has no trustworthy turn state, whatever the last
/// frame said.
fn turn_for(dot: Dot, presence: Option<&Presence>) -> Option<Turn> {
    match dot {
        Dot::Online => presence.and_then(|p| p.turn),
        Dot::Spawned | Dot::Booting | Dot::Stuck => None,
    }
}

// Four-worker ownership lives in daemons::RuntimeOwner, never fire-and-forget.
#[derive(Clone)]
pub(crate) struct WatchdogState(Arc<Mutex<Shared>>);
impl WatchdogState {
    pub(crate) fn new() -> Self { Self(Arc::new(Mutex::new(Shared::new()))) }
}
pub(crate) fn run_owned_worker(name: &str, shared: WatchdogState, app: Arc<Mutex<AppState>>, worker: crate::daemons::WorkerContext) {
    match name {
        "subscriber" => subscriber_loop(shared.0, worker),
        "decision" => decision_loop(shared.0, app, worker),
        "unread" => unread_sweep_loop(shared.0, app, worker),
        _ => {}
    }
}

/// How often the unread-age sweep queries BEADS. Each pass is one `bd query`
/// for ALL open messages (not per-agent), so this stays cheap.
const UNREAD_SWEEP_INTERVAL: Duration = Duration::from_secs(45);

/// Oldest-unread age past which a hub-online agent is declared present-but-deaf.
/// Comfortably above the hub's push latency (ms) and a normal read cycle, well
/// below "operator notices work stalled."
const UNREAD_DEAF_AGE: Duration = Duration::from_secs(90);

/// Minimum gap between inbox nudges to the same agent — never spam a pane.
const UNREAD_RENUDGE_GAP: Duration = Duration::from_secs(300);

/// The tier-1 inbox nudge typed into a present-but-deaf agent's pane.
const INBOX_NUDGE_TEXT: &str = "Inbox check (watchdog): unread BEADS messages are waiting for you — the push wake did not fire. Call get_messages now, process each message, then mark_as_read.";

/// One pass result: recipient name → oldest unread message age.
fn query_oldest_unread(work: &crate::daemons::RuntimeWork) -> Option<HashMap<String, Duration>> {
    let tools=work.tools().ok()?;
    let input=work.client(&tools.bd,vec!["list".into(),"--status=open".into(),"--type=message".into(),"--json".into(),"--limit=513".into()]).ok()?;
    let out = crate::daemons::run_client(work, input, Duration::from_secs(3), 1024*1024, 64*1024).ok()?;
    if !out.accepted { return None; }
    let rows: Vec<serde_json::Value> = serde_json::from_slice(&out.stdout).ok()?;
    if rows.len() > 512 { return None; }
    let at = now();
    let mut oldest: HashMap<String, Duration> = HashMap::new();
    for row in rows {
        // Title format: "[from->to] preview…" — recipient is between "->" and "]".
        let Some(title) = row.get("title").and_then(|v| v.as_str()) else {
            return None;
        };
        let Some(recipient) = title
            .split(']')
            .next()
            .and_then(|head| head.split("->").nth(1))
            .map(|r| r.trim().to_string())
        else {
            return None;
        };
        let Some(created) = row
            .get("created_at")
            .and_then(|v| v.as_str())
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        else {
            return None;
        };
        if !crate::daemon_registry::valid_name(&recipient) {return None;}
        let created_st: SystemTime = UNIX_EPOCH + Duration::from_millis(created.timestamp_millis().max(0) as u64);
        let age = at.duration_since(created_st).unwrap_or_default();
        let entry = oldest.entry(recipient).or_default();
        if age > *entry {
            *entry = age;
        }
    }
    Some(oldest)
}

fn unread_online(shared: &Shared, name: &str) -> bool {
    shared.subscriber_connected && shared.presence.get(name).is_some_and(|p| p.online)
}

fn unread_sweep_loop(shared: Arc<Mutex<Shared>>, app_state: Arc<Mutex<AppState>>, worker: crate::daemons::WorkerContext) {
    let mut last_nudge: HashMap<String, SystemTime> = HashMap::new();
    loop {
        if worker.wait(UNREAD_SWEEP_INTERVAL) { return; }

        let oldest = {
            let Ok(work) = worker.admit(None) else { continue; };
            let Ok(_body) = work.body() else { continue; };
            let Some(oldest) = query_oldest_unread(&work) else { continue; };
            oldest
        };
        if oldest.is_empty() {
            continue;
        }

        // Snapshot roster: running, non-codex Claude agents with a window.
        let roster: Vec<(String, Option<String>)> = {
            let Ok(s) = app_state.lock() else { continue };
            s.agents
                .values()
                .filter(|a| a.status == "running" && !a.model.starts_with("codex/"))
                .map(|a| (a.name.clone(), a.tmux_window_id.clone()))
                .collect()
        };

        let at = now();
        for (name, window_id) in roster {
            let Some(age) = oldest.get(&name) else { continue };
            if *age < UNREAD_DEAF_AGE {
                continue;
            }
            // Only nudge agents the hub currently believes are online — an
            // offline agent is the presence-deafness path's job (re-kick),
            // and nudging an empty pane is pointless.
            let online = {
                let Ok(s) = shared.lock() else { continue };
                unread_online(&s, &name)
            };
            if !online {
                continue;
            }
            if let Some(last) = last_nudge.get(&name) {
                if at.duration_since(*last).unwrap_or_default() < UNREAD_RENUDGE_GAP {
                    continue;
                }
            }
            let Some(win) = window_id else {
                continue;
            };
            let _ = win; // target is resolved afresh by the one guarded actuator
            if worker.stopped() { return; }
            let Ok(work) = worker.admit(Some(&name)) else { continue; };
            let Ok(_body) = work.body() else { continue; };
            if matches!(work.nudge(&app_state, &name, NudgeProducer::UnreadNudge, NudgeInputs::production()), Ok(DispatchOutcome::DispatchAccepted)) {
                last_nudge.insert(name, at);
            }

        }
    }
}

/// Clear a stopped agent's presence + re-kick state so a deliberate stop is
/// never fought and a later restart begins from a clean slate. Called from
/// `stop_agent`. Cheap no-op if the watchdog never saw the agent.
pub fn on_agent_stopped(name: &str) {
    // The kickoff file is the eligibility gate; stop_agent removes it directly
    // (belt-and-braces). This hook additionally resets in-memory state via the
    // shared handle when present. We expose it through a process-global so
    // stop_agent (a Tauri command with no watchdog handle) can reach it.
    if let Some(shared) = global_shared() {
        if let Ok(mut s) = shared.lock() {
            s.presence.remove(name);
            s.watch.remove(name);
        }
    }
}

// A process-global handle to the watchdog's shared state, so `stop_agent` (which
// has no direct handle) can clear an agent on stop. Set once at spawn.
static GLOBAL_SHARED: Mutex<Option<Arc<Mutex<Shared>>>> = Mutex::new(None);

fn global_shared() -> Option<Arc<Mutex<Shared>>> {
    GLOBAL_SHARED.lock().ok().and_then(|g| g.clone())
}

fn subscriber_loop(shared: Arc<Mutex<Shared>>, worker: crate::daemons::WorkerContext) {
    if let Ok(mut g) = GLOBAL_SHARED.lock() { *g = Some(shared.clone()); }
    let mut backoff = Duration::from_secs(1);
    while !worker.stopped() {
        if let Ok(mut stream) = worker.subscriber() {
            if let Ok(initial) = stream.take_initial() {
                if worker.stopped() { break; }
                let mut map = HashMap::new();
                for update in initial { apply_presence_event(&mut map, &update.agent, &update.event, now()); }
                if map.len() <= 256 {
                    if let Ok(mut s) = shared.lock() {
                        if !worker.stopped() {
                            s.presence = map;
                            s.subscriber_connected = true;
                            s.connected_since = Some(now());
                        }
                    }
                    backoff = Duration::from_secs(1);
                    while !worker.stopped() {
                        match worker.subscriber_next(&mut stream) {
                            Ok(None) => continue,
                            Ok(Some(update)) => {
                                let Ok(mut s) = shared.lock() else { break; };
                                if worker.stopped() { break; }
                                if !s.presence.contains_key(&update.agent) && s.presence.len() >= 256 { break; }
                                apply_presence_event(&mut s.presence, &update.agent, &update.event, now());
                            }
                            Err(_) => break,
                        }
                    }
                }
            }
        }
        mark_disconnected(&shared);
        if worker.wait(backoff) { break; }
        backoff = (backoff * 2).min(SUBSCRIBER_RECONNECT_MAX);
    }
    mark_disconnected(&shared);
    if let Ok(mut g) = GLOBAL_SHARED.lock() {
        if g.as_ref().is_some_and(|v| Arc::ptr_eq(v, &shared)) { *g = None; }
    }
}

fn mark_disconnected(shared: &Arc<Mutex<Shared>>) {
    if let Ok(mut s) = shared.lock() {
        s.subscriber_connected = false;
        s.connected_since = None;
        // Turn-state is only as fresh as the last frame we received; with the
        // subscriber down nothing can refresh it, so drop it fleet-wide. The
        // online flag itself is left for the tick's trustworthiness gate
        // (and is wiped outright by validated snapshot replacement on the next reconnect).
        for p in s.presence.values_mut() {
            p.turn = None;
        }
    }
}

/// Fold one presence event into the map. Pure (clock injected) so the
/// transitions are unit-testable without a socket.
///
/// join / busy / idle → positive presence; leave → gone. busy & idle both map
/// to online (turn-state is not a separate dot color) but are additionally
/// remembered as `turn` (aperture-ull4y): `busy` → Busy, `idle` → Idle, `join`
/// leaves the prior value untouched (None on a fresh entry), `leave` clears it.
fn apply_presence_event(presence: &mut HashMap<String, Presence>, agent: &str, event: &str, at: SystemTime) {
    let positive = matches!(event, "join" | "busy" | "idle");
    let p = presence.entry(agent.to_string()).or_default();
    if positive {
        // Only (re)start the debounce clock on a transition into online —
        // a steady busy/idle stream must not keep resetting stability.
        if !p.online {
            p.online = true;
            p.online_since = Some(at);
        }
        match event {
            "busy" => p.turn = Some(Turn::Busy),
            "idle" => p.turn = Some(Turn::Idle),
            _ => {} // join: keep whatever we knew
        }
    } else {
        p.online = false;
        p.online_since = None;
        p.turn = None;
    }
}

fn decision_loop(shared: Arc<Mutex<Shared>>, app_state: Arc<Mutex<AppState>>, worker: crate::daemons::WorkerContext) {
    while !worker.stopped() {
        tick(&shared, &app_state, &worker);
        if worker.wait(TICK_INTERVAL) { break; }
    }
}

/// One evaluation pass: recompute every running agent's dot, write it onto
/// `AgentDef`, and fire a re-kick if the silence deadline (and this attempt's
/// response window) has lapsed.
fn tick(shared: &Arc<Mutex<Shared>>, app_state: &Arc<Mutex<AppState>>, worker: &crate::daemons::WorkerContext) {
    if worker.stopped() { return; }
    let at = now();

    // Snapshot the roster: (name, model, running, window_id). Locking AppState
    // briefly; the actuator's blocking work happens AFTER we drop the lock.
    let roster: Vec<(String, String, bool, Option<String>)> = {
        let s = match app_state.lock() {
            Ok(s) => s,
            Err(_) => return,
        };
        s.agents
            .values()
            .map(|a| {
                (
                    a.name.clone(),
                    a.model.clone(),
                    a.status == "running",
                    a.tmux_window_id.clone(),
                )
            })
            .collect()
    };

    // Managed seats have no legacy tmux/kickoff requirement. Classify outside
    // both mutexes: unknown membership is read-only/unknown, never a re-kick.
    let home = worker.home().ok();
    let managed: HashMap<String, bool> = roster.iter().filter_map(|(name, _, _, _)| {
        let membership = home.as_deref().map(|home| crate::teams::classify_managed_seat(home, name));
        match membership {
            Some(Ok(None)) => None,
            Some(Ok(Some(crate::teams::ManagedSeatState::Active { .. }))) => Some((name.clone(), true)),
            _ => Some((name.clone(), false)),
        }
    }).collect();

    // Bounded filesystem reads happen before Shared is acquired. Invalid input
    // is not absence: leave that seat's budgets/projection/effects untouched.
    let mut kickoffs = HashMap::new();
    for (name, _, running, _) in &roster {
        if worker.stopped() { return; }
        if managed.contains_key(name) { continue; }
        let value = if *running {
            home.as_deref().map(|home| read_kickoff_millis(home, name))
                .unwrap_or(KickoffRead::Unverified)
        } else { KickoffRead::Absent };
        kickoffs.insert(name.clone(), value);
    }
    if worker.stopped() { return; }

    // Is presence trustworthy right now? (§5 subscriber-down pause + grace.)
    let (subscriber_ok, past_grace) = {
        let s = shared.lock().unwrap();
        let past_grace = s
            .connected_since
            .map(|c| at.duration_since(c).unwrap_or_default() >= RECONNECT_GRACE)
            .unwrap_or(false);
        (s.subscriber_connected, past_grace)
    };

    let mut rekicks: Vec<RekickOrder> = Vec::new();
    let mut dot_writes: Vec<DotWrite> = Vec::new();

    {
        let mut s = shared.lock().unwrap();
        if worker.stopped() { return; }
        for (name, model, running, window_id) in &roster {
            if let Some(active) = managed.get(name) {
                dot_writes.push(managed_presence_write(name, *active,
                    subscriber_ok && past_grace, s.presence.get(name), at));
                // Observation only: managed lifecycle is never a legacy watchdog action.
                continue;
            }
            let kickoff_millis = match kickoffs.get(name) {
                Some(KickoffRead::Millis(value)) => Some(*value),
                Some(KickoffRead::Absent) => None,
                Some(KickoffRead::Unverified) | None => continue,
            };

            // Reset the attempt counter when a newer kickoff appears (fresh boot
            // or a prior re-kick that took) — a new kickoff is a clean slate.
            {
                let w = s.watch.entry(name.clone()).or_default();
                if w.tracked_kickoff_millis != kickoff_millis {
                    w.tracked_kickoff_millis = kickoff_millis;
                    w.attempts = 0;
                    w.last_attempt_at = None;
                    w.latched = false;
                }
            }

            // Presence is only trustworthy when THIS watchdog's subscriber is
            // connected AND past the reconnect grace. Otherwise we neither trust
            // a (possibly stale) online flag nor declare silence — a running,
            // kicked-off agent shows amber (re-establishing). This prevents both
            // a false-red storm on a hub bounce and a stale-green lie during an
            // outage; re-kicks are independently suppressed in this window below.
            let trustworthy = subscriber_ok && past_grace;
            let dot = if trustworthy {
                compute_dot(kickoff_millis, s.presence.get(name), at)
            } else if kickoff_millis.is_some() {
                Dot::Booting
            } else {
                Dot::Spawned
            };

            // Write the dot fields for the frontend poll. Stopped/kickoff-less
            // agents get None (the frontend derives spawned/booting locally).
            // `since` is derived from the source clocks (dot_since), NOT the
            // tick time, so it only moves on a real transition; `turn_state`
            // is None unless the dot is online (turn_for).
            if *running && kickoff_millis.is_some() {
                let presence = s.presence.get(name);
                let since = dot_since(dot, kickoff_millis, presence).unwrap_or(at);
                dot_writes.push(DotWrite {
                    name: name.clone(),
                    dot_state: Some(dot.as_str().to_string()),
                    dot_state_since: Some(iso8601(since)),
                    kickoff_fired_at: kickoff_millis.map(|m| iso8601(millis_to_systemtime(m))),
                    turn_state: turn_for(dot, presence).map(|t| t.as_str().to_string()),
                });
            } else {
                dot_writes.push(DotWrite {
                    name: name.clone(),
                    dot_state: None,
                    dot_state_since: None,
                    kickoff_fired_at: None,
                    turn_state: None,
                });
            }

            // Healthy (stable-online) → reset attempts + clear any latch.
            if dot == Dot::Online {
                let w = s.watch.entry(name.clone()).or_default();
                w.attempts = 0;
                w.last_attempt_at = None;
                w.latched = false;
                continue;
            }

            // Only stuck agents are re-kick candidates. And only when presence
            // is trustworthy (subscriber connected AND past the reconnect grace)
            // — otherwise a hub bounce would trigger a fleet-wide false storm.
            if dot != Dot::Stuck || !running || !subscriber_ok || !past_grace {
                continue;
            }

            let w = s.watch.entry(name.clone()).or_default();
            if w.latched {
                continue; // budget spent; red is latched, operator already rung.
            }

            // Respect this attempt's response window before firing the next.
            if let Some(last) = w.last_attempt_at {
                let gap_idx = (w.attempts.saturating_sub(1)).min(2) as usize;
                let gap =
                    Duration::from_secs(BACKOFF_GAPS_SECS[gap_idx] + agent_jitter_secs(name));
                if at.duration_since(last).unwrap_or_default() < gap {
                    continue; // still inside the window — wait for a join.
                }
            }

            if w.attempts >= MAX_ATTEMPTS {
                // Budget spent and still no join → latch red + ring operator once.
                w.latched = true;
                rekicks.push(RekickOrder::RingOperator { name: name.clone() });
                continue;
            }

            // Fire the next attempt. Tier: attempt 1 = nudge, 2-3 = respawn.
            w.attempts += 1;
            w.last_attempt_at = Some(at);
            let is_codex = model.starts_with("codex/");
            let tier = if w.attempts == 1 && !is_codex {
                // Codex has no separate nudge tier (a clean turn-injection
                // re-kick isn't reachable from the Rust watchdog); it respawns.
                RekickTier::Nudge
            } else {
                RekickTier::Respawn
            };
            rekicks.push(RekickOrder::Rekick {
                name: name.clone(),
                is_codex,
                window_id: window_id.clone(),
                tier,
                attempt: w.attempts,
            });
        }
    } // shared lock dropped before any blocking actuator work.

    // Apply dot-field writes to AppState (frontend reads these on its 3s poll).
    if !worker.stopped() && !dot_writes.is_empty() {
        if let Ok(mut a) = app_state.lock() {
            for w in dot_writes {
                if worker.stopped() { return; }
                if let Some(agent) = a.agents.get_mut(&w.name) {
                    agent.dot_state = w.dot_state;
                    agent.dot_state_since = w.dot_state_since;
                    agent.kickoff_fired_at = w.kickoff_fired_at;
                    agent.turn_state = w.turn_state;
                }
            }
        }
    }

    // Execute actuator orders (blocking tmux / boot work) with no locks held.
    for order in rekicks {
        if worker.stopped() { return; }
        match order {
            RekickOrder::Rekick {
                name,
                is_codex,
                window_id,
                tier,
                attempt,
            } => {
                // Respawn remains denied. Nudge enters the SAME admitted
                // journaled actuator as unread sweep, never the legacy helper.
                if matches!(tier, RekickTier::Respawn) { continue; }
                let _ = (is_codex, window_id, attempt);
                let Ok(work) = worker.admit(Some(&name)) else { continue; };
                let Ok(_body) = work.body() else { continue; };
                let _ = work.nudge(app_state, &name, NudgeProducer::RekickNudge, NudgeInputs::production());
            },
            RekickOrder::RingOperator { name } => ring_operator(app_state, &name, worker),
        }
    }
}

/// One agent's per-tick presence fields, staged under the watchdog lock and
/// applied to `AgentDef` under the AppState lock (never both at once).
struct DotWrite {
    name: String,
    dot_state: Option<String>,
    dot_state_since: Option<String>,
    kickoff_fired_at: Option<String>,
    turn_state: Option<String>,
}

/// Managed presence comes from the authenticated hub, not terminal existence.
/// No kickoff timestamp is fabricated and loss of evidence clears the turn.
fn managed_presence_write(name: &str, active: bool, trustworthy: bool,
    presence: Option<&Presence>, at: SystemTime) -> DotWrite {
    let current = presence.filter(|p| active && trustworthy && p.online &&
        p.online_since.is_some_and(|since|
            at.duration_since(since).unwrap_or_default() >= ONLINE_DEBOUNCE));
    DotWrite {
        name: name.into(),
        dot_state: current.map(|_| "online".into()),
        dot_state_since: current.and_then(|p| p.online_since).map(iso8601),
        kickoff_fired_at: None,
        turn_state: current.and_then(|p| p.turn).map(|turn| turn.as_str().into()),
    }
}

#[derive(Clone, Copy)]
enum RekickTier {
    Nudge,
    Respawn,
}

enum RekickOrder {
    Rekick {
        name: String,
        is_codex: bool,
        window_id: Option<String>,
        tier: RekickTier,
        attempt: u8,
    },
    RingOperator {
        name: String,
    },
}

#[cfg(test)]
fn execute_rekick(
    app_state: &Arc<Mutex<AppState>>,
    name: &str,
    is_codex: bool,
    window_id: Option<&str>,
    tier: RekickTier,
    attempt: u8,
) {
    // A team seat cannot be nudged or torn down through the standing-seat
    // recovery path. In particular, checking only boot_agent_headless is too
    // late: Respawn already killed its window/app-server before that call.
    // Filesystem truth, not AppState or UI filtering, is authoritative.
    if crate::agents::require_legacy_lifecycle(name).is_err() {
        eprintln!("[watchdog] re-kick denied: E_TEAM_LIFECYCLE_REQUIRED");
        return;
    }
    let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) else { return; };
    execute_rekick_at(app_state, name, is_codex, window_id, tier, attempt, &home);
}
#[cfg(test)]
fn execute_rekick_at(
    app_state: &Arc<Mutex<AppState>>, name: &str, is_codex: bool,
    window_id: Option<&str>, tier: RekickTier, attempt: u8, home: &std::path::Path,
) {
    // Unjoined watchdog work receives no positive Codex authority, including
    // stale is_codex=false after a model change. No pane teardown on denial.
    if crate::agents::detached_codex_denied_at(home, app_state, name, is_codex).is_err() { return; }
    // C3: even Claude/no-history Respawn has no tracked internal authority.
    // The external boot wrapper would reacquire RuntimeOwner's held lease only
    // AFTER destructive pane effects. Deny the entire tier before those effects.
    // D must replace this with tracked admission/drain/join, not pass an Arc or
    // simply remove this fence. Nudge remains the existing non-lifecycle path.
    if matches!(tier, RekickTier::Respawn) { return; }
    match tier {
        RekickTier::Nudge => {
            // Claude nudge: run the boot-routine turn in the EXISTING pane so the
            // agent restarts its inbox monitor — context preserved. tmux_send_keys
            // types the text literally then sends Enter as a separate key; we add
            // a delayed bare Enter as belt-and-braces against the TUI paste race
            // (syepg). No-op if we somehow lost the window id.
            let Some(win) = window_id else {
                eprintln!("[watchdog] {name}: nudge skipped — no window id");
                return;
            };
            eprintln!("[watchdog] {name}: re-kick attempt {attempt} — NUDGE (send-keys boot turn)");
            let _ = rekick_tmux_send_keys(win.to_string(), crate::launcher::KICKOFF_TEXT.into());
            std::thread::sleep(Duration::from_millis(700));
            let _ = rekick_tmux_send_keys(win.to_string(), String::new());
        }
        RekickTier::Respawn => {
            // Retained legacy body is unreachable under the unconditional tier
            // denial above; it is NOT a viable internal boot under RuntimeOwner.
            eprintln!("[watchdog] {name}: re-kick attempt {attempt} — RESPAWN");
            if let Some(win) = window_id {
                let _ = rekick_tmux_send_keys(win.to_string(), "C-c".into());
                std::thread::sleep(Duration::from_millis(300));
                let _ = rekick_tmux_kill_window(win.to_string());
            }
            // Codex (including retained history) was denied before any pane effect.
            std::thread::sleep(Duration::from_millis(300));
            // This external wrapper reacquires authority. D must replace the
            // legacy path before Respawn can ever be admitted again.
            match rekick_boot_agent_headless(name) {
                Ok(win) => {
                    eprintln!("[watchdog] {name}: respawned, new window {win}");
                    // aperture-3x136: write the fresh window id back into
                    // AppState. boot_agent_headless runs stateless (CI-callable)
                    // so it can't do this itself — and without the writeback
                    // every subsequent respawn kills the long-dead OLD id
                    // (no-op) and orphans another window (the 4-windows-per-
                    // codex-agent incident, 2026-07-19).
                    if let Ok(mut a) = app_state.lock() {
                        if let Some(agent) = a.agents.get_mut(name) {
                            agent.tmux_window_id = Some(win);
                            agent.status = "running".into();
                        }
                    }
                }
                Err(e) => eprintln!("[watchdog] {name}: respawn failed: {e}"),
            }
        }
    }
}

// Test interception is at every native effect site, not a parallel decision
// algorithm. These legacy C3 oracles compile only for tests; production
// decision/unread paths use the single D admitted actuator above.
#[cfg(test)]
fn rekick_tmux_send_keys(window: String, text: String) -> Result<(), String> {
    #[cfg(test)] {
        let _ = (window, text);
        C3_REKICK_EFFECTS.with(|v| { let mut e = v.borrow_mut(); e.pane_keys += 1; e.pane_sentinel = "keys-sent"; });
        return Ok(());
    }
    #[cfg(not(test))] { crate::tmux::tmux_send_keys(window, text) }
}
#[cfg(test)]
fn rekick_tmux_kill_window(window: String) -> Result<(), String> {
    #[cfg(test)] {
        let _ = window;
        C3_REKICK_EFFECTS.with(|v| { let mut e = v.borrow_mut(); e.pane_kills += 1; e.pane_sentinel = "removed"; });
        return Ok(());
    }
    #[cfg(not(test))] { crate::tmux::tmux_kill_window(window) }
}
#[cfg(test)]
fn rekick_boot_agent_headless(name: &str) -> Result<String, String> {
    #[cfg(test)] {
        let _ = name;
        C3_REKICK_EFFECTS.with(|v| v.borrow_mut().external_boots += 1);
        return Err("E_FIXTURE_EXTERNAL_BOOT_INTERCEPTED".into());
    }
    #[cfg(not(test))] { crate::boot_agent_headless(name) }
}
#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct C3RekickEffects {
    pub pane_keys: usize,
    pub pane_kills: usize,
    pub external_boots: usize,
    pub pane_sentinel: &'static str,
}
#[cfg(test)]
impl Default for C3RekickEffects {
    fn default() -> Self { Self { pane_keys: 0, pane_kills: 0, external_boots: 0, pane_sentinel: "owned-pane-intact" } }
}
#[cfg(test)]
thread_local! {
    static C3_REKICK_EFFECTS: std::cell::RefCell<C3RekickEffects> = std::cell::RefCell::new(C3RekickEffects::default());
}

/// After the attempt budget is spent with no hub-join, escalate to the operator:
/// light the attention badge on the stuck agent's card (the idiomatic operator
/// alert) and log loudly. The red dot is already showing; this is the extra ring.
fn ring_operator(app_state: &Arc<Mutex<AppState>>, name: &str, worker: &crate::daemons::WorkerContext) {
    eprintln!(
        "[watchdog] {name}: STUCK after {MAX_ATTEMPTS} re-kick attempts with no hub presence — \
         latching red + ringing operator. Manual intervention needed."
    );
    if let Ok(mut a) = app_state.lock() {
        if worker.stopped() { return; }
        if let Some(agent) = a.agents.get_mut(name) {
            // attention_reason = "crash" (aperture-ull4y); overwrites a lit
            // "message" badge — see agents::light_attention for the precedence.
            crate::agents::light_attention(agent, crate::agents::AttentionReason::Crash);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kickoff_ago(secs: u64) -> Option<u64> {
        let now_millis = now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64;
        Some(now_millis - secs * 1000)
    }

    #[test]
    fn spawned_when_no_kickoff() {
        assert_eq!(compute_dot(None, None, now()).as_str(), "spawned");
    }

    #[test]
    fn booting_within_deadline_no_presence() {
        assert_eq!(compute_dot(kickoff_ago(10), None, now()).as_str(), "booting");
    }

    #[test]
    fn stuck_past_deadline_no_presence() {
        assert_eq!(compute_dot(kickoff_ago(90), None, now()).as_str(), "stuck");
    }

    #[test]
    fn online_wins_even_past_deadline_when_stable() {
        // Joined and held longer than the debounce → stable online, green, even
        // though 90s > the 60s deadline (online always wins).
        let p = Presence {
            online: true,
            online_since: Some(now() - (ONLINE_DEBOUNCE + Duration::from_secs(1))),
            turn: None,
        };
        assert_eq!(compute_dot(kickoff_ago(90), Some(&p), now()).as_str(), "online");
    }

    #[test]
    fn momentary_join_is_not_green_yet_debounce() {
        // Joined just now (< debounce): not stable → still booting (within
        // deadline), NOT green. This is the flap-proofing (C3).
        let p = Presence {
            online: true,
            online_since: Some(now()),
            turn: None,
        };
        assert_eq!(compute_dot(kickoff_ago(5), Some(&p), now()).as_str(), "booting");
    }

    #[test]
    fn flapping_past_deadline_reads_stuck_not_online() {
        // A momentary (un-stable) join past the deadline must read stuck, not a
        // deceptive green — the whole point of the debounce.
        let p = Presence {
            online: true,
            online_since: Some(now()),
            turn: None,
        };
        assert_eq!(compute_dot(kickoff_ago(90), Some(&p), now()).as_str(), "stuck");
    }

    #[test]
    fn left_agent_past_deadline_is_stuck() {
        let p = Presence {
            online: false,
            online_since: None,
            turn: None,
        };
        assert_eq!(compute_dot(kickoff_ago(90), Some(&p), now()).as_str(), "stuck");
    }

    #[test]
    fn jitter_is_bounded_and_deterministic() {
        for n in ["vance", "rex", "izzy", "cipher", "peppy", "scout", "wheatley"] {
            let j = agent_jitter_secs(n);
            assert!(j < JITTER_CEILING_SECS);
            assert_eq!(j, agent_jitter_secs(n)); // deterministic
        }
    }

    // ---- aperture-ull4y: hub turn-state carried through ----

    #[test]
    fn turn_state_busy_idle_leave_transitions() {
        let mut m: HashMap<String, Presence> = HashMap::new();
        let t0 = now();
        apply_presence_event(&mut m, "vance", "join", t0);
        assert!(m["vance"].online);
        assert_eq!(m["vance"].turn, None, "join alone carries no turn frame");

        apply_presence_event(&mut m, "vance", "busy", t0);
        assert_eq!(m["vance"].turn, Some(Turn::Busy));
        assert_eq!(m["vance"].online_since, Some(t0), "busy must not reset the debounce clock");

        apply_presence_event(&mut m, "vance", "idle", t0);
        assert_eq!(m["vance"].turn, Some(Turn::Idle));

        apply_presence_event(&mut m, "vance", "leave", t0);
        assert!(!m["vance"].online);
        assert_eq!(m["vance"].turn, None, "leave clears turn");
        assert_eq!(m["vance"].online_since, None);
    }

    #[test]
    fn join_after_busy_keeps_prior_turn() {
        let mut m: HashMap<String, Presence> = HashMap::new();
        apply_presence_event(&mut m, "rex", "busy", now());
        apply_presence_event(&mut m, "rex", "join", now());
        assert_eq!(m["rex"].turn, Some(Turn::Busy));
    }

    #[test]
    fn busy_frame_alone_creates_online_entry_with_turn() {
        // A busy frame with no prior join (e.g. subscriber reconnected mid-turn)
        // must both mark online and carry the turn.
        let mut m: HashMap<String, Presence> = HashMap::new();
        apply_presence_event(&mut m, "izzy", "busy", now());
        assert!(m["izzy"].online);
        assert_eq!(m["izzy"].turn, Some(Turn::Busy));
    }

    #[test]
    fn turn_state_is_none_unless_dot_is_online() {
        // Rule: turn_state must be None whenever dot_state != online, even if
        // the last frame we saw for the agent was busy/idle.
        let p = Presence {
            online: true,
            online_since: Some(now()),
            turn: Some(Turn::Busy),
        };
        assert_eq!(turn_for(Dot::Booting, Some(&p)), None);
        assert_eq!(turn_for(Dot::Stuck, Some(&p)), None);
        assert_eq!(turn_for(Dot::Spawned, Some(&p)), None);
        assert_eq!(turn_for(Dot::Online, Some(&p)), Some(Turn::Busy));
        assert_eq!(turn_for(Dot::Online, None), None);
        assert_eq!(Turn::Busy.as_str(), "busy");
        assert_eq!(Turn::Idle.as_str(), "idle");
    }

    // ---- aperture-ull4y: dot_state_since is the state's START, not the tick ----

    #[test]
    fn since_is_stable_across_ticks_in_same_state() {
        // Two ticks 5s apart, same state (booting) → identical `since`.
        let kickoff = kickoff_ago(10);
        let t1 = now();
        let t2 = t1 + Duration::from_secs(5);
        let d1 = compute_dot(kickoff, None, t1);
        let d2 = compute_dot(kickoff, None, t2);
        assert_eq!(d1.as_str(), "booting");
        assert_eq!(d2.as_str(), "booting");
        let s1 = dot_since(d1, kickoff, None).unwrap();
        let s2 = dot_since(d2, kickoff, None).unwrap();
        assert_eq!(s1, s2);
        assert_eq!(s1, millis_to_systemtime(kickoff.unwrap()));

        // Same for a stable-online agent: `since` is online_since both ticks.
        let joined = t1 - (ONLINE_DEBOUNCE + Duration::from_secs(1));
        let p = Presence {
            online: true,
            online_since: Some(joined),
            turn: Some(Turn::Idle),
        };
        let o1 = compute_dot(kickoff, Some(&p), t1);
        let o2 = compute_dot(kickoff, Some(&p), t2);
        assert_eq!(o1.as_str(), "online");
        assert_eq!(o2.as_str(), "online");
        assert_eq!(dot_since(o1, kickoff, Some(&p)), Some(joined));
        assert_eq!(dot_since(o2, kickoff, Some(&p)), Some(joined));
    }

    #[test]
    fn since_moves_on_transition_and_stuck_keeps_kickoff_clock() {
        let kickoff = kickoff_ago(10);
        let kickoff_at = millis_to_systemtime(kickoff.unwrap());
        let t1 = now();

        // booting (no presence) → online (stable join): since jumps from the
        // kickoff clock to the join clock.
        let booting = compute_dot(kickoff, None, t1);
        assert_eq!(dot_since(booting, kickoff, None), Some(kickoff_at));
        let joined = t1 - (ONLINE_DEBOUNCE + Duration::from_secs(1));
        let p = Presence {
            online: true,
            online_since: Some(joined),
            turn: None,
        };
        let online = compute_dot(kickoff, Some(&p), t1);
        assert_eq!(online.as_str(), "online");
        let since_online = dot_since(online, kickoff, Some(&p)).unwrap();
        assert_eq!(since_online, joined);
        assert_ne!(since_online, kickoff_at);

        // A NEW kickoff (re-kick / respawn) while still not online → since moves
        // to the new kickoff.
        let kickoff2 = kickoff_ago(2);
        let booting2 = compute_dot(kickoff2, None, t1);
        assert_eq!(booting2.as_str(), "booting");
        assert_ne!(dot_since(booting2, kickoff2, None), dot_since(booting, kickoff, None));

        // stuck keeps the kickoff clock (tooltip: "kickoff sent {N}s ago").
        let old = kickoff_ago(90);
        let stuck = compute_dot(old, None, t1);
        assert_eq!(stuck.as_str(), "stuck");
        assert_eq!(dot_since(stuck, old, None), Some(millis_to_systemtime(old.unwrap())));

        // spawned has no source clock → None (caller falls back to the tick).
        assert_eq!(dot_since(Dot::Spawned, None, None), None);
    }
}

#[cfg(test)]
mod managed_presence_tests {
    use super::*;
    #[test]
    fn managed_turn_does_not_require_a_terminal_or_legacy_kickoff() {
        let at = now();
        let mut presence = Presence { online: true,
            online_since: Some(at - ONLINE_DEBOUNCE), turn: Some(Turn::Busy) };
        let busy = managed_presence_write("team-worker", true, true, Some(&presence), at);
        assert_eq!(busy.turn_state.as_deref(), Some("busy"));
        assert_eq!(busy.dot_state.as_deref(), Some("online"));
        assert!(busy.kickoff_fired_at.is_none());
        presence.turn = Some(Turn::Idle);
        assert_eq!(managed_presence_write("team-worker", true, true, Some(&presence), at)
            .turn_state.as_deref(), Some("idle"));
    }
    #[test]
    fn managed_absent_untrusted_inactive_or_unstable_presence_is_unknown() {
        let at = now();
        for (active, trusted, online, stable) in [
            (false,true,true,true), (true,false,true,true),
            (true,true,false,true), (true,true,true,false),
        ] {
            let presence = Presence { online, online_since: Some(if stable {at-ONLINE_DEBOUNCE} else {at}), turn: Some(Turn::Busy) };
            let view = managed_presence_write("team-worker", active, trusted, Some(&presence), at);
            assert!(view.turn_state.is_none()); assert!(view.dot_state.is_none());
        }
        assert!(managed_presence_write("team-worker", true, true, None, at).turn_state.is_none());
        let joined = Presence { online: true, online_since: Some(at-ONLINE_DEBOUNCE), turn: None };
        assert!(managed_presence_write("team-worker", true, true, Some(&joined), at).turn_state.is_none());
    }
}

#[cfg(test)]
pub(crate) fn c3_rekick_fixture(state: &Arc<Mutex<AppState>>, name: &str, home: &std::path::Path, is_codex: bool) -> C3RekickEffects {
    C3_REKICK_EFFECTS.with(|v| *v.borrow_mut() = C3RekickEffects::default());
    execute_rekick_at(state, name, is_codex, Some("owned-fixture-pane"), RekickTier::Respawn, 1, home);
    C3_REKICK_EFFECTS.with(|v| v.borrow().clone())
}
#[cfg(test)]
pub(crate) fn c3_nudge_fixture(state: &Arc<Mutex<AppState>>, name: &str, home: &std::path::Path) -> C3RekickEffects {
    C3_REKICK_EFFECTS.with(|v| *v.borrow_mut() = C3RekickEffects::default());
    execute_rekick_at(state, name, false, Some("owned-fixture-pane"), RekickTier::Nudge, 1, home);
    C3_REKICK_EFFECTS.with(|v| v.borrow().clone())
}

// D's local immutable effect facts. Team transaction journal/schema is untouched.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct EffectTarget {
    window: String, pane: String, pid: u32, birth: String, uid: u32,
}
#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum NudgeProducer { RekickNudge, UnreadNudge }
#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub(crate) enum DispatchOutcome { NoDispatch, DispatchAccepted, Unknown }
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct DispatchObservation { spawned: bool, accepted: bool }
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
enum EffectFact {
    Intent { version: u8, id: String, seat: String, producer: NudgeProducer, payload: String,
        target: EffectTarget, plan: String, at_ms: u64 },
    Outcome { version: u8, id: String, seat: String, outcome: DispatchOutcome,
        observations: Vec<DispatchObservation>, at_ms: u64 },
}
static EFFECT_SERIAL: Mutex<()> = Mutex::new(());
struct EffectHistory { entries: usize, seats: usize, existing: bool, blocked: bool }
fn effect_now() -> Result<u64, String> {
    u64::try_from(now().duration_since(UNIX_EPOCH).map_err(|_| "E_WATCHDOG_CLOCK")?.as_millis())
        .map_err(|_| "E_WATCHDOG_CLOCK".into())
}
fn effect_id(v: &str) -> bool { uuid::Uuid::parse_str(v).is_ok_and(|id| id.to_string() == v) }
fn target_valid(t: &EffectTarget) -> bool {
    fn tmux_id(s: &str, prefix: char) -> bool {
        s.starts_with(prefix) && s.len() > 1 && s.len() <= 32 && s[1..].bytes().all(|b| b.is_ascii_digit())
    }
    tmux_id(&t.window, '@') && tmux_id(&t.pane, '%') && t.pid > 1 && t.pid <= i32::MAX as u32
        && !t.birth.is_empty() && t.birth.len() <= 64 && t.birth.bytes().all(|b| b.is_ascii_digit() || b == b'.')
        && t.uid == unsafe { libc::geteuid() }
}
fn read_effect(path: &std::path::Path) -> Result<EffectFact, String> {
    use std::os::unix::fs::{OpenOptionsExt, MetadataExt};
    use std::io::Read;
    let mut f = std::fs::OpenOptions::new().read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK).open(path)
        .map_err(|_| "E_WATCHDOG_FACT")?;
    let m = f.metadata().map_err(|_| "E_WATCHDOG_FACT")?;
    if !m.is_file() || m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o777 != 0o600
        || m.nlink() != 1 || m.len() > 8192 { return Err("E_WATCHDOG_FACT".into()); }
    let mut b = Vec::new();
    (&mut f).take(8193).read_to_end(&mut b).map_err(|_| "E_WATCHDOG_FACT")?;
    if b.len() > 8192 { return Err("E_WATCHDOG_FACT".into()); }
    serde_json::from_slice(&b).map_err(|_| "E_WATCHDOG_FACT".into())
}
fn effect_history(root: &std::path::Path, seat: &str, at: u64) -> Result<EffectHistory, String> {
    let mut h = EffectHistory { entries: 0, seats: 0, existing: false, blocked: false };
    match std::fs::symlink_metadata(root) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(h),
        Err(_) => return Err("E_WATCHDOG_PATH".into()),
        Ok(_) => {}
    }
    crate::controller::private_dir_readonly(root)?;
    for entry in std::fs::read_dir(root).map_err(|_| "E_WATCHDOG_PATH")? {
        let entry = entry.map_err(|_| "E_WATCHDOG_PATH")?;
        h.entries += 1; h.seats += 1;
        if h.entries > 512 || h.seats > 128 { return Err("E_WATCHDOG_CAPACITY".into()); }
        let name = entry.file_name().into_string().map_err(|_| "E_WATCHDOG_PATH")?;
        if !crate::daemon_registry::valid_name(&name) { return Err("E_WATCHDOG_PATH".into()); }
        crate::controller::private_dir_readonly(&entry.path())?;
        let mut intents = HashMap::new();
        let mut outcomes = HashMap::new();
        for file in std::fs::read_dir(entry.path()).map_err(|_| "E_WATCHDOG_PATH")? {
            h.entries += 1;
            if h.entries > 512 { return Err("E_WATCHDOG_CAPACITY".into()); }
            let file = file.map_err(|_| "E_WATCHDOG_PATH")?;
            let fname = file.file_name().into_string().map_err(|_| "E_WATCHDOG_FACT")?;
            match read_effect(&file.path())? {
                EffectFact::Intent { version, id, seat, producer, payload, target, plan, at_ms } => {
                    if version != 1 || !effect_id(&id) || seat != name || fname != format!("{id}.intent.json")
                        || !target_valid(&target) || payload != match producer { NudgeProducer::RekickNudge => "boot_nudge", NudgeProducer::UnreadNudge => "inbox_nudge" }
                        || plan.len() != 64 || !plan.bytes().all(|b| b.is_ascii_hexdigit())
                        || intents.insert(id, at_ms).is_some() { return Err("E_WATCHDOG_FACT".into()); }
                }
                EffectFact::Outcome { version, id, seat, outcome, observations, at_ms } => {
                    if version != 1 || !effect_id(&id) || seat != name || fname != format!("{id}.outcome.json")
                        || observations.len() > 4
                        || (outcome == DispatchOutcome::NoDispatch && observations.iter().any(|o| o.spawned))
                        || (outcome == DispatchOutcome::DispatchAccepted && (observations.len() != 4 || observations.iter().any(|o| !o.spawned || !o.accepted)))
                        || observations.iter().any(|o| o.accepted && !o.spawned)
                        || outcomes.insert(id, (outcome, at_ms)).is_some() { return Err("E_WATCHDOG_FACT".into()); }
                }
            }
        }
        if intents.is_empty() || outcomes.iter().any(|(id, (_, time))| intents.get(id).is_none_or(|start| time < start)) { return Err("E_WATCHDOG_FACT".into()); }
        if name == seat {
            h.existing = true;
            for id in intents.keys() {
                match outcomes.get(id) {
                    None | Some((DispatchOutcome::Unknown, _)) => h.blocked = true,
                    Some((DispatchOutcome::DispatchAccepted, then)) if at < *then || at - then < 300_000 => h.blocked = true,
                    _ => {}
                }
            }
        }
    }
    Ok(h)
}
fn write_effect(path: &std::path::Path, fact: &EffectFact) -> Result<(), String> {
    if serde_json::to_vec(fact).map_err(|_| "E_WATCHDOG_FACT")?.len() > 8192 { return Err("E_WATCHDOG_FACT".into()); }
    crate::journal::write_private_json_atomic(path, fact, false)
}
#[cfg(test)]
pub(crate) struct NudgeFixture {
    target: EffectTarget,
    clients: Vec<crate::daemons::ClientInput>,
    // Actual dispatch-boundary scheduling hook; finite channel, not a policy mock.
    pause: Option<(usize, std::sync::mpsc::SyncSender<()>, Mutex<std::sync::mpsc::Receiver<()>>)>,
    target_after: Option<(usize, EffectTarget)>,
    crash: Option<u8>,
}
pub(crate) struct NudgeInputs {
    #[cfg(test)] fixture: Option<NudgeFixture>,
}
impl NudgeInputs {
    pub(crate) fn production() -> Self { Self { #[cfg(test)] fixture: None } }
    fn target(&self, work: &crate::daemons::RuntimeWork, index: usize, window:&str) -> Result<EffectTarget, String> {
        #[cfg(test)] if let Some(f) = &self.fixture {
            if let Some((after, changed)) = &f.target_after { if index >= *after { return Ok(changed.clone()); } }
            return Ok(f.target.clone());
        }
        let _ = index;
        if window.len()>16 || !window.starts_with('@') || window.len()<2 || !window[1..].bytes().all(|b|b.is_ascii_digit()){return Err("E_WATCHDOG_TARGET".into());}
        let bytes=crate::tmux::local_output(work,vec!["display-message".into(),"-p".into(),"-t".into(),window.into(),"#{window_id}|#{pane_id}|#{pane_pid}".into()])?;
        let text=std::str::from_utf8(&bytes).map_err(|_|"E_WATCHDOG_TARGET")?.trim_end_matches('\n');
        let fields=text.split('|').collect::<Vec<_>>();
        if fields.len()!=3 || fields[0]!=window{return Err("E_WATCHDOG_TARGET".into());}
        let pid=fields[2].parse::<u32>().map_err(|_|"E_WATCHDOG_TARGET")?;
        let native=crate::team_process::observe(pid).map_err(|_|"E_WATCHDOG_TARGET")?.ok_or("E_WATCHDOG_TARGET")?;
        let target=EffectTarget{window:window.into(),pane:fields[1].into(),pid,birth:native.identity.start_time,uid:native.uid};
        if !target_valid(&target){return Err("E_WATCHDOG_TARGET".into());}Ok(target)
    }
    fn client(&self, work:&crate::daemons::RuntimeWork,index: usize,target:&EffectTarget,producer:NudgeProducer) -> Result<crate::daemons::ClientInput, String> {
        #[cfg(test)] if let Some(f) = &self.fixture { return f.clients.get(index).cloned().ok_or_else(|| "E_FIXTURE_CLIENT".into()); }
        if index>=4{return Err("E_WATCHDOG_TARGET".into());}
        let mut args=vec!["send-keys".into(),"-t".into(),target.pane.clone()];
        if index%2==0 {
            args.push("-l".into());
            args.push(if index==2 {String::new()} else {match producer {NudgeProducer::RekickNudge=>crate::launcher::KICKOFF_TEXT.into(),NudgeProducer::UnreadNudge=>INBOX_NUDGE_TEXT.into()}});
        } else {args.push("Enter".into());}
        work.client(&work.tools()?.tmux,args)
    }
}
fn effect_plan(agent: &crate::state::AgentDef) -> Result<String, String> {
    use sha2::Digest;
    let bytes = serde_json::to_vec(&(agent.name.as_str(), agent.model.as_str(), agent.role.as_str(),
        agent.prompt_file.as_str(), agent.tmux_window_id.as_ref(), agent.status.as_str())).map_err(|_| "E_WATCHDOG_PLAN")?;
    Ok(format!("{:x}", sha2::Sha256::digest(bytes)))
}
pub(crate) fn guarded_nudge(
    lease: &crate::controller::ControllerLock, work: &crate::daemons::RuntimeWork,
    state: &Arc<Mutex<AppState>>, seat: &str, producer: NudgeProducer, inputs: NudgeInputs,
) -> Result<DispatchOutcome, String> {
    work.check_open()?;
    let home = lease.run_dir()?.parent().and_then(std::path::Path::parent).ok_or("E_WATCHDOG_PATH")?;
    crate::agents::detached_codex_denied_at(home, state, seat, false)?;
    let plan = state.lock().map_err(|_| "E_WATCHDOG_PLAN")?.agents.get(seat).ok_or("E_WATCHDOG_PLAN")?.clone();
    let fingerprint = effect_plan(&plan)?;
    let slot = lease.codex_slot(seat)?;
    let _operation = slot.enter()?;
    let _serial = EFFECT_SERIAL.lock().map_err(|_| "E_WATCHDOG_JOURNAL_UNAVAILABLE")?;
    let target = inputs.target(work, 0,plan.tmux_window_id.as_deref().ok_or("E_WATCHDOG_TARGET")?)?;
    if !target_valid(&target) || plan.tmux_window_id.as_deref() != Some(&target.window) { return Err("E_WATCHDOG_TARGET".into()); }
    let recheck = |index| -> Result<(), String> {
        work.check_open()?;
        crate::agents::detached_codex_denied_at(home, state, seat, false)?;
        let actual = state.lock().map_err(|_| "E_WATCHDOG_PLAN")?.agents.get(seat).ok_or("E_WATCHDOG_PLAN")?.clone();
        if effect_plan(&actual)? != fingerprint || inputs.target(work, index,&target.window)? != target { return Err("E_WATCHDOG_TARGET".into()); }
        let expected = crate::team_replacement::ProcessIdentity { pid: target.pid, start_time: target.birth.clone() };
        if crate::team_process::state(&expected) != crate::team_replacement::ProcessState::Same
            || !crate::team_process::observe(target.pid).ok().flatten().is_some_and(|p| p.identity == expected && p.uid == target.uid) { return Err("E_WATCHDOG_TARGET".into()); }
        Ok(())
    };
    recheck(0)?;
    let _trusted_input = inputs.client(work,0,&target,producer)?; // prerequisite before intent, no command spawned
    let root = lease.run_dir()?.join("watchdog");
    let at = effect_now()?;
    let history = effect_history(&root, seat, at)?;
    if history.blocked { return Err("E_WATCHDOG_UNRESOLVED_OR_COOLDOWN".into()); }
    let needed = if history.existing { 2 } else { 3 };
    if history.entries + needed > 512 || (!history.existing && history.seats >= 128) { return Err("E_WATCHDOG_CAPACITY".into()); }
    crate::journal::ensure_private_dir(&root)?;
    let dir = root.join(seat);
    crate::journal::ensure_private_dir(&dir)?;
    let id = uuid::Uuid::new_v4().to_string();
    write_effect(&dir.join(format!("{id}.intent.json")), &EffectFact::Intent {
        version: 1, id: id.clone(), seat: seat.into(), producer,
        payload: match producer { NudgeProducer::RekickNudge => "boot_nudge", NudgeProducer::UnreadNudge => "inbox_nudge" }.into(),
        target: target.clone(), plan: fingerprint.clone(), at_ms: at,
    })?;
    #[cfg(test)] if inputs.fixture.as_ref().is_some_and(|f| f.crash == Some(0)) { std::process::exit(73); }
    let until = std::time::Instant::now() + Duration::from_millis(8700);
    let mut observations = Vec::new();
    for index in 0..4 {
        #[cfg(test)] if let Some((point, arrived, resume)) = inputs.fixture.as_ref().and_then(|f| f.pause.as_ref()) {
            if *point == index {
                arrived.send(()).map_err(|_| "E_FIXTURE_CHANNEL")?;
                resume.lock().unwrap().recv_timeout(Duration::from_secs(3)).map_err(|_| "E_FIXTURE_DEADLINE")?;
            }
        }
        if index == 2 && work.wait_open(Duration::from_millis(700)).is_err() { break; }
        if recheck(index).is_err() { break; }
        let left = until.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() { break; }
        let input = match inputs.client(work,index,&target,producer) { Ok(v) => v, Err(_) => break };
        match crate::daemons::run_client(work, input, left.min(Duration::from_secs(2)), 64*1024, 64*1024) {
            Ok(result) => {
                observations.push(DispatchObservation { spawned: result.spawned, accepted: result.accepted });
                if !result.accepted { break; }
            }
            Err(_) => {
                // Retention/setup failure after spawn cannot be proven NoDispatch.
                observations.push(DispatchObservation { spawned: true, accepted: false });
                break;
            }
        }
    }
    #[cfg(test)] if inputs.fixture.as_ref().is_some_and(|f| f.crash == Some(1)) { std::process::exit(74); }
    let outcome = if !observations.iter().any(|o| o.spawned) { DispatchOutcome::NoDispatch }
        else if observations.len() == 4 && observations.iter().all(|o| o.accepted) { DispatchOutcome::DispatchAccepted }
        else { DispatchOutcome::Unknown };
    lease.verify_live()?;
    write_effect(&dir.join(format!("{id}.outcome.json")), &EffectFact::Outcome {
        version: 1, id, seat: seat.into(), outcome, observations, at_ms: effect_now()?,
    })?;
    Ok(outcome)
}

#[cfg(test)]
#[path = "watchdog_effect_tests.rs"]
mod effect_tests;
