/**
 * hub-client — the canonical inbox-monitor client for Claude agents.
 *
 * Agents run this via a bash-based Monitor:
 *
 *   Monitor({
 *     command: "node <repo>/mcp-server/dist/hub-client.js <agent-name>",
 *     persistent: true,
 *   })
 *
 * Why this exists (aperture-1qwty): the Monitor tool's native ws source is
 * RECEIVE-ONLY — it cannot send the hello frame the hub requires to identify
 * the connection. An agent that connects without hello is an anonymous socket:
 * no presence registration, no unread replay, no push delivery. This wrapper
 * sends the hello on open and streams every hub frame as one stdout line
 * (= one Monitor event).
 *
 * Behaviour:
 *   - URL from APERTURE_HUB_URL (default ws://127.0.0.1:4517)
 *   - perMessageDeflate OFF (hub requirement — banked gotcha)
 *   - hello: { type: "hello", role: "agent", agent: <argv[2]> }
 *     plus token: APERTURE_HUB_TOKEN (or APERTURE_HUB_TOKEN_FILE); the hub
 *     rejects a hello without a valid token (close 4001)
 *   - each incoming frame → one stdout line
 *   - the client RECONNECTS by itself (aperture-oeb6q). A hub blip (Tauri
 *     supervisor respawn, ~2s), a 1001 hub shutdown, a 1006 abnormal close,
 *     an RST, ECONNREFUSED while the hub is coming back — all of these are
 *     retried forever with jittered exponential backoff (1s base, ×2, 30s
 *     cap, ±25% jitter). Every new socket re-sends the hello; the hub's
 *     unread replay on hello delivers everything missed while disconnected.
 *     You do NOT need to restart the monitor for these — the stdout lines say
 *     so explicitly:
 *       HUB_RECONNECTING code=<code|error> reason=<…> — will retry with backoff, no action needed
 *         (once, at the start of an outage; later failed attempts are silent)
 *       HUB_STILL_DISCONNECTED attempts=<n> elapsed=<s>s
 *         (every 5 minutes while still down)
 *       HUB_RECONNECTED after <n> attempt(s), <s>s — unread messages replay now
 *   - client-side liveness: the hub pings every 30s; if NO frame of any kind
 *     arrives for 90s the hub is wedged (alive, not talking) and the client
 *     prints HUB_SOCKET_STALE, terminates the socket and reconnects.
 *   - the process EXITS only when reconnecting would be wrong:
 *       HUB_SOCKET_CLOSED code=4000 … → exit 0. A NEWER monitor for this
 *         agent connected and replaced this one. Do NOT restart it —
 *         reconnecting here would fight the newer monitor (flap loop).
 *       HUB_SOCKET_CLOSED code=4001 … → exit 1. The hello was rejected
 *         (bad token or agent name). Retrying cannot help; fix the token /
 *         name and restart the inbox monitor.
 *       HUB_CLIENT_ERROR … → exit 2. Missing agent name or token at startup.
 *
 * Env overrides (tests / tuning):
 *   APERTURE_HUB_STALE_MS       — liveness deadline (default 90000)
 *   APERTURE_HUB_BACKOFF_CAP_MS — max reconnect delay (default 30000)
 */

import WebSocket from "ws";
import { readFileSync } from "node:fs";

/** First reconnect delay; doubles on every consecutive failure. */
const BACKOFF_BASE_MS = 1_000;
/** Ceiling for the reconnect delay (before jitter). */
const BACKOFF_CAP_MS = Number(process.env.APERTURE_HUB_BACKOFF_CAP_MS) || 30_000;
/** ±25% jitter so a fleet of clients doesn't stampede a respawning hub. */
const BACKOFF_JITTER = 0.25;
/** No frame (message or ping) for this long → the hub is wedged; reconnect. */
const STALE_MS = Number(process.env.APERTURE_HUB_STALE_MS) || 150_000;
/** Cadence of the HUB_STILL_DISCONNECTED reminder during a long outage. */
const STILL_DISCONNECTED_EVERY_MS = 5 * 60_000;
/** Hub close code: a newer connection for this agent name replaced ours. */
const CLOSE_REPLACED = 4000;
/** Hub close code: hello rejected (bad/missing token, bad agent name). */
const CLOSE_HELLO_REJECTED = 4001;

const agent = process.argv[2];
if (!agent) {
  console.log("HUB_CLIENT_ERROR missing agent name — usage: hub-client.js <agent-name>");
  process.exit(2);
}

const url = process.env.APERTURE_HUB_URL ?? "ws://127.0.0.1:4517";
let token = process.env.APERTURE_HUB_TOKEN;
const tokenFile = process.env.APERTURE_HUB_TOKEN_FILE;
if (!token && tokenFile) {
  try {
    token = readFileSync(tokenFile, "utf8");
  } catch {
    console.log("HUB_CLIENT_ERROR unable to read agent token file");
    process.exit(2);
  }
}
if (!token) {
  console.log("HUB_CLIENT_ERROR missing agent token");
  process.exit(2);
}

