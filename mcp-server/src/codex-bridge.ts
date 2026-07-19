/**
 * Aperture comms-layer v2 — Phase 2 Codex bridge (Protocol 2, "steered turns").
 *
 * The ws-hub owns one CodexBridgeClient per Codex agent. Each client speaks
 * JSON-RPC 2.0 over a WebSocket on the agent's app-server unix socket
 * (~/.aperture/run/<agent>.sock, spawned by Tauri as
 * `codex app-server --listen unix://<path>`):
 *
 *   connect → initialize → initialized → thread/list → bind newest thread
 *   (retry until the TUI creates one) → thread/resume → presence "join"
 *   → unread BEADS replay as an injected turn.
 *
 * Delivery: full message bodies are fetched from BEADS (same unread query the
 * hub replay uses) and injected via `turn/start` when the thread is idle,
 * `turn/steer` when a turn is active. An in-memory delivered-set per agent
 * (keyed by message id) prevents double-injection within one hub lifetime;
 * BEADS unread state remains the durable truth — the agent acks with
 * mark_as_read via its aperture-bus MCP tools.
 *
 * Presence: connected+bound = present (join/leave), turn activity = busy/idle,
 * broadcast through the same hub presence channel as Claude Monitor sockets.
 *
 * Reconnect: socket missing or dropped is normal (agent not running). Retry
 * every 10s with quiet logs — one "offline" line per outage, not per attempt.
 *
 * Env:
 *   APERTURE_AGENTS_DIR — manifest tree (default ~/.claude/aperture)
 *   APERTURE_RUN_DIR    — socket dir    (default ~/.aperture/run)
 */
import { readdirSync, readFileSync } from "node:fs";
import { homedir } from "node:os";
import { join, resolve } from "node:path";
import WebSocket from "ws";
import { getUnreadMessages } from "./beads.js";

const AGENTS_DIR = process.env.APERTURE_AGENTS_DIR ?? resolve(homedir(), ".claude", "aperture");
const RUN_DIR = process.env.APERTURE_RUN_DIR ?? resolve(homedir(), ".aperture", "run");

const RECONNECT_MS = 10_000; // socket missing/dropped → retry cadence
const THREAD_POLL_MS = 10_000; // no thread yet (TUI not started) → re-list cadence
const RPC_TIMEOUT_MS = 10_000; // control-plane calls (initialize, thread/*)
const TURN_RPC_TIMEOUT_MS = 600_000; // turn/start may not respond until the turn ends
const DISCOVERY_MS = 60_000; // manifest re-scan cadence

export type PresenceEvent = "join" | "leave" | "busy" | "idle";

export interface BridgeHooks {
  broadcastPresence: (agent: string, event: PresenceEvent) => void;
  log: (event: string, fields?: Record<string, unknown>) => void;
  /** Testing hook: skip the BEADS unread replay/delivery path. */
  skipReplay?: boolean;
  /** Testing hook: observe raw JSON-RPC notifications from the app-server. */
  onNotification?: (agent: string, method: string, params: unknown) => void;
}

export interface BridgeOptions {
  /** Bind the newest thread + replay unread after initialize (default true).
   *  The smoke test disables this and drives thread/start manually. */
  autoBind?: boolean;
}

/** Codex agents = dirs under the aperture tree whose manifest model starts with "codex/". */
export function discoverCodexAgents(dir: string = AGENTS_DIR): string[] {
  let entries: string[];
  try {
    entries = readdirSync(dir);
  } catch {
    return [];
  }
  const out: string[] = [];
  for (const name of entries) {
    try {
      const raw = readFileSync(join(dir, name, "manifest.json"), "utf8");
      const manifest = JSON.parse(raw) as Record<string, unknown>;
      if (typeof manifest.model === "string" && manifest.model.startsWith("codex/")) {
        out.push(name);
      }
    } catch {
      // no manifest / unreadable / not a dir — not an agent dir, skip
    }
  }
  return out;
}

interface Pending {
  method: string;
  resolve: (v: unknown) => void;
  reject: (e: Error) => void;
  timer: NodeJS.Timeout;
}

interface JsonRpcError {
  code?: number;
  message?: string;
}

function formatBeadsMessage(row: Record<string, unknown>): string {
  const id = typeof row.id === "string" ? row.id : "unknown";
  const title = typeof row.title === "string" ? row.title : "";
  const from = title.match(/\[(.+?)->(.+?)\]/)?.[1] ?? "unknown";
  // Full body: description is the full message content (title only holds a preview).
  const body =
    typeof row.description === "string" && row.description.length > 0
      ? row.description
      : title.replace(/^\[.+?->.+?\]\s*/, "");
  return (
    `[BEADS message from ${from} | id ${id}] ${body}` +
    ` — reply via your aperture-bus MCP tools (get_messages / send_message);` +
    ` ack with mark_as_read after processing.`
  );
}

