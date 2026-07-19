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
 *                       {type:"presence", agent, event:"join"|"leave", ts}.
 *   - role=producer   → MCP send-queue drains. Send
 *                       {type:"notify", to, id, from, preview}; the hub
 *                       forwards {type:"message", id, from, preview} to the
 *                       recipient if connected and always acks {type:"ok", id}.
 *
 * Delivery semantics: at-least-once, idempotent by message id. A missed push
 * (recipient offline, hub down) is covered by unread replay on reconnect —
 * BEADS remains the store of record; this is transport only.
 *
 * Env:
 *   APERTURE_WS_PORT        — listen port (default 4517, loopback only)
 *   APERTURE_HUB_SKIP_REPLAY=1 — skip the bd unread-replay on agent connect
 *                                (testing hook; smoke tests have no BEADS)
 */
import { WebSocketServer, WebSocket } from "ws";
import { getUnreadMessages } from "./beads.js";

const HOST = "127.0.0.1";
const PORT = Number(process.env.APERTURE_WS_PORT ?? 4517);
const SKIP_REPLAY = process.env.APERTURE_HUB_SKIP_REPLAY === "1";
const HEARTBEAT_MS = 30_000;

type Role = "agent" | "subscriber" | "producer";

interface Conn {
  role: Role | null; // null until a valid hello arrives
  agent: string | null;
  isAlive: boolean;
}

const conns = new Map<WebSocket, Conn>();
/** Presence map: agent name → its (single) live socket. */
const agents = new Map<string, WebSocket>();

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

function broadcastPresence(agent: string, event: "join" | "leave"): void {
  const msg = { type: "presence", agent, event, ts: new Date().toISOString() };
  for (const [ws, conn] of conns) {
    if (conn.role === "subscriber") send(ws, msg);
  }
  log("presence", { agent, presence: event });
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
  if (role === "agent" && !agent) {
    log("bad_hello", { reason: "agent_name_required" });
    return false;
  }
  conn.role = role;
  conn.agent = agent;
  log("hello", { role, agent });

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

function handleNotify(ws: WebSocket, msg: Record<string, unknown>): void {
  const to = typeof msg.to === "string" ? msg.to : "";
  const id = typeof msg.id === "string" ? msg.id : "";
  const from = typeof msg.from === "string" ? msg.from : "unknown";
  const preview = typeof msg.preview === "string" ? msg.preview : "";
  const target = agents.get(to);
  if (target) {
    send(target, { type: "message", id, from, preview });
    log("notify_forwarded", { to, id, from });
  } else {
    // Recipient offline: no-op — unread replay on reconnect covers it.
    log("notify_offline", { to, id, from });
  }
  // Always ack so the producer's await resolves.
  send(ws, { type: "ok", id });
}

const wss = new WebSocketServer({ host: HOST, port: PORT });

wss.on("listening", () => {
  log("listening", { host: HOST, port: PORT, skipReplay: SKIP_REPLAY });
});

wss.on("error", (err) => {
  log("server_error", { error: err.message });
});

wss.on("connection", (ws) => {
  const conn: Conn = { role: null, agent: null, isAlive: true };
  conns.set(ws, conn);

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
      }
      return;
    }

    if (conn.role === "producer" && msg.type === "notify") {
      handleNotify(ws, msg);
      return;
    }

    // Anything else post-hello is ignored (logged for forensics).
    log("ignored_message", { role: conn.role, agent: conn.agent, type: String(msg.type) });
  });

  ws.on("close", () => {
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
