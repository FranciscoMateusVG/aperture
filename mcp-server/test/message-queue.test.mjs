// aperture-ktwoy — guardrail tests for the durable send_message queue.
// Run after `npm run build`:  node test/message-queue.test.mjs
// Exercises the queue against a controllable mock flush (no bd/server needed):
// instant enqueue, FIFO ordering, retry-on-failure-without-loss, and
// crash-replay (a fresh instance recovers persisted messages from disk).
import { MessageQueue } from "../dist/message-queue.js";
import { mkdtempSync, existsSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import assert from "node:assert/strict";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const tmp = mkdtempSync(join(tmpdir(), "mq-test-"));
const silent = () => {};
let pass = 0;

try {
  // ── Test 1: enqueue is instant; FIFO flush; disk drains on success ──
  {
    const qf = join(tmp, "q1.jsonl");
    const delivered = [];
    const q = new MessageQueue({ queueFilePath: qf, flush: async (m) => { delivered.push(m.content); }, retryDelayMs: 50, log: silent });
    q.start();
    const t0 = Date.now();
    q.enqueue("rex", "glados", "m1");
    q.enqueue("rex", "glados", "m2");
    q.enqueue("rex", "glados", "m3");
    const enqMs = Date.now() - t0;
    assert.ok(enqMs < 50, `enqueue must be instant, was ${enqMs}ms`);
    await sleep(200);
    assert.deepEqual(delivered, ["m1", "m2", "m3"], "FIFO order preserved");
    assert.equal(q.pending, 0, "queue fully drained");
    assert.equal(existsSync(qf) ? readFileSync(qf, "utf8").trim() : "", "", "disk file drained");
    console.log("✓ test 1: instant enqueue + FIFO + disk drain"); pass++;
  }

  // ── Test 2: a failing flush HOLDS the message, retries, never loses it ──
  {
    const qf = join(tmp, "q2.jsonl");
    let attempts = 0; const delivered = [];
    const q = new MessageQueue({
      queueFilePath: qf,
      flush: async (m) => { attempts++; if (attempts <= 3) throw new Error("backend down"); delivered.push(m.content); },
      retryDelayMs: 30, loudThreshold: 100, log: silent,
    });
    q.start();
    q.enqueue("rex", "glados", "retry-me");
    await sleep(40);
    assert.equal(q.pending, 1, "message held after failed flush");
    assert.ok(readFileSync(qf, "utf8").includes("retry-me"), "message persisted on disk during retries");
    await sleep(400);
    assert.deepEqual(delivered, ["retry-me"], "delivered after retries (no loss, no dup)");
    assert.equal(q.pending, 0, "drained after eventual success");
    console.log("✓ test 2: failure holds + retries + persists, no loss"); pass++;
  }

  // ── Test 3: crash-replay — a fresh instance recovers persisted messages ──
  {
    const qf = join(tmp, "q3.jsonl");
    // Instance A: backend always down → nothing drains, both held on disk, then "crash".
    const qA = new MessageQueue({ queueFilePath: qf, flush: async () => { throw new Error("down"); }, retryDelayMs: 100000, loudThreshold: 100, log: silent });
    qA.start();
    qA.enqueue("rex", "glados", "survive-1");
    qA.enqueue("rex", "glados", "survive-2");
    await sleep(30);
    const disk = readFileSync(qf, "utf8");
    assert.ok(disk.includes("survive-1") && disk.includes("survive-2"), "both messages persisted pre-crash");
    // "Crash": abandon qA. Fresh instance qB with a working backend replays from disk.
    const delivered = [];
    const qB = new MessageQueue({ queueFilePath: qf, flush: async (m) => { delivered.push(m.content); }, retryDelayMs: 30, log: silent });
    qB.start();
    await sleep(200);
    assert.deepEqual(delivered, ["survive-1", "survive-2"], "replayed in FIFO order across the crash, no loss");
    assert.equal(qB.pending, 0, "replay drained");
    console.log("✓ test 3: crash-replay across a fresh instance, no loss, FIFO preserved"); pass++;
  }

  console.log(`\nALL ${pass}/3 PASS`);
  rmSync(tmp, { recursive: true, force: true });
  process.exit(0);
} catch (e) {
  console.error(`\nFAIL (after ${pass} passing): ${e && e.message ? e.message : e}`);
  rmSync(tmp, { recursive: true, force: true });
  process.exit(1);
}
