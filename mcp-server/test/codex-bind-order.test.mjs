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
import { chmodSync, mkdirSync, mkdtempSync, writeFileSync, readFileSync, existsSync, statSync, rmSync } from "node:fs";
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
const AGENTS = join(TMP, "agents");
const TEAMS = join(TMP, "teams");
mkdirSync(AGENTS);
mkdirSync(TEAMS);
mkdirSync(join(AGENTS, "glados"));
writeFileSync(
  join(AGENTS, "glados", "manifest.json"),
  JSON.stringify({ name: "glados", model: "claude/test", window: "glados", role: "orchestrator", enabled: true }),
);
writeFileSync(join(AGENTS, "glados", "prompt.md"), "fixture");
writeFileSync(UNREAD_FILE, "[]\n");
writeFileSync(BD_STUB, `#!/bin/sh\ncat "$FAKE_BD_UNREAD_FILE"\n`, { mode: 0o755 });

// Module-load-time env for dist/codex-bridge.js + dist/beads.js — MUST be set
// before the dynamic import below.
process.env.APERTURE_RUN_DIR = TMP; // thread-ready files land here
process.env.HOME = TMP;
process.env.APERTURE_AGENTS_DIR = AGENTS;
process.env.APERTURE_TEAMS_DIR = TEAMS;
process.env.BD_PATH = BD_STUB; // beads.ts shells this instead of real bd
process.env.FAKE_BD_UNREAD_FILE = UNREAD_FILE;
// aperture-oeb6q pins: shrink the bridge's wall-clock cadences so socket-death
// reconnect and the single inject re-pump happen in ~100ms instead of 10s/5s.
process.env.APERTURE_CODEX_RECONNECT_MS = "100";
process.env.APERTURE_CODEX_INJECT_RETRY_MS = "100";
const INJECT_RETRY_MS = 100;

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
  return {
    id,
    title: `[${from}->${to}] ${body.slice(0, 60)}`,
    description: body,
    status: "open",
    issue_type: "message",
    ephemeral: true,
  };
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

/** Simulate app-server death mid-turn: kill every client connection but keep
 *  listening, so the bridge's reconnect finds the same server (same threads). */
function dropClients(server) {
  for (const ws of server.sockets) ws.terminate();
}

function makeHooks(beforeManagedOwnerReadback) {
  const logs = [];
  const presence = [];
  return {
    logs,
    presence,
    hooks: {
      broadcastPresence: (_agent, event) => presence.push(event),
      log: (event, fields = {}) => logs.push({ event, ...fields }),
      beforeManagedOwnerReadback,
      // NB: no skipReplay — delivery is the subject under test.
    },
  };
}

let sockCounter = 0;
async function scenario(t, {
  threads = [],
  delays = {},
  failures = {},
  threadStartModel,
  threadStartReasoning,
  mcpStatus, mcpProbe,
  beforeManagedOwnerReadback,
} = {}) {
  const agent = `cbx${++sockCounter}`;
  mkdirSync(join(AGENTS, agent));
  writeFileSync(
    join(AGENTS, agent, "manifest.json"),
    JSON.stringify({ name: agent, model: "codex/gpt-test", window: agent, role: "test", enabled: true }),
  );
  writeFileSync(join(AGENTS, agent, "prompt.md"), "fixture");
  const sock = join(TMP, `${agent}.sock`);
  assert.ok(sock.length < 100, `socket path too long for sun_path: ${sock}`);
  const server = new FakeAppServer(sock, { threads, delays, failures, threadStartModel, threadStartReasoning, mcpStatus, mcpProbe });
  await server.start();
  const { hooks, logs, presence } = makeHooks(beforeManagedOwnerReadback);
  const bridge = new CodexBridgeClient(agent, sock, hooks);
  t.after(async () => {
    bridge.stop();
    await server.close();
    setUnread([]);
  });
  return { agent, server, bridge, logs, presence };
}

