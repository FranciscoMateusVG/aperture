// aperture-trgpo — recall / recall_full / recall_stats tools + the memory-recall hook.
// Run after `pnpm build`:  node --test test/memory-tools.test.mjs
//
// Boots the REAL MCP server (dist/index.js) as a child over stdio and talks to
// it with the SDK's own Client (same harness as get-presence-tool.test.mjs), so
// the assertions cover the tool descriptions and reply strings agents see.
//
// Isolation — every path the server + hook touch is env-overridable:
//   HOME / APERTURE_MAILBOX      → mailbox + send-queue land in a temp dir
//   APERTURE_RUN_DIR             → memory-index cache dir (temp)
//   APERTURE_MEMORY_CACHE        → cache file (temp)
//   APERTURE_MEMORY_META         → sidecar (temp; one entry tagged secret)
//   BD_PATH                      → a stub `bd`: `memories --json` prints FAKE_BANK;
//                                  when <TMP>/bd-fail exists it exits 1 instead
//   APERTURE_WS_PORT=1           → hub-notify's push fails fast
//
// The fake bank has 5 entries: one with a `token=…` span (partial secret →
// [REDACTED] on every surface), one tagged secret in the sidecar (excluded
// entirely), three plain ones. No test-only flag in index.ts.
import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdtempSync, mkdirSync, writeFileSync, rmSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StdioClientTransport } from "@modelcontextprotocol/sdk/client/stdio.js";

const HERE = dirname(fileURLToPath(import.meta.url));
const SERVER = resolve(HERE, "..", "dist", "index.js");
const HOOK = resolve(HERE, "..", "dist", "memory-recall.js");
assert.ok(existsSync(SERVER), `build first: ${SERVER} missing`);
assert.ok(existsSync(HOOK), `build first: ${HOOK} missing`);

const TMP = mkdtempSync(join(tmpdir(), "memory-tools-"));
const HOME = join(TMP, "home");
const RUN = join(TMP, "run");
const AGENTS = join(TMP, "agents");
const SIDECAR = join(TMP, "memory-meta.json");
const CACHE = join(RUN, "memory-index.json");
const BD_STUB = join(TMP, "bd");
const BD_FAIL_FLAG = join(TMP, "bd-fail");
const TOKEN_FILE = join(TMP, "hub-tokens", "wheatley.token");

for (const d of [HOME, RUN, join(AGENTS, "shared"), join(AGENTS, "wheatley"), dirname(TOKEN_FILE)]) {
  mkdirSync(d, { recursive: true });
}
writeFileSync(TOKEN_FILE, "ab".repeat(32), { mode: 0o600 });

// ── fixtures ──
const RAW_TOKEN = "dk_live_abc123XYZ789secretvalue0001";
const SECRET_KEY = "mini-ssh-root-password";
const PARTIAL_SECRET_KEY = "dokploy-api-header-note";
const FAKE_BANK = {
  "hub-self-heal-pr-46":
    "The WS hub self-heals on reconnect: a producer socket that drops is replayed from BEADS unread on the next hello. Landed in PR #46 (ws-hub.ts). Banked 2026-07-19.",
  [PARTIAL_SECRET_KEY]:
    `Dokploy API calls need the x-api-key header. Current value token=${RAW_TOKEN} lives in the drawer; rotate quarterly. Banked 2026-08-01.`,
  [SECRET_KEY]: "Root password for the mac mini is hunter2-mini-2026. Do not share.",
  "playwright-tobevisible-overflow-clip":
    "Playwright toBeVisible passes on an element clipped by overflow:hidden — assert boundingBox intersects the viewport instead. Banked 2026-09-05 (aperture-acr3t).",
  "incluir-runner-mac-mini":
    "monorepo-incluir CI runs on the Mac Mini self-hosted runner; auto-merge is enabled for trusted actors only. Banked 2026-08-20.",
};
const SIDECAR_META = {
  "hub-self-heal-pr-46": { project: "aperture", tags: ["comms", "hub"], entities: ["ws-hub"], updated: "2026-07-19" },
  [PARTIAL_SECRET_KEY]: { project: "aperture", tags: ["infra"], updated: "2026-08-01" },
  [SECRET_KEY]: { project: "aperture", tags: ["secret", "infra"], updated: "2026-08-01" },
  "playwright-tobevisible-overflow-clip": { project: "aperture", tags: ["testing"], updated: "2026-09-05" },
  "incluir-runner-mac-mini": { project: "incluir", tags: ["ci"], updated: "2026-08-20" },
};
writeFileSync(SIDECAR, JSON.stringify(SIDECAR_META, null, 2));
writeFileSync(join(TMP, "bank.json"), JSON.stringify(FAKE_BANK));
writeFileSync(
  BD_STUB,
  [
    "#!/bin/sh",
    `if [ -f "${BD_FAIL_FLAG}" ]; then echo 'stub: dolt server unreachable' >&2; exit 1; fi`,
    `if [ "$1" = "memories" ]; then cat "${join(TMP, "bank.json")}"; exit 0; fi`,
    `echo '{"id":"aperture-stub1"}'`,
    "",
  ].join("\n"),
  { mode: 0o755 },
);

