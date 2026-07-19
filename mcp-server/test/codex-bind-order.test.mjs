/**
 * codex-bind-order.test.mjs — bind-order pins for CodexBridgeClient
 * (bead aperture-xt16e, failure mode #7: injection-before-bind).
 *
 * Drives dist/codex-bridge.js against a scripted fake app-server
 * (test/fixtures/fake-appserver.mjs) speaking WS JSON-RPC over a unix socket.
 *
 * BEADS isolation: deliverUnread() shells out to `bd` via beads.ts. We do NOT
 * use the skipReplay hook here because it gates deliverUnread entirely (first
 * line of deliverUnread returns when hooks.skipReplay is set) — these tests
 * exist to exercise delivery. Instead BD_PATH (env, read at beads.js module
 * load) points at a stub shell script written into this test's tmp dir that
 * cats $FAKE_BD_UNREAD_FILE — a JSON file each test rewrites to control the
 * unread set. No real bd, no shared fixtures with other test files.
 *
 * OBSERVED PRE-BIND DELIVERY SEMANTICS (current code, incl. PR #34):
 *   - deliverUnread() early-returns while this.threadId === null, so a hub
 *     notify arriving before bind is a silent no-op at the transport level.
 *   - thread/list bind path: bindThread() calls this.deliver() right after
 *     thread/resume + bindToThread — the pre-bind message is RE-FETCHED from
 *     BEADS and injected exactly once, after resume. Not lost. (test: pin b)
 *   - thread/start bootstrap path (fresh session): bindThread() refetches
 *     unread after it owns the new thread, then steers those rows into the
 *     active kickoff turn. (test: fresh-session symmetry pin below)
 *
 * Run: cd mcp-server && pnpm build && node --test test/codex-bind-order.test.mjs
 */
import test from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, writeFileSync, readFileSync, existsSync, statSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, dirname, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

// ── tmp tree (short paths: macOS sun_path is 104 bytes) ──
let TMP = mkdtempSync(join(tmpdir(), "cbx-"));
if (TMP.length > 80) {
  // fallback: /tmp keeps socket paths well under sun_path
  TMP = mkdtempSync("/tmp/cbx-");
}
const UNREAD_FILE = join(TMP, "unread.json");
const BD_STUB = join(TMP, "bd-stub");
writeFileSync(UNREAD_FILE, "[]\n");
writeFileSync(BD_STUB, `#!/bin/sh\ncat "$FAKE_BD_UNREAD_FILE"\n`, { mode: 0o755 });

// Module-load-time env for dist/codex-bridge.js + dist/beads.js — MUST be set
// before the dynamic import below.
process.env.APERTURE_RUN_DIR = TMP; // thread-ready files land here
process.env.BD_PATH = BD_STUB; // beads.ts shells this instead of real bd
process.env.FAKE_BD_UNREAD_FILE = UNREAD_FILE;

const here = dirname(fileURLToPath(import.meta.url));
const { CodexBridgeClient, CODEX_KICKOFF_TEXT } = await import(
  pathToFileURL(resolve(here, "..", "dist", "codex-bridge.js")).href
);
const { FakeAppServer } = await import(pathToFileURL(join(here, "fixtures", "fake-appserver.mjs")).href);

// ── helpers ──

function setUnread(rows) {
  writeFileSync(UNREAD_FILE, JSON.stringify(rows) + "\n");
}

function msgRow(id, from, to, body) {
  return { id, title: `[${from}->${to}] ${body.slice(0, 60)}`, description: body };
}

async function waitFor(cond, what, timeoutMs = 5000, stepMs = 20) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (cond()) return;
    await new Promise((r) => setTimeout(r, stepMs));
  }
  assert.fail(`timed out after ${timeoutMs}ms waiting for: ${what}`);
}

function delay(ms) {
  return new Promise((r) => setTimeout(r, ms));
}

function makeHooks() {
  const logs = [];
  const presence = [];
  return {
    logs,
    presence,
    hooks: {
      broadcastPresence: (_agent, event) => presence.push(event),
      log: (event, fields = {}) => logs.push({ event, ...fields }),
      // NB: no skipReplay — delivery is the subject under test.
    },
  };
}

let sockCounter = 0;
async function scenario(t, { threads = [], delays = {}, failures = {} } = {}) {
  const agent = `cbx${++sockCounter}`;
  const sock = join(TMP, `${agent}.sock`);
  assert.ok(sock.length < 100, `socket path too long for sun_path: ${sock}`);
  const server = new FakeAppServer(sock, { threads, delays, failures });
  await server.start();
  const { hooks, logs, presence } = makeHooks();
  const bridge = new CodexBridgeClient(agent, sock, hooks);
  t.after(async () => {
    bridge.stop();
    await server.close();
    setUnread([]);
  });
  return { agent, server, bridge, logs, presence };
}

