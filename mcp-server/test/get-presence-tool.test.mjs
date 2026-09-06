// aperture-oeb6q — get_presence tool + presence-aware send_message acks.
// Run after `pnpm build`:  node --test test/get-presence-tool.test.mjs
//
// Boots the REAL MCP server (dist/index.js) as a child over stdio and talks to
// it with the SDK's own Client, so the assertions cover the tool descriptions
// and reply strings agents actually see — not a copy of the formatter.
//
// Isolation: every path the server touches at boot is env-overridable —
//   HOME / APERTURE_MAILBOX      → mailbox + send-queue land in a temp dir
//   APERTURE_RUN_DIR             → presence.json we seed per test
//   APERTURE_AGENTS_DIR          → roster = temp agent dirs (shared/ excluded)
//   BD_PATH                      → a stub `bd` that logs argv and acks the
//                                  queued createMessage flush with a fake id
//   APERTURE_WS_PORT=1           → hub-notify's push fails fast (nothing real
//                                  is ever contacted)
//   TZ=UTC                       → "since hh:mm:ss" is rendered in local time
// Hub liveness is `process.kill(hub_pid, 0)`: hub-up seeds hub_pid with THIS
// test process's pid (alive by construction); hub-down uses a pid outside the
// kernel's range (ESRCH) or no file at all. No test-only flag in index.ts.
import { test, before, after } from "node:test";
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

const TMP = mkdtempSync(join(tmpdir(), "presence-tool-"));
const HOME = join(TMP, "home");
const RUN = join(TMP, "run");
const AGENTS = join(TMP, "agents");
const PRESENCE = join(RUN, "presence.json");
const BD_STUB = join(TMP, "bd");
const BD_LOG = join(TMP, "bd-calls.log");

const ROSTER = ["glados", "izzy", "peppy", "rex", "wheatley"];
for (const d of [HOME, RUN, join(AGENTS, "shared"), ...ROSTER.map((n) => join(AGENTS, n))]) {
  mkdirSync(d, { recursive: true });
}
writeFileSync(
  BD_STUB,
  `#!/bin/sh\nprintf '%s\\n' "$*" >> "${BD_LOG}"\necho '{"id":"aperture-stub1"}'\n`,
  { mode: 0o755 },
);

// A pid the kernel cannot have handed out (macOS pid_max 99999, Linux ≤ 2^22).
const DEAD_PID = 2147483647;
// Seed with `agents` = {name: {state, since}}; `rex` is deliberately absent → offline.
const seedPresence = (hubPid, agents) =>
  writeFileSync(PRESENCE, JSON.stringify({ hub_pid: hubPid, updated_at: "2026-09-06T14:03:11.204Z", agents }));
const LIVE_AGENTS = {
  glados: { state: "busy", since: "2026-09-06T13:58:02.000Z" },
  izzy: { state: "idle", since: "2026-09-06T14:01:40.000Z" },
  peppy: { state: "online", since: "2026-09-06T14:00:11.000Z" },
  wheatley: { state: "idle", since: "2026-09-06T14:00:00.000Z" },
};

let client;
before(async () => {
  const transport = new StdioClientTransport({
    command: process.execPath,
    args: [SERVER],
    env: {
      PATH: process.env.PATH,
      TZ: "UTC",
      HOME,
      AGENT_NAME: "wheatley",
      AGENT_ROLE: "test",
      APERTURE_MAILBOX: join(TMP, "mailbox"),
      APERTURE_RUN_DIR: RUN,
      APERTURE_AGENTS_DIR: AGENTS,
      BD_PATH: BD_STUB,
      APERTURE_WS_PORT: "1",
    },
    stderr: "pipe",
  });
  client = new Client({ name: "presence-tool-test", version: "0.0.0" });
  await client.connect(transport);
});
after(async () => {
  await client?.close();
  rmSync(TMP, { recursive: true, force: true });
});

const call = async (name, args = {}) => {
  const r = await client.callTool({ name, arguments: args });
  return { text: r.content.map((c) => c.text).join(""), isError: r.isError === true };
};

