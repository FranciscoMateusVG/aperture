/**
 * Aperture comms-layer v2 — Phase 1 WS hub (aperture-bus delivery transport).
 *
 * Standalone singleton daemon (`node dist/ws-hub.js`). The per-agent MCP
 * servers are many (one process per agent); this hub is ONE process that owns
 * the WebSocket delivery plane:
 *
 *   - role=agent      → Claude Monitors. Presence-registered by agent name
 *                       (one socket per name; a new connection replaces the
 *                       old). On connect, unread BEADS messages are replayed.
 *   - role=subscriber → presence watchers (GLaDOS, launcher UI). Receive
 *                       {type:"presence", agent, event, ts} with event one of
 *                       "join"|"leave"|"busy"|"idle" (busy/idle come from
 *                       Codex bridge turn state; see codex-bridge.ts).
 *   - role=producer   → MCP send-queue drains. Send
 *                       {type:"notify", to, id, from, preview}; the hub
 *                       forwards {type:"message", id, from, preview} to the
 *                       recipient if connected and always acks
 *                       {type:"ok", id, outcome} with outcome one of
 *                       "forwarded" (Monitor socket got the push), "codex"
 *                       (injected into the Codex bridge), "offline" (nobody
 *                       to push to — unread replay on reconnect covers it).
 *
 * Delivery semantics: at-least-once, idempotent by message id. A missed push
 * (recipient offline, hub down) is covered by unread replay on reconnect —
 * BEADS remains the store of record; this is transport only.
 *
 * Presence is also mirrored to ~/.aperture/run/presence.json on every change
 * (see presence-snapshot.ts) so MCP servers can answer "who is online" without
 * a hub round-trip. Cleared to zero agents at startup.
 *
 * Env:
 *   APERTURE_WS_PORT        — listen port (default 4517, loopback only)
 *   APERTURE_HUB_SKIP_REPLAY=1 — skip the bd unread-replay on agent connect
 *                                (testing hook; smoke tests have no BEADS)
 *   APERTURE_RUN_DIR        — where presence.json lands (default ~/.aperture/run)
 */
