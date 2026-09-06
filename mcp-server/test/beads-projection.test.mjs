// aperture-84bby — MCP payload bounds for get_messages / query_tasks / search_tasks.
// Run after `pnpm build`:  node --test test/beads-projection.test.mjs
//
// Boots the REAL MCP server (dist/index.js) as a child over stdio and talks to
// it with the SDK's own Client (same pattern as get-presence-tool.test.mjs), so
// the assertions cover the argv actually handed to `bd` and the reply strings
// agents actually see.
//
// `bd` is a stub script (BD_PATH) that appends its argv — tab-separated, one
// line per invocation — to a log, then prints whatever the current scenario
// file holds. Each test writes the scenario before calling a tool, so one
// long-lived server child serves every test.
//
// Pins:
//   (a) get_messages passes `-n 200`, renders oldest-first, and appends the
//       cap notice ONLY when exactly 200 rows came back; the aperture-q6gov
//       non-array → ERROR path is intact.
//   (b) include_done:true → `--all`; omitted/false → no `--all`, and no
//       client-side status post-filter is needed (bd's default excludes closed).
//   (c) priority_max → `--priority-max N` on list/search (bd 1.0.2 has the
//       flag); on ready (no flag) → post-filter + `-n 0`, caller limit applied
//       client-side after the filter.
//   (d) limit → `-n <n>` on list/ready/search; absent → no `-n` at all.
//   (e) non-JSON / non-array bd stdout on list → short `ERROR:` reply, never
//       the raw dump; show keeps fields:"full" as the raw opt-in.
import { test, before, after, beforeEach } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, mkdirSync, writeFileSync, rmSync, existsSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StdioClientTransport } from "@modelcontextprotocol/sdk/client/stdio.js";

const HERE = dirname(fileURLToPath(import.meta.url));
const SERVER = resolve(HERE, "..", "dist", "index.js");
assert.ok(existsSync(SERVER), `build first: ${SERVER} missing`);

const AGENT = "wheatley";
const TMP = mkdtempSync(join(tmpdir(), "beads-projection-"));
const HOME = join(TMP, "home");
const RUN = join(TMP, "run");
const AGENTS = join(TMP, "agents");
const BD_STUB = join(TMP, "bd");
const BD_LOG = join(TMP, "bd-calls.log");
const SCENARIO = join(TMP, "scenario.out");

for (const d of [HOME, RUN, join(AGENTS, "shared"), join(AGENTS, AGENT), join(AGENTS, "glados")]) {
  mkdirSync(d, { recursive: true });
}
// Stub bd: log argv (tab-joined, one line per call), print the scenario file
// verbatim. The `\t` below are real tab characters once JS writes the file.
writeFileSync(
  BD_STUB,
  [
    "#!/bin/sh",
    'line=""',
    'for a in "$@"; do line="${line}${a}\t"; done',
    `printf '%s\\n' "\${line%\t}" >> "${BD_LOG}"`,
    `cat "${SCENARIO}"`,
    "",
  ].join("\n"),
  { mode: 0o755 },
);

const setScenario = (v) => writeFileSync(SCENARIO, typeof v === "string" ? v : JSON.stringify(v));
const calls = () =>
  (existsSync(BD_LOG) ? readFileSync(BD_LOG, "utf8") : "")
    .split("\n")
    .filter(Boolean)
    .map((l) => l.split("\t"));
const lastCall = () => {
  const c = calls();
  assert.ok(c.length > 0, "expected at least one bd invocation");
  return c[c.length - 1];
};
/** true when argv contains the flag followed by exactly `value` (or the flag alone when value undefined). */
const hasFlag = (argv, flag, value) => {
  const i = argv.indexOf(flag);
  if (i < 0) return false;
  return value === undefined ? true : argv[i + 1] === String(value);
};

const task = (id, priority, extra = {}) => ({
  id,
  title: `t ${id}`,
  status: "open",
  priority,
  labels: ["project:aperture"],
  description: "d",
  ...extra,
});
const message = (i, createdAt) => ({
  id: `aperture-wisp-${String(i).padStart(4, "0")}`,
  title: `[glados->${AGENT}] hello ${i}`,
  description: `body ${i}`,
  status: "open",
  issue_type: "message",
  created_at: createdAt,
});
// bd query emits NEWEST-first; build n messages that way so the sort is observable.
const newestFirstMessages = (n) =>
  Array.from({ length: n }, (_, k) => {
    const i = n - k; // n, n-1, …, 1
    return message(i, new Date(Date.UTC(2026, 8, 1, 0, 0, i)).toISOString());
  });

