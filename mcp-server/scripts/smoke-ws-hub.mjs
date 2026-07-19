#!/usr/bin/env node
// Smoke test for the comms-layer v2 WS hub (dist/ws-hub.js).
//
// Starts the hub on a random port with APERTURE_HUB_SKIP_REPLAY=1 (no BEADS
// in the loop), then asserts:
//   1. subscriber receives {type:"presence", agent:"testbot", event:"join"}
//      when the fake agent connects
//   2. producer notify → producer gets {type:"ok"}, agent gets {type:"message"}
//   3. offline recipient notify still acks ok (no-op forward)
//   4. non-hello first message → socket closed by hub
//   5. agent disconnect → subscriber receives leave
//   6. SIGTERM → hub exits 0
//
// Run: node scripts/smoke-ws-hub.mjs   (from mcp-server/, after npx tsc)

import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import { dirname, resolve } from "node:path";
import WebSocket from "ws";

const here = dirname(fileURLToPath(import.meta.url));
const hubPath = resolve(here, "..", "dist", "ws-hub.js");
const port = 20000 + Math.floor(Math.random() * 20000);

let failed = false;
const pass = (msg) => console.log(`PASS: ${msg}`);
const fail = (msg) => {
  failed = true;
  console.error(`FAIL: ${msg}`);
};

// Global watchdog so a broken hub can't hang the script.
const watchdog = setTimeout(() => {
  console.error("FAIL: smoke test timed out after 15s");
  hub?.kill("SIGKILL");
  process.exit(1);
}, 15000);
watchdog.unref();

const hub = spawn(process.execPath, [hubPath], {
  env: {
    ...process.env,
    APERTURE_WS_PORT: String(port),
    APERTURE_HUB_SKIP_REPLAY: "1",
  },
  stdio: ["ignore", "ignore", "pipe"],
});
hub.stderr.on("data", (d) => process.stderr.write(`[hub] ${d}`));

function connect() {
  return new Promise((resolvePromise, reject) => {
    const ws = new WebSocket(`ws://127.0.0.1:${port}`);
    ws.on("open", () => resolvePromise(ws));
    ws.on("error", reject);
  });
}

async function connectWithRetry(deadlineMs = 5000) {
  const deadline = Date.now() + deadlineMs;
  for (;;) {
    try {
      return await connect();
    } catch {
      if (Date.now() > deadline) throw new Error("hub never became connectable");
      await new Promise((r) => setTimeout(r, 100));
    }
  }
}

/** Wait for the next JSON message on ws matching pred. */
function waitFor(ws, pred, what, timeoutMs = 3000) {
  return new Promise((resolvePromise, reject) => {
    const timer = setTimeout(
      () => reject(new Error(`timed out waiting for ${what}`)),
      timeoutMs,
    );
    const onMessage = (data) => {
      let msg;
      try {
        msg = JSON.parse(data.toString());
      } catch {
        return;
      }
      if (pred(msg)) {
        clearTimeout(timer);
        ws.off("message", onMessage);
        resolvePromise(msg);
      }
    };
    ws.on("message", onMessage);
  });
}

function waitForClose(ws, what, timeoutMs = 3000) {
  return new Promise((resolvePromise, reject) => {
    const timer = setTimeout(
      () => reject(new Error(`timed out waiting for ${what}`)),
      timeoutMs,
    );
    ws.on("close", () => {
      clearTimeout(timer);
      resolvePromise();
    });
  });
}

const hello = (ws, role, agent) =>
  ws.send(JSON.stringify(agent ? { type: "hello", role, agent } : { type: "hello", role }));

try {
  // ── Setup: subscriber first, then the fake agent ──
  const subscriber = await connectWithRetry();
  hello(subscriber, "subscriber");

  const agent = await connect();
  const joinPromise = waitFor(
    subscriber,
    (m) => m.type === "presence" && m.agent === "testbot" && m.event === "join",
    "subscriber join event",
  );
  hello(agent, "agent", "testbot");
  await joinPromise;
  pass("subscriber received presence join for testbot");

  // ── Producer notify → ok ack + delivery to agent ──
  const producer = await connect();
  hello(producer, "producer");
  const okPromise = waitFor(
    producer,
    (m) => m.type === "ok" && m.id === "msg-1",
    "producer ok ack",
  );
  const deliveryPromise = waitFor(
    agent,
    (m) => m.type === "message" && m.id === "msg-1" && m.from === "glados",
    "agent message delivery",
  );
  producer.send(
    JSON.stringify({
      type: "notify",
      to: "testbot",
      id: "msg-1",
      from: "glados",
      preview: "hello testbot, this is a smoke test",
    }),
  );
  await okPromise;
  pass("producer received ok ack for msg-1");
  const delivered = await deliveryPromise;
  pass(`agent socket received message event: ${JSON.stringify(delivered)}`);

  // ── Notify for an offline agent → still acks ok, no crash ──
  const okOffline = waitFor(
    producer,
    (m) => m.type === "ok" && m.id === "msg-2",
    "producer ok ack for offline recipient",
  );
  producer.send(
    JSON.stringify({ type: "notify", to: "nobody-home", id: "msg-2", from: "glados", preview: "x" }),
  );
  await okOffline;
  pass("notify to offline recipient acked ok (no-op forward)");

  // ── Non-hello first message → hub closes the socket ──
  const rogue = await connect();
  const rogueClosed = waitForClose(rogue, "rogue socket close");
  rogue.send(JSON.stringify({ type: "notify", to: "testbot", id: "evil" }));
  await rogueClosed;
  pass("non-hello first message → socket closed by hub");

  // ── Agent disconnect → subscriber gets leave ──
  const leavePromise = waitFor(
    subscriber,
    (m) => m.type === "presence" && m.agent === "testbot" && m.event === "leave",
    "subscriber leave event",
  );
  agent.close();
  await leavePromise;
  pass("subscriber received presence leave for testbot");

  // ── SIGTERM → clean exit 0 ──
  const exitCode = await new Promise((resolvePromise) => {
    hub.on("exit", (code) => resolvePromise(code));
    hub.kill("SIGTERM");
  });
  if (exitCode === 0) pass("hub exited 0 on SIGTERM");
  else fail(`hub exited ${exitCode} on SIGTERM (expected 0)`);

  subscriber.close();
  producer.close();
} catch (e) {
  fail(e.message);
  hub.kill("SIGKILL");
}

clearTimeout(watchdog);
if (failed) {
  console.error("SMOKE TEST: FAILED");
  process.exit(1);
}
console.log("SMOKE TEST: ALL PASSED");
process.exit(0);
