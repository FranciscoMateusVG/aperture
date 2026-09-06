// aperture-trgpo — hook-safety suite for dist/presence-hint.js.
//
// presence-hint runs as a Claude Code hook (UserPromptSubmit / PreToolUse →
// busy, Stop → idle) and must be fast and must never fail the session. Pins:
//
//   1. busy   — hello {role:"producer", agent:"vex", token:<file bytes>} then
//               {type:"presence_hint", event:"busy"}; exit 0 < 500ms, empty stdout
//   2. idle   — same, event:"idle"
//   3. no APERTURE_HUB_TOKEN_FILE — exit 0 immediately, NO connection attempt
//   4. nothing listening — exit 0 within 300ms (ECONNREFUSED, not the timeout)
//   5. bad argv (sleepy) — exit 0, NO connection attempt, one stderr line
//   6. hub never acks — exit 0 at ~APERTURE_HINT_TIMEOUT_MS (150)
//
// Every child is spawned with stdin left OPEN and a JSON payload written to it
// (as Claude Code does) to prove the process cannot hang on stdin.
//
// The "hub" is a bare ws WebSocketServer in the test process: it records the
// frames and acks presence_hint with {type:"ok"} (unless told not to).
//
// Run: node --test test/presence-hint.test.mjs   (from mcp-server/, after pnpm build)