test("get_presence is registered and its description explains every state", async () => {
  const { tools } = await client.listTools();
  const t = tools.find((x) => x.name === "get_presence");
  assert.ok(t, "get_presence missing from tools/list");
  for (const needle of ["online =", "busy =", "idle =", "offline =", "unknown =", "presence snapshot", "call it freely"]) {
    assert.match(t.description, new RegExp(needle.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")));
  }
  assert.deepEqual(Object.keys(t.inputSchema.properties ?? {}), [], "get_presence takes no parameters");
});

test("get_presence: hub up → header + one padded row per roster agent, absent agent = offline", async () => {
  seedPresence(process.pid, LIVE_AGENTS);
  const { text, isError } = await call("get_presence");
  assert.equal(isError, false);
  assert.equal(
    text,
    [
      "Hub: up (snapshot 14:03:11)",
      "glados    busy     since 13:58:02",
      "izzy      idle     since 14:01:40",
      "peppy     online   since 14:00:11",
      "rex       offline",
      "wheatley  idle     since 14:00:00",
    ].join("\n"),
  );
});

test("send_message ack carries recipient presence (busy / idle / online / offline)", async () => {
  seedPresence(process.pid, LIVE_AGENTS);
  const cases = [
    ["glados", "Queued for glados. glados is busy (since 13:58:02). It will interrupt their current turn as a Monitor event."],
    ["izzy", "Queued for izzy. izzy is idle (since 14:01:40)."],
    ["peppy", "Queued for peppy. peppy is online (since 14:00:11)."],
    ["rex", "Queued for rex. rex is offline. It will be pushed when they reconnect (unread replay); nothing is lost."],
  ];
  for (const [to, expected] of cases) {
    const { text, isError } = await call("send_message", { to, message: `hi ${to}` });
    assert.equal(isError, false, `send to ${to} errored: ${text}`);
    assert.equal(text, expected);
    assert.doesNotMatch(text, /poller/i, "stale poller wording must be gone");
  }
});

test("hub down (dead hub_pid): get_presence says so, send_message reports unknown + replay", async () => {
  seedPresence(DEAD_PID, LIVE_AGENTS);
  const p = await call("get_presence");
  assert.equal(p.isError, false);
  assert.equal(
    p.text,
    "Hub: down — presence unknown for all agents. The launcher may be closed or the hub restarting; retry in a few seconds.",
  );
  const s = await call("send_message", { to: "glados", message: "hub down send" });
  assert.equal(s.isError, false);
  assert.equal(
    s.text,
    "Queued for glados. glados's presence is unknown (hub down). It will be pushed when they reconnect (unread replay); nothing is lost.",
  );
});

test("hub down (no presence file at all): same down message, send still succeeds", async () => {
  rmSync(PRESENCE, { force: true });
  const p = await call("get_presence");
  assert.equal(p.text.startsWith("Hub: down — presence unknown for all agents."), true);
  const s = await call("send_message", { to: "rex", message: "no file send" });
  assert.equal(s.isError, false);
  assert.match(s.text, /^Queued for rex\. rex's presence is unknown \(hub down\)\./);
});

test("retired recipients (sage/atlas/sterling) error with the routing hint; other typos get no hint", async () => {
  for (const to of ["sage", "atlas", "sterling"]) {
    const { text, isError } = await call("send_message", { to, message: "x" });
    assert.equal(isError, true, `${to} must be rejected`);
    assert.match(text, /^ERROR: Unknown recipient/);
    const validList = text.split("\n")[0].split("Valid recipients are:")[1];
    assert.doesNotMatch(validList, /\b(sage|atlas|sterling)\b/, "retired names must not be listed as valid");
    assert.match(text, /\nsage\/atlas\/sterling were retired 2026-07-19 — route SEO\/content to vance, docs to the implementing agent, QA sign-off to izzy\.$/);
  }
  const typo = await call("send_message", { to: "nobody", message: "x" });
  assert.equal(typo.isError, true);
  assert.doesNotMatch(typo.text, /retired/);
  assert.match(typo.text, /Valid recipients are: glados, wheatley, peppy, izzy, vance, rex, scout, cipher, operator\./);
});

test("send_message tool description no longer lists retired agents", async () => {
  const { tools } = await client.listTools();
  const t = tools.find((x) => x.name === "send_message");
  assert.doesNotMatch(t.description, /\b(sage|atlas|sterling)\b/);
  assert.doesNotMatch(t.inputSchema.properties.to.description, /\b(sage|atlas|sterling)\b/);
  const ct = tools.find((x) => x.name === "create_task");
  assert.doesNotMatch(ct.inputSchema.properties.assignee.description, /\b(sage|atlas|sterling)\b/);
});

test("get_identity describes hub-push delivery, not file contents", async () => {
  const { text } = await call("get_identity");
  const id = JSON.parse(text);
  assert.equal(id.name, "wheatley");
  assert.doesNotMatch(id.description, /file contents/);
  assert.match(id.description, /Claude agents receive hub push events on their inbox Monitor/);
  assert.match(id.description, /Codex agents receive .*injected turns/);
  assert.match(id.description, /get_messages.*mark_as_read/);
});

test("queued sends were flushed to (stub) bd as message creates — the presence ack never blocked the queue", async () => {
  // The flush is async (queue worker); give it a moment.
  const deadline = Date.now() + 3000;
  let log = "";
  while (Date.now() < deadline) {
    log = existsSync(BD_LOG) ? readFileSync(BD_LOG, "utf8") : "";
    if ((log.match(/--type message/g) ?? []).length >= 6) break;
    await new Promise((r) => setTimeout(r, 50));
  }
  const creates = (log.match(/^create \[wheatley->/gm) ?? []).length;
  assert.equal(creates, 6, `expected 6 flushed creates, saw ${creates}:\n${log}`);
});
