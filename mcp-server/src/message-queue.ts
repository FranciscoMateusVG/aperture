import { appendFileSync, readFileSync, writeFileSync, renameSync, existsSync, mkdirSync } from "node:fs";
import { dirname } from "node:path";

/**
 * Durable fire-and-forget queue for agent-to-agent BEADS messages
 * (aperture-ktwoy).
 *
 * WHY THIS EXISTS
 * ───────────────
 * `send_message` used to `await createMessage()` synchronously, which shells
 * out to the `bd` CLI. Per-call `bd` overhead is multi-second (process spawn +
 * connection + Dolt commit), so every message send blocked the agent for ~2-5s.
 * `send_message` is the swarm's hot path AND is read-after-write-safe — the
 * sender never re-reads a sent message, and the recipient receives it via the
 * 5s poller — so a small async flush delay is invisible. We therefore ENQUEUE
 * the message, return INSTANTLY, and flush to BEADS on a background loop.
 *
 * Only `send_message` is queued. create/update/close/store_artifact stay
 * SYNCHRONOUS because they have cross-agent read-after-write exposure (option
 * (c), aperture-ktwoy) — a queued update could be read stale.
 *
 * DELIVERY SEMANTICS: at-least-once. enqueue() persists to disk BEFORE
 * returning, so a crash never LOSES a message (replayed on restart). A crash
 * in the tiny window between a successful flush and the disk rewrite can
 * re-deliver one message (duplicate) — acceptable for messages; we never drop.
 *
 * FAILURE MODE (Cipher/Peppy guardrails): if the BEADS backend is unreachable,
 * the queue HOLDS + retries with backoff (disk-persisted, survives restart).
 * It NEVER silently falls back to a divergent local store (that would be
 * split-brain). After a loud-threshold of consecutive failures it logs a clear
 * "beads bus unreachable" error to stderr, then keeps retrying — fail-loud,
 * never fail-to-stale, never fail-silent.
 *
 * ORDERING: strict FIFO. The drain processes the head entry fully (flush +
 * disk rewrite) before the next, so message order is preserved.
 *
 * CONCURRENCY: the MCP is a single-threaded Node event loop, one process per
 * agent. A single `draining` guard prevents overlapping drain passes; enqueue
 * and the drain loop never interleave a partial file write (rewrite is atomic
 * via temp-file + rename).
 */

export interface QueuedMessage {
  from: string;
  to: string;
  content: string;
  ts: number;
  attempts: number;
}

/** Flushes one message to the backend. Resolves on success, rejects on failure. */
export type FlushFn = (msg: QueuedMessage) => Promise<void>;

export interface MessageQueueOptions {
  /** Absolute path to the durability file (JSONL, one QueuedMessage per line). */
  queueFilePath: string;
  /** Performs the actual backend write (e.g. createMessage). */
  flush: FlushFn;
  /** Retry backoff in ms after a failed flush (default 5000). */
  retryDelayMs?: number;
  /** Consecutive-failure count at which we start logging loud errors (default 3). */
  loudThreshold?: number;
  /** Logger (default: console.error to stderr — stdout is the MCP protocol channel). */
  log?: (msg: string) => void;
}

export class MessageQueue {
  private readonly queueFilePath: string;
  private readonly flush: FlushFn;
  private readonly retryDelayMs: number;
  private readonly loudThreshold: number;
  private readonly log: (msg: string) => void;

  private queue: QueuedMessage[] = [];
  private draining = false;
  private consecutiveFailures = 0;
  private retryTimer: ReturnType<typeof setTimeout> | null = null;

  constructor(opts: MessageQueueOptions) {
    this.queueFilePath = opts.queueFilePath;
    this.flush = opts.flush;
    this.retryDelayMs = opts.retryDelayMs ?? 5000;
    this.loudThreshold = opts.loudThreshold ?? 3;
    // IMPORTANT: never log to stdout — that is the MCP stdio protocol channel.
    this.log = opts.log ?? ((m: string) => console.error(m));
  }