export class CodexBridgeClient {
  readonly agent: string;
  readonly sockPath: string;

  private readonly hooks: BridgeHooks;
  private readonly autoBind: boolean;

  private ws: WebSocket | null = null;
  private nextId = 1;
  private readonly pending = new Map<number, Pending>();

  private threadId: string | null = null;
  private turnActive = false;
  private joined = false;
  private stopped = false;
  private offlineLogged = false;
  private initialized = false;

  private reconnectTimer: NodeJS.Timeout | null = null;
  private readyWaiters: Array<(v: void) => void> = [];

  /** Message ids injected during this hub lifetime (double-injection guard). */
  private readonly delivered = new Set<string>();
  /** Serializes deliverUnread runs so a notify during replay can't double-inject. */
  private deliverChain: Promise<void> = Promise.resolve();

  constructor(agent: string, sockPath: string, hooks: BridgeHooks, opts?: BridgeOptions) {
    this.agent = agent;
    this.sockPath = sockPath;
    this.hooks = hooks;
    this.autoBind = opts?.autoBind ?? true;
  }

  get isBound(): boolean {
    return this.joined;
  }

  get isTurnActive(): boolean {
    return this.turnActive;
  }

  get boundThreadId(): string | null {
    return this.threadId;
  }

  start(): void {
    this.stopped = false;
    this.connect();
  }

  stop(): void {
    this.stopped = true;
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
    const ws = this.ws;
    this.ws = null;
    try {
      ws?.close(1001, "bridge stopping");
    } catch {
      // already closed
    }
    this.failAllPending(new Error("bridge stopped"));
  }

  /** Resolves once the socket is connected and `initialize` has completed. */
  waitReady(timeoutMs = 15_000): Promise<void> {
    if (this.initialized) return Promise.resolve();
    return new Promise((res, rej) => {
      const timer = setTimeout(() => rej(new Error("waitReady timed out")), timeoutMs);
      timer.unref?.();
      this.readyWaiters.push(() => {
        clearTimeout(timer);
        res();
      });
    });
  }

  /** Start a fresh thread on the app-server (used by the smoke test). */
  async startThread(params: Record<string, unknown> = {}): Promise<string> {
    const result = (await this.request("thread/start", params)) as Record<string, unknown> | null;
    const direct = result?.threadId ?? result?.id;
    if (typeof direct === "string") return direct;
    const wrapped = result?.thread;
    if (wrapped && typeof wrapped === "object") {
      const id = (wrapped as Record<string, unknown>).id;
      if (typeof id === "string") return id;
    }
    throw new Error(`thread/start returned no thread id: ${JSON.stringify(result)}`);
  }

  /**
   * Deliver all unread BEADS messages for this agent as one injected turn.
   * Called on bind (reconnect-replay) and on every hub notify. Serialized;
   * idempotent within this hub lifetime via the delivered-set.
   */
  deliver(): void {
    this.deliverChain = this.deliverChain.then(() => this.deliverUnread());
  }

  // ── connection lifecycle ──

  private connect(): void {
    if (this.stopped) return;
    let ws: WebSocket;
    try {
      // node `ws` unix-socket connect: ws+unix://<socket path>:<request path>.
      // perMessageDeflate MUST be off — codex's WS server hangs up on the
      // permessage-deflate extension offer (verified against codex-cli 0.144.6).
      ws = new WebSocket(`ws+unix://${this.sockPath}:/`, { perMessageDeflate: false });
    } catch (e: unknown) {
      this.logOffline(e instanceof Error ? e.message : String(e));
      this.scheduleReconnect();
      return;
    }
    this.ws = ws;

    ws.on("open", () => {
      if (this.ws !== ws) return;
      void this.handshake(ws);
    });
    ws.on("message", (data) => {
      if (this.ws !== ws) return;
      this.onMessage(data.toString());
    });
    ws.on("error", (err) => {
      // ECONNREFUSED / ENOENT while the agent isn't running is normal.
      this.logOffline(err.message);
    });
    ws.on("close", () => {
      if (this.ws !== ws) return;
      this.onClose();
    });
  }