function writeManagedOwner(agent, state, receipt = null) {
  const ownerDir = join(TMP, "owner");
  mkdirSync(ownerDir, { recursive: true, mode: 0o700 });
  chmodSync(ownerDir, 0o700);
  const tokenId = "a".repeat(64);
  writeFileSync(join(ownerDir, `${agent}.json`), JSON.stringify({
    schema_version: 1,
    seat: agent,
    generation: 1,
    state,
    reservation_nonce_sha256: state === "starting" ? "b".repeat(64) : null,
    provisional_token_id: state === "starting" ? tokenId : null,
    requested: { harness: "codex", model: "gpt-6-astra", reasoning: "high" },
    incarnation: {
      pid: 321,
      start_time: 1_790_000_000_000_001,
      thread_id: receipt?.thread_id ?? "",
      token_id: tokenId,
      harness: "codex",
      model: "gpt-6-astra",
      reasoning: "high",
      observed: state === "active",
      processes: [{ pid: 321, start_time: 1_790_000_000_000_001, ppid: 1, pgid: 321, cmdline_sha256: "c".repeat(64), cwd: "/tmp/managed-worktree" }],
    },
    since: "2026-09-20T00:00:00Z",
    writer: "launcher",
  }), { mode: 0o600 });
}

function mutateManagedOwner(agent, mutate) {
  const path = join(TMP, "owner", `${agent}.json`);
  const owner = JSON.parse(readFileSync(path, "utf8"));
  mutate(owner);
  writeFileSync(path, `${JSON.stringify(owner)}\n`, { mode: 0o600 });
}

function markManagedOwnerObserved(agent, receipt) {
  mutateManagedOwner(agent, (owner) => {
    owner.incarnation.thread_id = receipt.thread_id;
    owner.incarnation.model = receipt.actual_model;
    owner.incarnation.reasoning = receipt.actual_reasoning;
    owner.incarnation.observed = true;
  });
}

function makeManagedSeat(agent) {
  writeFileSync(join(AGENTS, agent, "TEAM"), "", { mode: 0o600 });
  writeFileSync(join(AGENTS, agent, ".complete"), "", { mode: 0o600 });
  const teamDir = join(TEAMS, `${agent}-team`);
  mkdirSync(teamDir, { mode: 0o700 });
  writeFileSync(join(teamDir, "state.json"), JSON.stringify({ state: "active", generation: 1 }), { mode: 0o600 });
  writeFileSync(join(teamDir, "team.json"), JSON.stringify({
    team: `${agent}-team`,
    project: "project:aperture",
    lead: agent,
    seats: [{ name: agent, role: "backend" }],
    grants: [],
  }), { mode: 0o600 });
}

// ── pins ──

