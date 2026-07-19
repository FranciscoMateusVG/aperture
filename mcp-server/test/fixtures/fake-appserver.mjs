/**
 * Fake Codex app-server for bridge tests (codex-bind-order.test.mjs).
 *
 * A WebSocket JSON-RPC 2.0 server on a unix socket, mimicking the surface
 * CodexBridgeClient touches: initialize/initialized, thread/list,
 * thread/resume, thread/start, turn/start, turn/steer, plus server→client
 * turn notifications. Modeled on scripts/smoke-codex-bridge.mjs's protocol
 * observations, but fully scripted — no codex-cli involved.
 *
 * Scriptable knobs:
 *   threads              — initial thread list (thread/list returns { data: [...] })
 *   delays[method] = ms  — delay the RESPONSE to that method (call is still
 *                          logged at arrival time)
 *   failures[method] = n — respond with a JSON-RPC error to the next n calls
 *                          of that method
 *   notify(method, params) — push a notification to every connected client
 *
 * Every inbound frame (requests AND client notifications like `initialized`)
 * is appended to `calls` in arrival order with a timestamp, so tests can
 * assert protocol ordering (e.g. turn/start strictly after thread/resume).
 */
import { createServer } from "node:http";
import { rmSync } from "node:fs";
import { WebSocketServer } from "ws";

function sleep(ms) {
  return new Promise((res) => setTimeout(res, ms));
}

export class FakeAppServer {
  constructor(sockPath, opts = {}) {
    this.sockPath = sockPath;
    this.threads = opts.threads ?? []; // [{ id: "..." }]
    this.delays = opts.delays ?? {}; // method → ms
    this.failures = opts.failures ?? {}; // method → remaining error count
    /** @type {{method: string, params: unknown, id: number|string|null, ts: number}[]} */
    this.calls = [];
    this.sockets = new Set();
    this.threadCounter = 0;
    this.http = null;
    this.wss = null;
  }

  async start() {
    try {
      rmSync(this.sockPath, { force: true });
    } catch {
      /* fresh path */
    }
    this.http = createServer();
    this.wss = new WebSocketServer({ server: this.http });
    this.wss.on("connection", (ws) => {
      this.sockets.add(ws);
      ws.on("close", () => this.sockets.delete(ws));
      ws.on("message", (data) => {
        void this.onMessage(ws, data.toString());
      });
    });
    await new Promise((res, rej) => {
      this.http.once("error", rej);
      this.http.listen(this.sockPath, res);
    });
  }

  async onMessage(ws, raw) {
    let msg;
    try {
      msg = JSON.parse(raw);
    } catch {
      return;
    }
    if (!msg || typeof msg.method !== "string") return;
    // Log at ARRIVAL time so `calls` order == wire order regardless of delays.
    this.calls.push({
      method: msg.method,
      params: msg.params ?? null,
      id: msg.id ?? null,
      ts: Date.now(),
    });
    if (msg.id === undefined || msg.id === null) return; // client notification (e.g. initialized)

    const delayMs = this.delays[msg.method] ?? 0;
    if (delayMs > 0) await sleep(delayMs);

    if ((this.failures[msg.method] ?? 0) > 0) {
      this.failures[msg.method] -= 1;
      this.send(ws, {
        jsonrpc: "2.0",
        id: msg.id,
        error: { code: -32000, message: `scripted failure for ${msg.method}` },
      });
      return;
    }

    let result = {};
    switch (msg.method) {
      case "thread/list":
        result = { data: this.threads };
        break;
      case "thread/start": {
        const id = `t-fake-${++this.threadCounter}`;
        this.threads.unshift({ id });
        result = { threadId: id };
        break;
      }
      // initialize, thread/resume, turn/start, turn/steer → empty result
      default:
        result = {};
    }
    this.send(ws, { jsonrpc: "2.0", id: msg.id, result });
  }

  send(ws, obj) {
    if (ws.readyState === ws.OPEN) ws.send(JSON.stringify(obj));
  }

  /** Push a server→client notification (turn/started, turn/completed, …). */
  notify(method, params = {}) {
    for (const ws of this.sockets) {
      this.send(ws, { jsonrpc: "2.0", method, params });
    }
  }

  callsOf(method) {
    return this.calls.filter((c) => c.method === method);
  }

  /** Index in the arrival-ordered log of the first call matching (method, pred). */
  indexOf(method, pred = () => true) {
    return this.calls.findIndex((c) => c.method === method && pred(c));
  }

  /** All turn/start + turn/steer calls whose params mention `needle`. */
  turnCallsContaining(needle) {
    return this.calls.filter(
      (c) =>
        (c.method === "turn/start" || c.method === "turn/steer") &&
        JSON.stringify(c.params ?? {}).includes(needle),
    );
  }

  async close() {
    for (const ws of this.sockets) {
      try {
        ws.terminate();
      } catch {
        /* already gone */
      }
    }
    this.sockets.clear();
    await new Promise((res) => (this.wss ? this.wss.close(() => res()) : res()));
    await new Promise((res) => (this.http ? this.http.close(() => res()) : res()));
    try {
      rmSync(this.sockPath, { force: true });
    } catch {
      /* fine */
    }
  }
}
