/** Hub-private reminder coalescer. No IO, credentials, authority or model turns.
 * Delivery callbacks remain in ws-hub. A pending notification is never an ACK.
 */
export interface ReminderClock {
  set(callback: () => void, ms: number): unknown;
  clear(timer: unknown): void;
  random(): number;
}
const CLOCK: ReminderClock = {
  set: (callback, ms) => { const t = setTimeout(callback, ms); t.unref(); return t; },
  clear: timer => clearTimeout(timer as ReturnType<typeof setTimeout>),
  random: Math.random,
};
export class ManagedInboxReminder<T> {
  private timer: unknown = null;
  private running = false;
  private started = false;
  private cancelled = false;
  private wanted = false;
  private version = 0;
  private backoff = 0;
  constructor(private readonly hooks: {
    valid(): boolean;
    unread(): Promise<readonly T[]>;
    send(rows: readonly T[]): void;
    error(): void;
  }, private readonly clock: ReminderClock = CLOCK) {}
  /** Hello replay finishes before enabling timer IO. Notifications received
   * during that query remain pending, without a parallel unread scan. */
  start(): void { if (!this.cancelled) { this.started = true; this.schedule(); } }
  pending(): void {
    if (this.cancelled) return;
    this.version++; this.wanted = true; this.schedule();
  }
  cancel(): void {
    this.cancelled = true; this.wanted = false; this.version++;
    if (this.timer !== null) this.clock.clear(this.timer);
    this.timer = null;
  }
  private current(): boolean {
    if (this.cancelled) return false;
    try { if (this.hooks.valid()) return true; } catch { /* fail closed */ }
    this.cancel(); return false;
  }
  private schedule(): void {
    if (!this.started || !this.wanted || this.running || this.timer !== null || !this.current()) return;
    const base = [30_000, 60_000, 120_000, 300_000][Math.min(this.backoff, 3)]!;
    const ms = Math.min(300_000, Math.round(base * (0.9 + 0.2 * this.clock.random())));
    this.timer = this.clock.set(() => { this.timer = null; void this.tick(); }, ms);
  }
  private async tick(): Promise<void> {
    if (this.running || !this.wanted || !this.current()) return;
    this.running = true;
    const version = this.version;
    try {
      const rows = await this.hooks.unread();
      if (!this.current()) return;
      if (rows.length === 0) {
        // A new notify during the old empty query must not be cancelled by it.
        if (this.version === version) { this.wanted = false; this.backoff = 0; }
      } else {
        this.hooks.send(rows); // synchronous final owner/socket fence in hub
        this.backoff = Math.min(this.backoff + 1, 3);
      }
    } catch {
      if (this.current()) { this.hooks.error(); this.backoff = Math.min(this.backoff + 1, 3); }
    } finally {
      this.running = false;
      this.schedule();
    }
  }
}
