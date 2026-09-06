// aperture-xt16e — hub protocol pin suite for the comms-v2 WS hub (dist/ws-hub.js).
//
// Pins the wire-protocol behaviors the boot-verification harness depends on,
// so regressions are caught here before the expensive L3 boot test:
//
//   1. hello-then-join        — subscriber sees presence join; agent hello gets NO ack
//   2. bad-first-frame        — garbage or non-hello first frame → close 4001
//   3. producer-notify-online — notify forwarded exactly once + {type:"ok", outcome:"forwarded"} ack
//   4. producer-notify-offline— offline recipient acked {outcome:"offline"}, notify_offline logged
//   5. agent_replaced (old alive) — old socket closed 4000, two joins / zero leaves,
//                                   delivery reaches the NEW socket only
//   6. agent_replaced (old dead first) — abrupt TCP death then fast re-hello:
//                                        new socket mapped, no dup delivery, no crash
//   7. post-hello garbage ignored — connection stays open, later delivery still works
//   8. shutdown               — SIGTERM → clean exit 0
//   9. presence snapshot (aperture-oeb6q) — ~/.aperture/run/presence.json (APERTURE_RUN_DIR):
//        a. startup clears a stale file; join → "online"; re-join (agent_replaced)
//           keeps state+since; leave → agent removed; late subscriber still gets
//           the {type:"presence", agent, event:"join", ts} snapshot frame
//        b. codex bridge turn state → "busy"/"idle", `since` moves only on a
//           transition; app-server socket loss → leave; ack outcome "codex"
//  10. presence_hint (aperture-trgpo) — producer frame {type:"presence_hint", event}
//        a. busy → subscriber frame + presence.json busy; repeat busy → NO frame,
//           since unchanged, ack applied:false; idle → frame + flip
//        b. hint for an agent with no Monitor socket → ignored (not_present), no
//           frame, no presence entry (a hint never creates presence)
//        c. hint carrying a foreign `agent` field → rejected (agent_mismatch), no ack
//        d. leave after busy → presence entry deleted
//        e. hint for a codex-bridged agent → ignored (codex_bridged)
//
// Run: node --test test/hub-protocol.test.mjs   (from mcp-server/, after pnpm build)
// Or:  pnpm test:hub
//
// FUTURE (untestable without hub changes — do NOT implement here):
//   - heartbeat_dead: HEARTBEAT_MS is hardcoded to 30_000 in src/ws-hub.ts, far too
//     slow for a unit test. Pinning "socket that misses a ping is terminated and its
//     agent gets a presence leave" needs an APERTURE_HUB_HEARTBEAT_MS env knob.
//   - replay: APERTURE_HUB_SKIP_REPLAY=1 is required to run without bd/BEADS, so the
//     unread-replay-on-connect path is only pinned as "replay_skipped" is logged.
//     Real replay coverage would need an injectable getUnreadMessages (or a bd stub
//     on PATH).

import { test } from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import WebSocket from "ws";
import { FakeAppServer } from "./fixtures/fake-appserver.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const hubPath = resolve(here, "..", "dist", "ws-hub.js");