let client;
before(async () => {
  setScenario([]);
  const transport = new StdioClientTransport({
    command: process.execPath,
    args: [SERVER],
    env: {
      PATH: process.env.PATH,
      TZ: "UTC",
      HOME,
      AGENT_NAME: AGENT,
      AGENT_ROLE: "test",
      APERTURE_MAILBOX: join(TMP, "mailbox"),
      APERTURE_RUN_DIR: RUN,
      APERTURE_AGENTS_DIR: AGENTS,
      BD_PATH: BD_STUB,
      APERTURE_WS_PORT: "1",
    },
    stderr: "pipe",
  });
  client = new Client({ name: "beads-projection-test", version: "0.0.0" });
  await client.connect(transport);
});
after(async () => {
  await client?.close();
  rmSync(TMP, { recursive: true, force: true });
});
beforeEach(() => {
  rmSync(BD_LOG, { force: true });
});

const call = async (name, args = {}) => {
  const r = await client.callTool({ name, arguments: args });
  return { text: r.content.map((c) => c.text).join(""), isError: r.isError === true };
};

// ── (a) get_messages ──────────────────────────────────────────────────────

test("get_messages: bd argv is the query with -n 200 (bounded, not -n 0)", async () => {
  setScenario([]);
  const { text, isError } = await call("get_messages");
  assert.equal(isError, false);
  assert.equal(text, "No unread messages.");
  assert.deepEqual(lastCall(), [
    "query",
    `type=message AND status=open AND title="->${AGENT}]"`,
    "--json",
    "-n",
    "200",
  ]);
});

test("get_messages: under the cap → oldest-first, no cap notice", async () => {
  setScenario(newestFirstMessages(3));
  const { text, isError } = await call("get_messages");
  assert.equal(isError, false);
  const ids = [...text.matchAll(/^\[(aperture-wisp-\d+)\] From glados: body (\d+)$/gm)].map((m) => m[2]);
  assert.deepEqual(ids, ["1", "2", "3"], "rendered oldest-first even though bd emitted newest-first");
  assert.doesNotMatch(text, /Showing the 200 most recent/);
  assert.doesNotMatch(text, /call get_messages again/);
});

test("get_messages: exactly 200 rows → oldest-first + cap notice appended once", async () => {
  setScenario(newestFirstMessages(200));
  const { text, isError } = await call("get_messages");
  assert.equal(isError, false);
  const blocks = text.split("\n\n");
  assert.equal(blocks.length, 201, "200 message blocks + 1 notice");
  assert.match(blocks[0], /^\[aperture-wisp-0001\] From glados: body 1$/);
  assert.match(blocks[199], /^\[aperture-wisp-0200\] From glados: body 200$/);
  assert.equal(
    blocks[200],
    "Showing the 200 most recent unread messages (oldest first); older messages are still queued. Call get_messages again after marking these read.",
  );
  assert.equal((text.match(/Showing the 200 most recent/g) ?? []).length, 1);
});

test("get_messages: 199 rows → no cap notice (notice is exactly-at-cap only)", async () => {
  setScenario(newestFirstMessages(199));
  const { text } = await call("get_messages");
  assert.equal(text.split("\n\n").length, 199);
  assert.doesNotMatch(text, /Showing the 200 most recent/);
});

test("get_messages: non-array bd body is still an ERROR, never an empty inbox (aperture-q6gov)", async () => {
  setScenario({ not: "an array" });
  const { text, isError } = await call("get_messages");
  assert.equal(isError, true);
  assert.match(text, /^ERROR: unexpected bd response shape for get_messages — expected a JSON array, got object/);
  assert.match(text, /NOT an empty inbox/);
});

// ── (b) include_done push-down ───────────────────────────────────────────

