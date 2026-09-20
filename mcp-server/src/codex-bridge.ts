/**
 * Aperture comms-layer v2 — Phase 2 Codex bridge (Protocol 2, "steered turns").
 *
 * The ws-hub owns one CodexBridgeClient per Codex agent. Each client speaks
 * JSON-RPC 2.0 over a WebSocket on the agent's app-server unix socket
 * (~/.aperture/run/<agent>.sock, spawned by Tauri as
 * `codex app-server --listen unix://<path>`):
 *
 *   connect → initialize → initialized → thread/list → either resume an
 *   existing TUI thread OR create+own a fresh kickoff thread → presence
 *   "join" → unread BEADS replay as an injected turn.
 *
 * Delivery: full message bodies are fetched from BEADS (same unread query the
 * hub replay uses) and injected via `turn/start` when the thread is idle,
 * `turn/steer` when a turn is active. An in-memory delivered-set per agent
 * (keyed by message id) prevents double-injection within one app-server
 * connection — it is cleared on socket loss so a rebind re-injects whatever
 * BEADS still reports unread. BEADS unread state remains the durable truth —
 * the agent acks with mark_as_read via its aperture-bus MCP tools.
 *
 * Inject failure (aperture-oeb6q): a rejected inject RPC releases its ids,
 * flips the turn-state assumption (the usual cause is a stale one) and
 * re-pumps exactly once after INJECT_RETRY_MS. A second failure logs
 * codex_inject_retry_exhausted and waits for the next notify or rebind.
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
import { closeSync, constants, existsSync, fstatSync, fsyncSync, linkSync, lstatSync, openSync, readdirSync, readFileSync, renameSync, rmSync, unlinkSync, writeFileSync } from "node:fs";
import { randomUUID } from "node:crypto";
import { homedir } from "node:os";
import { join, resolve } from "node:path";
import WebSocket from "ws";
import { getUnreadMessages } from "./beads.js";
import { isValidSeatName, loadSeatRegistry } from "./seat-registry.js";
import { managedOwnerMatches, readManagedActiveRuntime, readManagedOwner, readManagedStartingRuntime, type ManagedStartingRuntime } from "./managed-owner.js";
import { assertPrivateRuntimeDirectory } from "./private-runtime-path.js";

const AGENTS_DIR = process.env.APERTURE_AGENTS_DIR ?? resolve(homedir(), ".claude", "aperture");
const RUN_DIR = process.env.APERTURE_RUN_DIR ?? resolve(homedir(), ".aperture", "run");
const AGENT_CONFIG_PATH =
  process.env.APERTURE_AGENT_CONFIG_PATH ?? resolve(homedir(), ".aperture", "agent-config.json");

const RECONNECT_MS = envMs("APERTURE_CODEX_RECONNECT_MS", 10_000); // socket missing/dropped → retry cadence
const THREAD_POLL_MS = 10_000; // no thread yet (TUI not started) → re-list cadence
const KICKOFF_RETRY_INITIAL_MS = 500;
const KICKOFF_RETRY_MAX_MS = 10_000;
const RPC_TIMEOUT_MS = 10_000; // control-plane calls (initialize, thread/*)
const TURN_RPC_TIMEOUT_MS = 600_000; // turn/start may not respond until the turn ends
const DISCOVERY_MS = 60_000; // manifest re-scan cadence
const INJECT_RETRY_MS = envMs("APERTURE_CODEX_INJECT_RETRY_MS", 5_000); // failed inject RPC → single re-pump delay
const MANAGED_ACTIVE_WAIT_MS = envMs("APERTURE_MANAGED_ACTIVE_WAIT_MS", 10_000);
const MANAGED_RECEIPT_MAX_BYTES = 8 * 1024;

interface ManagedObservationReceipt {
  schema_version: 1;
  seat: string;
  generation: number;
  token_id: string;
  root_pid: number;
  root_start_time_us: number;
  thread_id: string;
  actual_model: string;
  actual_reasoning: string;
  observed_at_ms: number;
}

interface ManagedStartAttempt {
  schema_version: 1;
  seat: string;
  generation: number;
  token_id: string;
  root_pid: number;
  root_start_time_us: number;
  requested_model: string;
  requested_reasoning: string;
}

/** Positive-integer millisecond override from env (test knob); fallback otherwise. */
function envMs(name: string, fallback: number): number {
  const n = Number(process.env[name]);
  return Number.isInteger(n) && n > 0 ? n : fallback;
}