if (!existsSync(hubPath)) {
  throw new Error(
    `dist/ws-hub.js not found at ${hubPath} — build first: cd mcp-server && pnpm build (or: just build-mcp)`,
  );
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const TOKENS = {
  watchdog: "01".repeat(32),
  "izzy-test": "02".repeat(32),
  glados: "03".repeat(32),
  "dup-test": "04".repeat(32),
  dup2: "05".repeat(32),
  // aperture-trgpo: the codex-bridged agent needs a token too, so a producer
  // (hook hint client) can authenticate as it.
  cbx: "06".repeat(32),
};

/**
 * Spawn a fresh hub on a random high port.
 *   - APERTURE_HUB_SKIP_REPLAY=1 : no bd/BEADS in the loop
 *   - APERTURE_AGENTS_DIR=<tmp dir> : empty by default, so no Codex bridges are
 *     discovered and notify routing always takes the Monitor-socket path
 *     (isolation from the developer's real ~/.claude/aperture tree). Pass
 *     `codexAgent` to seed ONE codex manifest so the hub starts a bridge for it
 *     (its app-server socket is expected at <runDir>/<codexAgent>.sock).
 *   - APERTURE_RUN_DIR=<tmp dir> : presence.json + codex sockets land here, never
 *     in the developer's real ~/.aperture/run. Pass `staleSnapshot` to pre-seed
 *     presence.json before the hub boots (startup-clear pin).
 * Returns { port, proc, runDir, presenceFile, readPresence, stderrEvents, waitForEvent, stop }.
 * stderrEvents is the live array of parsed JSON log lines from hub stderr.
 */
async function spawnHub({ codexAgent = null, staleSnapshot = null } = {}) {
  const port = 20000 + Math.floor(Math.random() * 20000);
  const emptyAgentsDir = mkdtempSync(join(tmpdir(), "hub-test-agents-"));
  const tokenDir = mkdtempSync(join(tmpdir(), "hub-test-tokens-"));
  // Short prefix: the codex app-server unix socket lives here and sun_path is
  // ~104 bytes on macOS.
  const runDir = mkdtempSync(join(tmpdir(), "hr-"));
  const presenceFile = join(runDir, "presence.json");
  for (const [principal, token] of Object.entries(TOKENS)) {
    writeFileSync(join(tokenDir, `${principal}.token`), token, { mode: 0o600 });
  }
  if (codexAgent) {
    mkdirSync(join(emptyAgentsDir, codexAgent));
    writeFileSync(join(emptyAgentsDir, codexAgent, "manifest.json"), JSON.stringify({ model: "codex/gpt-5-codex" }));
  }
  if (staleSnapshot) writeFileSync(presenceFile, JSON.stringify(staleSnapshot));
  const proc = spawn(process.execPath, [hubPath], {
    env: {
      ...process.env,
      APERTURE_WS_PORT: String(port),
      APERTURE_HUB_SKIP_REPLAY: "1",
      APERTURE_AGENTS_DIR: emptyAgentsDir,
      APERTURE_HUB_TOKEN_DIR: tokenDir,
      APERTURE_RUN_DIR: runDir,
      // Never let the developer's real ~/.aperture/agent-config.json model
      // overrides leak into discovery.
      APERTURE_AGENT_CONFIG_PATH: join(runDir, "no-such-agent-config.json"),
      // Codex bridge reconnect cadence (default 10s) — fast so a bridge whose
      // socket appears/disappears during a test is observed promptly.
      APERTURE_CODEX_RECONNECT_MS: "200",
    },
    stdio: ["ignore", "ignore", "pipe"],
  });

  /** Parse presence.json as the hub last wrote it. */
  function readPresence() {
    return JSON.parse(readFileSync(presenceFile, "utf8"));
  }

  const stderrEvents = [];
  const waiters = []; // { pred, resolve }
  let buf = "";
  proc.stderr.on("data", (d) => {
    buf += d.toString();
    let nl;
    while ((nl = buf.indexOf("\n")) !== -1) {
      const line = buf.slice(0, nl);
      buf = buf.slice(nl + 1);
      let ev;
      try {
        ev = JSON.parse(line);
      } catch {
        continue; // non-JSON stderr noise
      }
      stderrEvents.push(ev);
      for (let i = waiters.length - 1; i >= 0; i--) {
        if (waiters[i].pred(ev)) {
          waiters[i].resolve(ev);
          waiters.splice(i, 1);
        }
      }
    }
  });

  /** Resolve when a stderr event matching pred has been seen (past or future). */
  function waitForEvent(pred, what, timeoutMs = 3000) {
    const already = stderrEvents.find(pred);
    if (already) return Promise.resolve(already);
    return new Promise((resolvePromise, reject) => {
      const timer = setTimeout(
        () => reject(new Error(`timed out waiting for stderr event: ${what}`)),
        timeoutMs,
      );
      waiters.push({
        pred,
        resolve: (ev) => {
          clearTimeout(timer);
          resolvePromise(ev);
        },
      });
    });
  }

  function stop() {
    proc.kill("SIGKILL");
    rmSync(emptyAgentsDir, { recursive: true, force: true });
    rmSync(tokenDir, { recursive: true, force: true });
    rmSync(runDir, { recursive: true, force: true });
  }

  await waitForEvent((e) => e.event === "listening", "listening", 5000);
  return { port, proc, runDir, presenceFile, readPresence, stderrEvents, waitForEvent, stop };
}

function connect(port) {
  return new Promise((resolvePromise, reject) => {
    const ws = new WebSocket(`ws://127.0.0.1:${port}`);
    ws.on("open", () => resolvePromise(ws));
    ws.on("error", reject);
  });
}

const hello = (ws, role, agent) => {
  const principal = agent ?? (role === "subscriber" ? "watchdog" : "glados");
  ws.send(JSON.stringify({ type: "hello", role, agent: principal, token: TOKENS[principal] }));
};

async function authenticate(hub, ws, role, agent) {
  const principal = agent ?? (role === "subscriber" ? "watchdog" : "glados");
  hello(ws, role, agent);
  await hub.waitForEvent(
    (e) => e.event === "hello" && e.role === role && e.agent === principal,
    `authenticated ${role} hello for ${principal}`,
  );
}

/** Attach a live frame recorder to a socket. Returns the array of parsed frames. */
function recordFrames(ws) {
  const frames = [];
  ws.on("message", (data) => {
    try {
      frames.push(JSON.parse(data.toString()));
    } catch {
      frames.push({ __unparseable: data.toString() });
    }
  });
  return frames;
}

/** Wait for the next frame on ws matching pred. */
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

/** Wait for close; resolves with {code, reason}. */
function waitForClose(ws, what, timeoutMs = 3000) {
  return new Promise((resolvePromise, reject) => {
    const timer = setTimeout(
      () => reject(new Error(`timed out waiting for ${what}`)),
      timeoutMs,
    );
    ws.on("close", (code, reason) => {
      clearTimeout(timer);
      resolvePromise({ code, reason: reason?.toString() ?? "" });
    });
  });
}

function closeAll(...sockets) {
  for (const ws of sockets) {
    try {
      ws.close();
    } catch {
      // already closed / destroyed
    }
  }
}

// ── a. hello-then-join ──────────────────────────────────────────────────────

test("agent hello: subscriber gets presence join; agent socket gets NO ack frame", async () => {
  const hub = await spawnHub();
  try {
    const subscriber = await connect(hub.port);
    await authenticate(hub, subscriber, "subscriber");

    const agent = await connect(hub.port);
    const agentFrames = recordFrames(agent);
    const joinPromise = waitFor(
      subscriber,
      (m) => m.type === "presence" && m.agent === "izzy-test" && m.event === "join",
      "presence join for izzy-test",
    );
    await authenticate(hub, agent, "agent", "izzy-test");
    const join = await joinPromise;
    assert.equal(typeof join.ts, "string", "presence event carries a ts");

    // Protocol pin: hello gets NO acknowledgement back to the agent socket.
    // (With SKIP_REPLAY the only possible frames would be an ack or replayed
    // messages; neither must appear.)
    await sleep(300);
    assert.deepEqual(agentFrames, [], "agent received zero frames after hello (no ack)");

    await hub.waitForEvent(
      (e) => e.event === "replay_skipped" && e.agent === "izzy-test",
      "replay_skipped log",
    );
    closeAll(subscriber, agent);
  } finally {
    hub.stop();
  }
});

// ── b. bad-first-frame → 4001 ───────────────────────────────────────────────

test("first frame must be hello: garbage JSON and valid-JSON non-hello both close 4001", async () => {
  const hub = await spawnHub();
  try {
    // Unparseable first frame.
    const garbage = await connect(hub.port);
    const garbageClosed = waitForClose(garbage, "garbage-sender close");
    garbage.send("this is {not json");
    const g = await garbageClosed;
    assert.equal(g.code, 4001, "garbage first frame → close code 4001");
    await hub.waitForEvent((e) => e.event === "bad_first_message", "bad_first_message log");

    // Valid JSON but not a hello.
    const nonHello = await connect(hub.port);
    const nonHelloClosed = waitForClose(nonHello, "non-hello-sender close");
    nonHello.send(JSON.stringify({ type: "notify", to: "someone", id: "evil" }));
    const n = await nonHelloClosed;
    assert.equal(n.code, 4001, "non-hello first frame → close code 4001");

    // Hello with an invalid role is also rejected with 4001.
    const badRole = await connect(hub.port);
    const badRoleClosed = waitForClose(badRole, "bad-role-sender close");
    badRole.send(JSON.stringify({ type: "hello", role: "admin" }));
    const b = await badRoleClosed;
    assert.equal(b.code, 4001, "hello with invalid role → close code 4001");
    await hub.waitForEvent((e) => e.event === "bad_hello", "bad_hello log");
  } finally {
    hub.stop();
  }
});

test("hello authentication fails closed for missing, invalid, and wrong-principal tokens", async () => {
  const hub = await spawnHub();
  try {
    const cases = [
      { type: "hello", role: "agent", agent: "izzy-test" },
      { type: "hello", role: "agent", agent: "izzy-test", token: "ff".repeat(32) },
      { type: "hello", role: "agent", agent: "izzy-test", token: TOKENS.glados },
      { type: "hello", role: "subscriber", agent: "watchdog" },
      { type: "hello", role: "producer", agent: "glados", token: "malformed" },
    ];
    for (const frame of cases) {
      const ws = await connect(hub.port);
      const closed = waitForClose(ws, "unauthenticated hello close");
      ws.send(JSON.stringify(frame));
      const result = await closed;
      assert.equal(result.code, 4001, "auth failure uses the generic hello rejection code");
      assert.equal(result.reason, "expected hello", "auth failure does not disclose credential details");
    }
    assert.equal(
      hub.stderrEvents.some((e) => e.event === "presence" && e.agent === "izzy-test"),
      false,
      "failed authentication never registers presence",
    );
  } finally {
    hub.stop();
  }
});

test("authenticated producer cannot spoof notify.from", async () => {
  const hub = await spawnHub();
  try {
    const agent = await connect(hub.port);
    await authenticate(hub, agent, "agent", "izzy-test");
    const frames = recordFrames(agent);

    const producer = await connect(hub.port);
    await authenticate(hub, producer, "producer", "glados");
    producer.send(
      JSON.stringify({ type: "notify", to: "izzy-test", id: "spoof", from: "mallory", preview: "x" }),
    );

    await sleep(250);
    assert.equal(frames.some((f) => f.id === "spoof"), false, "spoofed notification is not delivered");
    assert.equal(
      hub.stderrEvents.some((e) => e.event === "notify_forwarded" && e.id === "spoof"),
      false,
      "spoofed notification is not treated as forwarded",
    );
    closeAll(agent, producer);
  } finally {
    hub.stop();
  }
});

// ── c. producer notify → online agent ───────────────────────────────────────

test("producer notify to online agent: exactly-once delivery + ok ack with outcome:'forwarded'", async () => {
  const hub = await spawnHub();
  try {
    const agent = await connect(hub.port);
    await authenticate(hub, agent, "agent", "izzy-test");
    const agentFrames = recordFrames(agent);

    const producer = await connect(hub.port);
    await authenticate(hub, producer, "producer");

    const okPromise = waitFor(producer, (m) => m.type === "ok" && m.id === "msg-1", "ok ack");
    const deliveryPromise = waitFor(
      agent,
      (m) => m.type === "message" && m.id === "msg-1",
      "message delivery",
    );
    producer.send(
      JSON.stringify({ type: "notify", to: "izzy-test", id: "msg-1", from: "glados", preview: "ping" }),
    );
    const ack = await okPromise;
    // aperture-oeb6q: the ack says what the hub actually did.
    assert.deepEqual(
      ack,
      { type: "ok", id: "msg-1", outcome: "forwarded" },
      "ack is exactly {type:'ok', id, outcome:'forwarded'} when the Monitor socket is connected",
    );
    const delivered = await deliveryPromise;
    assert.deepEqual(
      delivered,
      { type: "message", id: "msg-1", from: "glados", preview: "ping" },
      "delivered frame is {type:'message', id, from, preview}",
    );

    // Exactly once: no duplicate push for the same notify.
    await sleep(200);
    const copies = agentFrames.filter((f) => f.type === "message" && f.id === "msg-1");
    assert.equal(copies.length, 1, "message msg-1 delivered exactly once");

    await hub.waitForEvent(
      (e) => e.event === "notify_forwarded" && e.to === "izzy-test" && e.id === "msg-1",
      "notify_forwarded log",
    );
    closeAll(agent, producer);
  } finally {
    hub.stop();
  }
});

// ── d. producer notify → offline agent ──────────────────────────────────────

test("producer notify to offline agent: acked ok with outcome:'offline', notify_offline logged, no crash", async () => {
  const hub = await spawnHub();
  try {
    const producer = await connect(hub.port);
    await authenticate(hub, producer, "producer");

    const okPromise = waitFor(producer, (m) => m.type === "ok" && m.id === "msg-2", "ok ack");
    producer.send(
      JSON.stringify({ type: "notify", to: "nobody-home", id: "msg-2", from: "glados", preview: "x" }),
    );
    const ack = await okPromise;
    // aperture-oeb6q: an honest ack — nobody was pushed to; replay covers it.
    assert.deepEqual(
      ack,
      { type: "ok", id: "msg-2", outcome: "offline" },
      "ack is exactly {type:'ok', id, outcome:'offline'} when no socket is connected for the recipient",
    );
    await hub.waitForEvent(
      (e) => e.event === "notify_offline" && e.to === "nobody-home" && e.id === "msg-2",
      "notify_offline log",
    );

    // Hub survived: a second notify still round-trips.
    const okAgain = waitFor(producer, (m) => m.type === "ok" && m.id === "msg-3", "second ok ack");
    producer.send(
      JSON.stringify({ type: "notify", to: "nobody-home", id: "msg-3", from: "glados", preview: "y" }),
    );
    assert.equal((await okAgain).outcome, "offline", "second ack also reports offline");
    assert.equal(hub.proc.exitCode, null, "hub process still alive");
    closeAll(producer);
  } finally {
    hub.stop();
  }
});

// ── e. agent_replaced, ordering 1: old socket still alive ───────────────────

test("agent_replaced (old alive): old closed 4000, two joins / zero leaves, delivery to new only", async () => {
  const hub = await spawnHub();
  try {
    const subscriber = await connect(hub.port);
    await authenticate(hub, subscriber, "subscriber");
    const presenceFrames = recordFrames(subscriber);

    const sockA = await connect(hub.port);
    const framesA = recordFrames(sockA);
    const firstJoin = waitFor(
      subscriber,
      (m) => m.type === "presence" && m.agent === "dup-test" && m.event === "join",
      "first join",
    );
    await authenticate(hub, sockA, "agent", "dup-test");
    await firstJoin;

    const sockB = await connect(hub.port);
    const framesB = recordFrames(sockB);
    const aClosed = waitForClose(sockA, "old socket close");
    await authenticate(hub, sockB, "agent", "dup-test");

    const closed = await aClosed;
    assert.equal(closed.code, 4000, "replaced socket closed with code 4000");
    assert.equal(closed.reason, "replaced by newer connection", "close reason names replacement");
    await hub.waitForEvent(
      (e) => e.event === "agent_replaced" && e.agent === "dup-test",
      "agent_replaced log",
    );

    // Presence pin: TWO joins for dup-test, ZERO leaves (the replaced socket's
    // close handler must not broadcast a spurious leave).
    await sleep(300);
    const joins = presenceFrames.filter(
      (m) => m.type === "presence" && m.agent === "dup-test" && m.event === "join",
    );
    const leaves = presenceFrames.filter(
      (m) => m.type === "presence" && m.agent === "dup-test" && m.event === "leave",
    );
    assert.equal(joins.length, 2, "subscriber saw exactly two joins for dup-test");
    assert.equal(leaves.length, 0, "subscriber saw zero leaves for dup-test");

    // Delivery pin: notify now reaches B, and never A (A is closed).
    const producer = await connect(hub.port);
    await authenticate(hub, producer, "producer");
    const deliveryB = waitFor(sockB, (m) => m.type === "message" && m.id === "msg-dup", "delivery to B");
    producer.send(
      JSON.stringify({ type: "notify", to: "dup-test", id: "msg-dup", from: "glados", preview: "z" }),
    );
    await deliveryB;
    await sleep(200);
    assert.equal(
      framesA.filter((f) => f.type === "message").length,
      0,
      "old socket A never received a message frame",
    );
    assert.equal(
      framesB.filter((f) => f.type === "message" && f.id === "msg-dup").length,
      1,
      "new socket B received msg-dup exactly once",
    );
    closeAll(subscriber, sockB, producer);
  } finally {
    hub.stop();
  }
});

// ── f. agent_replaced, ordering 2: old socket dies abruptly first ───────────

test("agent_replaced (old dead first): abrupt TCP death then fast re-hello — new socket mapped, no dup, no crash", async () => {
  const hub = await spawnHub();
  try {
    const subscriber = await connect(hub.port);
    await authenticate(hub, subscriber, "subscriber");
    const presenceFrames = recordFrames(subscriber);

    const sockA = await connect(hub.port);
    const joinA = waitFor(
      subscriber,
      (m) => m.type === "presence" && m.agent === "dup2" && m.event === "join",
      "join for A",
    );
    await authenticate(hub, sockA, "agent", "dup2");
    await joinA;

    // Abrupt death: destroy the TCP socket with no close frame, then re-hello
    // as fast as possible so B's hello races the server noticing A's death.
    sockA.terminate();
    const sockB = await connect(hub.port);
    const framesB = recordFrames(sockB);
    await authenticate(hub, sockB, "agent", "dup2");

    // Hard invariant 1: B is the mapped socket — a notify reaches it.
    const producer = await connect(hub.port);
    await authenticate(hub, producer, "producer");
    const deliveryB = waitFor(sockB, (m) => m.type === "message" && m.id === "msg-dup2", "delivery to B");
    producer.send(
      JSON.stringify({ type: "notify", to: "dup2", id: "msg-dup2", from: "glados", preview: "q" }),
    );
    await deliveryB;

    // Hard invariant 2: exactly-once delivery, and the hub did not crash.
    await sleep(300);
    assert.equal(
      framesB.filter((f) => f.type === "message" && f.id === "msg-dup2").length,
      1,
      "B received msg-dup2 exactly once",
    );
    assert.equal(hub.proc.exitCode, null, "hub still alive after abrupt-death + replace race");

    // Observed ordering (recorded, NOT asserted as a hard invariant): which of
    // leave-for-A's-death vs join-for-B lands first depends on whether the
    // server's close handler for A fires before B's hello is processed.
    //   - If A's death is seen first:  join(A), leave(A), join(B)  → 2 joins, 1 leave
    //   - If B's hello wins the race:  join(A), join(B), no leave  → 2 joins, 0 leaves
    //     (agents.get("dup2") is already B when A's close handler runs, so the
    //     spurious-leave guard suppresses it — same guard as ordering 1.)
    // In local runs the loopback RST is delivered promptly, so the observed
    // sequence was join, leave, join (2 joins / 1 leave). Both orderings are
    // legal; the invariant asserted below is only "at most one leave" and
    // "exactly two joins".
    const joins = presenceFrames.filter(
      (m) => m.type === "presence" && m.agent === "dup2" && m.event === "join",
    );
    const leaves = presenceFrames.filter(
      (m) => m.type === "presence" && m.agent === "dup2" && m.event === "leave",
    );
    assert.equal(joins.length, 2, "exactly two joins for dup2 (A then B)");
    assert.ok(leaves.length <= 1, `at most one leave for dup2 (saw ${leaves.length})`);
    closeAll(subscriber, sockB, producer);
  } finally {
    hub.stop();
  }
});

// ── g. post-hello garbage is ignored, connection survives ───────────────────

test("post-hello junk from an agent is ignored; connection stays open and still receives", async () => {
  const hub = await spawnHub();
  try {
    const agent = await connect(hub.port);
    await authenticate(hub, agent, "agent", "izzy-test");
    // Give the hello a beat so the junk below is unambiguously post-hello.
    await sleep(100);

    // Valid JSON, unknown type → ignored_message (logged, socket kept).
    agent.send(JSON.stringify({ type: "weird-frame", payload: 42 }));
    await hub.waitForEvent(
      (e) => e.event === "ignored_message" && e.agent === "izzy-test" && e.type === "weird-frame",
      "ignored_message log",
    );

    // Unparseable post-hello frame → bad_message (logged, socket kept).
    // NOTE: the hub distinguishes the two — unparseable junk logs "bad_message",
    // parseable-but-unknown logs "ignored_message". Both leave the socket open.
    agent.send("also {not json at all");
    await hub.waitForEvent(
      (e) => e.event === "bad_message" && e.agent === "izzy-test",
      "bad_message log",
    );

    // The connection survived both: a later notify still reaches it.
    const producer = await connect(hub.port);
    await authenticate(hub, producer, "producer");
    const delivery = waitFor(agent, (m) => m.type === "message" && m.id === "after-junk", "delivery after junk");
    producer.send(
      JSON.stringify({ type: "notify", to: "izzy-test", id: "after-junk", from: "glados", preview: "still here" }),
    );
    await delivery;
    closeAll(agent, producer);
  } finally {
    hub.stop();
  }
});

// ── h. shutdown ─────────────────────────────────────────────────────────────

test("SIGTERM: hub logs shutdown and exits 0", async () => {
  const hub = await spawnHub();
  try {
    // A connected client makes shutdown exercise the close-all path too.
    const agent = await connect(hub.port);
    await authenticate(hub, agent, "agent", "izzy-test");

    const exitCode = await new Promise((resolvePromise) => {
      hub.proc.on("exit", (code) => resolvePromise(code));
      hub.proc.kill("SIGTERM");
    });
    assert.equal(exitCode, 0, "hub exited 0 on SIGTERM");
    assert.ok(
      hub.stderrEvents.some((e) => e.event === "shutdown" && e.signal === "SIGTERM"),
      "shutdown event logged with signal SIGTERM",
    );
    closeAll(agent);
  } finally {
    hub.stop(); // no-op if already exited; cleans the tmp agents dir
  }
});

// ── i. presence snapshot file (aperture-oeb6q) ──────────────────────────────

/** Poll until fn() is truthy (for count-based conditions past first occurrence). */
async function until(fn, what, timeoutMs = 3000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (fn()) return;
    await sleep(20);
  }
  throw new Error(`timed out waiting until: ${what}`);
}