  private async handshake(ws: WebSocket): Promise<void> {
    this.offlineLogged = false;
    this.hooks.log("codex_connected", { agent: this.agent, sock: this.sockPath });
    try {
      await this.request("initialize", {
        clientInfo: { name: "aperture-bus", title: "aperture-bus", version: "2.0" },
      });
      this.notifyServer("initialized", {});
      this.initialized = true;
      for (const w of this.readyWaiters.splice(0)) w();
      if (this.autoBind) await this.bindThread(ws);
    } catch (e: unknown) {
      this.hooks.log("codex_handshake_error", {
        agent: this.agent,
        error: e instanceof Error ? e.message : String(e),
      });
      try {
        ws.close();
      } catch {
        // ignore
      }
    }
  }

  /** thread/list until a thread exists (the TUI creates it), then resume + join. */
  private async bindThread(ws: WebSocket): Promise<void> {
    while (this.ws === ws && ws.readyState === WebSocket.OPEN && !this.stopped) {
      const result = (await this.request("thread/list", { limit: 5 })) as
        | Record<string, unknown>
        | null;
      const data = result?.data;
      const threads = Array.isArray(data) ? (data as Record<string, unknown>[]) : [];
      const newest = threads.find((t) => typeof t.id === "string");
      if (newest) {
        const threadId = newest.id as string;
        await this.request("thread/resume", { threadId });
        this.threadId = threadId;
        this.joined = true;
        this.hooks.log("codex_bound", { agent: this.agent, threadId });
        this.hooks.broadcastPresence(this.agent, "join");
        if (!this.hooks.skipReplay) this.deliver();
        return;
      }
      this.hooks.log("codex_no_thread_yet", { agent: this.agent });
      await delay(THREAD_POLL_MS);
    }
  }

  private onClose(): void {
    this.ws = null;
    this.initialized = false;
    this.threadId = null;
    this.failAllPending(new Error("socket closed"));
    this.setTurnActive(false);
    if (this.joined) {
      this.joined = false;
      this.hooks.log("codex_disconnected", { agent: this.agent });
      this.hooks.broadcastPresence(this.agent, "leave");
    }
    this.scheduleReconnect();
  }

