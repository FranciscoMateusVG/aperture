// aperture-oeb6q — unit pins for the presence-snapshot contract module
// (dist/presence-snapshot.js): the reader side that get_presence and
// send_message's recipient hint consume. The writer side is exercised
// end-to-end against a real hub in hub-protocol.test.mjs; here we only
// round-trip it to pin the on-disk shape.
//
// Pins:
//   1. write → read round-trip is lossless; no tmp file is left behind
//   2. presenceReport, hub up   — present agents carry state+since, absent
//                                 roster agents are "offline", sorted by name
//   3. presenceReport, hub down — isAlive false → every agent "unknown",
//                                 never "offline"; updated_at null
//   4. missing / corrupt / malformed file → hub "down", isAlive never consulted
//   5. readRoster — agent dirs only (no `shared/`, no dotfiles, no plain files),
//                   sorted; unreadable dir → built-in fallback roster
//   6. describePresence strings for every branch
//
// Run: node --test test/presence-snapshot.test.mjs   (from mcp-server/, after pnpm build)

import { test } from "node:test";
import assert from "node:assert/strict";
import { existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

// RUN_DIR / AGENTS_DIR are read from env at module load — point them at a
// sandbox BEFORE the dynamic import so nothing touches ~/.aperture.
const TMP = mkdtempSync(join(tmpdir(), "presence-snapshot-test-"));
process.env.APERTURE_RUN_DIR = join(TMP, "run");
process.env.APERTURE_AGENTS_DIR = join(TMP, "no-such-agents-dir");
const { writePresenceSnapshot, readPresenceSnapshot, presenceReport, readRoster, describePresence, PRESENCE_FILE } =
  await import("../dist/presence-snapshot.js");

process.on("exit", () => rmSync(TMP, { recursive: true, force: true }));

const alwaysAlive = () => true;
const neverAlive = () => false;

/** A roster dir with three agents, a `shared/` dir, a dotfile, and a stray file. */
function makeAgentsDir(name = "agents") {
  const dir = join(TMP, name);
  mkdirSync(dir, { recursive: true });
  for (const a of ["vance", "rex", "glados"]) mkdirSync(join(dir, a));
  mkdirSync(join(dir, "shared"));
  writeFileSync(join(dir, ".DS_Store"), "");
  writeFileSync(join(dir, "README.md"), "");
  return dir;
}

function writeSnap(file, snap) {
  writeFileSync(file, JSON.stringify(snap));
  return file;
}

// ── 1. round-trip ───────────────────────────────────────────────────────────

test("writePresenceSnapshot → readPresenceSnapshot round-trips losslessly and leaves no tmp file", () => {
  const snap = {
    hub_pid: 4242,
    updated_at: "2026-09-06T14:03:11.204Z",
    agents: {
      rex: { state: "busy", since: "2026-09-06T14:02:58.000Z" },
      vance: { state: "idle", since: "2026-09-06T14:00:00.000Z" },
    },
  };
  assert.equal(writePresenceSnapshot(snap), true, "write reports success");
  assert.equal(PRESENCE_FILE, join(process.env.APERTURE_RUN_DIR, "presence.json"), "PRESENCE_FILE honours APERTURE_RUN_DIR");
  assert.ok(existsSync(PRESENCE_FILE), "file exists at PRESENCE_FILE");
  assert.deepEqual(readPresenceSnapshot(), snap, "read equals what was written");
  assert.deepEqual(
    readdirSync(process.env.APERTURE_RUN_DIR).filter((f) => f.endsWith(".tmp")),
    [],
    "atomic write left no .tmp behind",
  );
  // Explicit file arg works too (what the tests below use).
  const other = join(TMP, "other.json");
  assert.equal(writePresenceSnapshot({ hub_pid: 1, updated_at: "", agents: {} }, other), true);
  assert.deepEqual(readPresenceSnapshot(other), { hub_pid: 1, updated_at: "", agents: {} });
});

// ── 2. hub up ───────────────────────────────────────────────────────────────

test("presenceReport (hub up): present agents carry state+since, absent roster agents are offline, sorted", () => {
  const agentsDir = makeAgentsDir("agents-up");
  const file = writeSnap(join(TMP, "up.json"), {
    hub_pid: 4242,
    updated_at: "2026-09-06T14:03:11.204Z",
    agents: {
      rex: { state: "busy", since: "2026-09-06T14:02:58.000Z" },
      vance: { state: "idle", since: "2026-09-06T14:00:00.000Z" },
      // Not in the roster dir: must NOT appear (roster is the dir, not the file).
      stranger: { state: "online", since: "2026-09-06T14:00:00.000Z" },
    },
  });
  const seen = [];
  const report = presenceReport({ file, agentsDir, isAlive: (pid) => (seen.push(pid), true) });
  assert.deepEqual(seen, [4242], "isAlive consulted exactly once with hub_pid");
  assert.deepEqual(report, {
    hub: "up",
    updated_at: "2026-09-06T14:03:11.204Z",
    agents: [
      { name: "glados", state: "offline", since: null },
      { name: "rex", state: "busy", since: "2026-09-06T14:02:58.000Z" },
      { name: "vance", state: "idle", since: "2026-09-06T14:00:00.000Z" },
    ],
  });
});

test("presenceReport (hub up, zero agents): every roster agent is offline — a fresh hub is not 'unknown'", () => {
  const agentsDir = makeAgentsDir("agents-empty");
  const file = writeSnap(join(TMP, "empty.json"), { hub_pid: 7, updated_at: "2026-09-06T00:00:00.000Z", agents: {} });
  const report = presenceReport({ file, agentsDir, isAlive: alwaysAlive });
  assert.equal(report.hub, "up");
  assert.deepEqual(
    report.agents.map((a) => a.state),
    ["offline", "offline", "offline"],
  );
});

// ── 3. hub down ─────────────────────────────────────────────────────────────

test("presenceReport (hub down, dead pid): every agent is unknown — never offline; updated_at null", () => {
  const agentsDir = makeAgentsDir("agents-down");
  const file = writeSnap(join(TMP, "down.json"), {
    hub_pid: 4242,
    updated_at: "2026-09-06T14:03:11.204Z",
    agents: { rex: { state: "busy", since: "2026-09-06T14:02:58.000Z" } },
  });
  const report = presenceReport({ file, agentsDir, isAlive: neverAlive });
  assert.deepEqual(report, {
    hub: "down",
    updated_at: null,
    agents: [
      { name: "glados", state: "unknown", since: null },
      { name: "rex", state: "unknown", since: null },
      { name: "vance", state: "unknown", since: null },
    ],
  });
  assert.ok(!report.agents.some((a) => a.state === "offline"), "a dead hub never yields 'offline'");
});

// ── 4. missing / corrupt file ───────────────────────────────────────────────

test("presenceReport: missing, corrupt, or malformed file → hub down, isAlive never consulted", () => {
  const agentsDir = makeAgentsDir("agents-bad");
  const writeRaw = (name, body) => {
    const f = join(TMP, name);
    writeFileSync(f, body);
    return f;
  };
  const cases = {
    missing: join(TMP, "does-not-exist.json"),
    "not json": writeRaw("corrupt.json", "{ torn"),
    "no hub_pid": writeSnap(join(TMP, "nopid.json"), { updated_at: "x", agents: {} }),
    "hub_pid not a number": writeSnap(join(TMP, "strpid.json"), { hub_pid: "4242", updated_at: "x", agents: {} }),
    "agents not an object": writeSnap(join(TMP, "noagents.json"), { hub_pid: 1, updated_at: "x", agents: "nope" }),
    "agents null": writeSnap(join(TMP, "nullagents.json"), { hub_pid: 1, updated_at: "x", agents: null }),
    "json array": writeRaw("array.json", "[]"),
  };
  for (const [label, file] of Object.entries(cases)) {
    let consulted = false;
    const report = presenceReport({ file, agentsDir, isAlive: () => ((consulted = true), true) });
    assert.equal(consulted, false, `${label}: isAlive not consulted`);
    assert.equal(report.hub, "down", `${label}: hub down`);
    assert.equal(report.updated_at, null, `${label}: updated_at null`);
    assert.deepEqual(
      report.agents.map((a) => a.state),
      ["unknown", "unknown", "unknown"],
      `${label}: every agent unknown`,
    );
    assert.equal(readPresenceSnapshot(file), null, `${label}: readPresenceSnapshot returns null`);
  }
});

test("readPresenceSnapshot: a non-string updated_at is normalised to '' rather than rejected", () => {
  const file = writeSnap(join(TMP, "noupdated.json"), { hub_pid: 1, agents: {} });
  assert.deepEqual(readPresenceSnapshot(file), { hub_pid: 1, updated_at: "", agents: {} });
});

// ── 5. readRoster ───────────────────────────────────────────────────────────

test("readRoster: agent dirs only — excludes shared/, dotfiles, plain files; sorted by name", () => {
  const agentsDir = makeAgentsDir("agents-roster");
  assert.deepEqual(readRoster(agentsDir), ["glados", "rex", "vance"]);
});

test("readRoster: unreadable or empty dir → built-in fallback roster", () => {
  const fallback = ["glados", "wheatley", "peppy", "izzy", "vance", "rex", "scout", "cipher"];
  assert.deepEqual(readRoster(join(TMP, "nope")), fallback, "missing dir → fallback");
  const onlyShared = join(TMP, "agents-only-shared");
  mkdirSync(join(onlyShared, "shared"), { recursive: true });
  assert.deepEqual(readRoster(onlyShared), fallback, "dir with only shared/ → fallback (zero agents is not a roster)");
  // The module-level default (APERTURE_AGENTS_DIR) points at a missing dir in this test → fallback.
  assert.deepEqual(readRoster(), fallback, "default agentsDir from env, missing → fallback");
});

// ── 6. describePresence ─────────────────────────────────────────────────────

test("describePresence: one honest line per branch", () => {
  const since = "2026-09-06T14:02:58.000Z";
  const hhmmss = new Date(since).toTimeString().slice(0, 8); // local time, same as the module
  const up = {
    hub: "up",
    updated_at: "2026-09-06T14:03:11.204Z",
    agents: [
      { name: "glados", state: "offline", since: null },
      { name: "rex", state: "busy", since },
      { name: "vance", state: "idle", since: null },
      { name: "scout", state: "online", since: "not-a-date" },
      { name: "odd", state: "unknown", since: null },
    ],
  };
  assert.equal(describePresence(up, "rex"), `rex is busy (since ${hhmmss})`);
  assert.equal(describePresence(up, "vance"), "vance is idle", "no since → no parenthetical");
  assert.equal(describePresence(up, "scout"), "scout is online", "unparseable since → no parenthetical");
  assert.equal(describePresence(up, "glados"), "glados is offline");
  assert.equal(describePresence(up, "nobody"), "nobody is offline", "not in the report at all → offline (hub is up)");
  assert.equal(describePresence(up, "odd"), "odd's presence is unknown");

  const down = { hub: "down", updated_at: null, agents: [{ name: "rex", state: "unknown", since: null }] };
  assert.equal(describePresence(down, "rex"), "rex's presence is unknown (hub down)");
  assert.equal(describePresence(down, "nobody"), "nobody's presence is unknown (hub down)", "hub down wins over roster membership");
});

// ── end-to-end through the real pid check ───────────────────────────────────

test("presenceReport with the default pid check: our own pid is alive, a dead pid is not", () => {
  const agentsDir = makeAgentsDir("agents-pid");
  const live = writeSnap(join(TMP, "live.json"), { hub_pid: process.pid, updated_at: "x", agents: {} });
  assert.equal(presenceReport({ file: live, agentsDir }).hub, "up", "own pid → up");
  // pid 0 / negative / non-integer are rejected before kill(0) is attempted.
  for (const pid of [0, -1, 1.5]) {
    const f = writeSnap(join(TMP, `pid-${String(pid).replace(/[^0-9]/g, "_")}.json`), { hub_pid: pid, updated_at: "x", agents: {} });
    assert.equal(presenceReport({ file: f, agentsDir }).hub, "down", `pid ${pid} → down`);
  }
});