// ── pins ──

test("happy-bind: existing thread → initialize < thread/list < thread/resume, presence join, injection works after", async (t) => {
  const { server, bridge, presence } = await scenario(t, { threads: [{ id: "t-exist" }] });

  bridge.start();
  await waitFor(() => bridge.isBound, "bridge bound");

  const iInit = server.indexOf("initialize");
  const iList = server.indexOf("thread/list");
  const iResume = server.indexOf("thread/resume");
  assert.ok(iInit >= 0, "initialize was called");
  assert.ok(iList > iInit, `thread/list (${iList}) after initialize (${iInit})`);
  assert.ok(iResume > iList, `thread/resume (${iResume}) after thread/list (${iList})`);
  assert.equal(
    server.calls[iResume].params?.threadId,
    "t-exist",
    "resumed the listed thread",
  );
  assert.equal(bridge.boundThreadId, "t-exist");
  assert.ok(presence.includes("join"), "presence join broadcast on bind");

  // Post-bind injection path: a notify (hub → bridge.deliver()) injects the
  // full BEADS body via turn/start on the bound thread.
  setUnread([msgRow("msg-a1", "glados", "cbx-a", "hello from the happy path")]);
  bridge.deliver();
  await waitFor(() => server.turnCallsContaining("msg-a1").length > 0, "msg-a1 injected");
  const inj = server.turnCallsContaining("msg-a1");
  assert.equal(inj.length, 1);
  assert.equal(inj[0].method, "turn/start", "idle thread → turn/start");
  assert.equal(inj[0].params.threadId, "t-exist");
  assert.match(
    inj[0].params.input[0].text,
    /hello from the happy path/,
    "full body injected, not just preview",
  );
});

test("HEADLINE injection-before-bind (existing thread, thread/list delayed): message is NOT lost — refetched on bind, injected exactly once, after thread/resume", async (t) => {
  const { server, bridge } = await scenario(t, {
    threads: [{ id: "t-b" }],
    delays: { "thread/list": 400 },
  });

  // The message is already unread in BEADS and the hub notify fires while
  // joined === false. deliverUnread() early-returns (threadId null) both
  // times — the push itself is a no-op pre-bind.
  setUnread([msgRow("m-b1", "glados", "cbx-b", "pre-bind message body")]);
  bridge.deliver(); // notify before the socket even connects
  bridge.start();
  await bridge.waitReady(5000);
  assert.equal(bridge.isBound, false, "not bound yet (thread/list held back)");
  bridge.deliver(); // notify mid-bind: initialized, thread/list still in flight

  await waitFor(() => bridge.isBound, "bridge bound after delayed thread/list");
  await waitFor(() => server.turnCallsContaining("m-b1").length > 0, "m-b1 injected post-bind");
  // Grace window: a buggy double-delivery would land within the same chain.
  await delay(300);

  const inj = server.turnCallsContaining("m-b1");
  assert.equal(inj.length, 1, "pre-bind message injected exactly once");
  assert.equal(inj[0].method, "turn/start");

  // The call log proves ordering: injection strictly after thread/resume.
  const iResume = server.indexOf("thread/resume");
  const iInject = server.calls.indexOf(inj[0]);
  assert.ok(iResume >= 0, "thread/resume happened");
  assert.ok(
    iInject > iResume,
    `turn/start (${iInject}) arrives after thread/resume (${iResume})`,
  );
  // And nothing was injected before bind completed.
  const preResumeTurns = server.calls
    .slice(0, iResume)
    .filter((c) => c.method.startsWith("turn/"));
  assert.equal(preResumeTurns.length, 0, "no turn injection before thread/resume");
});

test("injection-before-bind (fresh session, thread/start bootstrap): pre-bind message is refetched and steered at bind", async (t) => {
  const { server, bridge, logs } = await scenario(t, { threads: [] });

  setUnread([msgRow("m-f1", "glados", "cbx-f", "message racing a fresh session")]);
  bridge.deliver(); // hub notify while joined === false, no thread exists
  bridge.start();

  await waitFor(() => bridge.isBound, "bridge bound via thread/start bootstrap");
  await waitFor(
    () => server.callsOf("turn/start").length > 0,
    "kickoff turn injected",
  );
  await waitFor(() => server.turnCallsContaining("m-f1").length > 0, "m-f1 injected on fresh bind");
  await delay(300); // grace: a buggy double-delivery would land in the same chain

  // Kickoff went in, explicitly owned via thread/start (not thread/list).
  assert.ok(
    logs.some((l) => l.event === "codex_bound" && l.source === "thread_start"),
    "bound via thread_start ownership",
  );
  const kickoffs = server.turnCallsContaining(CODEX_KICKOFF_TEXT.slice(0, 40));
  assert.equal(kickoffs.length, 1, "exactly one kickoff turn");

  const inj = server.turnCallsContaining("m-f1");
  assert.equal(inj.length, 1, "pre-bind unread injected exactly once on fresh bind");
  assert.equal(inj[0].method, "turn/steer", "kickoff turn active → steer delivery");
});