  /**
   * Load any messages persisted from a previous process (crash-replay) and
   * kick the drain loop. Call once at MCP startup, after constructing.
   */
  start(): void {
    const dir = dirname(this.queueFilePath);
    if (!existsSync(dir)) mkdirSync(dir, { recursive: true });
    if (existsSync(this.queueFilePath)) {
      const raw = readFileSync(this.queueFilePath, "utf8");
      for (const line of raw.split("\n")) {
        const trimmed = line.trim();
        if (!trimmed) continue;
        try {
          const parsed = JSON.parse(trimmed) as QueuedMessage;
          if (parsed && typeof parsed.to === "string" && typeof parsed.content === "string") {
            this.queue.push(parsed);
          }
        } catch {
          // Skip a corrupt line rather than crash the whole replay.
        }
      }
      if (this.queue.length > 0) {
        this.log(`[message-queue] replaying ${this.queue.length} persisted message(s) on startup`);
      }
    }
    void this.drain();
  }

  /**
   * Enqueue a message and return immediately. Persists to disk BEFORE
   * returning so a crash never loses the message. Kicks the drain loop.
   */
  enqueue(from: string, to: string, content: string): void {
    const msg: QueuedMessage = { from, to, content, ts: Date.now(), attempts: 0 };
    this.queue.push(msg);
    // Durability: append the new entry to disk before we return. A crash
    // after this point will replay the message on restart.
    appendFileSync(this.queueFilePath, JSON.stringify(msg) + "\n");
    void this.drain();
  }

  /** Number of messages still pending (for diagnostics / loud-error reporting). */
  get pending(): number {
    return this.queue.length;
  }

  /**
   * Drain the FIFO head-first. One message fully (flush + disk rewrite) before
   * the next, so ordering is preserved. On failure, stop, schedule a retry,
   * and keep the head in place. Re-entrancy guarded by `draining`.
   */
  private async drain(): Promise<void> {
    if (this.draining) return;
    this.draining = true;
    try {
      while (this.queue.length > 0) {
        const head = this.queue[0]!;
        try {
          await this.flush(head);
        } catch (e: unknown) {
          // Flush failed — backend likely unreachable. Keep the head (FIFO),
          // back off, and retry later. NEVER drop, NEVER fall back to a local
          // store (split-brain).
          head.attempts += 1;
          this.consecutiveFailures += 1;
          if (this.consecutiveFailures >= this.loudThreshold) {
            const err = e instanceof Error ? e.message : String(e);
            this.log(
              `[message-queue] BEADS BUS UNREACHABLE — ${this.queue.length} message(s) queued, ` +
                `head retried ${head.attempts}x. Last error: ${err}. Holding + retrying every ` +
                `${this.retryDelayMs}ms (no fallback, no drop).`,
            );
          }
          this.scheduleRetry();
          return;
        }
        // Success: remove the head and atomically rewrite the durability file
        // from the remaining queue, BEFORE flushing the next entry.
        this.queue.shift();
        this.consecutiveFailures = 0;
        this.persistAll();
      }
    } finally {
      this.draining = false;
    }
  }

  private scheduleRetry(): void {
    if (this.retryTimer) return;
    this.retryTimer = setTimeout(() => {
      this.retryTimer = null;
      void this.drain();
    }, this.retryDelayMs);
    // Don't let a pending retry timer keep the process alive on its own — the
    // MCP's stdio transport owns process lifetime. (Also makes the unit test
    // exit cleanly instead of hanging on a dangling timer.)
    this.retryTimer.unref?.();
  }

  /**
   * Atomically rewrite the durability file from the in-memory queue.
   * temp-file + rename avoids a truncation window where a crash could lose
   * still-pending messages.
   */
  private persistAll(): void {
    const tmp = this.queueFilePath + ".tmp";
    const body = this.queue.map((m) => JSON.stringify(m)).join("\n");
    writeFileSync(tmp, body.length > 0 ? body + "\n" : "");
    renameSync(tmp, this.queueFilePath);
  }
}