test("presence snapshot (agent socket): startup clears stale file; join → online; re-join keeps since; leave removes; late subscriber still gets event:'join'", async () => {
  // A crashed hub left this behind: dead pid, phantom busy agent.
  const stale = {
    hub_pid: 999999,
    updated_at: "2000-01-01T00:00:00.000Z",
    agents: { ghost: { state: "busy", since: "2000-01-01T00:00:00.000Z" } },
  };
  const hub = await spawnHub({ staleSnapshot: stale });
  try {
    // Startup write: the file is rewritten with THIS hub's pid and zero agents
    // before the hub even listens.
    const boot = hub.readPresence();
    assert.equal(boot.hub_pid, hub.proc.pid, "startup snapshot carries the live hub pid");
    assert.deepEqual(boot.agents, {}, "startup snapshot has zero agents (stale ghost cleared)");
    assert.ok(boot.updated_at > stale.updated_at, "updated_at is fresh");

    // join → "online"
    const sockA = await connect(hub.port);
    await authenticate(hub, sockA, "agent", "dup-test");
    await hub.waitForEvent(
      (e) => e.event === "presence" && e.agent === "dup-test" && e.presence === "join",
      "presence join log",
    );
    const afterJoin = hub.readPresence();
    assert.deepEqual(Object.keys(afterJoin.agents), ["dup-test"], "only the joined agent is present");
    assert.equal(afterJoin.agents["dup-test"].state, "online", "join → state 'online'");
    const since = afterJoin.agents["dup-test"].since;
    assert.ok(!Number.isNaN(Date.parse(since)), "since is an ISO timestamp");
    assert.ok(afterJoin.updated_at >= boot.updated_at, "updated_at moves forward on join");

    // Byte-compat pin: a subscriber that connects AFTER the join still gets the
    // legacy snapshot frame — the stored "online" state maps back to event "join".
    const subscriber = await connect(hub.port);
    const snapFrame = waitFor(subscriber, (m) => m.type === "presence" && m.agent === "dup-test", "snapshot frame");
    await authenticate(hub, subscriber, "subscriber");
    const snap = await snapFrame;
    assert.deepEqual(Object.keys(snap).sort(), ["agent", "event", "ts", "type"], "snapshot frame has exactly {type, agent, event, ts}");
    assert.equal(snap.event, "join", "stored 'online' is presented to subscribers as event 'join'");
    assert.equal(typeof snap.ts, "string");

    // Re-join (agent_replaced) is NOT a transition: state and since are kept.
    await sleep(20); // make a since bump observable at ms resolution
    const sockB = await connect(hub.port);
    const aClosed = waitForClose(sockA, "old socket close");
    await authenticate(hub, sockB, "agent", "dup-test");
    await aClosed;
    await until(
      () => hub.stderrEvents.filter((e) => e.event === "presence" && e.agent === "dup-test" && e.presence === "join").length >= 2,
      "second join logged",
    );
    const afterRejoin = hub.readPresence();
    assert.equal(afterRejoin.agents["dup-test"].state, "online", "re-join keeps state");
    assert.equal(afterRejoin.agents["dup-test"].since, since, "re-join keeps since (not a transition)");

    // leave → agent removed from the file.
    sockB.close();
    await hub.waitForEvent(
      (e) => e.event === "presence" && e.agent === "dup-test" && e.presence === "leave",
      "presence leave log",
    );
    const afterLeave = hub.readPresence();
    assert.deepEqual(afterLeave.agents, {}, "leave deletes the agent from the snapshot");
    assert.equal(afterLeave.hub_pid, hub.proc.pid, "hub_pid stable across writes");
    assert.ok(afterLeave.updated_at >= afterJoin.updated_at, "updated_at moves forward on leave");
    closeAll(subscriber);
  } finally {
    hub.stop();
  }
});

