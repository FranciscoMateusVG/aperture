// aperture-xt16e — replay-on-reconnect pins for the comms-v2 WS hub
// (dist/ws-hub.js), failure mode #3: replay-exactly-once after offline.
//
// Unlike hub-protocol.test.mjs (which sets APERTURE_HUB_SKIP_REPLAY=1), this
// suite runs the REAL replay path: on agent hello, replayUnread() →
// beads.ts getUnreadMessages() shells out to
//
//   bd list --type message --status open --title-contains '->AGENT]' --include-infra
//     --sort id --reverse --json -n 0
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
//                                stderr `replay` count=2, exactly ONE bd list
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
import { createHash } from "node:crypto";
import { ManagedInboxReminder } from "../dist/managed-inbox-reminder.js";
import { spawn } from "node:child_process";
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
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
  wheatley: "16".repeat(32),
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
  const teamsDir = mkdtempSync(join(tmpdir(), "hub-replay-teams-"));
  const dataDir = mkdtempSync(join(tmpdir(), "hub-replay-bdstub-"));
  const homeDir = mkdtempSync(join(tmpdir(), "hub-replay-home-"));
  chmodSync(homeDir, 0o700);
  // aperture-oeb6q: the hub now writes presence.json under APERTURE_RUN_DIR on
  // boot and on every presence change — isolate it so a test hub never
  // clobbers the developer's real ~/.aperture/run/presence.json.
  const runDir = join(homeDir, ".aperture", "run");
  mkdirSync(runDir, { recursive: true, mode: 0o700 });
  chmodSync(join(homeDir, ".aperture"), 0o700);
  chmodSync(runDir, 0o700);
  const tokenDir = join(runDir, "hub-tokens");
  mkdirSync(tokenDir, { mode: 0o700 });
  for (const [principal, token] of Object.entries(TOKENS)) {
    writeFileSync(join(tokenDir, `${principal}.token`), token, { mode: 0o600 });
    if (principal !== "watchdog") {
      mkdirSync(join(emptyAgentsDir, principal), { recursive: true });
      writeFileSync(
        join(emptyAgentsDir, principal, "manifest.json"),
        JSON.stringify({ name: principal, model: "claude/test", window: principal, role: "test", enabled: true }),
      );
      writeFileSync(join(emptyAgentsDir, principal, "prompt.md"), "fixture");
    }
  }

  const env = {
    ...process.env,
    HOME: homeDir,
    APERTURE_WS_PORT: String(port),
    APERTURE_AGENTS_DIR: emptyAgentsDir,
    APERTURE_TEAMS_DIR: teamsDir,
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
    rmSync(teamsDir, { recursive: true, force: true });
    rmSync(dataDir, { recursive: true, force: true });
    rmSync(homeDir, { recursive: true, force: true });
  }

  await waitForEvent((e) => e.event === "listening", "listening", 5000);
  return { port, proc, dataDir, homeDir, runDir, tokenDir, teamsDir, agentsDir: emptyAgentsDir, stderrEvents, waitForEvent, until, stop };
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
  "list",
  "--type",
  "message",
  "--status",
  "open",
  "--title-contains",
  `->${agent}]`,
  "--include-infra",
  "--sort",
  "id",
  "--reverse",
  "--json",
  "-n",
  "0", // complete scan; beads.ts applies the 200 delivery cap after policy
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
    assert.deepEqual(msgs[0], row2Expected, "newest/tie-break row: 60-char preview, newline → space");
    assert.deepEqual(msgs[1], row1Expected, "older/tie-break row: id/from/preview match");
    assert.equal(msgs[0].preview.length, 60, "preview truncated to exactly 60 chars");

    // Evidence: exactly ONE bd invocation for this connect, exact argv shape.
    const calls = bdCalls(hub.dataDir);
    assert.equal(calls.length, 1, "exactly one bd call for the connect");
    assert.deepEqual(calls[0], unreadQueryArgv(RP), "bd argv matches getUnreadMessages shape");
    closeAll(agent);
  } finally {
    hub.stop();
  }
});