test("happy-bind: existing thread → initialize < thread/list < thread/resume, presence join, injection works after", async (t) => {
  const { agent, server, bridge, presence } = await scenario(t, { threads: [{ id: "t-exist" }] });

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
  setUnread([msgRow("msg-a1", "glados", agent, "hello from the happy path")]);
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

test("Codex deliverUnread rechecks current registry and does not inject after recipient authorization loss", async (t) => {
  const { agent, server, bridge, logs } = await scenario(t, { threads: [{ id: "t-auth-loss" }] });
  bridge.start();
  await waitFor(() => bridge.isBound, "bridge bound before authorization loss");

  writeFileSync(
    join(AGENTS, agent, "manifest.json"),
    JSON.stringify({ name: agent, model: "codex/gpt-test", window: agent, role: "test", enabled: false }),
  );
  setUnread([msgRow("m-withheld", "glados", agent, "body must never enter the Codex thread")]);
  bridge.deliver();
  await delay(300);

  assert.equal(server.turnCallsContaining("m-withheld").length, 0);
  assert.equal(
    server.calls.some((call) => JSON.stringify(call.params).includes("body must never enter")),
    false,
  );
  assert.equal(logs.some((entry) => entry.event === "codex_inject" && entry.ids?.includes("m-withheld")), false);
});

test("HEADLINE injection-before-bind (existing thread, thread/list delayed): message is NOT lost — refetched on bind, injected exactly once, after thread/resume", async (t) => {
  const { agent, server, bridge } = await scenario(t, {
    threads: [{ id: "t-b" }],
    delays: { "thread/list": 400 },
  });

  // The message is already unread in BEADS and the hub notify fires while
  // joined === false. deliverUnread() early-returns (threadId null) both
  // times — the push itself is a no-op pre-bind.
  setUnread([msgRow("m-b1", "glados", agent, "pre-bind message body")]);
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
  const { agent, server, bridge, logs } = await scenario(t, { threads: [] });

  setUnread([msgRow("m-f1", "glados", agent, "message racing a fresh session")]);
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

test("managed Starting generation owns one exact fresh thread and stays silent until Active", async (t) => {
  const { agent, server, bridge, logs, presence } = await scenario(t, { threads: [{ id: "old-thread" }] });
  makeManagedSeat(agent);
  writeManagedOwner(agent, "starting");
  bridge.start();

  const receiptPath = join(TMP, `${agent}.g1.managed-observation.json`);
  await waitFor(() => existsSync(receiptPath), "managed observation receipt");
  const receipt = JSON.parse(readFileSync(receiptPath, "utf8"));
  assert.equal(server.callsOf("thread/list").length, 0, "managed generation never inherits newest thread");
  assert.equal(server.callsOf("thread/start").length, 1, "one exact fresh thread/start");
  assert.deepEqual(server.callsOf("thread/start")[0].params, {
    model: "gpt-6-astra",
    allowProviderModelFallback: false,
    config: { model_reasoning_effort: "high" },
  });
  assert.equal(receipt.token_id, "a".repeat(64));
  assert.equal(receipt.root_pid, 321);
  assert.equal(receipt.root_start_time_us, 1_790_000_000_000_001);
  assert.equal(receipt.actual_model, "gpt-6-astra");
  assert.equal(receipt.actual_reasoning, "high");
  assert.equal(bridge.isBound, false, "receipt is not an Active owner");
  assert.deepEqual(presence, [], "no managed presence before Active");
  assert.equal(server.callsOf("mcpServerStatus/list").length, 0);
  assert.equal(server.callsOf("mcpServer/tool/call").length, 0, "no MCP tool proof before Active");
  assert.equal(server.callsOf("turn/start").length, 0, "no kickoff or work before Active");

  dropClients(server);
  await waitFor(() => server.callsOf("initialize").length >= 2, "managed bridge reconnect");
  await delay(100);
  assert.equal(server.callsOf("thread/start").length, 1, "reconnect reuses receipt without a second start");

  markManagedOwnerObserved(agent, receipt);
  await delay(50);
  assert.equal(bridge.isBound, false, "observed Starting transition is not delivery authority");
  writeManagedOwner(agent, "active", receipt);
  await waitFor(() => bridge.isBound, "managed bridge binds after native Active commit");
  assert.equal(bridge.boundThreadId, receipt.thread_id);
  assert.equal(server.callsOf("thread/resume").length, 0, "fresh managed thread is never resumed by heuristic");
  await waitFor(() => server.turnCallsContaining(CODEX_KICKOFF_TEXT.slice(0, 40)).length === 1, "managed kickoff after Active");
  assert.ok(logs.some((entry) => entry.event === "codex_managed_observed"));
  assert.ok(presence.includes("join"));
});

test("managed ambiguous or mismatched thread/start is never retried, bound, or delivered", async (t) => {
  const failed = await scenario(t, { threads: [], failures: { "thread/start": 1 } });
  makeManagedSeat(failed.agent);
  writeManagedOwner(failed.agent, "starting");
  failed.bridge.start();
  await waitFor(() => failed.server.callsOf("thread/start").length === 1, "managed failed start attempt");
  await delay(500);
  assert.equal(failed.server.callsOf("thread/start").length, 1, "ambiguous start is not retried on reconnect");
  assert.equal(failed.bridge.isBound, false);
  assert.equal(existsSync(join(TMP, `${failed.agent}.g1.managed-observation.json`)), false);

  const mismatched = await scenario(t, { threads: [], threadStartModel: "gpt-wrong" });
  makeManagedSeat(mismatched.agent);
  writeManagedOwner(mismatched.agent, "starting");
  mismatched.bridge.start();
  await waitFor(() => mismatched.server.callsOf("thread/start").length === 1, "managed mismatched start attempt");
  await delay(500);
  assert.equal(mismatched.server.callsOf("thread/start").length, 1, "model mismatch is terminal for the generation");
  assert.equal(mismatched.bridge.isBound, false);
  assert.equal(existsSync(join(TMP, `${mismatched.agent}.g1.managed-observation.json`)), false);
  assert.equal(mismatched.server.callsOf("turn/start").length, 0);
  assert.deepEqual(mismatched.presence, []);
});

test("managed Active reconnect resumes the owner thread, never the newest thread", async (t) => {
  const { agent, server, bridge } = await scenario(t, {
    threads: [{ id: "newest-unowned" }, { id: "owner-thread" }],
  });
  makeManagedSeat(agent);
  writeManagedOwner(agent, "active", { thread_id: "owner-thread" });
  bridge.start();
  await waitFor(() => bridge.isBound, "managed Active bridge bound");
  assert.equal(server.callsOf("thread/list").length, 0);
  assert.equal(server.callsOf("thread/start").length, 0);
  assert.equal(server.callsOf("thread/resume").length, 1);
  assert.equal(server.callsOf("thread/resume")[0].params.threadId, "owner-thread");
  assert.equal(bridge.boundThreadId, "owner-thread");
});

test("managed Starting state change between owner reads fails closed without legacy discovery", async (t) => {
  let changed = false;
  const { agent, server, bridge, logs, presence } = await scenario(t, {
    threads: [{ id: "legacy-thread-must-not-bind" }],
    beforeManagedOwnerReadback: (seat, phase) => {
      if (seat !== agent || phase !== "starting-bind" || changed) return;
      changed = true;
      mutateManagedOwner(agent, (owner) => {
        owner.state = "quarantined";
        owner.provisional_token_id = null;
      });
    },
  });
  makeManagedSeat(agent);
  writeManagedOwner(agent, "starting");
  bridge.start();

  await waitFor(
    () => logs.some((entry) => entry.event === "codex_handshake_error"),
    "managed nullable read fails handshake",
  );
  bridge.stop();
  assert.equal(server.callsOf("thread/list").length, 0);
  assert.equal(server.callsOf("thread/start").length, 0);
  assert.equal(server.callsOf("thread/resume").length, 0);
  assert.equal(bridge.isBound, false);
  assert.deepEqual(presence, []);
});

test("managed thread/start owner change rejects observation before publication", async (t) => {
  const { agent, server, bridge, logs, presence } = await scenario(t, {
    delays: { "thread/start": 200 },
  });
  makeManagedSeat(agent);
  writeManagedOwner(agent, "starting");
  bridge.start();
  await waitFor(() => server.callsOf("thread/start").length === 1, "managed delayed thread/start");
  mutateManagedOwner(agent, (owner) => {
    owner.incarnation.pid = 322;
    owner.incarnation.processes[0].pid = 322;
  });
  await waitFor(
    () => logs.some((entry) => entry.event === "codex_handshake_error"),
    "changed Starting owner rejected",
  );
  bridge.stop();

  assert.equal(existsSync(join(TMP, `${agent}.g1.managed-observation.json`)), false);
  assert.equal(server.callsOf("thread/start").length, 1);
  assert.equal(server.callsOf("thread/list").length, 0);
  assert.equal(server.callsOf("turn/start").length, 0);
  assert.equal(bridge.isBound, false);
  assert.deepEqual(presence, []);
});

test("managed Active owner change during resume rejects bind and delivery", async (t) => {
  const { agent, server, bridge, logs, presence } = await scenario(t, {
    threads: [{ id: "owner-thread" }],
    delays: { "thread/resume": 200 },
  });
  makeManagedSeat(agent);
  writeManagedOwner(agent, "active", { thread_id: "owner-thread" });
  bridge.start();
  await waitFor(() => server.callsOf("thread/resume").length === 1, "managed delayed resume");
  mutateManagedOwner(agent, (owner) => {
    owner.incarnation.pid = 322;
    owner.incarnation.processes[0].pid = 322;
  });
  await waitFor(
    () => logs.some((entry) => entry.event === "codex_handshake_error"),
    "changed Active owner rejected",
  );
  bridge.stop();

  assert.equal(server.callsOf("thread/list").length, 0);
  assert.equal(server.callsOf("thread/start").length, 0);
  assert.equal(server.callsOf("turn/start").length, 0);
  assert.equal(bridge.isBound, false);
  assert.deepEqual(presence, []);
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
  const { agent, server, bridge } = await scenario(t, { threads: [{ id: "t-d" }] });

  bridge.start();
  await waitFor(() => bridge.isBound, "bridge bound");

  // BEADS still reports the row unread on both fetches (agent hasn't acked) —
  // the in-memory delivered-set must dedupe the second pump.
  setUnread([msgRow("m-d1", "glados", agent, "replay-overlap message")]);
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
  const { agent, server, bridge } = await scenario(t, { threads: [{ id: "t-e" }] });

  bridge.start();
  await waitFor(() => bridge.isBound, "bridge bound");

  server.notify("turn/started", { threadId: "t-e" });
  await waitFor(() => bridge.isTurnActive, "turn marked active");

  setUnread([msgRow("m-e1", "glados", agent, "mid-turn message")]);
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
    msgRow("m-e1", "glados", agent, "mid-turn message"),
    msgRow("m-e2", "glados", agent, "post-turn message"),
  ]);
  bridge.deliver();
  await waitFor(() => server.turnCallsContaining("m-e2").length > 0, "m-e2 injected");
  const inj2 = server.turnCallsContaining("m-e2");
  assert.equal(inj2.length, 1);
  assert.equal(inj2[0].method, "turn/start", "idle again → turn/start");
  assert.equal(server.turnCallsContaining("m-e1").length, 1, "m-e1 still deduped");
});

test("aperture-oeb6q A — socket death after inject: delivered-set cleared on close, same unread id re-injected after reconnect + rebind", async (t) => {
  const { agent, server, bridge, logs } = await scenario(t, { threads: [{ id: "t-r" }] });

  bridge.start();
  await waitFor(() => bridge.isBound, "bridge bound");

  setUnread([msgRow("m-r1", "glados", agent, "message riding a turn that dies")]);
  bridge.deliver();
  await waitFor(() => server.turnCallsContaining("m-r1").length === 1, "m-r1 injected once");

  // The app-server dies mid-turn (user abort / crash / hub-side drop). BEADS
  // still reports m-r1 unread — the agent never acked it. Pre-fix, `delivered`
  // survived onClose and the rebind replay filtered m-r1 out forever.
  dropClients(server);
  await waitFor(() => !bridge.isBound, "bridge observed the close");
  assert.ok(logs.some((l) => l.event === "codex_disconnected"), "codex_disconnected logged");

  await waitFor(() => bridge.isBound, "bridge reconnected + rebound", 5000);
  await waitFor(() => server.turnCallsContaining("m-r1").length === 2, "m-r1 re-injected after rebind");
  await delay(300); // grace: an over-eager fix would triple-inject within the same chain

  const inj = server.turnCallsContaining("m-r1");
  assert.equal(inj.length, 2, "exactly two injections: one per app-server connection");
  const resumes = server.callsOf("thread/resume");
  assert.equal(resumes.length, 2, "rebind resumed the surviving thread again");
  assert.ok(
    server.calls.indexOf(inj[1]) > server.calls.indexOf(resumes[1]),
    "re-injection lands after the second thread/resume",
  );
  assert.equal(inj[1].method, "turn/start", "fresh connection resets turn-state → turn/start");
  assert.equal(
    logs.filter((l) => l.event === "codex_inject_retry").length,
    0,
    "socket-loss rejection is handled by the rebind replay, not the inject re-pump",
  );
});

test("aperture-oeb6q B — inject error: one re-pump with flipped turn-state after INJECT_RETRY_MS; second failure → codex_inject_retry_exhausted, no third attempt", async (t) => {
  const { agent, server, bridge, logs } = await scenario(t, {
    threads: [{ id: "t-x" }],
    failures: { "turn/start": 1, "turn/steer": 1 },
  });

  bridge.start();
  await waitFor(() => bridge.isBound, "bridge bound");
  // Let the bind-triggered empty unread replay finish before this test writes
  // its row; otherwise that fetch and the explicit pump race and bypass the
  // retry timer by legitimately scheduling two independent deliveries.
  await delay(50);

  setUnread([msgRow("m-x1", "glados", agent, "message hitting a stale turn-state")]);
  bridge.deliver();
  await waitFor(
    () => logs.some((l) => l.event === "codex_inject_error" && l.method === "turn/start"),
    "first inject (turn/start) errored",
  );
  assert.equal(bridge.isTurnActive, true, "turn/start refused → bridge now assumes the thread is busy");

  await waitFor(
    () => server.turnCallsContaining("m-x1").length === 2,
    "single re-pump attempted",
    INJECT_RETRY_MS * 5,
  );
  const inj = server.turnCallsContaining("m-x1");
  assert.equal(inj[0].method, "turn/start", "first attempt assumed idle");
  assert.equal(inj[1].method, "turn/steer", "retry uses the other method (flipped turn-state)");
  assert.ok(
    inj[1].ts - inj[0].ts >= INJECT_RETRY_MS * 0.8,
    `retry waited ~INJECT_RETRY_MS (got ${inj[1].ts - inj[0].ts}ms)`,
  );
  assert.equal(logs.filter((l) => l.event === "codex_inject_retry").length, 1, "codex_inject_retry logged once");

  // The retry is scripted to fail too → exhausted, and NO automatic third try.
  await waitFor(() => logs.some((l) => l.event === "codex_inject_retry_exhausted"), "exhausted logged");
  const exhausted = logs.find((l) => l.event === "codex_inject_retry_exhausted");
  assert.deepEqual(exhausted.ids, ["m-x1"], "exhausted log names the ids");
  assert.equal(exhausted.method, "turn/steer");
  await delay(INJECT_RETRY_MS * 5);
  assert.equal(server.turnCallsContaining("m-x1").length, 2, "no third attempt within 5× the interval");
  assert.equal(logs.filter((l) => l.event === "codex_inject_retry").length, 1, "no second re-pump armed");

  // Ids were released both times: the NEXT hub notify is the next chance.
  bridge.deliver();
  await waitFor(() => server.turnCallsContaining("m-x1").length === 3, "next notify re-injects");
});

test.after(() => {
  try {
    rmSync(TMP, { recursive: true, force: true });
  } catch {
    /* tmp reaper handles it */
  }
});

// Protocol shapes pinned to codex-cli 0.155.1 generate-ts (no real harness).
async function activeMcpScenario(t, opts = {}) {
  const s = await scenario(t, opts);
  makeManagedSeat(s.agent);
  writeManagedOwner(s.agent, "active", { thread_id: "t-mcp-existing" });
  setUnread([msgRow("m-ready-proof", "glados", s.agent, "fixture mission must wait")]);
  s.bridge.start();
  return s;
}

test("managed MCP catalog plus real RO protocol call precede delivery on exact Active thread", async t => {
  const s = await activeMcpScenario(t, { delays: { "mcpServer/tool/call": 100 } });
  await waitFor(() => s.server.callsOf("mcpServer/tool/call").length === 1, "RO proof started");
  assert.equal(s.server.callsOf("turn/start").length, 0);
  await waitFor(() => s.server.turnCallsContaining("m-ready-proof").length === 1, "mission released");
  const probe = s.server.callsOf("mcpServer/tool/call")[0];
  assert.deepEqual(probe.params, { threadId: "t-mcp-existing", server: "aperture-bus", tool: "get_messages", arguments: {} });
  assert.ok(s.server.indexOf("turn/start") > s.server.indexOf("mcpServer/tool/call"));
  assert.equal(s.bridge.mcpReadiness, "ready");
  assert.equal(s.server.callsOf("thread/start").length, 0);
});

for (const variant of ["failed", "missing", "empty", "required", "invalid-tool", "discovery", "duplicate", "cursor", "probe", "rpc"]) {
  test(`managed MCP ${variant} is blocked: no turn, no retry, sanitized error`, async t => {
    const s = await scenario(t);
    makeManagedSeat(s.agent); writeManagedOwner(s.agent, "active", { thread_id: "t-mcp-existing" });
    const row = s.server.mcpStatus.data[0];
    if (variant === "failed" || variant === "starting") row.runtimeStatus = variant;
    if (variant === "missing") s.server.mcpStatus.data.shift();
    if (variant === "empty") row.tools = {};
    if (variant === "required") delete row.tools.send_message;
    if (variant === "invalid-tool") row.tools.send_message = null;
    if (variant === "discovery") row.toolsError = "PRIVATE_ERROR_SENTINEL";
    if (variant === "duplicate") s.server.mcpStatus.data.push({ ...row });
    if (variant === "cursor") s.server.mcpStatus.nextCursor = "same-cursor";
    if (variant === "probe") s.server.mcpProbe = { content: [{ type: "text", text: "PRIVATE_ERROR_SENTINEL" }], isError: true };
    if (variant === "rpc") s.server.failures["mcpServerStatus/list"] = 1;
    setUnread([msgRow("m-private", "glados", s.agent, "fixture")]);
    s.bridge.start();
    await waitFor(() => s.bridge.mcpReadiness === "blocked", "MCP denial");
    s.bridge.deliver(); await delay(150);
    assert.equal(s.server.calls.filter(c => c.method.startsWith("turn/")).length, 0);
    assert.equal(s.server.callsOf("initialize").length, 1, "no reconnect to retry admission");
    assert.equal(s.bridge.isBound, false);
    assert.equal(JSON.stringify(s.logs).includes("PRIVATE_ERROR_SENTINEL"), false);
  });
}

test("MCP readiness owner drift during RO proof never binds or sends", async t => {
  const s = await activeMcpScenario(t, { delays: { "mcpServer/tool/call": 100 } });
  await waitFor(() => s.server.callsOf("mcpServer/tool/call").length === 1, "probe");
  mutateManagedOwner(s.agent, o => { o.generation++; });
  await waitFor(() => s.bridge.mcpReadiness === "blocked", "owner drift blocked");
  assert.equal(s.server.callsOf("turn/start").length, 0);
  assert.equal(s.bridge.isBound, false);
});

test("later MCP failure revalidates before next mission and retains unread intent", async t => {
  const s = await activeMcpScenario(t);
  await waitFor(() => s.server.turnCallsContaining("m-ready-proof").length === 1, "initial admission");
  s.server.mcpStatus.data[0].runtimeStatus = "failed";
  s.server.notify("mcpServer/startupStatus/updated", { threadId: "t-mcp-existing", name: "aperture-bus", status: "failed", error: "PRIVATE_ERROR_SENTINEL" });
  await waitFor(() => s.bridge.mcpReadiness === "blocked", "readiness invalidated");
  setUnread([msgRow("m-held", "glados", s.agent, "later mission")]);
  s.bridge.deliver(); await delay(150);
  assert.equal(s.server.turnCallsContaining("m-held").length, 0);
  assert.equal(JSON.stringify(s.logs).includes("PRIVATE_ERROR_SENTINEL"), false);
});

test("managed pending MCP startup may settle without a new thread or premature kickoff", async t => {
  const s = await scenario(t);
  makeManagedSeat(s.agent); writeManagedOwner(s.agent, "active", { thread_id: "t-mcp-existing" });
  s.server.mcpStatus.data[0].runtimeStatus = "starting";
  s.bridge.start();
  await waitFor(() => s.server.callsOf("mcpServerStatus/list").length > 0, "pending catalog");
  assert.equal(s.server.callsOf("mcpServer/tool/call").length, 0);
  assert.equal(s.server.callsOf("turn/start").length, 0);
  s.server.mcpStatus.data[0].runtimeStatus = "connected";
  await waitFor(() => s.bridge.mcpReadiness === "ready", "startup settled");
  assert.equal(s.server.callsOf("thread/start").length, 0);
  assert.equal(s.server.callsOf("initialize").length, 1);
});

test("failure during initial read proof cannot publish readiness", async t => {
  const s = await activeMcpScenario(t, { delays: { "mcpServer/tool/call": 100 } });
  await waitFor(() => s.server.callsOf("mcpServer/tool/call").length === 1, "proof in flight");
  s.server.notify("mcpServer/startupStatus/updated", { threadId: "t-mcp-existing", name: "aperture-bus", status: "failed", error: "PRIVATE_ERROR_SENTINEL" });
  await waitFor(() => s.bridge.mcpReadiness === "blocked", "failure fenced");
  await delay(150);
  assert.equal(s.bridge.isBound, false);
  assert.equal(s.server.callsOf("turn/start").length, 0);
});

test("managed MCP startup deadline is finite and never releases a pending catalog", async t => {
  const s = await scenario(t);
  makeManagedSeat(s.agent); writeManagedOwner(s.agent, "active", { thread_id: "t-mcp-existing" });
  s.server.mcpStatus.data[0].runtimeStatus = "starting";
  s.bridge.start();
  await waitFor(() => s.bridge.mcpReadiness === "blocked", "20-second startup deadline", 22_000);
  assert.equal(s.server.callsOf("mcpServer/tool/call").length, 0);
  assert.equal(s.server.callsOf("turn/start").length, 0);
  assert.equal(s.server.callsOf("initialize").length, 1);
  assert.ok(s.logs.some(x => x.code === "E_MCP_STARTUP_TIMEOUT"));
});