test("presence snapshot (codex bridge): busy/idle set state, since moves only on transition; socket loss → leave; notify ack outcome:'codex'", async () => {
  const hub = await spawnHub({ codexAgent: "cbx" });
  // The bridge starts before the fake app-server exists (codex_offline), then
  // reconnects within APERTURE_CODEX_RECONNECT_MS once the socket appears.
  const server = new FakeAppServer(join(hub.runDir, "cbx.sock"), { threads: [{ id: "t-1" }] });
  let serverClosed = false;
  const closeServer = async () => {
    if (serverClosed) return;
    serverClosed = true;
    await server.close();
  };
  try {
    await server.start();
    await hub.waitForEvent((e) => e.event === "codex_bound" && e.agent === "cbx", "codex_bound", 5000);
    await hub.waitForEvent(
      (e) => e.event === "presence" && e.agent === "cbx" && e.presence === "join",
      "codex join log",
    );
    const online = hub.readPresence().agents.cbx;
    assert.equal(online.state, "online", "bridge bind → 'online'");

    // online → busy: a transition, since moves.
    await sleep(20);
    server.notify("turn/started", { threadId: "t-1" });
    await hub.waitForEvent((e) => e.event === "presence" && e.agent === "cbx" && e.presence === "busy", "busy log");
    const busy = hub.readPresence().agents.cbx;
    assert.equal(busy.state, "busy", "turn/started → 'busy'");
    assert.ok(busy.since > online.since, "online→busy transition moved since");

    // Late subscriber sees the stored "busy" state as event "busy" (legacy frame).
    const subscriber = await connect(hub.port);
    const snapFrame = waitFor(subscriber, (m) => m.type === "presence" && m.agent === "cbx", "snapshot frame");
    await authenticate(hub, subscriber, "subscriber");
    const snap = await snapFrame;
    assert.deepEqual(Object.keys(snap).sort(), ["agent", "event", "ts", "type"]);
    assert.equal(snap.event, "busy", "stored 'busy' is presented to subscribers as event 'busy'");

    // Repeated busy signals are NOT a transition: the file must be untouched.
    // (The bridge itself dedups turnActive, so these never reach the hub — the
    // pin still holds either way: nothing about cbx changes.)
    await sleep(20);
    server.notify("turn/started", { threadId: "t-1" });
    server.notify("thread/status/changed", { threadId: "t-1", status: "active" });
    await sleep(150);
    assert.deepEqual(hub.readPresence().agents.cbx, busy, "repeated busy leaves state + since untouched");

    // Producer notify to a codex agent → ack outcome "codex".
    const producer = await connect(hub.port);
    await authenticate(hub, producer, "producer");
    const okPromise = waitFor(producer, (m) => m.type === "ok" && m.id === "msg-cbx", "ok ack");
    producer.send(JSON.stringify({ type: "notify", to: "cbx", id: "msg-cbx", from: "glados", preview: "hi" }));
    assert.deepEqual(await okPromise, { type: "ok", id: "msg-cbx", outcome: "codex" }, "codex recipient → outcome 'codex'");
    await hub.waitForEvent((e) => e.event === "notify_codex" && e.id === "msg-cbx", "notify_codex log");

    // busy → idle: a transition, since moves.
    await sleep(20);
    server.notify("turn/completed", { threadId: "t-1" });
    await hub.waitForEvent((e) => e.event === "presence" && e.agent === "cbx" && e.presence === "idle", "idle log");
    const idle = hub.readPresence().agents.cbx;
    assert.equal(idle.state, "idle", "turn/completed → 'idle'");
    assert.ok(idle.since > busy.since, "busy→idle transition moved since");

    // App-server socket loss → bridge leave → agent removed from the file.
    await closeServer();
    await hub.waitForEvent(
      (e) => e.event === "presence" && e.agent === "cbx" && e.presence === "leave",
      "codex leave log",
      5000,
    );
    assert.deepEqual(hub.readPresence().agents, {}, "leave deletes the codex agent from the snapshot");
    closeAll(subscriber, producer);
  } finally {
    await closeServer();
    hub.stop();
  }
});

