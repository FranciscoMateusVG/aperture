#!/usr/bin/env node
// Live smoke test for the comms-layer v2 Codex bridge (dist/codex-bridge.js).
//
// Spawns a real `codex app-server --listen unix:///tmp/smoke-codex.sock`
// (codex-cli must be installed + authed), then uses CodexBridgeClient to:
//   1. connect WS-over-unix-socket + JSON-RPC initialize/initialized
//   2. thread/start a fresh ephemeral thread
//   3. inject a turn: "Reply with exactly: BRIDGE_OK"
//   4. assert an agentMessage notification containing BRIDGE_OK
//
// Run: node scripts/smoke-codex-bridge.mjs   (from mcp-server/, after npx tsc)

import { spawn } from "node:child_process";
import { existsSync, mkdirSync, rmSync } from "node:fs";
import { homedir } from "node:os";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";

const here = dirname(fileURLToPath(import.meta.url));
const { CodexBridgeClient } = await import(resolve(here, "..", "dist", "codex-bridge.js"));

// NB: codex app-server refuses sockets under /tmp — the symlinked path fails
// its "is a directory" check and /private/tmp fails its ownership/perms check
// ("Operation not permitted"). It requires a user-owned dir, so we use the
// production socket dir (~/.aperture/run), same place Tauri will spawn into.
const RUN_DIR = resolve(homedir(), ".aperture", "run");
const SOCK = resolve(RUN_DIR, "smoke-codex.sock");
mkdirSync(RUN_DIR, { recursive: true });
const TURN_WAIT_MS = 120_000;

let failed = false;
const pass = (msg) => console.log(`PASS: ${msg}`);
const fail = (msg) => {
  failed = true;
  console.error(`FAIL: ${msg}`);
};

function delay(ms) {
  return new Promise((r) => setTimeout(r, ms));
}

// ── spawn app-server ──
try {
  rmSync(SOCK, { force: true });
} catch {}

const server = spawn("codex", ["app-server", "--listen", `unix://${SOCK}`], {
  stdio: ["ignore", "pipe", "pipe"],
});
let serverLog = "";
server.stdout.on("data", (d) => (serverLog += d.toString()));
server.stderr.on("data", (d) => (serverLog += d.toString()));
server.on("exit", (code, sig) => {
  if (!cleaningUp) fail(`codex app-server exited early (code=${code} sig=${sig})\n${serverLog.slice(-2000)}`);
});
let cleaningUp = false;

// wait for the socket to appear
{
  const deadline = Date.now() + 15_000;
  while (!existsSync(SOCK) && Date.now() < deadline) await delay(200);
  if (!existsSync(SOCK)) {
    fail(`socket ${SOCK} never appeared\n${serverLog.slice(-2000)}`);
    server.kill();
    process.exit(1);
  }
  pass(`app-server listening on ${SOCK}`);
}

// ── bridge connect + initialize ──
const notifications = [];
let sawBridgeOk = false;
let resolveBridgeOk;
const bridgeOkSeen = new Promise((r) => (resolveBridgeOk = r));

const bridge = new CodexBridgeClient(
  "smoke",
  SOCK,
  {
    broadcastPresence: (agent, event) => console.log(`  presence: ${agent} ${event}`),
    log: (event, fields = {}) => console.log(`  bridge: ${event} ${JSON.stringify(fields)}`),
    skipReplay: true, // no BEADS in the loop
    onNotification: (_agent, method, params) => {
      notifications.push({ method, params });
      const flat = JSON.stringify(params ?? {});
      if (flat.includes("BRIDGE_OK") && /agent_?[Mm]essage|item/.test(`${method} ${flat}`)) {
        if (!sawBridgeOk) {
          sawBridgeOk = true;
          resolveBridgeOk();
        }
      }
    },
  },
  { autoBind: false }, // smoke drives thread/start manually
);

async function cleanup(code) {
  cleaningUp = true;
  try {
    bridge.stop();
  } catch {}
  try {
    server.kill("SIGTERM");
  } catch {}
  await delay(300);
  try {
    server.kill("SIGKILL");
  } catch {}
  try {
    rmSync(SOCK, { force: true });
  } catch {}
  process.exit(code);
}

try {
  bridge.start();
  await bridge.waitReady(15_000);
  pass("WS handshake + initialize/initialized completed");
} catch (e) {
  fail(`initialize failed: ${e.message}\n${serverLog.slice(-2000)}`);
  await cleanup(1);
}

// ── fresh ephemeral thread ──
let threadId;
try {
  threadId = await bridge.startThread();
  pass(`thread/start → ${threadId}`);
} catch (e) {
  fail(`thread/start failed: ${e.message}\n${serverLog.slice(-2000)}`);
  await cleanup(1);
}

// ── inject a turn and await BRIDGE_OK ──
const turnResult = bridge
  .request("turn/start", {
    threadId,
    input: [{ type: "text", text: "Reply with exactly: BRIDGE_OK" }],
  })
  .catch((e) => ({ _error: e.message }));

const winner = await Promise.race([
  bridgeOkSeen.then(() => "notified"),
  turnResult.then(() => "responded"),
  delay(TURN_WAIT_MS).then(() => "timeout"),
]);

if (!sawBridgeOk && winner === "responded") {
  // turn finished; the message may be in the RPC result rather than a notification
  const res = await turnResult;
  if (JSON.stringify(res ?? {}).includes("BRIDGE_OK")) sawBridgeOk = true;
  else await Promise.race([bridgeOkSeen, delay(3000)]); // late notifications
}

if (sawBridgeOk) {
  pass("agentMessage contains BRIDGE_OK");
} else {
  fail(
    `no BRIDGE_OK within ${winner === "timeout" ? TURN_WAIT_MS + "ms" : "turn"}; ` +
      `saw ${notifications.length} notifications: ` +
      notifications
        .slice(-10)
        .map((n) => n.method)
        .join(", ") +
      `\nlast payloads: ${JSON.stringify(notifications.slice(-3)).slice(0, 1500)}` +
      `\nserver log tail: ${serverLog.slice(-1500)}`,
  );
}

console.log(failed ? "\nSMOKE: FAILED" : "\nSMOKE: ALL PASS");
await cleanup(failed ? 1 : 0);