test("no-thread-at-connect: thread/start failure retried with backoff, no crash, bind completes + thread-ready file published", async (t) => {
  const { agent, server, bridge, logs } = await scenario(t, {
    threads: [],
    failures: { "thread/start": 1 },
  });

  bridge.start();
  // First thread/start errors → codex_kickoff_retry (500ms backoff) → succeeds.
  await waitFor(() => bridge.isBound, "bound after kickoff retry", 8000);

  assert.ok(
    logs.some((l) => l.event === "codex_kickoff_retry"),
    "scripted thread/start failure logged as codex_kickoff_retry",
  );
  assert.ok(
    logs.some((l) => l.event === "codex_kickoff_injected"),
    "kickoff injected after retry",
  );
  assert.equal(server.callsOf("thread/start").length, 2, "thread/start retried exactly once");

  // NOTE (current behavior, PR #34): the codex_no_thread_yet 10s poll branch
  // is unreachable on a fresh session — an empty thread/list now always takes
  // the thread/start bootstrap (with its own 500ms→10s backoff) instead of
  // logging codex_no_thread_yet. Pinned by absence:
  assert.ok(
    !logs.some((l) => l.event === "codex_no_thread_yet"),
    "fresh session takes bootstrap path, not the no_thread_yet poll",
  );

  // Explicit ownership: thread-ready file for the launcher's `codex resume`.
  const readyPath = join(TMP, `${agent}.thread-id`);
  await waitFor(() => existsSync(readyPath), "thread-ready file written");
  assert.equal(readFileSync(readyPath, "utf8").trim(), bridge.boundThreadId);
  assert.equal(statSync(readyPath).mode & 0o777, 0o600, "thread-ready file is mode 0600");
});

test("double-injection guard: same message id notified twice → exactly one turn injection", async (t) => {
  const { server, bridge } = await scenario(t, { threads: [{ id: "t-d" }] });

  bridge.start();
  await waitFor(() => bridge.isBound, "bridge bound");

  // BEADS still reports the row unread on both fetches (agent hasn't acked) —
  // the in-memory delivered-set must dedupe the second pump.
  setUnread([msgRow("m-d1", "glados", "cbx-d", "replay-overlap message")]);
  bridge.deliver();
  bridge.deliver();
  await waitFor(() => server.turnCallsContaining("m-d1").length > 0, "m-d1 injected");
  await delay(300);
  // Third notify after the first injection fully settled — still deduped.
  bridge.deliver();
  await delay(300);

  assert.equal(
    server.turnCallsContaining("m-d1").length,
    1,
    "delivered-set suppresses re-injection of an already-injected id",
  );
});

test("turn-state serialization: message during an active turn → turn/steer, not a second turn/start", async (t) => {
  const { server, bridge } = await scenario(t, { threads: [{ id: "t-e" }] });

  bridge.start();
  await waitFor(() => bridge.isBound, "bridge bound");

  server.notify("turn/started", { threadId: "t-e" });
  await waitFor(() => bridge.isTurnActive, "turn marked active");

  setUnread([msgRow("m-e1", "glados", "cbx-e", "mid-turn message")]);
  bridge.deliver();
  await waitFor(() => server.turnCallsContaining("m-e1").length > 0, "m-e1 injected");

  const inj = server.turnCallsContaining("m-e1");
  assert.equal(inj.length, 1);
  assert.equal(inj[0].method, "turn/steer", "active turn → steer injection");
  assert.equal(
    inj.filter((c) => c.method === "turn/start").length,
    0,
    "no competing turn/start while a turn is active",
  );

  // Completion flips back to idle → subsequent delivery uses turn/start again.
  server.notify("turn/completed", { threadId: "t-e" });
  await waitFor(() => !bridge.isTurnActive, "turn idle after completion");
  setUnread([
    msgRow("m-e1", "glados", "cbx-e", "mid-turn message"),
    msgRow("m-e2", "glados", "cbx-e", "post-turn message"),
  ]);
  bridge.deliver();
  await waitFor(() => server.turnCallsContaining("m-e2").length > 0, "m-e2 injected");
  const inj2 = server.turnCallsContaining("m-e2");
  assert.equal(inj2.length, 1);
  assert.equal(inj2[0].method, "turn/start", "idle again → turn/start");
  assert.equal(server.turnCallsContaining("m-e1").length, 1, "m-e1 still deduped");
});

test.after(() => {
  try {
    rmSync(TMP, { recursive: true, force: true });
  } catch {
    /* tmp reaper handles it */
  }
});