// ── j. presence_hint (aperture-trgpo) ───────────────────────────────────────

test("presence_hint: busy → frame + snapshot busy; repeat busy → no frame, since kept; idle → flip; leave deletes", async () => {
  const hub = await spawnHub();
  try {
    const subscriber = await connect(hub.port);
    await authenticate(hub, subscriber, "subscriber");
    const presenceFrames = recordFrames(subscriber);

    const agent = await connect(hub.port);
    const joinPromise = waitFor(
      subscriber,
      (m) => m.type === "presence" && m.agent === "izzy-test" && m.event === "join",
      "presence join for izzy-test",
    );
    await authenticate(hub, agent, "agent", "izzy-test");
    await joinPromise;
    const online = hub.readPresence().agents["izzy-test"];
    assert.equal(online.state, "online", "join → 'online' before any hint");

    // The hook client authenticates as a PRODUCER for the same principal.
    const hinter = await connect(hub.port);
    await authenticate(hub, hinter, "producer", "izzy-test");

    // a. busy hint → subscriber frame + snapshot flip + ack applied:true.
    await sleep(20);
    const busyFrame = waitFor(
      subscriber,
      (m) => m.type === "presence" && m.agent === "izzy-test" && m.event === "busy",
      "presence busy frame",
    );
    const busyAck = waitFor(hinter, (m) => m.type === "ok" && m.hint === "busy", "busy ack");
    hinter.send(JSON.stringify({ type: "presence_hint", event: "busy" }));
    assert.deepEqual(
      await busyAck,
      { type: "ok", hint: "busy", applied: true },
      "first busy hint is acked {type:'ok', hint:'busy', applied:true}",
    );
    const frame = await busyFrame;
    assert.deepEqual(Object.keys(frame).sort(), ["agent", "event", "ts", "type"], "busy frame is the legacy presence shape");
    const busy = hub.readPresence().agents["izzy-test"];
    assert.equal(busy.state, "busy", "hint busy → snapshot state 'busy'");
    assert.ok(busy.since > online.since, "online→busy transition moved since");
    await hub.waitForEvent(
      (e) => e.event === "presence_hint" && e.agent === "izzy-test" && e.hint === "busy" && e.applied === true,
      "presence_hint log (applied)",
    );

    // Repeated busy (every PreToolUse fires one): NO subscriber frame, NO log
    // line, since untouched — but still acked so the hook client can exit.
    await sleep(20);
    const framesBefore = presenceFrames.filter((m) => m.type === "presence" && m.agent === "izzy-test").length;
    const logsBefore = hub.stderrEvents.filter((e) => e.event === "presence" && e.agent === "izzy-test").length;
    const repeatAck = waitFor(hinter, (m) => m.type === "ok" && m.hint === "busy", "repeat busy ack");
    hinter.send(JSON.stringify({ type: "presence_hint", event: "busy" }));
    assert.deepEqual(
      await repeatAck,
      { type: "ok", hint: "busy", applied: false },
      "repeat busy hint is acked applied:false",
    );
    await sleep(200);
    assert.equal(
      presenceFrames.filter((m) => m.type === "presence" && m.agent === "izzy-test").length,
      framesBefore,
      "repeat busy hint produced NO additional subscriber frame",
    );
    assert.equal(
      hub.stderrEvents.filter((e) => e.event === "presence" && e.agent === "izzy-test").length,
      logsBefore,
      "repeat busy hint produced NO additional presence log line",
    );
    assert.deepEqual(hub.readPresence().agents["izzy-test"], busy, "repeat busy leaves state + since untouched");

    // idle hint → frame + flip, since moves.
    await sleep(20);
    const idleFrame = waitFor(
      subscriber,
      (m) => m.type === "presence" && m.agent === "izzy-test" && m.event === "idle",
      "presence idle frame",
    );
    const idleAck = waitFor(hinter, (m) => m.type === "ok" && m.hint === "idle", "idle ack");
    hinter.send(JSON.stringify({ type: "presence_hint", event: "idle" }));
    assert.deepEqual(await idleAck, { type: "ok", hint: "idle", applied: true }, "idle hint acked applied:true");
    await idleFrame;
    const idle = hub.readPresence().agents["izzy-test"];
    assert.equal(idle.state, "idle", "hint idle → snapshot state 'idle'");
    assert.ok(idle.since > busy.since, "busy→idle transition moved since");

    // d. Monitor socket closes after a hint → leave still deletes the entry
    // (a hint must not pin the agent in the map past its socket).
    const leaveFrame = waitFor(
      subscriber,
      (m) => m.type === "presence" && m.agent === "izzy-test" && m.event === "leave",
      "presence leave frame",
    );
    agent.close();
    await leaveFrame;
    assert.deepEqual(hub.readPresence().agents, {}, "leave after hints deletes the agent from the snapshot");
    closeAll(subscriber, hinter);
  } finally {
    hub.stop();
  }
});

