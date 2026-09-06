// aperture-oeb6q — reconnect/backoff/liveness suite for dist/hub-client.js.
//
// Pins the stdout lines agents' prompts key off, and the exit discipline:
//
//   1. hub closes 1001 (shutdown)  — HUB_RECONNECTING, re-hello on a new socket,
//                                    HUB_RECONNECTED, process still alive
//   2. hub closes 4000 (replaced)  — HUB_SOCKET_CLOSED code=4000, exit 0
//   3. hub closes 4001 (rejected)  — HUB_SOCKET_CLOSED code=4001, exit 1
//   4. nothing listening at start  — keeps retrying (≥2 attempts), connects
//                                    once a hub appears, never exits
//   5. wedged hub (no frames)      — HUB_SOCKET_STALE, reconnect, re-hello
//
// The "hub" here is a bare ws WebSocketServer in the test process — the client
// only needs a peer that accepts hello and closes with chosen codes, so the
// real hub (and bd/BEADS) stay out of the loop.
//
// Run: node --test test/hub-client-reconnect.test.mjs   (from mcp-server/, after pnpm build)

import { test } from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { WebSocketServer } from "ws";

const here = dirname(fileURLToPath(import.meta.url));
const clientPath = resolve(here, "..", "dist", "hub-client.js");