test("query_tasks list: include_done:true → --all; omitted → no --all and no -n", async () => {
  setScenario([task("a-1", 2)]);
  await call("query_tasks", { mode: "list", include_done: true });
  let argv = lastCall();
  assert.equal(argv[0], "list");
  assert.ok(hasFlag(argv, "--all"), `expected --all in ${argv.join(" ")}`);
  assert.ok(hasFlag(argv, "--assignee", AGENT), "list mode still defaults assignee to the caller");

  rmSync(BD_LOG, { force: true });
  await call("query_tasks", { mode: "list" });
  argv = lastCall();
  assert.ok(!hasFlag(argv, "--all"), `unexpected --all in ${argv.join(" ")}`);
  assert.ok(!argv.includes("-n"), `no -n expected without limit, got ${argv.join(" ")}`);

  rmSync(BD_LOG, { force: true });
  await call("query_tasks", { mode: "list", include_done: false });
  assert.ok(!hasFlag(lastCall(), "--all"));
});

test("query_tasks list: closed rows bd chose to return are passed through (no client status post-filter)", async () => {
  // With --all pushed down, filtering is bd's job; a closed row in the JSON
  // must survive so include_done:true actually returns history.
  setScenario([task("a-1", 2, { status: "closed" }), task("a-2", 1)]);
  const { text } = await call("query_tasks", { mode: "list", include_done: true });
  assert.deepEqual(JSON.parse(text).map((t) => t.id), ["a-1", "a-2"]);
});

test("search_tasks: include_done:true → --all; omitted → not", async () => {
  setScenario([]);
  await call("search_tasks", { include_done: true });
  assert.ok(hasFlag(lastCall(), "--all"));
  rmSync(BD_LOG, { force: true });
  await call("search_tasks", {});
  assert.ok(!hasFlag(lastCall(), "--all"));
});

// ── (c) priority_max ─────────────────────────────────────────────────────

test("priority_max on list/search → --priority-max N pushed to bd, no -n 0", async () => {
  setScenario([task("a-1", 0)]);
  await call("query_tasks", { mode: "list", priority_max: 2 });
  let argv = lastCall();
  assert.ok(hasFlag(argv, "--priority-max", 2), `expected --priority-max 2 in ${argv.join(" ")}`);
  assert.ok(!hasFlag(argv, "-n", 0), `must not force -n 0 when bd filters: ${argv.join(" ")}`);

  rmSync(BD_LOG, { force: true });
  await call("search_tasks", { priority_max: 1 });
  argv = lastCall();
  assert.equal(argv[0], "list");
  assert.ok(hasFlag(argv, "--priority-max", 1));
  assert.ok(!hasFlag(argv, "-n", 0));
});

test("priority_max on ready (bd has no flag) → -n 0 to bd + client post-filter", async () => {
  setScenario([task("r-p0", 0), task("r-p1", 1), task("r-p2", 2), task("r-p3", 3)]);
  const { text, isError } = await call("query_tasks", { mode: "ready", priority_max: 1 });
  assert.equal(isError, false);
  const argv = lastCall();
  assert.equal(argv[0], "ready");
  assert.ok(!argv.includes("--priority-max"), `bd ready has no --priority-max: ${argv.join(" ")}`);
  assert.ok(hasFlag(argv, "-n", 0), `post-filter path must fetch unbounded: ${argv.join(" ")}`);
  assert.deepEqual(JSON.parse(text).map((t) => t.id), ["r-p0", "r-p1"]);
});

test("priority_max + limit on ready → bd still gets -n 0, limit applied after the filter", async () => {
  setScenario([task("r-p3", 3), task("r-p0", 0), task("r-p1", 1), task("r-p0b", 0)]);
  const { text } = await call("query_tasks", { mode: "ready", priority_max: 1, limit: 2 });
  const argv = lastCall();
  assert.ok(hasFlag(argv, "-n", 0), `caller limit must not truncate before the filter: ${argv.join(" ")}`);
  assert.ok(!hasFlag(argv, "-n", 2));
  assert.deepEqual(JSON.parse(text).map((t) => t.id), ["r-p0", "r-p1"]);
});

// ── (d) limit ────────────────────────────────────────────────────────────