test("presence_hint for an agent with NO Monitor socket is ignored: no frame, no presence entry, not_present logged", async () => {
  const hub = await spawnHub();
  try {
    const subscriber = await connect(hub.port);
    await authenticate(hub, subscriber, "subscriber");
    const presenceFrames = recordFrames(subscriber);

    // glados is authenticated as a producer but has no agent socket → offline.
    const hinter = await connect(hub.port);
    await authenticate(hub, hinter, "producer", "glados");
    const ack = waitFor(hinter, (m) => m.type === "ok" && m.hint === "busy", "ignored ack");
    hinter.send(JSON.stringify({ type: "presence_hint", event: "busy" }));
    assert.deepEqual(await ack, { type: "ok", hint: "busy", applied: false }, "ignored hint still acked applied:false");
    await hub.waitForEvent(
      (e) => e.event === "presence_hint_ignored" && e.reason === "not_present" && e.agent === "glados",
      "presence_hint_ignored not_present log",
    );

    await sleep(200);
    assert.deepEqual(presenceFrames, [], "subscriber saw NO presence frame — a hint never creates presence");
    assert.deepEqual(hub.readPresence().agents, {}, "no presence entry was created for glados");
    assert.equal(
      hub.stderrEvents.some((e) => e.event === "presence" && e.agent === "glados"),
      false,
      "no presence log line for glados",
    );
    closeAll(subscriber, hinter);
  } finally {
    hub.stop();
  }
});