import { test } from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { existsSync, mkdtempSync, mkdirSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { WebSocketServer } from "ws";

const here = dirname(fileURLToPath(import.meta.url));
const hintPath = resolve(here, "..", "dist", "presence-hint.js");

if (!existsSync(hintPath)) {
  throw new Error(
    `dist/presence-hint.js not found at ${hintPath} — build first: cd mcp-server && pnpm build (or: just build-mcp)`,
  );
}

const AGENT = "vex";
const TOKEN = "ab".repeat(32);
const randomPort = () => 20000 + Math.floor(Math.random() * 20000);

/** Temp token dir shaped like the launcher's: <tmp>/hub-tokens/<agent>.token */
function makeTokenFile() {
  const root = mkdtempSync(join(tmpdir(), "presence-hint-"));
  const dir = join(root, "hub-tokens");
  mkdirSync(dir);
  const file = join(dir, `${AGENT}.token`);
  writeFileSync(file, TOKEN, { mode: 0o600 });
  return { root, file };
}

/**
 * Fake hub on `port`. Records every frame per connection in `frames`,
 * counts connections, and acks presence_hint with {type:"ok"} when `ack`.
 */
function startFakeHub(port, { ack = true } = {}) {
  const wss = new WebSocketServer({ host: "127.0.0.1", port, perMessageDeflate: false });
  const frames = [];
  let connections = 0;
  wss.on("connection", (ws) => {
    connections++;
    ws.on("message", (data) => {
      let msg;
      try {
        msg = JSON.parse(data.toString());
      } catch {
        return;
      }
      frames.push(msg);
      if (ack && msg.type === "presence_hint") ws.send(JSON.stringify({ type: "ok" }));
    });
  });
  const ready = new Promise((r) => wss.once("listening", r));
  return {
    ready,
    frames,
    get connections() {
      return connections;
    },
    close: () =>
      new Promise((r) => {
        for (const c of wss.clients) c.terminate();
        wss.close(() => r());
      }),
  };
}

/**
 * Spawn dist/presence-hint.js with the given argv + env. stdin is piped and
 * left open with a hook-style JSON payload written to it. Resolves with
 * exit code, captured stdout/stderr, and wall-clock ms from spawn to exit.
 */
function runHint(args, env) {
  const childEnv = { ...process.env };
  delete childEnv.APERTURE_HUB_TOKEN_FILE;
  delete childEnv.APERTURE_HUB_URL;
  delete childEnv.APERTURE_HINT_TIMEOUT_MS;
  Object.assign(childEnv, env);
  return new Promise((resolveRun) => {
    const started = Date.now();
    const child = spawn(process.execPath, [hintPath, ...args], {
      env: childEnv,
      stdio: ["pipe", "pipe", "pipe"],
    });
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", (d) => (stdout += d.toString()));
    child.stderr.on("data", (d) => (stderr += d.toString()));
    // Claude Code pipes a JSON payload; never end() it — the hook must not care.
    child.stdin.on("error", () => {});
    child.stdin.write(JSON.stringify({ hook_event_name: "PreToolUse", session_id: "test" }));
    child.on("exit", (code) => {
      resolveRun({ code, stdout, stderr, ms: Date.now() - started });
    });
  });
}

const wait = (ms) => new Promise((r) => setTimeout(r, ms));

for (const event of ["busy", "idle"]) {
  test(`${event}: sends producer hello + presence_hint, exits 0 < 500ms, empty stdout`, async () => {
    const port = randomPort();
    const hub = startFakeHub(port);
    await hub.ready;
    const { root, file } = makeTokenFile();
    try {
      const r = await runHint([event], {
        APERTURE_HUB_TOKEN_FILE: file,
        APERTURE_HUB_URL: `ws://127.0.0.1:${port}`,
      });
      assert.equal(r.code, 0, `exit code (stderr: ${r.stderr})`);
      assert.equal(r.stdout, "", "no stdout on success");
      assert.equal(r.stderr, "", "no stderr on success");
      assert.ok(r.ms < 500, `expected < 500ms, took ${r.ms}ms`);
      assert.equal(hub.connections, 1, "exactly one connection");
      assert.deepEqual(hub.frames, [
        { type: "hello", role: "producer", agent: AGENT, token: TOKEN },
        { type: "presence_hint", event },
      ]);
    } finally {
      await hub.close();
      rmSync(root, { recursive: true, force: true });
    }
  });
}

test("no APERTURE_HUB_TOKEN_FILE: exits 0 immediately, no connection attempt", async () => {
  const port = randomPort();
  const hub = startFakeHub(port);
  await hub.ready;
  try {
    const r = await runHint(["busy"], { APERTURE_HUB_URL: `ws://127.0.0.1:${port}` });
    assert.equal(r.code, 0);
    assert.equal(r.stdout, "");
    assert.equal(r.stderr, "", "silent no-op");
    assert.ok(r.ms < 300, `expected immediate exit, took ${r.ms}ms`);
    await wait(50); // give a stray connect a chance to show up
    assert.equal(hub.connections, 0, "must not connect");
    assert.deepEqual(hub.frames, []);
  } finally {
    await hub.close();
  }
});

test("nothing listening on the port: exits 0 within 300ms (not the timeout)", async () => {
  const port = randomPort();
  const { root, file } = makeTokenFile();
  try {
    const r = await runHint(["busy"], {
      APERTURE_HUB_TOKEN_FILE: file,
      APERTURE_HUB_URL: `ws://127.0.0.1:${port}`,
      APERTURE_HINT_TIMEOUT_MS: "5000", // prove exit is driven by ECONNREFUSED
    });
    assert.equal(r.code, 0);
    assert.equal(r.stdout, "");
    assert.ok(r.ms < 300, `expected < 300ms, took ${r.ms}ms`);
    const lines = r.stderr.split("\n").filter(Boolean);
    assert.equal(lines.length, 1, `exactly one stderr line, got: ${JSON.stringify(r.stderr)}`);
    assert.match(lines[0], /^\[presence-hint\] hub unavailable: /);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("bad argv (sleepy): exits 0, no connection, one stderr line", async () => {
  const port = randomPort();
  const hub = startFakeHub(port);
  await hub.ready;
  const { root, file } = makeTokenFile();
  try {
    const r = await runHint(["sleepy"], {
      APERTURE_HUB_TOKEN_FILE: file,
      APERTURE_HUB_URL: `ws://127.0.0.1:${port}`,
    });
    assert.equal(r.code, 0);
    assert.equal(r.stdout, "");
    const lines = r.stderr.split("\n").filter(Boolean);
    assert.equal(lines.length, 1, `exactly one stderr line, got: ${JSON.stringify(r.stderr)}`);
    assert.match(lines[0], /^\[presence-hint\] ignoring unknown event "sleepy"/);
    await wait(50);
    assert.equal(hub.connections, 0, "must not connect");
  } finally {
    await hub.close();
    rmSync(root, { recursive: true, force: true });
  }
});

test("hub never acks: exits 0 at ~APERTURE_HINT_TIMEOUT_MS (150)", async () => {
  const port = randomPort();
  const hub = startFakeHub(port, { ack: false });
  await hub.ready;
  const { root, file } = makeTokenFile();
  try {
    const r = await runHint(["idle"], {
      APERTURE_HUB_TOKEN_FILE: file,
      APERTURE_HUB_URL: `ws://127.0.0.1:${port}`,
      APERTURE_HINT_TIMEOUT_MS: "150",
    });
    assert.equal(r.code, 0);
    assert.equal(r.stdout, "");
    assert.ok(r.ms >= 150, `must wait out the timeout, took ${r.ms}ms`);
    assert.ok(r.ms < 600, `should exit near the 150ms timeout, took ${r.ms}ms`);
    const lines = r.stderr.split("\n").filter(Boolean);
    assert.equal(lines.length, 1, `exactly one stderr line, got: ${JSON.stringify(r.stderr)}`);
    assert.match(lines[0], /^\[presence-hint\] timed out after 150ms/);
    assert.deepEqual(hub.frames, [
      { type: "hello", role: "producer", agent: AGENT, token: TOKEN },
      { type: "presence_hint", event: "idle" },
    ]);
  } finally {
    await hub.close();
    rmSync(root, { recursive: true, force: true });
  }
});
