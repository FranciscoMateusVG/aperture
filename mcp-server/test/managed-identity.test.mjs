import { test } from "node:test";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { chmodSync, mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const home = mkdtempSync(join(tmpdir(), "aperture-managed-owner-"));
const run = join(home, ".aperture", "run");
const owner = join(run, "owner");
for (const dir of [home, join(home, ".aperture"), run, owner]) {
  mkdirSync(dir, { recursive: true, mode: 0o700 });
  chmodSync(dir, 0o700);
}
process.env.HOME = home;
process.env.APERTURE_RUN_DIR = run;
process.env.APERTURE_OWNER_DIR = owner;
process.env.APERTURE_TEAM_GENERATION = "999";
const { managedHelloFields } = await import("../dist/managed-identity.js");
const { managedOwnerMatches, readManagedOwner } = await import("../dist/managed-owner.js");

const seat = "alpha-worker";
const token = "ab".repeat(32);
const digest = createHash("sha256").update(token).digest("hex");
const record = (state = "active", tokenId = digest) => ({
  schema_version: 1,
  seat,
  generation: 7,
  state,
  reservation_nonce_sha256: null,
  provisional_token_id: state === "starting" ? tokenId : null,
  requested: { harness: "claude", model: "claude/test", reasoning: null },
  incarnation: {
    pid: 123,
    start_time: 456,
    thread_id: "thread-alpha",
    token_id: tokenId,
    harness: "claude",
    model: "claude/test",
    reasoning: null,
    processes: [{ pid: 123, start_time: 456, ppid: 1, pgid: 123, cmdline_sha256: "a".repeat(64), cwd: "/tmp/work" }],
  },
  since: "2026-09-20T00:00:00Z",
  writer: "launcher",
});

test("starting owner exposes only its durably bound provisional token identity", () => {
  const starting = record("starting");
  starting.incarnation = null;
  writeFileSync(join(owner, `${seat}.json`), JSON.stringify(starting), { mode: 0o600 });
  const parsed = readManagedOwner(seat);
  assert.deepEqual(parsed, { seat, generation: 7, state: "starting", tokenId: digest });
  assert.equal(managedOwnerMatches(parsed, 7, digest, ["starting", "active"]), true);

  delete starting.provisional_token_id;
  writeFileSync(join(owner, `${seat}.json`), JSON.stringify(starting), { mode: 0o600 });
  assert.throws(() => readManagedOwner(seat), /invalid provisional owner identity/);

  starting.provisional_token_id = "c".repeat(64);
  starting.incarnation = record("starting").incarnation;
  writeFileSync(join(owner, `${seat}.json`), JSON.stringify(starting), { mode: 0o600 });
  assert.throws(() => readManagedOwner(seat), /invalid provisional owner identity/);
});

test("managed hello derives generation and token identity from the active OwnerRecord", () => {
  writeFileSync(join(owner, `${seat}.json`), JSON.stringify(record()), { mode: 0o600 });
  assert.deepEqual(managedHelloFields(seat, token), { generation: 7, token_id: digest });
  assert.notEqual(managedHelloFields(seat, token).generation, Number(process.env.APERTURE_TEAM_GENERATION), "environment generation is never authority");

  writeFileSync(join(owner, `${seat}.json`), JSON.stringify(record("starting")), { mode: 0o600 });
  assert.throws(() => managedHelloFields(seat, token), /owner identity is invalid/);
  writeFileSync(join(owner, `${seat}.json`), JSON.stringify(record("active", "c".repeat(64))), { mode: 0o600 });
  assert.throws(() => managedHelloFields(seat, token), /owner identity is invalid/);
});

test("standing seat without an owner record emits no managed authority fields", () => {
  assert.deepEqual(managedHelloFields("standing-seat", "cd".repeat(32)), {});
});