import { WebSocketServer, WebSocket } from "ws";
import { constants, closeSync, fstatSync, openSync, readFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import { createHash, timingSafeEqual } from "node:crypto";
import { getUnreadMessages } from "./beads.js";
import { startCodexBridges, type PresenceEvent } from "./codex-bridge.js";
import { writePresenceSnapshot, PRESENCE_FILE, type PresenceEntry, type PresenceState } from "./presence-snapshot.js";

const HOST = "127.0.0.1";
const PORT = Number(process.env.APERTURE_WS_PORT ?? 4517);
const SKIP_REPLAY = process.env.APERTURE_HUB_SKIP_REPLAY === "1";
const HEARTBEAT_MS = 30_000;
const MAX_FRAME_BYTES = 16 * 1024;
const TOKEN_DIR = process.env.APERTURE_HUB_TOKEN_DIR ?? join(homedir(), ".aperture", "run", "hub-tokens");
const AGENT_NAME = /^[a-z0-9][a-z0-9_-]{0,63}$/;

type Role = "agent" | "subscriber" | "producer";

interface Conn {
  role: Role | null; // null until a valid hello arrives
  agent: string | null;
  isAlive: boolean;
}

const conns = new Map<WebSocket, Conn>();
/** Presence map: agent name → its (single) live socket. */
const agents = new Map<string, WebSocket>();
/**
 * Latest presence event per agent (aperture-3x136). The hub previously only
 * BROADCAST presence and never stored it, so a subscriber connecting (or
 * reconnecting) AFTER an agent's join/busy/idle had no way to learn the agent
 * was present. This bit the watchdog subscriber: on any hub restart the codex
 * bridges re-join, but if the watchdog reconnects a beat later it misses those
 * joins — and since a codex bridge lives inside the hub, respawning the pane
 * never re-emits its join, so the watchdog false-re-kicked idle codex agents
 * forever. We now snapshot this map to every subscriber on hello.
 *
 * aperture-oeb6q: the map now holds {state, since} (the presence-snapshot
 * contract) instead of the raw last event, and is mirrored to presence.json
 * after every mutation. Rules: "join" → "online" only if the agent is NOT
 * already present (a re-join, e.g. agent_replaced, keeps state + since);
 * "busy"/"idle" → that state; "leave" → delete. `since` moves ONLY on a state
 * transition — a repeated "busy" frame must not bump it.
 */
const presenceState = new Map<string, PresenceEntry>();

/** Structured single-line JSON logging to stderr. */
function log(event: string, fields: Record<string, unknown> = {}): void {
  process.stderr.write(
    JSON.stringify({ ts: new Date().toISOString(), event, ...fields }) + "\n",
  );
}

function send(ws: WebSocket, obj: unknown): void {
  if (ws.readyState === WebSocket.OPEN) {
    ws.send(JSON.stringify(obj));
  }
}

/** Apply a presence event to the state map. Returns true if anything changed. */
function applyPresence(agent: string, event: PresenceEvent, ts: string): boolean {
  if (event === "leave") return presenceState.delete(agent);
  const current = presenceState.get(agent);
  if (event === "join") {
    if (current) return false; // already present: keep state + since
    presenceState.set(agent, { state: "online", since: ts });
    return true;
  }
  const next: PresenceState = event; // "busy" | "idle"
  if (current && current.state === next) return false; // repeated frame: since untouched
  presenceState.set(agent, { state: next, since: ts });
  return true;
}

/** Mirror the state map to presence.json (atomic; best-effort, logged on failure). */
function persistPresence(): void {
  const agents: Record<string, PresenceEntry> = {};
  for (const [name, entry] of presenceState) agents[name] = { ...entry };
  const ok = writePresenceSnapshot({ hub_pid: process.pid, updated_at: new Date().toISOString(), agents });
  if (!ok) log("presence_snapshot_write_failed", { file: PRESENCE_FILE });
}

/** Subscriber wire format is the EVENT name, not the stored state — the Rust
 *  watchdog consumes {type:"presence", agent, event, ts} and must not notice
 *  the storage change. "online" maps back to "join". */
function stateToEvent(state: PresenceState): PresenceEvent {
  return state === "online" ? "join" : state;
}

function broadcastPresence(agent: string, event: PresenceEvent): void {
  const ts = new Date().toISOString();
  // Remember current state so a later subscriber hello can be snapshotted,
  // and mirror it to disk for the MCP servers' get_presence.
  if (applyPresence(agent, event, ts)) persistPresence();
  const msg = { type: "presence", agent, event, ts };
  for (const [ws, conn] of conns) {
    if (conn.role === "subscriber") send(ws, msg);
  }
  log("presence", { agent, presence: event });
}

/** Read a launcher-provisioned token without following symlinks or accepting
 * group/world-readable credentials. The value is never included in logs. */
function readAgentToken(agent: string): Buffer | null {
  if (!AGENT_NAME.test(agent)) return null;
  let fd: number | null = null;
  try {
    fd = openSync(join(TOKEN_DIR, `${agent}.token`), constants.O_RDONLY | constants.O_NOFOLLOW);
    const stat = fstatSync(fd);
    if (!stat.isFile() || (stat.mode & 0o077) !== 0) return null;
    if (typeof process.getuid === "function" && stat.uid !== process.getuid()) return null;
    const token = readFileSync(fd);
    return token.length >= 32 && token.length <= 256 ? token : null;
  } catch {
    return null;
  } finally {
    if (fd !== null) closeSync(fd);
  }
}

function validAgentToken(agent: string, presented: unknown): boolean {
  if (typeof presented !== "string" || presented.length > 256) return false;
  const expected = readAgentToken(agent);
  if (!expected) return false;
  // Compare fixed-length digests so token length is not observable through an
  // early-return timing difference.
  const actualDigest = createHash("sha256").update(presented).digest();
  const expectedDigest = createHash("sha256").update(expected).digest();
  return timingSafeEqual(actualDigest, expectedDigest);
}

/**
 * Replay unread BEADS messages to a freshly connected agent.
 * Reuses the MCP server's unread-query (shells out to `bd` via beads.ts).
 * Failure is non-fatal: the agent can always pull via get_messages.
 */
async function replayUnread(agent: string, ws: WebSocket): Promise<void> {
  if (SKIP_REPLAY) {
    log("replay_skipped", { agent });
    return;
  }
  try {
    const raw = await getUnreadMessages(agent);
    const rows = JSON.parse(raw);
    if (!Array.isArray(rows)) {
      log("replay", { agent, count: 0 });
      return;
    }
    let count = 0;
    for (const r of rows as Record<string, unknown>[]) {
      if (typeof r.id !== "string") continue;
      const title = typeof r.title === "string" ? r.title : "";
      const from = title.match(/\[(.+?)->(.+?)\]/)?.[1] ?? "unknown";
      const body = typeof r.description === "string" ? r.description : "";
      const preview = body.slice(0, 60).replace(/\n/g, " ");
      send(ws, { type: "message", id: r.id, from, preview });
      count++;
    }
    log("replay", { agent, count });
  } catch (e: unknown) {
    log("replay_error", {
      agent,
      error: e instanceof Error ? e.message : String(e),
    });
  }
}

function handleHello(ws: WebSocket, conn: Conn, msg: Record<string, unknown>): boolean {
  const role = msg.role;
  if (role !== "agent" && role !== "subscriber" && role !== "producer") {
    log("bad_hello", { reason: "invalid_role", role: String(role) });
    return false;
  }
  const agent = typeof msg.agent === "string" && msg.agent.length > 0 ? msg.agent : null;
  if (!agent) {
    log("bad_hello", { reason: "principal_required", role });
    return false;
  }
  if (!validAgentToken(agent, msg.token)) {
    log("bad_hello", { reason: "invalid_token", role, agent });
    return false;
  }
  conn.role = role;
  conn.agent = agent;
  log("hello", { role, agent });

  if (role === "subscriber") {
    // aperture-3x136: hand the newcomer the current presence of everyone so it
    // isn't blind to agents that joined before it (re)connected — the fix for
    // the watchdog false-re-kick loop on hub restart.
    for (const [name, entry] of presenceState) {
      send(ws, { type: "presence", agent: name, event: stateToEvent(entry.state), ts: new Date().toISOString() });
    }
  }

  if (role === "agent" && agent) {
    // One socket per agent name — a new connection replaces the old one.
    // The replaced socket's close handler sees it is no longer the mapped
    // socket and does NOT broadcast a spurious leave.
    const old = agents.get(agent);
    if (old && old !== ws) {
      log("agent_replaced", { agent });
      old.close(4000, "replaced by newer connection");
    }
    agents.set(agent, ws);
    broadcastPresence(agent, "join");
    void replayUnread(agent, ws);
  }
  return true;
}

function handleNotify(ws: WebSocket, conn: Conn, msg: Record<string, unknown>): void {
  const to = typeof msg.to === "string" ? msg.to : "";
  const id = typeof msg.id === "string" ? msg.id : "";
  const from = conn.agent ?? "unknown";
  if (typeof msg.from === "string" && msg.from !== from) {
    log("notify_rejected", { reason: "from_mismatch", agent: from });
    return;
  }
  const preview = typeof msg.preview === "string" ? msg.preview : "";
  const target = agents.get(to);
  let outcome: NotifyOutcome;
  if (codexBridges.has(to)) {
    // Codex agent: no Monitor socket — deliver by injecting a turn into its
    // app-server thread. The bridge fetches the full body from BEADS itself.
    codexBridges.deliver(to);
    outcome = "codex";
    log("notify_codex", { to, id, from });
  } else if (target) {
    send(target, { type: "message", id, from, preview });
    outcome = "forwarded";
    log("notify_forwarded", { to, id, from });
  } else {
    // Recipient offline: no-op — unread replay on reconnect covers it.
    outcome = "offline";
    log("notify_offline", { to, id, from });
  }
  // Always ack so the producer's await resolves — and say what actually
  // happened, so the producer's log line is honest (aperture-oeb6q).
  send(ws, { type: "ok", id, outcome });
}

/** What the hub actually did with a notify — carried on the ok ack. */
type NotifyOutcome = "forwarded" | "codex" | "offline";

// aperture-oeb6q: clear the presence snapshot at startup — BEFORE the codex
// bridges start, so this write is guaranteed to carry zero agents and a stale
// file left by a crashed hub (dead hub_pid, phantom agents) can't lie past
// this boot. Every later join/busy/idle/leave rewrites the file.
persistPresence();

// Phase 2: Codex bridge clients — one WS-over-unix-socket JSON-RPC client per
// discovered Codex agent (manifest model "codex/…"). A connected+bound bridge
// counts as presence for that agent; busy/idle tracks turn state.
const codexBridges = startCodexBridges({
  broadcastPresence,
  log,
  skipReplay: SKIP_REPLAY,
});

const wss = new WebSocketServer({ host: HOST, port: PORT, maxPayload: MAX_FRAME_BYTES });

wss.on("listening", () => {
  log("listening", { host: HOST, port: PORT, skipReplay: SKIP_REPLAY });
});

wss.on("error", (err) => {
  log("server_error", { error: err.message });
  // aperture-3x136: a fatal listen error (EADDRINUSE from a stale/orphan hub,
  // EACCES, etc.) means this process will never serve. Previously we only
  // logged and stayed alive — the Rust supervisor's try_wait then saw the
  // child as "still running" and never respawned, so freeing the port by
  // hand did NOT self-heal. Exit non-zero so the supervisor's respawn loop
  // fires: once the squatter is gone (its own shutdown sweep, or the
  // supervisor's residual-listener kill), the next spawn binds cleanly.
  const code = (err as NodeJS.ErrnoException).code;
  if (code === "EADDRINUSE" || code === "EACCES" || code === "EADDRNOTAVAIL") {
    log("server_error_fatal_exit", { code });
    process.exit(1);
  }
});

wss.on("connection", (ws) => {
  const conn: Conn = { role: null, agent: null, isAlive: true };
  conns.set(ws, conn);
  const helloDeadline = setTimeout(() => {
    if (conn.role === null) ws.close(4001, "expected hello");
  }, 5_000);
  helloDeadline.unref?.();

  ws.on("pong", () => {
    conn.isAlive = true;
  });

  ws.on("message", (data) => {
    let msg: Record<string, unknown>;
    try {
      const parsed = JSON.parse(data.toString());
      if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
        throw new Error("not an object");
      }
      msg = parsed as Record<string, unknown>;
    } catch {
      if (conn.role === null) {
        // First message must be a valid hello.
        log("bad_first_message", { reason: "unparseable" });
        ws.close(4001, "expected hello");
      } else {
        log("bad_message", { role: conn.role, agent: conn.agent });
      }
      return;
    }

    if (conn.role === null) {
      if (msg.type !== "hello" || !handleHello(ws, conn, msg)) {
        ws.close(4001, "expected hello");
      } else {
        clearTimeout(helloDeadline);
      }
      return;
    }

    if (conn.role === "producer" && msg.type === "notify") {
      handleNotify(ws, conn, msg);
      return;
    }

    // Anything else post-hello is ignored (logged for forensics).
    log("ignored_message", { role: conn.role, agent: conn.agent, type: String(msg.type) });
  });

  ws.on("close", () => {
    clearTimeout(helloDeadline);
    conns.delete(ws);
    if (conn.role === "agent" && conn.agent && agents.get(conn.agent) === ws) {
      agents.delete(conn.agent);
      broadcastPresence(conn.agent, "leave");
    }
  });

  ws.on("error", (err) => {
    log("socket_error", { role: conn.role, agent: conn.agent, error: err.message });
  });
});

// Heartbeat: ping every 30s; terminate sockets that missed the previous ping.
// terminate() fires the close handler → leave broadcast for agents.
const heartbeat = setInterval(() => {
  for (const [ws, conn] of conns) {
    if (!conn.isAlive) {
      log("heartbeat_dead", { role: conn.role, agent: conn.agent });
      ws.terminate();
      continue;
    }
    conn.isAlive = false;
    ws.ping();
  }
}, HEARTBEAT_MS);
heartbeat.unref?.();

function shutdown(signal: string): void {
  log("shutdown", { signal });
  clearInterval(heartbeat);
  codexBridges.stop();
  for (const ws of conns.keys()) {
    ws.close(1001, "hub shutting down");
  }
  wss.close(() => {
    process.exit(0);
  });
  // Belt-and-braces: don't hang forever if a socket won't close.
  setTimeout(() => process.exit(0), 2000).unref?.();
}

process.on("SIGTERM", () => shutdown("SIGTERM"));
process.on("SIGINT", () => shutdown("SIGINT"));
