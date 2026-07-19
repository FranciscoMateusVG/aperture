#!/usr/bin/env node
// stub-hello.mjs — fake "booted Claude agent with a live Monitor".
//
// Connects to the aperture WS hub, sends the agent hello
//   {"type":"hello","role":"agent","agent":"<name>"}
// (no ack is expected — the hub replies nothing to an agent hello), then
// stays connected until SIGTERM/SIGINT, so presence "join" is observable and
// "leave" fires on kill.
//
// Args:
//   --agent <name>   agent name for the hello (required)
//   --port <n>       hub port (default: $APERTURE_WS_PORT or 4517)
//
// Connect is retried with 500ms backoff up to 20 attempts to survive the
// hub-not-up-yet race (pinned failure mode #5 — an agent pane can win the
// boot race against the hub daemon).
//
// Uses ws from mcp-server/node_modules (run `just build-mcp` / pnpm install
// there first). perMessageDeflate:false — required for codex unix sockets,
// tolerated by the hub, kept uniform here.

import { createRequire } from "node:module";

const require = createRequire(
  new URL("../../../mcp-server/package.json", import.meta.url),
);
const WebSocket = require("ws");

function argValue(flag) {
  const i = process.argv.indexOf(flag);
  return i !== -1 && i + 1 < process.argv.length ? process.argv[i + 1] : null;
}

const agent = argValue("--agent");
if (!agent) {
  process.stderr.write("stub-hello: --agent <name> is required\n");
  process.exit(2);
}
const port = Number(argValue("--port") ?? process.env.APERTURE_WS_PORT ?? 4517);
const url = `ws://127.0.0.1:${port}`;

const RETRY_MS = 500;
const MAX_ATTEMPTS = 20;

function log(event, fields = {}) {
  process.stderr.write(
    JSON.stringify({ ts: new Date().toISOString(), event, agent, ...fields }) + "\n",
  );
}

function connectOnce() {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(url, { perMessageDeflate: false });
    ws.on("open", () => resolve(ws));
    ws.on("error", (err) => reject(err));
  });
}

let ws = null;
for (let attempt = 1; attempt <= MAX_ATTEMPTS; attempt++) {
  try {
    ws = await connectOnce();
    break;
  } catch (err) {
    log("stub_connect_retry", { attempt, error: err.message });
    if (attempt === MAX_ATTEMPTS) {
      log("stub_connect_failed", { attempts: MAX_ATTEMPTS, url });
      process.exit(1);
    }
    await new Promise((r) => setTimeout(r, RETRY_MS));
  }
}

ws.send(JSON.stringify({ type: "hello", role: "agent", agent }));
log("stub_hello_sent", { url });

// Any pushed messages (replay / notify_forwarded) are logged for forensics.
ws.on("message", (data) => {
  log("stub_received", { raw: data.toString().slice(0, 200) });
});

ws.on("close", (code, reason) => {
  log("stub_socket_closed", { code, reason: reason.toString() });
  // agent_replaced (4000) or hub shutdown (1001) — exit cleanly either way.
  process.exit(0);
});

function shutdown(signal) {
  log("stub_shutdown", { signal });
  try {
    ws.close(1000, "stub shutting down");
  } catch {
    /* already closed */
  }
  setTimeout(() => process.exit(0), 500).unref?.();
}
process.on("SIGTERM", () => shutdown("SIGTERM"));
process.on("SIGINT", () => shutdown("SIGINT"));

// Keep the event loop alive indefinitely (the WS keeps it alive anyway, but
// belt-and-braces against a silent close before our handler is attached).
setInterval(() => {}, 1 << 30);
