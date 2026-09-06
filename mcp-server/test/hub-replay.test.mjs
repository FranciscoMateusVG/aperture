// aperture-xt16e — replay-on-reconnect pins for the comms-v2 WS hub
// (dist/ws-hub.js), failure mode #3: replay-exactly-once after offline.
//
// Unlike hub-protocol.test.mjs (which sets APERTURE_HUB_SKIP_REPLAY=1), this
// suite runs the REAL replay path: on agent hello, replayUnread() →
// beads.ts getUnreadMessages() shells out to
//
//   bd query 'type=message AND status=open AND title="->AGENT]"' --json -n 200
//
// and each returned row becomes one {type:"message", id, from, preview} frame
// (from = first group of /\[(.+?)->(.+?)\]/ on title; preview = first 60 chars
// of description, newlines → spaces). We intercept that child `bd` with the
// stub at test/fixtures/bd-stub/bd, driven by BD_STUB_DIR seed files.
//
// FINDING (bd interception): prepending the stub dir to the hub's PATH is NOT
// sufficient on a machine with a real bd install. beads.ts bdEnv() prepends
// "/opt/homebrew/bin:/usr/local/bin:" to PATH for every child bd invocation
// (src/beads.ts ~line 17), and the real bd lives at /opt/homebrew/bin/bd, so
// the real binary shadows any stub placed on the inherited PATH. beads.ts
// already exposes an env hook, `BD_PATH` (src/beads.ts line 6:
// `const BD_PATH = process.env.BD_PATH ?? "bd"`), so this suite sets BOTH:
// PATH prepend (documents intent, and is what actually resolves on a machine
// without /opt/homebrew/bin/bd) AND BD_PATH=<abs stub path> (the mechanism
// that is guaranteed to win). Zero hub/beads code changes.
//
// Pins:
//   a. replay-on-connect       — 2 seeded unread rows → exactly 2 message
//                                frames (id + from + 60-char preview truncation),
//                                stderr `replay` count=2, exactly ONE bd query
//                                call with the exact getUnreadMessages argv
//   b. replay-exactly-once-per-connect — rows still unread (agent never called
//                                mark_as_read) → reconnect replays the same 2
//                                frames again (at-least-once BY DESIGN, BEADS
//                                is source of record); within a single
//                                connection each frame arrives exactly once
//   c. replay-after-read       — one row removed (mark_as_read closed it) →
//                                only the remaining row replays on reconnect
//   d. no-unread               — no seed file → hello succeeds, zero message
//                                frames, presence join still fires
//   e. bd-failure              — bd exits 1 → replay_error logged, connection
//                                survives, zero replay frames, live notify
//                                still delivers afterwards
//   f. replay-vs-live ordering — replay in flight while a producer notify
//                                lands → both frames arrive, no duplicates,
//                                no crash (observed ordering recorded below)
//
// Run: node --test test/hub-replay.test.mjs   (from mcp-server/, after pnpm build)
// Or:  pnpm test:replay

import { test } from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import WebSocket from "ws";

const here = dirname(fileURLToPath(import.meta.url));
const hubPath = resolve(here, "..", "dist", "ws-hub.js");
const stubBinDir = resolve(here, "fixtures", "bd-stub");
const stubBdPath = join(stubBinDir, "bd");