function managedArtifactPath(owner: ManagedStartingRuntime, kind: "start-attempt" | "observation"): string {
  if (!isValidSeatName(owner.seat) || !Number.isSafeInteger(owner.generation) || owner.generation < 1) {
    throw new Error("E_MANAGED_OBSERVATION_INVALID: invalid owner selector");
  }
  assertSecureRunDir();
  assertPrivateRuntimeDirectory(RUN_DIR);
  return join(RUN_DIR, `${owner.seat}.g${owner.generation}.managed-${kind}.json`);
}

function readPrivateJson(path: string): Record<string, unknown> | null {
  let fd: number | null = null;
  try {
    fd = openSync(path, constants.O_RDONLY | constants.O_NOFOLLOW);
    const stat = fstatSync(fd);
    if (
      !stat.isFile() ||
      stat.nlink !== 1 ||
      (stat.mode & 0o077) !== 0 ||
      stat.size < 2 ||
      stat.size > MANAGED_RECEIPT_MAX_BYTES ||
      (typeof process.getuid === "function" && stat.uid !== process.getuid())
    ) {
      throw new Error("E_MANAGED_OBSERVATION_INVALID: unsafe receipt");
    }
    const parsed: unknown = JSON.parse(readFileSync(fd, "utf8"));
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
      throw new Error("E_MANAGED_OBSERVATION_INVALID: malformed receipt");
    }
    return parsed as Record<string, unknown>;
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return null;
    if (error instanceof SyntaxError) throw new Error("E_MANAGED_OBSERVATION_INVALID: malformed receipt");
    throw error;
  } finally {
    if (fd !== null) closeSync(fd);
  }
}

function persistPrivateJsonNoReplace(path: string, value: Record<string, unknown>): void {
  if (readPrivateJson(path) !== null) {
    throw new Error("E_MANAGED_OBSERVATION_CONFLICT: receipt already exists");
  }
  const temp = `${path}.${randomUUID()}.tmp`;
  const bytes = Buffer.from(`${JSON.stringify(value)}\n`, "utf8");
  if (bytes.length > MANAGED_RECEIPT_MAX_BYTES) {
    throw new Error("E_MANAGED_OBSERVATION_INVALID: receipt exceeds limit");
  }
  let fd: number | null = null;
  try {
    fd = openSync(temp, constants.O_WRONLY | constants.O_CREAT | constants.O_EXCL | constants.O_NOFOLLOW, 0o600);
    writeFileSync(fd, bytes);
    fsyncSync(fd);
    closeSync(fd);
    fd = null;
    linkSync(temp, path);
    unlinkSync(temp);
    const dirFd = openSync(RUN_DIR, constants.O_RDONLY | constants.O_DIRECTORY | constants.O_NOFOLLOW);
    try {
      fsyncSync(dirFd);
    } finally {
      closeSync(dirFd);
    }
    if (readPrivateJson(path) === null) {
      throw new Error("E_MANAGED_OBSERVATION_INVALID: receipt readback failed");
    }
  } catch (error) {
    if (fd !== null) closeSync(fd);
    try {
      unlinkSync(temp);
    } catch {
      // no temp residue
    }
    throw error;
  }
}

function expectedStartAttempt(owner: ManagedStartingRuntime): ManagedStartAttempt {
  if (owner.requested.harness !== "codex" || owner.requested.reasoning === null) {
    throw new Error("E_MODEL_UNVERIFIED: managed Codex tuple is incomplete");
  }
  return {
    schema_version: 1,
    seat: owner.seat,
    generation: owner.generation,
    token_id: owner.tokenId,
    root_pid: owner.pid,
    root_start_time_us: owner.startTimeUs,
    requested_model: owner.requested.model,
    requested_reasoning: owner.requested.reasoning,
  };
}

function exactRecord(value: Record<string, unknown>, expected: Record<string, unknown>): boolean {
  return JSON.stringify(value) === JSON.stringify(expected);
}