// ── Outage state ────────────────────────────────────────────────────────────
// outageStartedAt is null while connected. attempts counts failed connection
// attempts (or drops) since the outage began — i.e. the number of reconnects
// tried when the next one succeeds.
let outageStartedAt: number | null = null;
let attempts = 0;
let reconnectTimer: NodeJS.Timeout | null = null;
let stillDisconnectedTimer: NodeJS.Timeout | null = null;

function fmtSeconds(ms: number): string {
  const s = ms / 1000;
  return Number.isInteger(s) ? String(s) : s.toFixed(1);
}

/** Write one stdout line, then exit — the callback guarantees the line is flushed. */
function exitWithLine(line: string, code: number): void {
  process.stdout.write(`${line}\n`, () => process.exit(code));
}

function backoffDelay(attempt: number): number {
  const raw = Math.min(BACKOFF_CAP_MS, BACKOFF_BASE_MS * 2 ** (attempt - 1));
  const jitter = 1 + (Math.random() * 2 - 1) * BACKOFF_JITTER;
  return Math.round(raw * jitter);
}

/**
 * A socket died (close with a retryable code, or error). Announce the outage
 * once, then schedule the next attempt. Called at most once per socket.
 */
function scheduleReconnect(code: string, reason: string): void {
  attempts += 1;
  if (outageStartedAt === null) {
    outageStartedAt = Date.now();
    console.log(`HUB_RECONNECTING code=${code} reason=${reason} — will retry with backoff, no action needed`);
    stillDisconnectedTimer = setInterval(() => {
      const elapsed = Date.now() - (outageStartedAt ?? Date.now());
      console.log(`HUB_STILL_DISCONNECTED attempts=${attempts} elapsed=${fmtSeconds(elapsed)}s`);
    }, STILL_DISCONNECTED_EVERY_MS);
    stillDisconnectedTimer.unref();
  }
  // NOT unref'd on purpose: during backoff this timer is the only thing
  // keeping the process alive.
  reconnectTimer = setTimeout(connect, backoffDelay(attempts));
}

function onConnected(): void {
  if (outageStartedAt !== null) {
    const elapsed = Date.now() - outageStartedAt;
    console.log(`HUB_RECONNECTED after ${attempts} attempt(s), ${fmtSeconds(elapsed)}s — unread messages replay now`);
  }
  outageStartedAt = null;
  attempts = 0;
  reconnectTimer = null;
  if (stillDisconnectedTimer) {
    clearInterval(stillDisconnectedTimer);
    stillDisconnectedTimer = null;
  }
}

function connect(): void {
  const ws = new WebSocket(url, { perMessageDeflate: false });
  // ws emits `error` then `close` for a failed connect (and for most runtime
  // errors); whichever fires first owns the reconnect decision for this socket.
  let settled = false;
  let staleTimer: NodeJS.Timeout | null = null;

  const armStale = (): void => {
    if (staleTimer) clearTimeout(staleTimer);
    staleTimer = setTimeout(() => {
      console.log(`HUB_SOCKET_STALE no frame for ${fmtSeconds(STALE_MS)}s — reconnecting`);
      ws.terminate(); // → close 1006 → reconnect path below
    }, STALE_MS);
    staleTimer.unref();
  };
  const disarmStale = (): void => {
    if (staleTimer) clearTimeout(staleTimer);
    staleTimer = null;
  };

  ws.on("open", () => {
    const hello: Record<string, unknown> = { type: "hello", role: "agent", agent };
    hello.token = token;
    ws.send(JSON.stringify(hello));
    onConnected();
    armStale();
  });

  ws.on("message", (data) => {
    armStale();
    console.log(data.toString());
  });

  // The hub's 30s heartbeat ping counts as liveness (ws auto-pongs it).
  ws.on("ping", () => {
    armStale();
  });

  ws.on("close", (code, reason) => {
    disarmStale();
    const why = reason.toString();
    if (code === CLOSE_REPLACED) {
      exitWithLine(
        `HUB_SOCKET_CLOSED code=${code} reason=${why} — a newer inbox monitor for this agent took over; this one exits, do not restart it`,
        0,
      );
      return;
    }
    if (code === CLOSE_HELLO_REJECTED) {
      exitWithLine(
        `HUB_SOCKET_CLOSED code=${code} reason=${why} — hello rejected (token or agent name); fix and restart your inbox monitor`,
        1,
      );
      return;
    }
    if (settled) return;
    settled = true;
    scheduleReconnect(String(code), why);
  });

  ws.on("error", (err) => {
    disarmStale();
    if (settled) return;
    settled = true;
    scheduleReconnect("error", err.message);
  });
}

connect();