if (!existsSync(hubPath)) {
  throw new Error(
    `dist/ws-hub.js not found at ${hubPath} — build first: cd mcp-server && pnpm build (or: just build-mcp)`,
  );
}
if (!existsSync(stubBdPath)) {
  throw new Error(`bd stub not found at ${stubBdPath}`);
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const TOKENS = {
  watchdog: "11".repeat(32),
  "rp-test": "12".repeat(32),
  "rp-empty": "13".repeat(32),
  "rl-test": "14".repeat(32),
  glados: "15".repeat(32),
};

/**
 * Spawn a fresh hub on a random high port with the bd stub wired in.
 *   - NO APERTURE_HUB_SKIP_REPLAY (explicitly stripped from the inherited env)
 *     — the whole point of this suite is the real replay path
 *   - APERTURE_AGENTS_DIR=<empty tmp dir> : no Codex bridges discovered
 *   - BD_STUB_DIR=<fresh tmp dir> : per-hub seed files + bd-calls.log
 *   - PATH=<stub dir>:$PATH AND BD_PATH=<stub bd> — see FINDING in header
 * Returns { port, proc, dataDir, stderrEvents, waitForEvent, until, stop }.
 */
async function spawnHub({ bdFail = false } = {}) {
  const port = 20000 + Math.floor(Math.random() * 20000);
  const emptyAgentsDir = mkdtempSync(join(tmpdir(), "hub-replay-agents-"));
  const dataDir = mkdtempSync(join(tmpdir(), "hub-replay-bdstub-"));
  const tokenDir = mkdtempSync(join(tmpdir(), "hub-replay-tokens-"));
  // aperture-oeb6q: the hub now writes presence.json under APERTURE_RUN_DIR on
  // boot and on every presence change — isolate it so a test hub never
  // clobbers the developer's real ~/.aperture/run/presence.json.
  const runDir = mkdtempSync(join(tmpdir(), "hub-replay-run-"));
  for (const [principal, token] of Object.entries(TOKENS)) {
    writeFileSync(join(tokenDir, `${principal}.token`), token, { mode: 0o600 });
  }

  const env = {
    ...process.env,
    APERTURE_WS_PORT: String(port),
    APERTURE_AGENTS_DIR: emptyAgentsDir,
    APERTURE_HUB_TOKEN_DIR: tokenDir,
    APERTURE_RUN_DIR: runDir,
    BD_STUB_DIR: dataDir,
    PATH: `${stubBinDir}:${process.env.PATH ?? ""}`,
    BD_PATH: stubBdPath,
  };
  delete env.APERTURE_HUB_SKIP_REPLAY; // real replay, always
  if (bdFail) {
    env.BD_STUB_FAIL = "1";
  } else {
    delete env.BD_STUB_FAIL;
  }

  const proc = spawn(process.execPath, [hubPath], {
    env,
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

  /** Poll until fn() is truthy (for count-based conditions past first occurrence). */
  async function until(fn, what, timeoutMs = 4000) {
    const deadline = Date.now() + timeoutMs;
    while (Date.now() < deadline) {
      if (fn()) return;
      await sleep(25);
    }
    throw new Error(`timed out waiting until: ${what}`);
  }

  function stop() {
    proc.kill("SIGKILL");
    rmSync(emptyAgentsDir, { recursive: true, force: true });
    rmSync(dataDir, { recursive: true, force: true });
    rmSync(tokenDir, { recursive: true, force: true });
    rmSync(runDir, { recursive: true, force: true });
  }

  await waitForEvent((e) => e.event === "listening", "listening", 5000);
  return { port, proc, dataDir, stderrEvents, waitForEvent, until, stop };
}

/** Seed $BD_STUB_DIR/unread-<agent>.json with unread message rows. */
function seedUnread(dataDir, agent, rows) {
  writeFileSync(join(dataDir, `unread-${agent}.json`), JSON.stringify(rows));
}

/** Parse bd-calls.log → array of argv arrays (tab-separated lines). */
function bdCalls(dataDir) {
  const logPath = join(dataDir, "bd-calls.log");
  if (!existsSync(logPath)) return [];
  return readFileSync(logPath, "utf8")
    .split("\n")
    .filter((l) => l.length > 0)
    .map((l) => l.split("\t"));
}

/** The exact argv shape beads.ts getUnreadMessages passes to bd. */
const unreadQueryArgv = (agent) => [
  "query",
  `type=message AND status=open AND title="->${agent}]"`,
  "--json",
  "-n",
  "200", // UNREAD_LIMIT (aperture-84bby): bounded replay, was 0 (unlimited)
];

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

function closeAll(...sockets) {
  for (const ws of sockets) {
    try {
      ws.close();
    } catch {
      // already closed / destroyed
    }
  }
}

// Canonical two-row seed for agent rp-test. Row 2's description is >60 chars
// and contains a newline, pinning the preview contract:
// preview = description.slice(0, 60).replace(/\n/g, " ").
const RP = "rp-test";
const row1 = {
  id: "ap-r1",
  title: `[glados->${RP}] status check`,
  description: "status check",
  status: "open",
  issue_type: "message",
};
const row2Desc =
  "urgent: the panopticon rebooted\nplease check tmux pane 3 for the full stack trace and logs";
const row2 = {
  id: "ap-r2",
  title: `[wheatley->${RP}] urgent: the panopticon rebooted please check`,
  description: row2Desc,
  status: "open",
  issue_type: "message",
};
const row1Expected = { type: "message", id: "ap-r1", from: "glados", preview: "status check" };
const row2Expected = {
  type: "message",
  id: "ap-r2",
  from: "wheatley",
  preview: row2Desc.slice(0, 60).replace(/\n/g, " "),
};

// ── a. replay-on-connect ────────────────────────────────────────────────────

test("replay-on-connect: 2 unread rows → exactly 2 message frames, one bd query with exact argv", async () => {
  const hub = await spawnHub();
  try {
    seedUnread(hub.dataDir, RP, [row1, row2]);

    const agent = await connect(hub.port);
    const frames = recordFrames(agent);
    await authenticate(hub, agent, "agent", RP);

    const replayEv = await hub.waitForEvent(
      (e) => e.event === "replay" && e.agent === RP,
      "replay log",
    );
    assert.equal(replayEv.count, 2, "hub logged replay of 2 messages");

    await hub.until(
      () => frames.filter((f) => f.type === "message").length >= 2,
      "2 replayed message frames",
    );
    // Settle window: no third frame, no duplicates.
    await sleep(300);
    const msgs = frames.filter((f) => f.type === "message");
    assert.equal(msgs.length, 2, "exactly 2 message frames replayed");
    assert.deepEqual(msgs[0], row1Expected, "row 1 frame: id/from/preview match");
    assert.deepEqual(msgs[1], row2Expected, "row 2 frame: 60-char preview, newline → space");
    assert.equal(msgs[1].preview.length, 60, "preview truncated to exactly 60 chars");

    // Evidence: exactly ONE bd invocation for this connect, exact argv shape.
    const calls = bdCalls(hub.dataDir);
    assert.equal(calls.length, 1, "exactly one bd call for the connect");
    assert.deepEqual(calls[0], unreadQueryArgv(RP), "bd argv matches getUnreadMessages shape");
    closeAll(agent);
  } finally {
    hub.stop();
  }
});

// ── b. replay-exactly-once-per-connect ──────────────────────────────────────

test("replay-exactly-once-per-connect: rows still unread → reconnect replays same 2 frames again; no dup within a connect", async () => {
  const hub = await spawnHub();
  try {
    seedUnread(hub.dataDir, RP, [row1, row2]);

    // Connect #1.
    const first = await connect(hub.port);
    const firstFrames = recordFrames(first);
    await authenticate(hub, first, "agent", RP);
    await hub.until(
      () => firstFrames.filter((f) => f.type === "message").length >= 2,
      "first-connect replay",
    );
    await sleep(200);
    assert.deepEqual(
      firstFrames.filter((f) => f.type === "message"),
      [row1Expected, row2Expected],
      "connect #1: each frame exactly once, in row order",
    );

    // Disconnect (agent never called mark_as_read — rows stay unread in the
    // stub dir, exactly as they would stay open in BEADS).
    first.close();
    await hub.waitForEvent(
      (e) => e.event === "presence" && e.agent === RP && e.presence === "leave",
      "leave after disconnect",
    );

    // Connect #2 → the SAME 2 frames replay again. This is per-CONNECT
    // at-least-once BY DESIGN: BEADS is the store of record; until the agent
    // closes the rows via mark_as_read, every reconnect replays them.
    const second = await connect(hub.port);
    const secondFrames = recordFrames(second);
    await authenticate(hub, second, "agent", RP);
    await hub.until(
      () => secondFrames.filter((f) => f.type === "message").length >= 2,
      "second-connect replay",
    );
    await sleep(200);
    assert.deepEqual(
      secondFrames.filter((f) => f.type === "message"),
      [row1Expected, row2Expected],
      "connect #2: same 2 frames replayed again, each exactly once",
    );

    // Two connects → exactly two bd queries, no more (not duplicate-within-
    // one-connect: dup delivery across connects comes from reconnects only).
    const calls = bdCalls(hub.dataDir);
    assert.equal(calls.length, 2, "exactly one bd query per connect (2 connects → 2 calls)");
    assert.deepEqual(calls[0], unreadQueryArgv(RP));
    assert.deepEqual(calls[1], unreadQueryArgv(RP));
    closeAll(second);
  } finally {
    hub.stop();
  }
});

// ── c. replay-after-read ────────────────────────────────────────────────────

test("replay-after-read: one row closed via mark_as_read → reconnect replays only the remaining row", async () => {
  const hub = await spawnHub();
  try {
    seedUnread(hub.dataDir, RP, [row1, row2]);

    const first = await connect(hub.port);
    const firstFrames = recordFrames(first);
    await authenticate(hub, first, "agent", RP);
    await hub.until(
      () => firstFrames.filter((f) => f.type === "message").length >= 2,
      "first-connect replay of both rows",
    );
    first.close();
    await hub.waitForEvent(
      (e) => e.event === "presence" && e.agent === RP && e.presence === "leave",
      "leave after disconnect",
    );

    // Simulate mark_as_read on ap-r1: `bd close ap-r1 --reason delivered`
    // flips its status to closed, so status=open no longer matches it.
    seedUnread(hub.dataDir, RP, [row2]);

    const second = await connect(hub.port);
    const secondFrames = recordFrames(second);
    await authenticate(hub, second, "agent", RP);
    await hub.until(
      () => secondFrames.filter((f) => f.type === "message").length >= 1,
      "second-connect replay",
    );
    await sleep(300);
    assert.deepEqual(
      secondFrames.filter((f) => f.type === "message"),
      [row2Expected],
      "only the still-unread row replays; the read row does not",
    );
    closeAll(second);
  } finally {
    hub.stop();
  }
});

// ── d. no-unread ────────────────────────────────────────────────────────────

test("no-unread: agent with no seed file → hello succeeds, zero message frames, join still fires", async () => {
  const hub = await spawnHub();
  try {
    const subscriber = await connect(hub.port);
    await authenticate(hub, subscriber, "subscriber");

    const agent = await connect(hub.port);
    const frames = recordFrames(agent);
    const joinPromise = waitFor(
      subscriber,
      (m) => m.type === "presence" && m.agent === "rp-empty" && m.event === "join",
      "presence join for rp-empty",
    );
    await authenticate(hub, agent, "agent", "rp-empty");
    await joinPromise;

    const replayEv = await hub.waitForEvent(
      (e) => e.event === "replay" && e.agent === "rp-empty",
      "replay log",
    );
    assert.equal(replayEv.count, 0, "replay ran with count 0 (stub returned [])");

    await sleep(300);
    assert.deepEqual(frames, [], "agent received zero frames");
    assert.equal(hub.proc.exitCode, null, "hub still alive");
    // The empty result still came from a real bd query.
    assert.deepEqual(bdCalls(hub.dataDir), [unreadQueryArgv("rp-empty")]);
    closeAll(subscriber, agent);
  } finally {
    hub.stop();
  }
});

// ── e. bd-failure ───────────────────────────────────────────────────────────

test("bd-failure: bd exits 1 → replay_error logged, connection survives, live notify still delivers", async () => {
  const hub = await spawnHub({ bdFail: true });
  try {
    const agent = await connect(hub.port);
    const frames = recordFrames(agent);
    await authenticate(hub, agent, "agent", RP);

    const errEv = await hub.waitForEvent(
      (e) => e.event === "replay_error" && e.agent === RP,
      "replay_error log",
    );
    // beads.ts runBd rejects with the child's stderr — the stub's message
    // surfaces in the hub log, proving the error path carries diagnostics.
    assert.match(String(errEv.error), /simulated bd failure/, "replay_error carries bd stderr");

    // Connection survives: zero frames so far, socket still open.
    await sleep(200);
    assert.deepEqual(frames, [], "no frames delivered on failed replay");
    assert.equal(agent.readyState, WebSocket.OPEN, "agent socket still open");

    // Live delivery still works on the surviving connection.
    const producer = await connect(hub.port);
    await authenticate(hub, producer, "producer");
    const delivery = waitFor(
      agent,
      (m) => m.type === "message" && m.id === "live-after-fail",
      "live delivery after replay failure",
    );
    producer.send(
      JSON.stringify({
        type: "notify",
        to: RP,
        id: "live-after-fail",
        from: "glados",
        preview: "still alive?",
      }),
    );
    const delivered = await delivery;
    assert.deepEqual(delivered, {
      type: "message",
      id: "live-after-fail",
      from: "glados",
      preview: "still alive?",
    });
    assert.equal(hub.proc.exitCode, null, "hub survived the bd failure");
    closeAll(agent, producer);
  } finally {
    hub.stop();
  }
});

// ── f. replay-vs-live ordering ──────────────────────────────────────────────

test("replay-vs-live: notify racing an in-flight replay → both frames arrive, no duplicates, no crash", async () => {
  const hub = await spawnHub();
  try {
    const RL = "rl-test";
    seedUnread(hub.dataDir, RL, [
      {
        id: "ap-rl1",
        title: `[glados->${RL}] queued while offline`,
        description: "queued while offline",
        status: "open",
        issue_type: "message",
      },
    ]);

    // Subscriber + producer connect and hello FIRST, so the notify below has
    // no connection-setup latency of its own.
    const subscriber = await connect(hub.port);
    await authenticate(hub, subscriber, "subscriber");
    const producer = await connect(hub.port);
    await authenticate(hub, producer, "producer");
    await hub.waitForEvent((e) => e.event === "hello" && e.role === "producer", "producer hello");

    const agent = await connect(hub.port);
    const frames = recordFrames(agent);
    const joinPromise = waitFor(
      subscriber,
      (m) => m.type === "presence" && m.agent === RL && m.event === "join",
      "join for rl-test",
    );
    await authenticate(hub, agent, "agent", RL);
    // The join broadcast happens synchronously inside handleHello BEFORE
    // replayUnread's bd child has spawned, so firing the notify the instant
    // the join is observed guarantees (1) the agent is mapped → the live
    // frame cannot fall into the notify_offline hole, and (2) the replay bd
    // query (a whole process spawn) is still in flight → genuine race.
    await joinPromise;
    producer.send(
      JSON.stringify({
        type: "notify",
        to: RL,
        id: "live-1",
        from: "glados",
        preview: "you're online",
      }),
    );

    await hub.until(
      () => frames.filter((f) => f.type === "message").length >= 2,
      "both replay and live frames",
    );
    await sleep(300);
    const msgs = frames.filter((f) => f.type === "message");
    assert.equal(msgs.length, 2, "exactly 2 message frames: 1 replayed + 1 live, no duplicates");
    assert.equal(
      msgs.filter((f) => f.id === "ap-rl1").length,
      1,
      "replayed frame arrived exactly once",
    );
    assert.equal(
      msgs.filter((f) => f.id === "live-1").length,
      1,
      "live frame arrived exactly once",
    );
    assert.equal(hub.proc.exitCode, null, "hub alive after replay/live race");

    // OBSERVED ORDERING (recorded, deliberately NOT asserted): the live
    // notify is handled on the WS message path while the replay is stalled
    // on a full `bd` process spawn+exit, so in every local run the live
    // frame ("live-1") arrived BEFORE the replayed frame ("ap-rl1"):
    // live-then-replay. Nothing in the hub orders these two paths — a
    // consumer must treat replay/live interleaving as unordered and dedupe
    // by message id (BEADS row id) if it matters.
    closeAll(subscriber, producer, agent);
  } finally {
    hub.stop();
  }
});