test("authorization loss withholds replay durably while preserving the original open message and id", async () => {
  const hub = await spawnHub();
  try {
    const original = {
      id: "ap-withheld-1",
      title: `[glados->${RP}] reassign/cancel instruction`,
      description: "reassign/cancel leaves the target bead byte-identical",
      status: "open",
      issue_type: "message",
      labels: [],
    };
    seedUnread(hub.dataDir, RP, [original]);
    const seedPath = join(hub.dataDir, `unread-${RP}.json`);
    const before = readFileSync(seedPath);
    writeFileSync(
      join(hub.agentsDir, "glados", "manifest.json"),
      JSON.stringify({ name: "glados", model: "claude/test", window: "glados", role: "test", enabled: false }),
    );

    const agent = await connect(hub.port);
    const frames = recordFrames(agent);
    await authenticate(hub, agent, "agent", RP);
    const replayEv = await hub.waitForEvent(
      (e) => e.event === "replay" && e.agent === RP,
      "withheld replay log",
    );
    assert.equal(replayEv.count, 0);
    await sleep(200);
    assert.deepEqual(frames.filter((f) => f.type === "message"), [], "body is not delivered after authorization loss");
    assert.deepEqual(bdCalls(hub.dataDir), [
      unreadQueryArgv(RP),
      ["update", original.id, "--add-label", "withheld:unknown_sender", "--json"],
    ]);
    assert.deepEqual(readFileSync(seedPath), before, "the source row stays byte-identical and open; no reassign/cancel side effect");
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
      [row2Expected, row1Expected],
      "connect #1: each frame exactly once, in deterministic newest/id order",
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
      [row2Expected, row1Expected],
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

// The coalescer has no IO/authority. Clock and callbacks are the complete seam.
function fakeReminderClock(random = 0.5) {
  let seq = 0;
  const tasks = new Map(), delays = [];
  return {
    tasks, delays,
    set(fn, ms) { const id = ++seq; tasks.set(id, fn); delays.push(ms); return id; },
    clear(id) { tasks.delete(id); }, random: () => random,
    fire() { assert.equal(tasks.size, 1); const [id, fn] = tasks.entries().next().value; tasks.delete(id); fn(); return fn; },
  };
}
const flushReminder = async () => { await Promise.resolve(); await Promise.resolve(); await Promise.resolve(); };
const deferred = () => { let resolve, reject; const promise = new Promise((a,b) => {resolve=a;reject=b;}); return {promise,resolve,reject}; };
function reminderHarness(clock = fakeReminderClock()) {
  let valid = true, rows = [{ id: "ap-pending", from: "glados" }], queries = 0, sends = [], errors = 0;
  const r = new ManagedInboxReminder({ valid: () => valid,
    unread: async () => {queries++;return await rows;}, send: x => sends.push(x), error: () => errors++ }, clock);
  return {r,clock,sends, setRows:x=>{rows=x;}, invalidate:()=>{valid=false;}, counts:()=>({queries,errors})};
}
test("managed reminder stays singleflight, uses bounded backoff, never acknowledges on send", async () => {
  const h = reminderHarness(); h.r.pending(); h.r.pending(); assert.equal(h.clock.tasks.size,0);
  h.r.start(); h.r.start(); assert.equal(h.clock.tasks.size,1);
  for (let i=0;i<6;i++) {h.clock.fire();await flushReminder();}
  assert.deepEqual(h.clock.delays,[30_000,60_000,120_000,300_000,300_000,300_000,300_000]);
  assert.equal(h.sends.length,6); assert.equal(h.counts().queries,6);
  h.setRows([]); h.clock.fire(); await flushReminder();
  assert.equal(h.clock.tasks.size,0); // only authoritative read/empty stops reminders
  h.r.pending(); assert.equal(h.clock.delays.at(-1),30_000); h.r.cancel();
  for (const rand of [0,1]) {const j=reminderHarness(fakeReminderClock(rand));j.r.pending();j.r.start();for(let i=0;i<8;i++){j.clock.fire();await flushReminder();}assert.ok(j.clock.delays.every(n=>n>=27000&&n<=300000));j.r.cancel();}
});
test("managed reminder coalesces notify during pending query; old empty cannot cancel new notify", async () => {
  const h=reminderHarness(), d=deferred();h.setRows(d.promise);h.r.pending();h.r.start();h.clock.fire();
  for(let i=0;i<20;i++) h.r.pending();
  assert.equal(h.counts().queries,1); assert.equal(h.clock.tasks.size,0);
  d.resolve([]);await flushReminder();assert.equal(h.clock.tasks.size,1);
  h.setRows([{id:"ap-new",from:"glados"}]);h.clock.fire();await flushReminder();
  assert.equal(h.sends[0][0].id,"ap-new");h.r.cancel();
});
test("managed reminder cancellation fences queued callback and in-flight query; auth drift denies send", async () => {
  for(const mode of ["queued","query","auth-drift"]) {
    const h=reminderHarness(),d=deferred();h.setRows(d.promise);h.r.pending();h.r.start();
    const fn=h.clock.tasks.values().next().value;
    if(mode==="queued"){h.r.cancel();fn();assert.equal(h.counts().queries,0);}
    else {h.clock.fire();if(mode==="query")h.r.cancel();else h.invalidate();d.resolve([{id:"ap-pending",from:"glados"}]);await flushReminder();}
    assert.equal(h.sends.length,0);assert.equal(h.clock.tasks.size,0);
    h.r.pending();h.r.start();assert.equal(h.clock.tasks.size,0);
  }
});
test("managed reminder query failure is not empty or ACK; same pending ID survives provider outage", async () => {
  const h=reminderHarness(),d=deferred();h.setRows(d.promise);h.r.pending();h.r.start();h.clock.fire();d.reject(new Error("fixture"));await flushReminder();
  assert.equal(h.counts().errors,1);assert.equal(h.clock.tasks.size,1);assert.equal(h.sends.length,0);
  h.setRows([{id:"ap-pending",from:"glados"}]);h.clock.fire();await flushReminder();
  assert.equal(h.sends[0][0].id,"ap-pending");h.r.cancel();
});
function managedFixture(hub, seat, harness="claude", observed=true) {
  const team="reminder",token="aa".repeat(32),tokenId=createHash("sha256").update(token).digest("hex");
  const dir=join(hub.agentsDir,seat);mkdirSync(dir,{recursive:true});
  for(const file of ["TEAM",".complete","prompt.md"])writeFileSync(join(dir,file),file==="TEAM"?team:"fixture");
  writeFileSync(join(dir,"manifest.json"),JSON.stringify({name:seat,model:`${harness}/test`,window:seat,role:"qa",enabled:true}));
  mkdirSync(join(hub.teamsDir,team),{recursive:true});
  const seats=["reminder-claude","reminder-codex","reminder-unobserved"].map(name=>({name,role:"qa"}));
  writeFileSync(join(hub.teamsDir,team,"state.json"),JSON.stringify({state:"active",generation:1}));
  writeFileSync(join(hub.teamsDir,team,"team.json"),JSON.stringify({team,project:"project:aperture",lead:"reminder-claude",seats}));
  const ownerDir=join(hub.runDir,"owner");mkdirSync(ownerDir,{recursive:true,mode:0o700});
  const tuple={harness,model:harness==="claude"?"claude-sonnet-5":"gpt-6-astra",reasoning:harness==="claude"?null:"high"};
  const owner={schema_version:1,seat,generation:1,state:"active",provisional_token_id:null,requested:tuple,
    incarnation:{...tuple,pid:123,start_time:456,thread_id:"fixture-thread",token_id:tokenId,observed}};
  const ownerPath=join(ownerDir,`${seat}.json`);writeFileSync(ownerPath,JSON.stringify(owner),{mode:0o600});
  writeFileSync(join(hub.tokenDir,`${seat}.token`),token,{mode:0o600});
  return {owner,ownerPath,hello:(ws,role="agent")=>ws.send(JSON.stringify({type:"hello",role,agent:seat,token,generation:1,token_id:tokenId}))};
}
test("real hub wiring arms only owner-proven managed Claude; reminder has IDs but no mission body", async () => {
  const hub=await spawnHub(), sockets=[];
  try {
    const c=managedFixture(hub,"reminder-claude"), x=managedFixture(hub,"reminder-codex","codex"), u=managedFixture(hub,"reminder-unobserved","claude",false);
    const rows=[{id:"ap-reminder",title:"[glados->reminder-claude] fixture",issue_type:"message",status:"open",description:"PRIVATE_MISSION_SENTINEL"}];
    seedUnread(hub.dataDir,"reminder-claude",rows);
    for(const [fixture,role] of [[c,"producer"],[c,"subscriber"],[x,"agent"],[u,"agent"]]) {
      const ws=await connect(hub.port);sockets.push(ws);fixture.hello(ws,role);
    }
    const legacy=await connect(hub.port);sockets.push(legacy);hello(legacy,"agent","rp-empty");
    const agent=await connect(hub.port);sockets.push(agent);const frames=[];agent.on("message",m=>frames.push(JSON.parse(m)));c.hello(agent);
    await hub.waitForEvent(e=>e.event==="managed_inbox_reminder_armed","managed Claude armed");
    await hub.waitForEvent(e=>e.event==="replay"&&e.agent==="reminder-claude","managed replay");
    await hub.until(()=>frames.some(f=>f.reminder),"native timer reminder (no fake hub clock)",35000);
    const reminder=frames.find(f=>f.reminder);assert.deepEqual(reminder.pending_ids,["ap-reminder"]);
    assert.equal(JSON.stringify(reminder).includes("PRIVATE_MISSION_SENTINEL"),false);
    assert.deepEqual(hub.stderrEvents.filter(e=>e.event==="managed_inbox_reminder_armed").map(e=>e.agent),["reminder-claude"]);
    assert.equal(bdCalls(hub.dataDir).filter(a=>a[0]==="close").length,0,"reminder is never ACK");
    // Owner thread drift invalidates current binding, even with same token/seat.
    c.owner.incarnation.thread_id="other-thread";writeFileSync(c.ownerPath,JSON.stringify(c.owner),{mode:0o600});
    const producer=await connect(hub.port);sockets.push(producer);hello(producer,"producer","glados");
    producer.send(JSON.stringify({type:"notify",to:"reminder-claude",id:"ap-new",preview:"fixture"}));
    await hub.waitForEvent(e=>e.event==="notify_forwarded"&&e.id==="ap-new","new notify after drift");
    await hub.waitForEvent(e=>e.event==="managed_inbox_reminder_cancelled"&&e.agent==="reminder-claude","native binding drift cancels");
    // Fake-clock oracles above additionally cover cancellation during await;
    // this real hub oracle proves native owner classification and recheck.
  } finally {closeAll(...sockets);hub.stop();}
});
