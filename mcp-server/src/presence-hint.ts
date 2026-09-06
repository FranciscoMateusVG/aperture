import WebSocket from "ws";
import { readFileSync } from "node:fs";
import { basename } from "node:path";

/**
 * presence-hint — one-shot turn-state hint from a Claude agent to the WS hub.
 *
 * Claude Code runs this as a hook (aperture-trgpo), wired in the agent's
 * `.claude/settings.json`:
 *
 *   UserPromptSubmit → node <repo>/mcp-server/dist/presence-hint.js busy
 *   PreToolUse       → node <repo>/mcp-server/dist/presence-hint.js busy
 *   Stop             → node <repo>/mcp-server/dist/presence-hint.js idle
 *
 * so a Claude agent's busy/idle state reaches the hub the same way the codex
 * bridge's does. The hub (ws-hub.ts) turns the frame into a presence
 * broadcast for subscribers (the Tauri launcher's state chips).
 *
 * Frames, in order, on one producer socket:
 *   { type: "hello", role: "producer", agent, token }
 *   { type: "presence_hint", event }            // event ∈ {busy, idle}
 * then resolve on the first {type:"ok"}, OR close, OR error, OR the hard
 * timeout (APERTURE_HINT_TIMEOUT_MS, default 800ms).
 *
 * Hooks run SYNCHRONOUSLY inside the agent's session, so this MUST be fast
 * and MUST NEVER fail the session:
 *   - ALWAYS exits 0, on every path. A dead hub (ECONNREFUSED) exits
 *     immediately on the error event, not at the timeout.
 *   - No stdout, ever. On a failure path, exactly one stderr line prefixed
 *     `[presence-hint]` — Claude Code only surfaces hook stderr on non-zero
 *     exit, so this is purely for manual debugging.
 *   - stdin (the hook's JSON payload) is never read; the process leaves via
 *     process.exit(0) after settling so an open stdin cannot keep it alive.
 *
 * No-op rules (exit 0, silently):
 *   - APERTURE_HUB_TOKEN_FILE unset, or the file unreadable → this is the
 *     operator's own plain `claude` session in the repo, or some non-Aperture
 *     context. Nothing to report, nobody to report it to.
 *   - argv[2] not in {busy, idle} → exit 0 with one stderr line (a typo in
 *     settings.json must never break a hook).
 *
 * Identity: the launcher exports APERTURE_HUB_TOKEN_FILE=…/hub-tokens/<name>.token
 * into the agent's shell but NOT AGENT_NAME, so `agent` is derived from the
 * token file's basename minus `.token`, validated against the same
 * /^[a-z0-9_-]{1,32}$/ the hub enforces. The token is sent as the raw file
 * contents (no trim) — the hub compares bytes, exactly as hub-notify.ts does.
 *
 * Hub URL: APERTURE_HUB_URL (default ws://127.0.0.1:4517), like hub-client.ts.
 * perMessageDeflate is OFF (hub requirement — banked gotcha).
 */

const AGENT_NAME = /^[a-z0-9_-]{1,32}$/;
const DEFAULT_TIMEOUT_MS = 800;
const TOKEN_SUFFIX = ".token";

/** The only stderr channel. One line, prefixed, never on the success path. */
function warn(msg: string): void {
  process.stderr.write(`[presence-hint] ${msg}\n`);
}

const event = process.argv[2];
if (event !== "busy" && event !== "idle") {
  warn(`ignoring unknown event ${JSON.stringify(event ?? "")} — usage: presence-hint.js <busy|idle>`);
  process.exit(0);
}

const tokenFile = process.env.APERTURE_HUB_TOKEN_FILE ?? "";
if (!tokenFile) {
  // Not an Aperture-launched agent: silent no-op.
  process.exit(0);
}

const agent = basename(tokenFile, TOKEN_SUFFIX);
if (!tokenFile.endsWith(TOKEN_SUFFIX) || !AGENT_NAME.test(agent)) {
  warn(`cannot derive agent name from APERTURE_HUB_TOKEN_FILE=${tokenFile}`);
  process.exit(0);
}

let token: string;
try {
  token = readFileSync(tokenFile, "utf8");
} catch {
  // Token file missing/unreadable: treat like "not an Aperture agent".
  process.exit(0);
}

const url = process.env.APERTURE_HUB_URL ?? "ws://127.0.0.1:4517";
const envTimeout = Number(process.env.APERTURE_HINT_TIMEOUT_MS);
const timeoutMs = Number.isFinite(envTimeout) && envTimeout > 0 ? envTimeout : DEFAULT_TIMEOUT_MS;

let settled = false;
let ws: WebSocket | null = null;

/** Settle exactly once, then leave. `reason` (if any) is the single stderr line. */
function finish(reason?: string): void {
  if (settled) return;
  settled = true;
  clearTimeout(timer);
  if (reason) warn(reason);
  try {
    ws?.terminate();
  } catch {
    // already closed
  }
  process.exit(0);
}

const timer = setTimeout(() => finish(`timed out after ${timeoutMs}ms awaiting hub ack`), timeoutMs);

try {
  ws = new WebSocket(url, { perMessageDeflate: false });
} catch (e: unknown) {
  finish(`hub connect failed: ${e instanceof Error ? e.message : String(e)}`);
}

ws!.on("open", () => {
  ws!.send(JSON.stringify({ type: "hello", role: "producer", agent, token }));
  ws!.send(JSON.stringify({ type: "presence_hint", event }));
});

ws!.on("message", (data) => {
  try {
    const msg = JSON.parse(data.toString());
    if (msg?.type === "ok") finish();
  } catch {
    // non-JSON frame: keep waiting; the timeout is the backstop
  }
});

ws!.on("error", (err) => {
  finish(`hub unavailable: ${err.message}`);
});

ws!.on("close", (code) => {
  // Closed before an ok: hello rejected (4001) or hub gone mid-flight.
  finish(`hub closed before ack (code=${code})`);
});