test("presence_hint carrying a foreign `agent` field is rejected: agent_mismatch logged, no state change, no ack", async () => {
  const hub = await spawnHub();
  try {
    const subscriber = await connect(hub.port);
    await authenticate(hub, subscriber, "subscriber");

    const agent = await connect(hub.port);
    const joinPromise = waitFor(
      subscriber,
      (m) => m.type === "presence" && m.agent === "izzy-test" && m.event === "join",
      "presence join for izzy-test",
    );
    await authenticate(hub, agent, "agent", "izzy-test");
    await joinPromise;
    const presenceFrames = recordFrames(subscriber);
    const online = hub.readPresence().agents["izzy-test"];

    // glados (producer) tries to flip izzy-test busy by naming it in the frame.
    const mallory = await connect(hub.port);
    await authenticate(hub, mallory, "producer", "glados");
    const malloryFrames = recordFrames(mallory);
    mallory.send(JSON.stringify({ type: "presence_hint", event: "busy", agent: "izzy-test" }));
    await hub.waitForEvent(
      (e) => e.event === "presence_hint_rejected" && e.reason === "agent_mismatch" && e.agent === "glados",
      "presence_hint_rejected agent_mismatch log",
    );

    // Invalid event value from a legitimate principal is rejected too.
    const hinter = await connect(hub.port);
    await authenticate(hub, hinter, "producer", "izzy-test");
    const hinterFrames = recordFrames(hinter);
    hinter.send(JSON.stringify({ type: "presence_hint", event: "sleeping" }));
    await hub.waitForEvent(
      (e) => e.event === "presence_hint_rejected" && e.reason === "bad_event" && e.agent === "izzy-test",
      "presence_hint_rejected bad_event log",
    );

    await sleep(200);
    assert.deepEqual(presenceFrames, [], "subscriber saw no presence frame from rejected hints");
    assert.deepEqual(malloryFrames, [], "rejected (agent_mismatch) hint gets no ack");
    assert.deepEqual(hinterFrames, [], "rejected (bad_event) hint gets no ack");
    assert.deepEqual(hub.readPresence().agents["izzy-test"], online, "izzy-test is still 'online' with its original since");
    assert.equal(hub.proc.exitCode, null, "hub still alive");
    closeAll(subscriber, agent, mallory, hinter);
  } finally {
    hub.stop();
  }
});