function readManagedObservation(owner: ManagedStartingRuntime): ManagedObservationReceipt | null {
  const raw = readPrivateJson(managedArtifactPath(owner, "observation"));
  if (raw === null) return null;
  const expected = expectedStartAttempt(owner);
  const keys = Object.keys(raw).sort().join(",");
  if (
    keys !==
      "actual_model,actual_reasoning,generation,observed_at_ms,root_pid,root_start_time_us,schema_version,seat,thread_id,token_id" ||
    raw.schema_version !== 1 ||
    raw.seat !== expected.seat ||
    raw.generation !== expected.generation ||
    raw.token_id !== expected.token_id ||
    raw.root_pid !== expected.root_pid ||
    raw.root_start_time_us !== expected.root_start_time_us ||
    typeof raw.thread_id !== "string" ||
    !/^[A-Za-z0-9-]{1,128}$/.test(raw.thread_id) ||
    raw.actual_model !== expected.requested_model ||
    raw.actual_reasoning !== expected.requested_reasoning ||
    !Number.isSafeInteger(raw.observed_at_ms) ||
    (raw.observed_at_ms as number) < 1
  ) {
    throw new Error("E_MANAGED_OBSERVATION_CONFLICT: receipt does not match owner");
  }
  return raw as unknown as ManagedObservationReceipt;
}

/**
 * First turn for a freshly launched Codex app-server.
 *
 * This is intentionally static. It is control-plane text, not a BEADS
 * message, so it must never interpolate agent, user, or message content.
 * Provider-aware since aperture-1socy: this text is ONLY ever injected into
 * Codex sessions, whose inbox is delivered by this bridge. It must not tell the
 * agent to start the Claude inbox monitor / hub-client or to look for a hub
 * token (a fresh Codex Rex spent ~75 s doing exactly that). The Claude kickoff
 * (KICKOFF_TEXT in src-tauri/src/launcher.rs) keeps the monitor step.
 * Regression: mcp-server/test/codex-startup-instructions.test.mjs.
 */
export const CODEX_KICKOFF_TEXT =
  "Session start. Your inbox is delivered by the Aperture bridge as injected turns: do not start a monitor process or hub-client and do not look for a hub token. Check get_messages now, process any unread messages, and mark each read after you handle it. Then await scoped dispatch.";

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

/**
 * Codex agents = manifest model merged with panel-persisted model overrides.
 *
 * Runtime manifests are repo symlinks and intentionally remain immutable;
 * agent-config.json is the panel's authoritative override layer.
 */
export function discoverCodexAgents(
  dir: string = AGENTS_DIR,
  configPath: string = AGENT_CONFIG_PATH,
): string[] {
  let entries: string[];
  try {
    entries = readdirSync(dir);
  } catch {
    return [];
  }
  const overrides = readModelOverrides(configPath);
  const registry = loadSeatRegistry({ agentsRoot: dir });
  const out: string[] = [];
  for (const name of entries) {
    if (!registry.seats.has(name)) continue;
    try {
      const raw = readFileSync(join(dir, name, "manifest.json"), "utf8");
      const manifest = JSON.parse(raw) as Record<string, unknown>;
      const manifestModel = typeof manifest.model === "string" ? manifest.model : "";
      const model = overrides[name] ?? manifestModel;
      if (model.startsWith("codex/")) {
        out.push(name);
      }
    } catch {
      // no manifest / unreadable / not a dir — not an agent dir, skip
    }
  }
  return out;
}

