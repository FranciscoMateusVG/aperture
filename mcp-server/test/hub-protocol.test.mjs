// aperture-xt16e — hub protocol pin suite for the comms-v2 WS hub (dist/ws-hub.js).
//
// Pins the wire-protocol behaviors the boot-verification harness depends on,
// so regressions are caught here before the expensive L3 boot test:
//
//   1. hello-then-join        — subscriber sees presence join; agent hello gets NO ack
//   2. bad-first-frame        — garbage or non-hello first frame → close 4001
//   3. producer-notify-online — notify forwarded exactly once + {type:"ok"} ack
//   4. producer-notify-offline— offline recipient still acked, notify_offline logged
//   5. agent_replaced (old alive) — old socket closed 4000, two joins / zero leaves,
//                                   delivery reaches the NEW socket only
//   6. agent_replaced (old dead first) — abrupt TCP death then fast re-hello:
//                                        new socket mapped, no dup delivery, no crash
//   7. post-hello garbage ignored — connection stays open, later delivery still works
//   8. shutdown               — SIGTERM → clean exit 0
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
import { existsSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import WebSocket from "ws";

const here = dirname(fileURLToPath(import.meta.url));
const hubPath = resolve(here, "..", "dist", "ws-hub.js");

if (!existsSync(hubPath)) {
  throw new Error(
    `dist/ws-hub.js not found at ${hubPath} — build first: cd mcp-server && pnpm build (or: just build-mcp)`,
  );
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/**
 * Spawn a fresh hub on a random high port.
 *   - APERTURE_HUB_SKIP_REPLAY=1 : no bd/BEADS in the loop
 *   - APERTURE_AGENTS_DIR=<empty tmp dir> : no Codex bridges discovered, so
 *     notify routing always takes the Monitor-socket path (isolation from the
 *     developer's real ~/.claude/aperture tree)
 * Returns { port, proc, stderrEvents, waitForEvent, stop }.
 * stderrEvents is the live array of parsed JSON log lines from hub stderr.
 */
async function spawnHub() {
  const port = 20000 + Math.floor(Math.random() * 20000);
  const emptyAgentsDir = mkdtempSync(join(tmpdir(), "hub-test-agents-"));
  const proc = spawn(process.execPath, [hubPath], {
    env: {
      ...process.env,
      APERTURE_WS_PORT: String(port),
      APERTURE_HUB_SKIP_REPLAY: "1",
      APERTURE_AGENTS_DIR: emptyAgentsDir,
    },
    stdio: ["ignore", "ignore", "pipe"],
  });

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
  }

  await waitForEvent((e) => e.event === "listening", "listening", 5000);
  return { port, proc, stderrEvents, waitForEvent, stop };
}

function connect(port) {
  return new Promise((resolvePromise, reject) => {
    const ws = new WebSocket(`ws://127.0.0.1:${port}`);
    ws.on("open", () => resolvePromise(ws));
    ws.on("error", reject);
  });
}

const hello = (ws, role, agent) =>
  ws.send(JSON.stringify(agent ? { type: "hello", role, agent } : { type: "hello", role }));

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
    hello(subscriber, "subscriber");

    const agent = await connect(hub.port);
    const agentFrames = recordFrames(agent);
    const joinPromise = waitFor(
      subscriber,
      (m) => m.type === "presence" && m.agent === "izzy-test" && m.event === "join",
      "presence join for izzy-test",
    );
    hello(agent, "agent", "izzy-test");
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

// ── c. producer notify → online agent ───────────────────────────────────────

test("producer notify to online agent: exactly-once delivery + ok ack", async () => {
  const hub = await spawnHub();
  try {
    const agent = await connect(hub.port);
    hello(agent, "agent", "izzy-test");
    const agentFrames = recordFrames(agent);

    const producer = await connect(hub.port);
    hello(producer, "producer");

    const okPromise = waitFor(producer, (m) => m.type === "ok" && m.id === "msg-1", "ok ack");
    const deliveryPromise = waitFor(
      agent,
      (m) => m.type === "message" && m.id === "msg-1",
      "message delivery",
    );
    producer.send(
      JSON.stringify({ type: "notify", to: "izzy-test", id: "msg-1", from: "glados", preview: "ping" }),
    );
    await okPromise;
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

test("producer notify to offline agent: still acked ok, notify_offline logged, no crash", async () => {
  const hub = await spawnHub();
  try {
    const producer = await connect(hub.port);
    hello(producer, "producer");

    const okPromise = waitFor(producer, (m) => m.type === "ok" && m.id === "msg-2", "ok ack");
    producer.send(
      JSON.stringify({ type: "notify", to: "nobody-home", id: "msg-2", from: "glados", preview: "x" }),
    );
    await okPromise;
    await hub.waitForEvent(
      (e) => e.event === "notify_offline" && e.to === "nobody-home" && e.id === "msg-2",
      "notify_offline log",
    );

    // Hub survived: a second notify still round-trips.
    const okAgain = waitFor(producer, (m) => m.type === "ok" && m.id === "msg-3", "second ok ack");
    producer.send(
      JSON.stringify({ type: "notify", to: "nobody-home", id: "msg-3", from: "glados", preview: "y" }),
    );
    await okAgain;
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
    hello(subscriber, "subscriber");
    const presenceFrames = recordFrames(subscriber);

    const sockA = await connect(hub.port);
    const framesA = recordFrames(sockA);
    const firstJoin = waitFor(
      subscriber,
      (m) => m.type === "presence" && m.agent === "dup-test" && m.event === "join",
      "first join",
    );
    hello(sockA, "agent", "dup-test");
    await firstJoin;

    const sockB = await connect(hub.port);
    const framesB = recordFrames(sockB);
    const aClosed = waitForClose(sockA, "old socket close");
    hello(sockB, "agent", "dup-test");

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
    hello(producer, "producer");
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
    hello(subscriber, "subscriber");
    const presenceFrames = recordFrames(subscriber);

    const sockA = await connect(hub.port);
    const joinA = waitFor(
      subscriber,
      (m) => m.type === "presence" && m.agent === "dup2" && m.event === "join",
      "join for A",
    );
    hello(sockA, "agent", "dup2");
    await joinA;

    // Abrupt death: destroy the TCP socket with no close frame, then re-hello
    // as fast as possible so B's hello races the server noticing A's death.
    sockA.terminate();
    const sockB = await connect(hub.port);
    const framesB = recordFrames(sockB);
    hello(sockB, "agent", "dup2");

    // Hard invariant 1: B is the mapped socket — a notify reaches it.
    const producer = await connect(hub.port);
    hello(producer, "producer");
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
    hello(agent, "agent", "izzy-test");
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
    hello(producer, "producer");
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
    hello(agent, "agent", "izzy-test");

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
