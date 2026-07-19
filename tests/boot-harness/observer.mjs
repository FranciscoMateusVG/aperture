#!/usr/bin/env node
// observer.mjs — hub observer/asserter for the boot verification harness
// (aperture-xt16e).
//
// Connects to the WS hub as a subscriber
//   {"type":"hello","role":"subscriber"}
// and watches {"type":"presence", agent, event, ts} broadcasts until every
// expected agent has joined (and, if required, its codex bridge has bound).
//
// Args:
//   --expect-agent <name>         expected to presence-join (repeatable)
//   --timeout-s <n>               overall deadline (default 60)
//   --port <n>                    hub port (default $APERTURE_WS_PORT or 4517)
//   --hub-log <file>              hub stderr log to tail for codex_bound
//                                 events (subscribers can't see bridge binds
//                                 over the WS protocol — they're hub-side
//                                 JSON log lines only)
//   --require-bridge-bind <name>  require a codex_bound log event for this
//                                 agent (repeatable; requires --hub-log)
//   --out <file>                  write a results JSON (time-to-hello per
//                                 agent, in ms) for SLA drift tracking
//
// Output: one JSON line per observed event with monotonic elapsed ms.
// Exit 0 when all expectations are met within the timeout; exit 1 with a
// summary of missing expectations on timeout.

import { readFileSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";

const require = createRequire(
  new URL("../../mcp-server/package.json", import.meta.url),
);
const WebSocket = require("ws");

// ── Arg parsing ──
const expectedAgents = [];
const requiredBinds = [];
let timeoutS = 60;
let port = Number(process.env.APERTURE_WS_PORT ?? 4517);
let hubLog = null;
let outFile = null;

const argv = process.argv.slice(2);
for (let i = 0; i < argv.length; i++) {
  const next = () => {
    if (i + 1 >= argv.length) {
      process.stderr.write(`observer: ${argv[i]} requires a value\n`);
      process.exit(2);
    }
    return argv[++i];
  };
  switch (argv[i]) {
    case "--expect-agent":
      expectedAgents.push(next());
      break;
    case "--require-bridge-bind":
      requiredBinds.push(next());
      break;
    case "--timeout-s":
      timeoutS = Number(next());
      break;
    case "--port":
      port = Number(next());
      break;
    case "--hub-log":
      hubLog = next();
      break;
    case "--out":
      outFile = next();
      break;
    default:
      process.stderr.write(`observer: unknown arg ${argv[i]}\n`);
      process.exit(2);
  }
}
if (expectedAgents.length === 0) {
  process.stderr.write("observer: at least one --expect-agent is required\n");
  process.exit(2);
}
if (requiredBinds.length > 0 && !hubLog) {
  process.stderr.write("observer: --require-bridge-bind requires --hub-log\n");
  process.exit(2);
}

// ── State ──
const t0 = process.hrtime.bigint();
const elapsedMs = () => Number((process.hrtime.bigint() - t0) / 1_000_000n);
/** agent name → time-to-hello (ms from observer start) */
const joined = new Map();
/** agent name → time-to-codex_bound (ms from observer start) */
const bound = new Map();

function emit(obj) {
  process.stdout.write(JSON.stringify({ elapsed_ms: elapsedMs(), ...obj }) + "\n");
}

function missing() {
  return {
    agents: expectedAgents.filter((a) => !joined.has(a)),
    binds: requiredBinds.filter((a) => !bound.has(a)),
  };
}

function writeResults(ok) {
  if (!outFile) return;
  const results = {
    ok,
    started_at: new Date(Date.now() - elapsedMs()).toISOString(),
    port,
    timeout_s: timeoutS,
    expected_agents: expectedAgents,
    required_binds: requiredBinds,
    time_to_hello_ms: Object.fromEntries(joined),
    time_to_bind_ms: Object.fromEntries(bound),
    missing: missing(),
  };
  try {
    writeFileSync(outFile, JSON.stringify(results, null, 2) + "\n");
  } catch (e) {
    process.stderr.write(`observer: failed to write ${outFile}: ${e.message}\n`);
  }
}

function finish(ok) {
  writeResults(ok);
  if (ok) {
    emit({ event: "observer_done", ok: true });
    process.exit(0);
  }
  const m = missing();
  emit({ event: "observer_timeout", ok: false, missing: m });
  process.stderr.write(
    `observer: TIMEOUT after ${timeoutS}s — missing agents: [${m.agents.join(", ")}]` +
      (requiredBinds.length ? ` missing binds: [${m.binds.join(", ")}]` : "") +
      "\n",
  );
  process.exit(1);
}

function checkDone() {
  const m = missing();
  if (m.agents.length === 0 && m.binds.length === 0) finish(true);
}

const deadline = setTimeout(() => finish(false), timeoutS * 1000);
deadline.unref?.();
// The timeout must actually fire even if the WS keeps the loop alive — do not
// unref the deadline if a socket is open. (unref + re-ref dance avoided by
// simply keeping it referenced.)
deadline.ref?.();

// ── Hub log tail (codex_bound is hub-stderr-only, invisible to subscribers) ──
if (hubLog) {
  let offset = 0;
  let partial = "";
  const poll = setInterval(() => {
    let text;
    try {
      text = readFileSync(hubLog, "utf8");
    } catch {
      return; // log not there yet
    }
    if (text.length <= offset) return;
    const chunk = partial + text.slice(offset);
    offset = text.length;
    const lines = chunk.split("\n");
    partial = lines.pop() ?? "";
    for (const line of lines) {
      if (!line.trim()) continue;
      let obj;
      try {
        obj = JSON.parse(line);
      } catch {
        continue;
      }
      if (obj.event === "codex_bound" && typeof obj.agent === "string") {
        if (!bound.has(obj.agent)) {
          bound.set(obj.agent, elapsedMs());
          emit({ event: "codex_bound", agent: obj.agent, source: "hub-log" });
          checkDone();
        }
      }
    }
  }, 250);
  poll.unref?.();
}

// ── Subscriber connection (retry to survive hub-not-up-yet) ──
const url = `ws://127.0.0.1:${port}`;
const RETRY_MS = 500;
const MAX_ATTEMPTS = 20;

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
    emit({ event: "observer_connect_retry", attempt, error: err.message });
    if (attempt === MAX_ATTEMPTS) {
      process.stderr.write(`observer: hub never became connectable at ${url}\n`);
      writeResults(false);
      process.exit(1);
    }
    await new Promise((r) => setTimeout(r, RETRY_MS));
  }
}

ws.send(JSON.stringify({ type: "hello", role: "subscriber" }));
emit({ event: "observer_subscribed", url });

ws.on("message", (data) => {
  let msg;
  try {
    msg = JSON.parse(data.toString());
  } catch {
    return;
  }
  if (msg.type !== "presence") return;
  emit({ event: "presence", agent: msg.agent, presence: msg.event });
  if (msg.event === "join" && !joined.has(msg.agent)) {
    joined.set(msg.agent, elapsedMs());
    checkDone();
  }
});

ws.on("close", (code) => {
  emit({ event: "observer_socket_closed", code });
  // Hub died before expectations were met → fail fast rather than idle.
  writeResults(false);
  process.stderr.write("observer: hub connection closed before expectations met\n");
  process.exit(1);
});

process.on("SIGTERM", () => {
  writeResults(false);
  process.exit(1);
});