const BASE_ENV = {
  PATH: process.env.PATH,
  TZ: "UTC",
  HOME,
  APERTURE_MAILBOX: join(TMP, "mailbox"),
  APERTURE_RUN_DIR: RUN,
  APERTURE_MEMORY_CACHE: CACHE,
  APERTURE_MEMORY_META: SIDECAR,
  APERTURE_AGENTS_DIR: AGENTS,
  BD_PATH: BD_STUB,
  APERTURE_WS_PORT: "1",
};

const setBdFailing = (on) => (on ? writeFileSync(BD_FAIL_FLAG, "") : rmSync(BD_FAIL_FLAG, { force: true }));

let client;
before(async () => {
  const transport = new StdioClientTransport({
    command: process.execPath,
    args: [SERVER],
    env: { ...BASE_ENV, AGENT_NAME: "wheatley", AGENT_ROLE: "test" },
    stderr: "pipe",
  });
  client = new Client({ name: "memory-tools-test", version: "0.0.0" });
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

/** Spawn the hook with `payload` piped on stdin (ended unless keepStdinOpen). */
function runHook({ payload, env = {}, keepStdinOpen = false, timeoutMs = 4000 }) {
  return new Promise((resolve) => {
    const started = Date.now();
    const child = spawn(process.execPath, [HOOK], { env: { ...BASE_ENV, ...env }, stdio: ["pipe", "pipe", "pipe"] });
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", (d) => (stdout += d));
    child.stderr.on("data", (d) => (stderr += d));
    const killer = setTimeout(() => child.kill("SIGKILL"), timeoutMs);
    child.on("close", (code, signal) => {
      clearTimeout(killer);
      resolve({ code, signal, stdout, stderr, ms: Date.now() - started });
    });
    if (payload !== undefined) child.stdin.write(typeof payload === "string" ? payload : JSON.stringify(payload));
    if (!keepStdinOpen) child.stdin.end();
  });
}
const HOOK_ENV = { APERTURE_HUB_TOKEN_FILE: TOKEN_FILE };
const hookPayload = (prompt) => ({ hook_event_name: "UserPromptSubmit", prompt, session_id: "s1", cwd: TMP });

// ── tool registration ──

test("recall / recall_full / recall_stats are registered; descriptions state redaction + secret exclusion", async () => {
  const { tools } = await client.listTools();
  for (const name of ["recall", "recall_full", "recall_stats"]) {
    const t = tools.find((x) => x.name === name);
    assert.ok(t, `${name} missing from tools/list`);
    assert.match(t.description, /redact/i, `${name} description must say bodies are redacted`);
    assert.match(t.description, /secret/i, `${name} description must say secrets are excluded`);
  }
  const recallTool = tools.find((x) => x.name === "recall");
  assert.deepEqual(Object.keys(recallTool.inputSchema.properties), ["query", "k", "offset", "project", "tags", "include_superseded"]);
  assert.deepEqual(recallTool.inputSchema.required, ["query"]);
  const fullTool = tools.find((x) => x.name === "recall_full");
  assert.deepEqual(Object.keys(fullTool.inputSchema.properties), ["key", "max_bytes"]);
  const statsTool = tools.find((x) => x.name === "recall_stats");
  assert.deepEqual(Object.keys(statsTool.inputSchema.properties ?? {}), [], "recall_stats takes no parameters");
});

// ── error path (works against memory-index stubs too) ──

test("bd failing → recall reply starts with `ERROR: memory index unavailable`, isError, no fallback dump", async () => {
  setBdFailing(true);
  try {
    const { text, isError } = await call("recall", { query: "hub reconnect" });
    assert.equal(isError, true);
    assert.match(text, /^ERROR: memory index unavailable: /);
    for (const key of Object.keys(FAKE_BANK)) assert.doesNotMatch(text, new RegExp(key));
    assert.doesNotMatch(text, /hunter2/);
    assert.doesNotMatch(text, new RegExp(RAW_TOKEN));
  } finally {
    setBdFailing(false);
  }
});

// ── recall ──

test("recall: ranked lines `key · score · age · tags · gist` + footer; top hit for 'hub reconnect' is the hub memory", async () => {
  const { text, isError } = await call("recall", { query: "hub reconnect replay" });
  assert.equal(isError, false, text);
  const lines = text.split("\n");
  const footer = lines.pop();
  assert.match(footer, /^total=\d+ next_offset=(\d+|none) index_built_at=\S+$/);
  assert.ok(lines.length >= 1, "expected at least one ranked line");
  assert.match(lines[0], /^hub-self-heal-pr-46 · \d+\.\d{2} · (\d+d|age\?) · [^·]+ · .+$/);
  assert.match(lines[0], /comms/);
  assert.match(lines[0], /hub/);
});

test("recall: k + offset page through results; footer next_offset advances", async () => {
  const first = await call("recall", { query: "mac mini", k: 1 });
  assert.equal(first.isError, false, first.text);
  const footer = first.text.split("\n").pop();
  const m = footer.match(/^total=(\d+) next_offset=(\d+|none)/);
  assert.ok(m, footer);
  if (m[2] !== "none") {
    assert.equal(m[2], "1");
    const second = await call("recall", { query: "mac mini", k: 1, offset: 1 });
    assert.equal(second.isError, false, second.text);
    assert.notEqual(second.text.split("\n")[0], first.text.split("\n")[0], "offset=1 must return a different top line");
  }
});

test("recall: project filter restricts hits", async () => {
  const { text, isError } = await call("recall", { query: "mac mini runner", project: "incluir" });
  assert.equal(isError, false, text);
  const lines = text.split("\n").slice(0, -1).filter((l) => l !== "(no matches)");
  assert.ok(lines.length >= 1, "expected the incluir memory to match");
  for (const l of lines) assert.match(l, /^incluir-runner-mac-mini · /);
});

test("recall never shows the secret-tagged key or raw secret spans, even when queried for them", async () => {
  for (const args of [
    { query: "root password mac mini" },
    { query: SECRET_KEY },
    { query: "hunter2" },
    { query: RAW_TOKEN },
    { query: "root password", include_superseded: true },
    { query: "password", tags: ["secret"] },
  ]) {
    const { text } = await call("recall", args);
    assert.doesNotMatch(text, new RegExp(SECRET_KEY), `secret-tagged key leaked for ${JSON.stringify(args)}`);
    assert.doesNotMatch(text, /hunter2/, `secret body leaked for ${JSON.stringify(args)}`);
    assert.doesNotMatch(text, new RegExp(RAW_TOKEN), `raw token leaked for ${JSON.stringify(args)}`);
  }
});

// ── recall_full ──

test("recall_full on the partial-secret entry shows [REDACTED] and not the raw token; header is one line", async () => {
  const { text, isError } = await call("recall_full", { key: PARTIAL_SECRET_KEY });
  assert.equal(isError, false, text);
  const [header, ...body] = text.split("\n");
  assert.match(header, new RegExp(`^${PARTIAL_SECRET_KEY} · bytes_total=\\d+ · truncated=(yes|no) · supersedes=\\S+ · superseded_by=\\S+$`));
  const bodyText = body.join("\n");
  assert.match(bodyText, /\[REDACTED\]/);
  assert.doesNotMatch(text, new RegExp(RAW_TOKEN));
  assert.match(bodyText, /Dokploy API calls/);
});

test("recall_full honours max_bytes (truncated=yes + notice) and rejects out-of-range values", async () => {
  const { text, isError } = await call("recall_full", { key: "hub-self-heal-pr-46", max_bytes: 256 });
  assert.equal(isError, false, text);
  const body = FAKE_BANK["hub-self-heal-pr-46"];
  if (body.length > 256) assert.match(text.split("\n")[0], /truncated=yes/);
  else assert.match(text.split("\n")[0], /truncated=no/);
  const bad = await client.callTool({ name: "recall_full", arguments: { key: "hub-self-heal-pr-46", max_bytes: 10 } }).catch((e) => e);
  const badText = bad instanceof Error ? bad.message : bad.content.map((c) => c.text).join("");
  assert.ok((bad instanceof Error) || bad.isError === true, `max_bytes=10 must be rejected: ${badText}`);
});

test("recall_full on the secret-tagged key → ERROR, nothing of the body leaks; unknown key → same ERROR", async () => {
  for (const key of [SECRET_KEY, "no-such-memory-xyz"]) {
    const { text, isError } = await call("recall_full", { key });
    assert.equal(isError, true, `${key}: ${text}`);
    assert.equal(text, `ERROR: no such memory (or it is secret-excluded): ${key}`);
    assert.doesNotMatch(text, /hunter2/);
  }
});

// ── recall_stats ──

test("recall_stats shows counts (by project/tag, secret_excluded ≥ 1) and never the secret key or any body", async () => {
  const { text, isError } = await call("recall_stats");
  assert.equal(isError, false, text);
  assert.match(text, /^total=\d+ live=\d+ standing=\d+ superseded=\d+ secret_excluded=\d+ redacted_spans=\d+$/m);
  assert.match(text, /^index_built_at=\S+ cache_age_seconds=(\d+|none)$/m);
  assert.match(text, /^by_project: .*aperture=\d+/m);
  assert.match(text, /^by_project: .*incluir=1/m);
  assert.match(text, /^by_tag: .*comms=1/m);
  const secretExcluded = Number(text.match(/secret_excluded=(\d+)/)[1]);
  assert.ok(secretExcluded >= 1, "the sidecar-tagged secret entry must be counted as excluded");
  assert.doesNotMatch(text, new RegExp(SECRET_KEY));
  assert.doesNotMatch(text, /hunter2/);
  assert.doesNotMatch(text, new RegExp(RAW_TOKEN));
  assert.doesNotMatch(text, /self-heals on reconnect/, "stats must not carry bodies");
});

// ── memory-recall.js hook ──

test("hook: piped UserPromptSubmit payload → header + ≤ 3 `- key · gist (age)` lines, ≤ 600 bytes, exit 0", async () => {
  const r = await runHook({ payload: hookPayload("why does the hub replay messages after a reconnect?"), env: HOOK_ENV });
  assert.equal(r.code, 0, `exit=${r.code} signal=${r.signal} stderr=${r.stderr}`);
  const lines = r.stdout.trimEnd().split("\n");
  assert.equal(lines[0], "[memory recall] top matches for this prompt — use recall_full(key) for detail:");
  const items = lines.slice(1);
  assert.ok(items.length >= 1 && items.length <= 3, `expected 1..3 item lines, got ${items.length}:\n${r.stdout}`);
  for (const l of items) assert.match(l, /^- \S+ · .+ \((\d+d|age\?)\)$/);
  assert.match(items[0], /^- hub-self-heal-pr-46 · /);
  assert.ok(Buffer.byteLength(r.stdout) <= 600, `stdout is ${Buffer.byteLength(r.stdout)} bytes`);
  assert.doesNotMatch(r.stdout, new RegExp(SECRET_KEY));
  assert.doesNotMatch(r.stdout, new RegExp(RAW_TOKEN));
});

test("hook: bead id in the prompt / APERTURE_ACTIVE_BEAD join the query terms", async () => {
  const r = await runHook({ payload: hookPayload("continue with the playwright fix from aperture-acr3t please"), env: HOOK_ENV });
  assert.equal(r.code, 0, r.stderr);
  assert.match(r.stdout, /^- playwright-tobevisible-overflow-clip · /m);
  const viaEnv = await runHook({ payload: hookPayload("keep going with the active bead"), env: { ...HOOK_ENV, APERTURE_ACTIVE_BEAD: "aperture-acr3t" } });
  assert.equal(viaEnv.code, 0, viaEnv.stderr);
  assert.match(viaEnv.stdout, /^- playwright-tobevisible-overflow-clip · /m);
});

test("hook: slash command prompt (/compact) → empty stdout, exit 0", async () => {
  const r = await runHook({ payload: hookPayload("/compact"), env: HOOK_ENV });
  assert.equal(r.code, 0);
  assert.equal(r.stdout, "");
  const r2 = await runHook({ payload: hookPayload("/loop 5m check the deploy status of everything"), env: HOOK_ENV });
  assert.equal(r2.code, 0);
  assert.equal(r2.stdout, "");
});

test("hook: prompt shorter than 12 chars → empty stdout, exit 0", async () => {
  const r = await runHook({ payload: hookPayload("ok go"), env: HOOK_ENV });
  assert.equal(r.code, 0);
  assert.equal(r.stdout, "");
});

test("hook: no APERTURE_HUB_TOKEN_FILE → empty stdout, exit 0, no bd call", async () => {
  const r = await runHook({ payload: hookPayload("why does the hub replay messages after a reconnect?"), env: {} });
  assert.equal(r.code, 0);
  assert.equal(r.stdout, "");
  assert.ok(r.ms < 1000, `should exit immediately, took ${r.ms}ms`);
});

test("hook: bd failing → `[recall unavailable: …]` on stdout, exit 0", async () => {
  setBdFailing(true);
  try {
    const r = await runHook({ payload: hookPayload("why does the hub replay messages after a reconnect?"), env: HOOK_ENV });
    assert.equal(r.code, 0, r.stderr);
    assert.match(r.stdout, /^\[recall unavailable: .+\]\n$/);
    assert.doesNotMatch(r.stdout, /\n.*\n.*\n/, "exactly one line");
  } finally {
    setBdFailing(false);
  }
});

test("hook: stdin never closes → proceeds after the 300ms grace with what was read, exit 0", async () => {
  const r = await runHook({ payload: hookPayload("why does the hub replay messages after a reconnect?"), env: HOOK_ENV, keepStdinOpen: true });
  assert.equal(r.code, 0, `exit=${r.code} signal=${r.signal} stderr=${r.stderr}`);
  assert.ok(r.ms < 3000, `hung on open stdin: ${r.ms}ms`);
  assert.match(r.stdout, /^(\[memory recall\]|\[recall unavailable: )/);
});

test("hook: hard timeout (APERTURE_RECALL_TIMEOUT_MS=1) → `[recall unavailable: timed out…]`, exit 0", async () => {
  const r = await runHook({ payload: hookPayload("why does the hub replay messages after a reconnect?"), env: { ...HOOK_ENV, APERTURE_RECALL_TIMEOUT_MS: "1" } });
  assert.equal(r.code, 0, r.stderr);
  assert.match(r.stdout, /^\[recall unavailable: timed out after 1ms\]\n$/);
});