test("limit → -n <n> on list, ready and search", async () => {
  setScenario([]);
  await call("query_tasks", { mode: "list", limit: 7 });
  assert.ok(hasFlag(lastCall(), "-n", 7), lastCall().join(" "));

  rmSync(BD_LOG, { force: true });
  await call("query_tasks", { mode: "ready", limit: 3 });
  assert.ok(hasFlag(lastCall(), "-n", 3), lastCall().join(" "));

  rmSync(BD_LOG, { force: true });
  await call("search_tasks", { limit: 12 });
  assert.ok(hasFlag(lastCall(), "-n", 12), lastCall().join(" "));
});

test("limit schema: rejects 0 and 501, accepts 500", async () => {
  setScenario([]);
  for (const bad of [0, 501, 1.5]) {
    const r = await call("query_tasks", { mode: "list", limit: bad });
    assert.equal(r.isError, true, `limit ${bad} must be rejected`);
  }
  const ok = await call("search_tasks", { limit: 500 });
  assert.equal(ok.isError, false);
  assert.ok(hasFlag(lastCall(), "-n", 500));
});

test("tool descriptions document the cap and how to raise it", async () => {
  const { tools } = await client.listTools();
  const q = tools.find((t) => t.name === "query_tasks");
  const s = tools.find((t) => t.name === "search_tasks");
  const g = tools.find((t) => t.name === "get_messages");
  assert.match(q.description, /CAPPED.*at most 50 tasks.*'ready' at most 10.*pass limit \(1-500\)/s);
  assert.match(s.description, /CAPPED at 50 tasks by default.*pass limit \(1-500\)/s);
  assert.ok(q.inputSchema.properties.limit, "query_tasks exposes limit");
  assert.ok(s.inputSchema.properties.limit, "search_tasks exposes limit");
  assert.match(g.description, /At most 200 per call/);
});

// ── (e) parse failure = error, not raw dump ──────────────────────────────

const JUNK = "bd: dolt server warming up…\n" + "Ignored pretty-printed table row ".repeat(3000); // ~100KB

test("list: non-JSON bd stdout → short ERROR reply, not the raw dump", async () => {
  setScenario(JUNK);
  const { text, isError } = await call("query_tasks", { mode: "list" });
  assert.equal(isError, true);
  assert.match(text, /^ERROR: bd returned unexpected output \(not valid JSON\)/);
  assert.match(text, /argv: \[".*list","--json"/, "argv is in the message");
  assert.match(text, /output \(first 300 chars of \d+\): bd: dolt server warming up/);
  assert.ok(text.length < 600, `reply must be short, was ${text.length} chars`);
});

test("list: JSON but not an array → ERROR, not the raw object", async () => {
  setScenario({ issues: [task("a-1", 1)] });
  const { text, isError } = await call("query_tasks", { mode: "list" });
  assert.equal(isError, true);
  assert.match(text, /^ERROR: bd returned unexpected output \(expected a JSON array, got object\)/);
  assert.ok(text.length < 600);
});

test("search: non-JSON bd stdout → short ERROR reply", async () => {
  setScenario(JUNK);
  const { text, isError } = await call("search_tasks", {});
  assert.equal(isError, true);
  assert.match(text, /^ERROR: bd returned unexpected output/);
  assert.ok(text.length < 600, `was ${text.length}`);
});

test("show: non-JSON → ERROR by default; fields:\"full\" keeps the raw opt-in passthrough", async () => {
  setScenario(JUNK);
  const dflt = await call("query_tasks", { mode: "show", id: "a-1" });
  assert.equal(dflt.isError, true);
  assert.match(dflt.text, /^ERROR: bd returned unexpected output \(not valid JSON\)/);
  assert.ok(dflt.text.length < 600, `was ${dflt.text.length}`);

  const full = await call("query_tasks", { mode: "show", id: "a-1", fields: "full" });
  assert.equal(full.isError, false);
  assert.equal(full.text, JUNK.trim(), "fields:full is the deliberate raw passthrough");
});

test("regression: summary projection still applies on a healthy list", async () => {
  setScenario([task("a-1", 1, { description: "x".repeat(500), notes: "n".repeat(10), extra_field: "dropped" })]);
  const { text, isError } = await call("query_tasks", { mode: "list" });
  assert.equal(isError, false);
  const [row] = JSON.parse(text);
  assert.equal(row.extra_field, undefined, "non-summary fields projected away");
  assert.equal(row.description.length, 201, "200 chars + ellipsis");
  assert.equal(row._truncated, true);
});
