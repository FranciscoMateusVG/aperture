#!/usr/bin/env node
/**
 * SIGKILL crash-durability test for MessageQueue (answers aperture-p27om).
 *
 * Scenario:
 *   1. Child A constructs a MessageQueue over a temp JSONL queue file with a
 *      deliberately slow fake flush (100ms each) that appends every flushed
 *      message to a ledger file. It enqueues 20 messages and starts draining.
 *   2. The parent SIGKILLs child A mid-drain, once ~5 flushes have landed.
 *   3. Child B starts with the SAME queue file (crash-replay path: start()
 *      loads persisted entries) and drains to completion.
 *   4. Assert on the ledger: all 20 messages present at-least-once, in order,
 *      zero lost. Duplicates are allowed (at-least-once semantics) but the
 *      de-duplicated first-occurrence sequence must be exactly msg-01..msg-20.
 *
 * No changes to message-queue.ts: the fake flush is injected via the existing
 * `flush` constructor option.
 *
 * Usage: node scripts/sigkill-replay.test.mjs        (parent / test driver)
 *        node scripts/sigkill-replay.test.mjs child ... (internal)
 */

import { spawn } from "node:child_process";
import { appendFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const __filename = fileURLToPath(import.meta.url);
const TOTAL = 20;
const KILL_AFTER_FLUSHES = 5;

// ─────────────────────────────────────────────────────────────── child mode ──
if (process.argv[2] === "child") {
  const [, , , mode, queueFile, ledgerFile, flushDelayStr] = process.argv;
  const flushDelayMs = Number(flushDelayStr);
  const { MessageQueue } = await import("../dist/message-queue.js");

  const queue = new MessageQueue({
    queueFilePath: queueFile,
    // Slow fake flush: simulate a multi-hundred-ms backend write, then record
    // the delivery in the ledger. The append is the "backend received it" line.
    flush: async (msg) => {
      await new Promise((r) => setTimeout(r, flushDelayMs));
      appendFileSync(ledgerFile, msg.content + "\n");
    },
    log: () => {}, // keep stderr quiet; the parent asserts on files, not logs
  });

  queue.start(); // loads any persisted entries (crash-replay) and kicks drain

  if (mode === "enqueue") {
    for (let i = 1; i <= TOTAL; i++) {
      queue.enqueue("child-a", "recipient", `msg-${String(i).padStart(2, "0")}`);
    }
  }

  // Exit cleanly once fully drained (replay child); the enqueue child never
  // gets here — it is SIGKILLed by the parent mid-drain.
  const poll = setInterval(() => {
    if (queue.pending === 0) {
      clearInterval(poll);
      process.exit(0);
    }
  }, 20);
  poll.unref?.();
  setInterval(() => {}, 1000); // hold the event loop open while draining
}

// ────────────────────────────────────────────────────────────── parent mode ──
else {
  const scratchBase =
    process.env.SIGKILL_TEST_TMPDIR ??
    "/private/tmp/claude-501/-Users-franciscomateus-projects-aperture/3c8a1760-6b95-4ef9-aa8c-98836a8e8599/scratchpad";
  mkdirSync(scratchBase, { recursive: true });
  const tmpDir = mkdtempSync(join(scratchBase, "sigkill-replay-"));
  const queueFile = join(tmpDir, "queue.jsonl");
  const ledgerFile = join(tmpDir, "ledger.log");

  const ledgerLines = () =>
    existsSync(ledgerFile)
      ? readFileSync(ledgerFile, "utf8").split("\n").filter((l) => l.trim() !== "")
      : [];

  const spawnChild = (mode, flushDelayMs) =>
    spawn(process.execPath, [__filename, "child", mode, queueFile, ledgerFile, String(flushDelayMs)], {
      cwd: dirname(dirname(__filename)), // mcp-server/ so ../dist resolves
      stdio: ["ignore", "inherit", "inherit"],
    });

  const fail = (msg) => {
    console.error(`FAIL: ${msg}`);
    console.error(`  tmpDir kept for inspection: ${tmpDir}`);
    process.exit(1);
  };

  // Phase 1: child A enqueues 20 with 100ms flushes; SIGKILL after ~5 flushes.
  const childA = spawnChild("enqueue", 100);
  const deadline = Date.now() + 15_000;
  while (ledgerLines().length < KILL_AFTER_FLUSHES) {
    if (Date.now() > deadline) fail("timed out waiting for first 5 flushes");
    if (childA.exitCode !== null) fail("child A exited before it could be SIGKILLed");
    await new Promise((r) => setTimeout(r, 20));
  }
  childA.kill("SIGKILL");
  await new Promise((r) => childA.once("exit", r));

  const flushedBeforeKill = ledgerLines().length;
  const persisted = readFileSync(queueFile, "utf8").split("\n").filter((l) => l.trim() !== "");
  console.log(
    `phase 1: child A SIGKILLed after ${flushedBeforeKill} flushes; ` +
      `${persisted.length} message(s) persisted in queue file`,
  );
  if (flushedBeforeKill >= TOTAL) fail("child A finished draining before the kill — test lost its race");
  if (persisted.length === 0) fail("queue file empty after kill — nothing left to replay");

  // Phase 2: child B replays the same queue file (fast flushes) to completion.
  const childB = spawnChild("replay", 10);
  const bExit = await new Promise((r) => childB.once("exit", r));
  if (bExit !== 0) fail(`child B exited with code ${bExit}`);
  console.log(`phase 2: child B replayed + drained to completion (exit 0)`);

  // Assertions on the ledger.
  const ledger = ledgerLines();
  const expected = Array.from({ length: TOTAL }, (_, i) => `msg-${String(i + 1).padStart(2, "0")}`);

  // 1) Zero lost: every message present at least once.
  const missing = expected.filter((m) => !ledger.includes(m));
  if (missing.length > 0) fail(`messages LOST: ${missing.join(", ")}`);

  // 2) In order: first-occurrence sequence (dedup) must be exactly msg-01..msg-20.
  const firstOccurrence = [...new Set(ledger)];
  if (JSON.stringify(firstOccurrence) !== JSON.stringify(expected)) {
    fail(`out of order — got: ${firstOccurrence.join(", ")}`);
  }

  // 3) No foreign entries.
  const foreign = ledger.filter((l) => !expected.includes(l));
  if (foreign.length > 0) fail(`unexpected ledger entries: ${foreign.join(", ")}`);

  const dupes = ledger.length - TOTAL;
  console.log(
    `PASS: all ${TOTAL} messages delivered at-least-once, in order, zero lost ` +
      `(${ledger.length} ledger entries, ${dupes} duplicate(s) — at-least-once allows this)`,
  );
  rmSync(tmpDir, { recursive: true, force: true });
}