  private scheduleReconnect(): void {
    if (this.stopped || this.reconnectTimer) return;
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = null;
      this.connect();
    }, RECONNECT_MS);
    this.reconnectTimer.unref?.();
  }

  /** One quiet log line per outage, not one per 10s retry. */
  private logOffline(reason: string): void {
    if (this.offlineLogged) return;
    this.offlineLogged = true;
    this.hooks.log("codex_offline", { agent: this.agent, sock: this.sockPath, reason });
  }

  // ── JSON-RPC plumbing ──

  request(method: string, params: Record<string, unknown> = {}, timeoutMs?: number): Promise<unknown> {
    const ws = this.ws;
    if (!ws || ws.readyState !== WebSocket.OPEN) {
      return Promise.reject(new Error(`not connected (${method})`));
    }
    const id = this.nextId++;
    const ms = timeoutMs ?? (method.startsWith("turn/") ? TURN_RPC_TIMEOUT_MS : RPC_TIMEOUT_MS);
    return new Promise((resolvePromise, rejectPromise) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        rejectPromise(new Error(`${method} timed out after ${ms}ms`));
      }, ms);
      timer.unref?.();
      this.pending.set(id, { method, resolve: resolvePromise, reject: rejectPromise, timer });
      ws.send(JSON.stringify({ jsonrpc: "2.0", id, method, params }));
    });
  }

  private notifyServer(method: string, params: Record<string, unknown>): void {
    const ws = this.ws;
    if (ws && ws.readyState === WebSocket.OPEN) {
      ws.send(JSON.stringify({ jsonrpc: "2.0", method, params }));
    }
  }

  private failAllPending(err: Error): void {
    for (const p of this.pending.values()) {
      clearTimeout(p.timer);
      p.reject(err);
    }
    this.pending.clear();
  }

  private onMessage(raw: string): void {
    let msg: Record<string, unknown>;
    try {
      const parsed = JSON.parse(raw);
      if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return;
      msg = parsed as Record<string, unknown>;
    } catch {
      return;
    }

    // Response to one of our requests.
    if (typeof msg.id === "number" && this.pending.has(msg.id)) {
      const p = this.pending.get(msg.id)!;
      this.pending.delete(msg.id);
      clearTimeout(p.timer);
      if (msg.error) {
        const err = msg.error as JsonRpcError;
        p.reject(new Error(`${p.method}: ${err.message ?? JSON.stringify(msg.error)}`));
      } else {
        p.resolve(msg.result ?? null);
      }
      return;
    }

    // Server → client notification (turn/thread state).
    if (typeof msg.method === "string") {
      this.onNotification(msg.method, msg.params);
    }
  }

  private onNotification(method: string, params: unknown): void {
    this.hooks.onNotification?.(this.agent, method, params);
    const p = (params && typeof params === "object" ? params : {}) as Record<string, unknown>;

    // Scope to our bound thread when the notification carries a threadId.
    const tid = p.threadId ?? p.thread_id;
    if (this.threadId && typeof tid === "string" && tid !== this.threadId) return;

    switch (method) {
      case "turn/started":
        this.setTurnActive(true);
        return;
      case "turn/completed":
      case "turn/failed":
      case "turn/aborted":
        this.setTurnActive(false);
        return;
      case "thread/status/changed": {
        const rawStatus = p.status;
        const status =
          typeof rawStatus === "string"
            ? rawStatus
            : rawStatus && typeof rawStatus === "object"
              ? String((rawStatus as Record<string, unknown>).type ?? "")
              : "";
        if (/^(idle|ready|completed)$/i.test(status)) this.setTurnActive(false);
        else if (/^(active|busy|running|generating|in_?progress|turn.*)$/i.test(status)) {
          this.setTurnActive(true);
        }
        return;
      }
      default:
        return; // item/*, token counts, etc. — not our concern
    }
  }

  private setTurnActive(active: boolean): void {
    if (this.turnActive === active) return;
    this.turnActive = active;
    if (this.joined) this.hooks.broadcastPresence(this.agent, active ? "busy" : "idle");
  }

  // ── delivery ──

  private async deliverUnread(): Promise<void> {
    if (!this.threadId || this.hooks.skipReplay) return;
    let rows: Record<string, unknown>[];
    try {
      const parsed = JSON.parse(await getUnreadMessages(this.agent));
      if (!Array.isArray(parsed)) return;
      rows = parsed as Record<string, unknown>[];
    } catch (e: unknown) {
      this.hooks.log("codex_deliver_query_error", {
        agent: this.agent,
        error: e instanceof Error ? e.message : String(e),
      });
      return;
    }

    const fresh = rows.filter((r) => typeof r.id === "string" && !this.delivered.has(r.id as string));
    if (fresh.length === 0) return;

    const ids = fresh.map((r) => r.id as string);
    const text = fresh.map(formatBeadsMessage).join("\n\n");
    for (const id of ids) this.delivered.add(id);

    const method = this.turnActive ? "turn/steer" : "turn/start";
    const params = { threadId: this.threadId, input: [{ type: "text", text }] };
    if (method === "turn/start") this.setTurnActive(true); // optimistic; notifications correct it

    this.hooks.log("codex_inject", { agent: this.agent, method, ids });
    // Fire-and-forget: turn/start's RPC response may not arrive until the turn
    // ends. Delivery truth stays in BEADS (unread until the agent acks).
    this.request(method, params).catch((e: Error) => {
      // Allow a later pump to retry these ids — the injection never went in.
      for (const id of ids) this.delivered.delete(id);
      this.hooks.log("codex_inject_error", { agent: this.agent, method, ids, error: e.message });
    });
  }
}

// ── manager (hub-facing surface) ──

export interface CodexBridgeManager {
  /** True if `agent` is a discovered Codex agent (bridge exists, bound or not). */
  has(agent: string): boolean;
  /** Trigger unread delivery for a Codex agent (no-op until its thread binds). */
  deliver(agent: string): void;
  stop(): void;
}

export function startCodexBridges(hooks: BridgeHooks): CodexBridgeManager {
  const bridges = new Map<string, CodexBridgeClient>();

  const scan = (): void => {
    for (const agent of discoverCodexAgents()) {
      if (bridges.has(agent)) continue;
      const client = new CodexBridgeClient(agent, join(RUN_DIR, `${agent}.sock`), hooks);
      bridges.set(agent, client);
      hooks.log("codex_bridge_added", { agent, sock: client.sockPath });
      client.start();
    }
  };

  scan();
  const timer = setInterval(scan, DISCOVERY_MS);
  timer.unref?.();

  return {
    has: (agent) => bridges.has(agent),
    deliver: (agent) => bridges.get(agent)?.deliver(),
    stop: () => {
      clearInterval(timer);
      for (const b of bridges.values()) b.stop();
      bridges.clear();
    },
  };
}

function delay(ms: number): Promise<void> {
  return new Promise((res) => {
    const t = setTimeout(res, ms);
    t.unref?.();
  });
}