test("presence_hint for a codex-bridged agent is ignored: the bridge owns turn state", async () => {
  const hub = await spawnHub({ codexAgent: "cbx" });
  const server = new FakeAppServer(join(hub.runDir, "cbx.sock"), { threads: [{ id: "t-1" }] });
  let serverClosed = false;
  const closeServer = async () => {
    if (serverClosed) return;
    serverClosed = true;
    await server.close();
  };
  try {
    await server.start();
    await hub.waitForEvent((e) => e.event === "codex_bound" && e.agent === "cbx", "codex_bound", 5000);
    await hub.waitForEvent(
      (e) => e.event === "presence" && e.agent === "cbx" && e.presence === "join",
      "codex join log",
    );
    const online = hub.readPresence().agents.cbx;

    const subscriber = await connect(hub.port);
    await authenticate(hub, subscriber, "subscriber");
    const presenceFrames = recordFrames(subscriber);

    const hinter = await connect(hub.port);
    await authenticate(hub, hinter, "producer", "cbx");
    const ack = waitFor(hinter, (m) => m.type === "ok" && m.hint === "busy", "codex-bridged ack");
    hinter.send(JSON.stringify({ type: "presence_hint", event: "busy" }));
    assert.deepEqual(await ack, { type: "ok", hint: "busy", applied: false }, "codex-bridged hint acked applied:false");
    await hub.waitForEvent(
      (e) => e.event === "presence_hint_ignored" && e.reason === "codex_bridged" && e.agent === "cbx",
      "presence_hint_ignored codex_bridged log",
    );
    await sleep(200);
    assert.equal(
      presenceFrames.filter((m) => m.agent === "cbx" && m.event !== "join").length,
      0,
      "no busy/idle frame reached the subscriber from the hint",
    );
    assert.deepEqual(hub.readPresence().agents.cbx, online, "codex agent state untouched by the hint");
    closeAll(subscriber, hinter);
  } finally {
    await closeServer();
    hub.stop();
  }
});
