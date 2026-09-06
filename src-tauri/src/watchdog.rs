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
use tungstenite::Message;

const HUB_URL: &str = "ws://127.0.0.1:4517";

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

fn run_dir() -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    format!("{}/.aperture/run", home)
}

fn kickoff_path(name: &str) -> String {
    format!("{}/{}.kickoff", run_dir(), name)
}

/// Read the kickoff-fired timestamp (epoch millis) the launcher persisted for
/// this agent. `None` = never kicked off (not yet eligible).
fn read_kickoff_millis(name: &str) -> Option<u64> {
    std::fs::read_to_string(kickoff_path(name))
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
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

/// Public entry: spawn the watchdog (subscriber thread + decision-loop thread).
/// Called once from `lib.rs::run` after the hub is spawned.
pub fn spawn_watchdog(app_state: Arc<Mutex<AppState>>) {
    let shared = Arc::new(Mutex::new(Shared::new()));

    // Subscriber thread — blocking sync WS client (tungstenite), quiet bounded
    // reconnect. Owns only presence facts + the connected flag.
    {
        let shared = Arc::clone(&shared);
        std::thread::spawn(move || subscriber_loop(shared));
    }

    // Decision-loop thread — recomputes dots and evaluates re-kicks on a tick.
    {
        let shared = Arc::clone(&shared);
        let app_state = Arc::clone(&app_state);
        std::thread::spawn(move || decision_loop(shared, app_state));
    }

    // Unread-age sweep thread (aperture-mler9) — the present-but-deaf guard.
    // Presence-deafness (above) catches a DISCONNECTED monitor; this catches
    // the connected-but-never-woken session: the hub forwards, the hub-client
    // prints, the Monitor captures to its output file — and the harness never
    // re-invokes the idle session (observed 2026-07-19 on opus/sonnet panes
    // while fable panes woke fine). The agent sits green with unread messages
    // aging indefinitely. Sweep: if a running, hub-online, non-codex agent's
    // oldest unread BEADS message exceeds UNREAD_DEAF_AGE, type a tier-1
    // inbox nudge into its pane (same actuator as the boot nudge). Isolated in
    // its own thread so a slow/hung `bd` can only ever stall the sweep itself.
    {
        let shared = Arc::clone(&shared);
        std::thread::spawn(move || unread_sweep_loop(shared, app_state));
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
fn query_oldest_unread() -> Option<HashMap<String, Duration>> {
    let home = std::env::var("HOME").ok()?;
    let out = std::process::Command::new("bd")
        .args(["query", "type=message AND status=open", "--json", "-n", "0"])
        .env("BEADS_DIR", format!("{}/.aperture/.beads", home))
        .env("BD_ACTOR", "watchdog")
        .env(
            "PATH",
            format!(
                "/opt/homebrew/bin:/usr/local/bin:{}",
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let rows: Vec<serde_json::Value> = serde_json::from_slice(&out.stdout).ok()?;
    let at = now();
    let mut oldest: HashMap<String, Duration> = HashMap::new();
    for row in rows {
        // Title format: "[from->to] preview…" — recipient is between "->" and "]".
        let Some(title) = row.get("title").and_then(|v| v.as_str()) else {
            continue;
        };
        let Some(recipient) = title
            .split(']')
            .next()
            .and_then(|head| head.split("->").nth(1))
            .map(|r| r.trim().to_string())
        else {
            continue;
        };
        let Some(created) = row
            .get("created_at")
            .and_then(|v| v.as_str())
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        else {
            continue;
        };
        let created_st: SystemTime = UNIX_EPOCH + Duration::from_millis(created.timestamp_millis().max(0) as u64);
        let age = at.duration_since(created_st).unwrap_or_default();
        let entry = oldest.entry(recipient).or_default();
        if age > *entry {
            *entry = age;
        }
    }
    Some(oldest)
}

fn unread_sweep_loop(shared: Arc<Mutex<Shared>>, app_state: Arc<Mutex<AppState>>) {
    let mut last_nudge: HashMap<String, SystemTime> = HashMap::new();
    loop {
        std::thread::sleep(UNREAD_SWEEP_INTERVAL);

        let Some(oldest) = query_oldest_unread() else {
            continue; // bd unavailable this pass — try again next interval.
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
                s.presence.get(&name).map(|p| p.online).unwrap_or(false)
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
            eprintln!(
                "[watchdog] {name}: present-but-deaf — oldest unread {}s old, nudging inbox check",
                age.as_secs()
            );
            let _ = crate::tmux::tmux_send_keys(win.clone(), INBOX_NUDGE_TEXT.into());
            std::thread::sleep(Duration::from_millis(700));
            let _ = crate::tmux::tmux_send_keys(win, String::new());
            last_nudge.insert(name, at);
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

fn subscriber_loop(shared: Arc<Mutex<Shared>>) {
    // Register the global handle for stop_agent's on_agent_stopped hook.
    if let Ok(mut g) = GLOBAL_SHARED.lock() {
        *g = Some(Arc::clone(&shared));
    }

    let mut backoff = Duration::from_secs(1);
    loop {
        match tungstenite::connect(HUB_URL) {
            Ok((mut socket, _resp)) => {
                let token_path = match crate::hub_auth::token_path("watchdog") {
                    Ok(p) => p,
                    Err(_) => { mark_disconnected(&shared); std::thread::sleep(backoff); continue; }
                };
                let token = match std::fs::read_to_string(token_path) {
                    Ok(t) => t,
                    Err(_) => { mark_disconnected(&shared); std::thread::sleep(backoff); continue; }
                };
                let hello = serde_json::json!({
                    "type": "hello", "role": "subscriber", "agent": "watchdog", "token": token
                }).to_string();
                if socket.send(Message::Text(hello.into())).is_err() {
                    mark_disconnected(&shared);
                    std::thread::sleep(backoff);
                    continue;
                }
                mark_connected(&shared);
                backoff = Duration::from_secs(1); // reset on a good connect

                // Blocking read loop — one presence frame per hub broadcast.
                loop {
                    match socket.read() {
                        Ok(Message::Text(txt)) => handle_presence_frame(&shared, txt.as_str()),
                        Ok(Message::Ping(_)) | Ok(Message::Pong(_)) => {}
                        Ok(Message::Close(_)) => break,
                        Ok(_) => {}
                        Err(_) => break, // socket dropped → reconnect
                    }
                }
                mark_disconnected(&shared);
            }
            Err(_) => {
                // Hub down / unreachable. Quiet, bounded backoff (C1: never a
                // tight reconnect loop, never a per-failure log flood).
                mark_disconnected(&shared);
            }
        }
        std::thread::sleep(backoff);
        backoff = (backoff * 2).min(SUBSCRIBER_RECONNECT_MAX);
    }
}

fn mark_connected(shared: &Arc<Mutex<Shared>>) {
    if let Ok(mut s) = shared.lock() {
        if !s.subscriber_connected {
            s.subscriber_connected = true;
            s.connected_since = Some(now());
            // On (re)connect the hub's presence is FRESH — nobody is registered
            // until they re-hello. Clear stale presence so an agent that did NOT
            // come back (genuinely dead across the bounce) can't retain a
            // stale-online flag and escape re-kick (the deceptive-green failure
            // this epic exists to kill). RECONNECT_GRACE gives live agents time
            // to re-hello before re-kicks resume.
            s.presence.clear();
        }
    }
}

fn mark_disconnected(shared: &Arc<Mutex<Shared>>) {
    if let Ok(mut s) = shared.lock() {
        s.subscriber_connected = false;
        s.connected_since = None;
        // Turn-state is only as fresh as the last frame we received; with the
        // subscriber down nothing can refresh it, so drop it fleet-wide. The
        // online flag itself is left for the tick's trustworthiness gate
        // (and is wiped outright by mark_connected on the next reconnect).
        for p in s.presence.values_mut() {
            p.turn = None;
        }
    }
}

/// Parse one `{type:"presence", agent, event}` frame and fold it into presence.
fn handle_presence_frame(shared: &Arc<Mutex<Shared>>, txt: &str) {
    let Ok(val) = serde_json::from_str::<serde_json::Value>(txt) else {
        return;
    };
    if val.get("type").and_then(|v| v.as_str()) != Some("presence") {
        return;
    }
    let Some(agent) = val.get("agent").and_then(|v| v.as_str()) else {
        return;
    };
    let Some(event) = val.get("event").and_then(|v| v.as_str()) else {
        return;
    };

    if let Ok(mut s) = shared.lock() {
        apply_presence_event(&mut s.presence, agent, event, now());
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

fn decision_loop(shared: Arc<Mutex<Shared>>, app_state: Arc<Mutex<AppState>>) {
    loop {
        tick(&shared, &app_state);
        std::thread::sleep(TICK_INTERVAL);
    }
}

/// One evaluation pass: recompute every running agent's dot, write it onto
/// `AgentDef`, and fire a re-kick if the silence deadline (and this attempt's
/// response window) has lapsed.
fn tick(shared: &Arc<Mutex<Shared>>, app_state: &Arc<Mutex<AppState>>) {
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
        for (name, model, running, window_id) in &roster {
            let kickoff_millis = if *running { read_kickoff_millis(name) } else { None };

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
    if !dot_writes.is_empty() {
        if let Ok(mut a) = app_state.lock() {
            for w in dot_writes {
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
        match order {
            RekickOrder::Rekick {
                name,
                is_codex,
                window_id,
                tier,
                attempt,
            } => execute_rekick(app_state, &name, is_codex, window_id.as_deref(), tier, attempt),
            RekickOrder::RingOperator { name } => ring_operator(app_state, &name),
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

fn execute_rekick(
    app_state: &Arc<Mutex<AppState>>,
    name: &str,
    is_codex: bool,
    window_id: Option<&str>,
    tier: RekickTier,
    attempt: u8,
) {
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
            let _ = crate::tmux::tmux_send_keys(win.to_string(), crate::launcher::KICKOFF_TEXT.into());
            std::thread::sleep(Duration::from_millis(700));
            let _ = crate::tmux::tmux_send_keys(win.to_string(), String::new());
        }
        RekickTier::Respawn => {
            // Respawn: a wedged agent won't answer a nudge, so tear the pane down
            // and re-boot fresh (context is already forfeit). For Codex, a FULL
            // app-server teardown forces the bridge to create + publish a fresh
            // thread-id — sidestepping the surviving-app-server / stale-thread-id
            // edge without a bridge change (GLaDOS's edge, wisp-gsrc0s).
            eprintln!("[watchdog] {name}: re-kick attempt {attempt} — RESPAWN");
            if let Some(win) = window_id {
                let _ = crate::tmux::tmux_send_keys(win.to_string(), "C-c".into());
                std::thread::sleep(Duration::from_millis(300));
                let _ = crate::tmux::tmux_kill_window(win.to_string());
            }
            if is_codex {
                crate::codex_appserver::stop_app_server(name);
                // Remove the socket + thread-id handoff files so the fresh boot's
                // spawn_app_server probe (r8n62) can't reuse a stale/orphaned
                // server and the pane won't read a stale thread id.
                let base = format!("{}/{}", run_dir(), name);
                let _ = std::fs::remove_file(format!("{base}.sock"));
                let _ = std::fs::remove_file(format!("{base}.thread-id"));
            }
            std::thread::sleep(Duration::from_millis(300));
            // In-process boot (keeps the child mapped — no new orphan) through
            // the real spawn path: fresh window + kickoff, which rewrites the
            // .kickoff file → the next tick sees a newer timestamp and resets
            // this agent's attempt counter to a clean slate.
            match crate::boot_agent_headless(name) {
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

/// After the attempt budget is spent with no hub-join, escalate to the operator:
/// light the attention badge on the stuck agent's card (the idiomatic operator
/// alert) and log loudly. The red dot is already showing; this is the extra ring.
fn ring_operator(app_state: &Arc<Mutex<AppState>>, name: &str) {
    eprintln!(
        "[watchdog] {name}: STUCK after {MAX_ATTEMPTS} re-kick attempts with no hub presence — \
         latching red + ringing operator. Manual intervention needed."
    );
    if let Ok(mut a) = app_state.lock() {
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