if (!existsSync(clientPath)) {
  throw new Error(
    `dist/hub-client.js not found at ${clientPath} — build first: cd mcp-server && pnpm build (or: just build-mcp)`,
  );
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const AGENT = "reconnect-test";
const TOKEN = "0a".repeat(32);
const randomPort = () => 20000 + Math.floor(Math.random() * 20000);

/**
 * Fake hub on `port`. `onConnection(ws, index)` runs per accepted socket
 * (index 0 = first). Records every hello frame in `hellos`.
 */
function startFakeHub(port, onConnection) {
  const wss = new WebSocketServer({ host: "127.0.0.1", port });
  const hellos = [];
  const waiters = [];
  let count = 0;
  wss.on("connection", (ws) => {
    const index = count++;
    ws.on("message", (data) => {
      let msg;
      try {
        msg = JSON.parse(data.toString());
      } catch {
        return;
      }
      if (msg.type === "hello") {
        hellos.push(msg);
        for (let i = waiters.length - 1; i >= 0; i--) {
          if (hellos.length >= waiters[i].n) {
            waiters[i].resolve();
            waiters.splice(i, 1);
          }
        }
        onConnection?.(ws, index, msg);
      }
    });
    ws.on("error", () => {});
  });
  const listening = new Promise((resolvePromise, reject) => {
    wss.once("listening", resolvePromise);
    wss.once("error", reject);
  });
  /** Resolve once at least `n` hellos have been received. */
  function waitForHellos(n, timeoutMs = 3000) {
    if (hellos.length >= n) return Promise.resolve();
    return new Promise((resolvePromise, reject) => {
      const timer = setTimeout(() => reject(new Error(`timed out waiting for hello #${n}`)), timeoutMs);
      waiters.push({
        n,
        resolve: () => {
          clearTimeout(timer);
          resolvePromise();
        },
      });
    });
  }
  function stop() {
    for (const ws of wss.clients) ws.terminate();
    wss.close();
  }
  return { wss, hellos, listening, waitForHellos, stop };
}

/**
 * Spawn dist/hub-client.js against `port` with fast backoff. Returns the live
 * stdout line array plus waitForLine / exit helpers.
 */
function spawnClient(port, extraEnv = {}) {
  const proc = spawn(process.execPath, [clientPath, AGENT], {
    env: {
      ...process.env,
      APERTURE_HUB_URL: `ws://127.0.0.1:${port}`,
      APERTURE_HUB_TOKEN: TOKEN,
      APERTURE_HUB_BACKOFF_CAP_MS: "50",
      ...extraEnv,
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  const lines = [];
  const waiters = [];
  let buf = "";
  proc.stdout.on("data", (d) => {
    buf += d.toString();
    let nl;
    while ((nl = buf.indexOf("\n")) !== -1) {
      const line = buf.slice(0, nl);
      buf = buf.slice(nl + 1);
      lines.push(line);
      for (let i = waiters.length - 1; i >= 0; i--) {
        if (waiters[i].pred(line)) {
          waiters[i].resolve(line);
          waiters.splice(i, 1);
        }
      }
    }
  });
  proc.stderr.on("data", () => {});
  const exited = new Promise((resolvePromise) => {
    proc.on("exit", (code) => resolvePromise(code));
  });

  /** Resolve with the first stdout line matching pred (past or future). */
  function waitForLine(pred, what, timeoutMs = 3000) {
    const already = lines.find(pred);
    if (already) return Promise.resolve(already);
    return new Promise((resolvePromise, reject) => {
      const timer = setTimeout(
        () => reject(new Error(`timed out waiting for stdout line: ${what}\nlines so far:\n${lines.join("\n")}`)),
        timeoutMs,
      );
      waiters.push({
        pred,
        resolve: (line) => {
          clearTimeout(timer);
          resolvePromise(line);
        },
      });
    });
  }
  function waitForExit(timeoutMs = 3000) {
    return Promise.race([
      exited,
      sleep(timeoutMs).then(() => {
        throw new Error(`client did not exit within ${timeoutMs}ms\nlines:\n${lines.join("\n")}`);
      }),
    ]);
  }
  function kill() {
    if (proc.exitCode === null) proc.kill("SIGKILL");
  }
  return { proc, lines, waitForLine, waitForExit, kill };
}

const startsWith = (prefix) => (line) => line.startsWith(prefix);

// ── 1. 1001 → reconnect ─────────────────────────────────────────────────────

test("hub close 1001: HUB_RECONNECTING, re-hello on new socket, HUB_RECONNECTED, still alive", async () => {
  const port = randomPort();
  const hub = startFakeHub(port, (ws, index) => {
    if (index === 0) ws.close(1001, "hub shutting down");
  });
  await hub.listening;
  const client = spawnClient(port);
  try {
    const reconnecting = await client.waitForLine(startsWith("HUB_RECONNECTING"), "HUB_RECONNECTING");
    assert.match(reconnecting, /^HUB_RECONNECTING code=1001 reason=hub shutting down — will retry with backoff, no action needed$/);

    await hub.waitForHellos(2);
    const reconnected = await client.waitForLine(startsWith("HUB_RECONNECTED"), "HUB_RECONNECTED");
    assert.match(reconnected, /^HUB_RECONNECTED after 1 attempt\(s\), [\d.]+s — unread messages replay now$/);

    // Both sockets sent the identical hello.
    assert.equal(hub.hellos.length, 2, "exactly two hellos (one per socket)");
    for (const h of hub.hellos) {
      assert.deepEqual(h, { type: "hello", role: "agent", agent: AGENT, token: TOKEN });
    }

    await sleep(150);
    assert.equal(client.proc.exitCode, null, "client process still alive after reconnect");
    assert.equal(
      client.lines.filter(startsWith("HUB_RECONNECTING")).length,
      1,
      "HUB_RECONNECTING printed exactly once for the outage",
    );
    assert.equal(client.lines.filter(startsWith("HUB_SOCKET_CLOSED")).length, 0, "no HUB_SOCKET_CLOSED on a retryable close");
  } finally {
    client.kill();
    hub.stop();
  }
});

// ── 2. 4000 → exit 0 ────────────────────────────────────────────────────────

test("hub close 4000 (replaced): HUB_SOCKET_CLOSED code=4000, exit 0, no reconnect", async () => {
  const port = randomPort();
  const hub = startFakeHub(port, (ws) => ws.close(4000, "replaced by newer connection"));
  await hub.listening;
  const client = spawnClient(port);
  try {
    const closed = await client.waitForLine(startsWith("HUB_SOCKET_CLOSED"), "HUB_SOCKET_CLOSED");
    assert.match(closed, /^HUB_SOCKET_CLOSED code=4000 reason=replaced by newer connection — /);
    assert.match(closed, /do not restart/);
    const code = await client.waitForExit();
    assert.equal(code, 0, "exit code 0 on 4000");
    await sleep(150);
    assert.equal(hub.hellos.length, 1, "no reconnect attempt after 4000");
    assert.equal(client.lines.filter(startsWith("HUB_RECONNECTING")).length, 0, "no HUB_RECONNECTING on 4000");
  } finally {
    client.kill();
    hub.stop();
  }
});

// ── 3. 4001 → exit 1 ────────────────────────────────────────────────────────

test("hub close 4001 (hello rejected): HUB_SOCKET_CLOSED code=4001, exit 1, no reconnect", async () => {
  const port = randomPort();
  const hub = startFakeHub(port, (ws) => ws.close(4001, "expected hello"));
  await hub.listening;
  const client = spawnClient(port);
  try {
    const closed = await client.waitForLine(startsWith("HUB_SOCKET_CLOSED"), "HUB_SOCKET_CLOSED");
    assert.equal(
      closed,
      "HUB_SOCKET_CLOSED code=4001 reason=expected hello — hello rejected (token or agent name); fix and restart your inbox monitor",
    );
    const code = await client.waitForExit();
    assert.equal(code, 1, "exit code 1 on 4001");
    await sleep(150);
    assert.equal(hub.hellos.length, 1, "no reconnect attempt after 4001");
  } finally {
    client.kill();
    hub.stop();
  }
});

// ── 4. nothing listening at startup → keeps retrying ────────────────────────

test("hub unreachable at startup: keeps retrying (≥2 attempts), connects once hub appears, never exits", async () => {
  const port = randomPort();
  const client = spawnClient(port);
  let hub = null;
  try {
    const reconnecting = await client.waitForLine(startsWith("HUB_RECONNECTING"), "HUB_RECONNECTING");
    assert.match(reconnecting, /^HUB_RECONNECTING code=error reason=connect ECONNREFUSED /);

    // With cap=50ms the client retries ~every 50ms; give it several attempts
    // against the dead port before a hub shows up.
    await sleep(400);
    assert.equal(client.proc.exitCode, null, "client still alive while hub unreachable");
    assert.equal(
      client.lines.filter(startsWith("HUB_RECONNECTING")).length,
      1,
      "repeated failed attempts stay silent (one HUB_RECONNECTING only)",
    );

    hub = startFakeHub(port);
    await hub.listening;
    await hub.waitForHellos(1);
    const reconnected = await client.waitForLine(startsWith("HUB_RECONNECTED"), "HUB_RECONNECTED");
    const m = reconnected.match(/^HUB_RECONNECTED after (\d+) attempt\(s\), [\d.]+s — unread messages replay now$/);
    assert.ok(m, `HUB_RECONNECTED line well-formed: ${reconnected}`);
    assert.ok(Number(m[1]) >= 2, `at least two connection attempts were made (saw ${m[1]})`);
    assert.deepEqual(hub.hellos[0], { type: "hello", role: "agent", agent: AGENT, token: TOKEN });

    await sleep(150);
    assert.equal(client.proc.exitCode, null, "client still alive after recovery");
  } finally {
    client.kill();
    hub?.stop();
  }
});

// ── 5. wedged hub → stale → reconnect ───────────────────────────────────────

test("stale detection: no frames for APERTURE_HUB_STALE_MS → HUB_SOCKET_STALE, reconnect, re-hello", async () => {
  const port = randomPort();
  // Fake hub accepts hello and never sends anything (no pings either).
  const hub = startFakeHub(port);
  await hub.listening;
  const client = spawnClient(port, { APERTURE_HUB_STALE_MS: "300" });
  try {
    await hub.waitForHellos(1);
    const stale = await client.waitForLine(startsWith("HUB_SOCKET_STALE"), "HUB_SOCKET_STALE");
    assert.equal(stale, "HUB_SOCKET_STALE no frame for 0.3s — reconnecting");

    const reconnecting = await client.waitForLine(startsWith("HUB_RECONNECTING"), "HUB_RECONNECTING after stale");
    assert.match(reconnecting, /^HUB_RECONNECTING code=1006 reason= — will retry with backoff, no action needed$/);

    await hub.waitForHellos(2);
    await client.waitForLine(startsWith("HUB_RECONNECTED"), "HUB_RECONNECTED after stale");
    assert.deepEqual(hub.hellos[1], { type: "hello", role: "agent", agent: AGENT, token: TOKEN });
    assert.equal(client.proc.exitCode, null, "client still alive after stale reconnect");

    const staleIdx = client.lines.findIndex(startsWith("HUB_SOCKET_STALE"));
    const reconnIdx = client.lines.findIndex(startsWith("HUB_RECONNECTING"));
    assert.ok(staleIdx < reconnIdx, "HUB_SOCKET_STALE precedes HUB_RECONNECTING");
  } finally {
    client.kill();
    hub.stop();
  }
});