/** Missing or malformed override state is non-fatal: manifests remain the fallback. */
function readModelOverrides(path: string): Record<string, string> {
  try {
    const parsed = JSON.parse(readFileSync(path, "utf8"));
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return {};
    return Object.fromEntries(
      Object.entries(parsed as Record<string, unknown>).filter(
        (entry): entry is [string, string] => typeof entry[1] === "string",
      ),
    );
  } catch {
    return {};
  }
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
  /** True once this app-server session has received its fresh-session kickoff. */
  private kickoffInjected = false;

  private reconnectTimer: NodeJS.Timeout | null = null;
  private readyWaiters: Array<(v: void) => void> = [];

  /** Message ids injected over the current app-server connection
   *  (double-injection guard; cleared in onClose — see there). */
  private readonly delivered = new Set<string>();
  /** Armed single re-pump after a failed inject RPC (aperture-oeb6q); null when none. */
  private injectRetryTimer: NodeJS.Timeout | null = null;
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
    this.clearInjectRetry();
    this.clearThreadReady();
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

  /**
   * Start a fresh thread on the app-server and bind this bridge to it.
   *
   * Do not leave ownership to `thread/list`'s newest-thread heuristic: a
   * remote TUI can create a competing thread between those calls.  The
   * thread/start response is authoritative for a bridge-created thread.
   */
  async startThread(params: Record<string, unknown> = {}): Promise<string> {
    const result = (await this.request("thread/start", params)) as Record<string, unknown> | null;
    const direct = result?.threadId ?? result?.id;
    if (typeof direct === "string") {
      this.bindToThread(direct, "thread_start"); // publishes thread-ready (3x136)
      return direct;
    }
    const wrapped = result?.thread;
    if (wrapped && typeof wrapped === "object") {
      const id = (wrapped as Record<string, unknown>).id;
      if (typeof id === "string") {
        this.bindToThread(id, "thread_start"); // publishes thread-ready (3x136)
        return id;
      }
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

  /**
   * Bind an existing TUI thread, or create one and inject the static kickoff
   * for a genuinely fresh app-server.  A reconnect to an existing thread
   * never re-kicks it.
   */
  private async bindThread(ws: WebSocket): Promise<void> {
    let managed: ManagedStartingRuntime | null = null;
    const principal = loadSeatRegistry().seats.get(this.agent);
    if (principal?.group === "team") {
      try {
        const owner = readManagedOwner(this.agent);
        if (owner.state === "starting") {
          managed = readManagedStartingRuntime(this.agent);
        } else if (owner.state === "active") {
          const active = readManagedActiveRuntime(this.agent);
          if (active === null || active.requested.harness !== "codex") {
            throw new Error("E_MODEL_UNVERIFIED: active managed Codex identity is invalid");
          }
          await this.request("thread/resume", { threadId: active.threadId });
          this.bindToThread(active.threadId, "thread_list");
          if (!this.hooks.skipReplay) this.deliver();
          return;
        } else {
          throw new Error("E_OWNER_CORRUPT: managed seat is not startable");
        }
      } catch (error) {
        if ((error as NodeJS.ErrnoException).code === "ENOENT") {
          throw new Error("E_OWNER_CORRUPT: managed seat owner is missing");
        }
        throw error;
      }
    }
    if (managed !== null) {
      await this.bindManagedStarting(ws, managed);
      return;
    }

    let kickoffRetryMs = KICKOFF_RETRY_INITIAL_MS;
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
        this.bindToThread(threadId, "thread_list");
        if (!this.hooks.skipReplay) this.deliver();
        return;
      }

      // No thread is the fresh-session condition.  Create the thread only
      // after the WS socket + initialize handshake above are complete. If the
      // app-server is briefly not ready, retry with bounded exponential
      // backoff rather than requiring a pane keystroke.
      if (!this.kickoffInjected) {
        try {
          const threadId = await this.startThread();
          this.kickoffInjected = true;
          this.setTurnActive(true); // optimistic; turn notifications correct it
          this.request("turn/start", {
            threadId,
            input: [{ type: "text", text: CODEX_KICKOFF_TEXT }],
          }).catch((e: Error) => {
            // The thread still exists and is explicitly owned.  Keep BEADS as
            // durable truth; a later notification/delivery can recover.
            this.hooks.log("codex_kickoff_inject_error", {
              agent: this.agent,
              threadId,
              error: e.message,
            });
          });
          this.hooks.log("codex_kickoff_injected", { agent: this.agent, threadId });
          if (!this.hooks.skipReplay) this.deliver();
          return;
        } catch (e: unknown) {
          this.hooks.log("codex_kickoff_retry", {
            agent: this.agent,
            error: e instanceof Error ? e.message : String(e),
            retryMs: kickoffRetryMs,
          });
          await delay(kickoffRetryMs);
          kickoffRetryMs = Math.min(kickoffRetryMs * 2, KICKOFF_RETRY_MAX_MS);
          continue;
        }
      }
      this.hooks.log("codex_no_thread_yet", { agent: this.agent, retryMs: THREAD_POLL_MS });
      await delay(THREAD_POLL_MS);
    }
  }

  /**
   * Bind a replacement Codex seat without inheriting a previous thread or
   * delivering work before the native owner transition is Active. The durable
   * attempt marker prevents an ambiguous thread/start from being retried.
   */
  private async bindManagedStarting(ws: WebSocket, owner: ManagedStartingRuntime): Promise<void> {
    const attempt = expectedStartAttempt(owner);
    const attemptPath = managedArtifactPath(owner, "start-attempt");
    const existingAttempt = readPrivateJson(attemptPath);
    let receipt = readManagedObservation(owner);

    if (receipt === null) {
      if (existingAttempt !== null) {
        if (!exactRecord(existingAttempt, attempt as unknown as Record<string, unknown>)) {
          throw new Error("E_FRESH_THREAD_UNVERIFIED: start attempt conflicts with owner");
        }
        throw new Error("E_FRESH_THREAD_UNVERIFIED: prior start outcome is ambiguous");
      }
      persistPrivateJsonNoReplace(attemptPath, attempt as unknown as Record<string, unknown>);

      let result: Record<string, unknown> | null;
      try {
        result = (await this.request("thread/start", {
          model: attempt.requested_model,
          allowProviderModelFallback: false,
          config: { model_reasoning_effort: attempt.requested_reasoning },
        })) as Record<string, unknown> | null;
      } catch {
        throw new Error("E_FRESH_THREAD_UNVERIFIED: managed thread start failed");
      }
      const thread = result?.thread;
      const threadId =
        thread && typeof thread === "object" && !Array.isArray(thread)
          ? (thread as Record<string, unknown>).id
          : undefined;
      if (
        typeof threadId !== "string" ||
        !/^[A-Za-z0-9-]{1,128}$/.test(threadId) ||
        result?.model !== attempt.requested_model ||
        result?.reasoningEffort !== attempt.requested_reasoning
      ) {
        throw new Error("E_MODEL_UNVERIFIED: thread/start did not prove the requested tuple");
      }
      receipt = {
        schema_version: 1,
        seat: owner.seat,
        generation: owner.generation,
        token_id: owner.tokenId,
        root_pid: owner.pid,
        root_start_time_us: owner.startTimeUs,
        thread_id: threadId,
        actual_model: result.model as string,
        actual_reasoning: result.reasoningEffort as string,
        observed_at_ms: Date.now(),
      };
      persistPrivateJsonNoReplace(
        managedArtifactPath(owner, "observation"),
        receipt as unknown as Record<string, unknown>,
      );
      this.hooks.log("codex_managed_observed", { agent: this.agent, generation: owner.generation });
    }

    const deadline = Date.now() + MANAGED_ACTIVE_WAIT_MS;
    while (this.ws === ws && ws.readyState === WebSocket.OPEN && !this.stopped) {
      const current = readManagedOwner(this.agent);
      if (managedOwnerMatches(current, owner.generation, owner.tokenId, ["active"])) {
        this.bindToThread(receipt.thread_id, "thread_start");
        this.kickoffInjected = true;
        this.setTurnActive(true);
        this.request("turn/start", {
          threadId: receipt.thread_id,
          input: [{ type: "text", text: CODEX_KICKOFF_TEXT }],
        }).catch((error: Error) => {
          this.hooks.log("codex_kickoff_inject_error", {
            agent: this.agent,
            threadId: receipt?.thread_id,
            error: error.message,
          });
        });
        this.hooks.log("codex_kickoff_injected", { agent: this.agent, threadId: receipt.thread_id });
        if (!this.hooks.skipReplay) this.deliver();
        return;
      }
      if (
        current.state !== "starting" ||
        current.generation !== owner.generation ||
        current.tokenId !== owner.tokenId
      ) {
        throw new Error("E_GENERATION_MISMATCH: managed owner changed before activation");
      }
      if (Date.now() >= deadline) {
        throw new Error("E_MODEL_UNVERIFIED: managed owner did not become active");
      }
      await delay(25);
    }
    throw new Error("E_FRESH_THREAD_UNVERIFIED: managed bridge disconnected before activation");
  }

  /** Make a known thread the bridge's sole delivery target and announce it once. */
  private bindToThread(threadId: string, source: "thread_start" | "thread_list"): void {
    const changed = this.threadId !== threadId;
    this.threadId = threadId;
    if (!this.joined) {
      this.joined = true;
      this.hooks.broadcastPresence(this.agent, "join");
    }
    if (changed) this.hooks.log("codex_bound", { agent: this.agent, threadId, source });
    // aperture-3x136: publish the pane handoff file on EVERY bind, not only on
    // thread_start. When the hub restarts while app-servers survive, the
    // bridge rebinds via thread_list — previously that path never wrote the
    // file, so pane launch scripts timed out and exec'd `codex resume ""`.
    if (changed || !existsSync(this.threadReadyPath())) this.publishThreadReady(threadId);
    this.ensureThreadReadyLoop();
    // aperture-3x136 (dots): the presence-dot state machine (watchdog::compute_dot)
    // needs a <agent>.kickoff timestamp to ever paint anything but grey/"spawned"
    // — green requires kickoff_millis AND stable presence. The Tauri launcher
    // writes .kickoff only for Claude agents (agents.rs); the earlier claim that
    // "Codex records its own at bridge-inject time" was never implemented, so
    // codex dots were permanently grey despite live join/busy/idle presence.
    // Bind is codex's "I'm live" moment — stamp it here, mirroring the Claude
    // launcher's write-at-launch.
    this.publishKickoffStamp();
  }

  /**
   * aperture-3x136 (dots): write ~/.aperture/run/<agent>.kickoff = unix-epoch
   * millis (ASCII) so the presence-dot state machine can leave the grey
   * "spawned" state. Idempotent per bind; a fresh stamp on reconnect is fine —
   * stable online presence wins over the booting/stuck deadline regardless.
   */
  private publishKickoffStamp(): void {
    try {
      writeFileSync(this.kickoffPath(), `${Date.now()}`, { encoding: "utf8" });
    } catch (e: unknown) {
      this.hooks.log("codex_kickoff_stamp_error", {
        agent: this.agent,
        error: e instanceof Error ? e.message : String(e),
      });
    }
  }

  private kickoffPath(): string {
    return join(RUN_DIR, `${this.agent}.kickoff`);
  }

  /**
   * aperture-3x136: the launcher removes <agent>.thread-id before every pane
   * (re)spawn and its wait gate expects the bridge to rewrite it. If the
   * bridge is ALREADY bound when that happens (surviving app-server, watchdog
   * re-kick), no bind event fires — so keep a small unref'd loop that
   * republishes whenever the file is missing while a thread is bound.
   */
  private threadReadyTimer: NodeJS.Timeout | null = null;
  private ensureThreadReadyLoop(): void {
    if (this.threadReadyTimer) return;
    this.threadReadyTimer = setInterval(() => {
      if (this.threadId && !existsSync(this.threadReadyPath())) {
        this.publishThreadReady(this.threadId);
      }
    }, 5000);
    this.threadReadyTimer.unref?.();
  }

  /**
   * Hand the exact bridge-created thread id to the launcher without argv or
   * log exposure. The Codex pane uses this to run `codex resume <id> --remote`
   * instead of opening a second, empty TUI thread.
   */
  private publishThreadReady(threadId: string): void {
    const path = this.threadReadyPath();
    const temp = `${path}.${process.pid}.tmp`;
    let fd: number | null = null;
    try {
      if (!/^[A-Za-z0-9-]{1,128}$/.test(threadId)) throw new Error("invalid thread id");
      assertSecureRunDir();
      fd = openSync(temp, constants.O_WRONLY | constants.O_CREAT | constants.O_EXCL | constants.O_NOFOLLOW, 0o600);
      writeFileSync(fd, `${threadId}\n`, { encoding: "utf8" });
      fsyncSync(fd);
      closeSync(fd);
      fd = null;
      renameSync(temp, path);
      this.hooks.log("codex_thread_ready", { agent: this.agent });
    } catch (e: unknown) {
      if (fd !== null) closeSync(fd);
      rmSync(temp, { force: true });
      // The bridge remains correctly bound; launcher retry/timeout supplies
      // the visible failure path instead of a silent ownership regression.
      this.hooks.log("codex_thread_ready_error", {
        agent: this.agent,
        error: e instanceof Error ? e.message : String(e),
      });
    }
  }

  private clearThreadReady(): void {
    try {
      rmSync(this.threadReadyPath(), { force: true });
    } catch {
      // stale readiness is cleared again by the launcher before each spawn
    }
  }

  private threadReadyPath(): string {
    if (!isValidSeatName(this.agent)) {
      throw new Error("invalid agent name for thread handoff");
    }
    return join(RUN_DIR, `${this.agent}.thread-id`);
  }

  private onClose(): void {
    this.ws = null;
    this.initialized = false;
    this.threadId = null;
    this.clearThreadReady();
    this.clearInjectRetry();
    // aperture-oeb6q: forget what was injected over the dead socket. BEADS is
    // the delivery truth — an id still unread after the rebind MUST be
    // re-injected: the turn it rode on died with the socket (user abort,
    // app-server crash, hub-side drop) and no later notify will re-push it,
    // so leaving it in `delivered` hid the message for the rest of the hub's
    // lifetime. Re-injecting an already-acked message is impossible because
    // getUnreadMessages no longer returns it.
    this.delivered.clear();
    // A replacement app-server is a fresh session.  If it has no existing
    // thread after initialize, bindThread will create exactly one kickoff.
    this.kickoffInjected = false;
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

  private async deliverUnread(retry = false): Promise<void> {
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

    this.hooks.log("codex_inject", { agent: this.agent, method, ids, retry });
    // Fire-and-forget: turn/start's RPC response may not arrive until the turn
    // ends. Delivery truth stays in BEADS (unread until the agent acks).
    this.request(method, params).catch((e: Error) => {
      // Allow a later pump to retry these ids — the injection never went in.
      for (const id of ids) this.delivered.delete(id);
      this.hooks.log("codex_inject_error", { agent: this.agent, method, ids, retry, error: e.message });
      this.onInjectFailure(method, ids, retry);
    });
  }

  /**
   * aperture-oeb6q: a rejected inject RPC used to leave its released ids
   * waiting for the NEXT hub notify — possibly hours away. The likeliest cause
   * is a stale turn-state (turn/start into a busy thread, turn/steer into an
   * idle one), so flip the assumption and re-pump once after INJECT_RETRY_MS.
   * Exactly one retry per failure: if that also fails, the next notify or
   * rebind is the next chance — never a tight loop against the app-server.
   */
  private onInjectFailure(method: string, ids: string[], retry: boolean): void {
    if (retry) {
      this.hooks.log("codex_inject_retry_exhausted", { agent: this.agent, method, ids });
      return;
    }
    // Socket-loss rejections arrive via failAllPending: onClose has already
    // reset turn-state and cleared `delivered`, and the rebind replay IS the
    // retry. Flipping turn-state here would poison the fresh connection.
    const ws = this.ws;
    if (this.stopped || !ws || ws.readyState !== WebSocket.OPEN) return;
    this.setTurnActive(method === "turn/start");
    if (this.injectRetryTimer) return; // one armed re-pump covers every failure in flight
    this.injectRetryTimer = setTimeout(() => {
      this.injectRetryTimer = null;
      this.hooks.log("codex_inject_retry", { agent: this.agent, ids, retryMs: INJECT_RETRY_MS });
      this.deliverChain = this.deliverChain.then(() => this.deliverUnread(true));
    }, INJECT_RETRY_MS);
    this.injectRetryTimer.unref?.();
  }

  private clearInjectRetry(): void {
    if (!this.injectRetryTimer) return;
    clearTimeout(this.injectRetryTimer);
    this.injectRetryTimer = null;
  }
}

/** The handoff directory is a trust boundary: an attacker-controlled symlink
 * would defeat the leaf-file O_NOFOLLOW check by redirecting the whole path. */
function assertSecureRunDir(): void {
  const stat = lstatSync(RUN_DIR);
  if (stat.isSymbolicLink() || !stat.isDirectory()) throw new Error("insecure run directory type");
  if (typeof process.getuid === "function" && stat.uid !== process.getuid()) {
    throw new Error("insecure run directory owner");
  }
  if ((stat.mode & 0o022) !== 0) throw new Error("insecure run directory permissions");
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
    const codexAgents = new Set(discoverCodexAgents());
    // Add newly-codex agents.
    for (const agent of codexAgents) {
      if (bridges.has(agent)) continue;
      const client = new CodexBridgeClient(agent, join(RUN_DIR, `${agent}.sock`), hooks);
      bridges.set(agent, client);
      hooks.log("codex_bridge_added", { agent, sock: client.sockPath });
      client.start();
    }
    // Prune agents whose effective model flipped AWAY from codex/ since the
    // last scan (e.g. the launcher restarted them as a claude-code session
    // and rewrote agent-config.json). Without this, the stale bridge lingers
    // forever and handleNotify keeps routing that agent's messages down the
    // codex path (notify_codex → dead ENOENT app-server socket) instead of
    // to their live Monitor WebSocket — silently killing push delivery for
    // the rest of the hub's lifetime. (aperture-avlz2)
    for (const agent of bridges.keys()) {
      if (codexAgents.has(agent)) continue;
      bridges.get(agent)?.stop();
      bridges.delete(agent);
      hooks.log("codex_bridge_removed", { agent, reason: "no_longer_codex" });
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
