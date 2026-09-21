import { test } from "node:test";
import assert from "node:assert/strict";
import { chmodSync, mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const home = mkdtempSync(join(tmpdir(), "aperture-revocation-home-"));
const run = join(home, ".aperture", "run");
const root = join(run, "revocations");
for (const dir of [home, join(home, ".aperture"), run, root]) {
  mkdirSync(dir, { recursive: true, mode: 0o700 });
  chmodSync(dir, 0o700);
}
process.env.HOME = home;
process.env.APERTURE_RUN_DIR = run;
process.env.APERTURE_REVOCATION_DIR = root;
const { identityIsRevoked, readRevocation, revokeGeneration } = await import("../dist/revocation-store.js");

const tokenId = (byte) => byte.repeat(64);

test("revocation floor survives reload and never forgets earlier generations", () => {
  const first = revokeGeneration("alpha-worker", 1, tokenId("a"));
  assert.equal(first.revoked_through_generation, 1);
  const second = revokeGeneration("alpha-worker", 2, tokenId("b"));
  assert.equal(second.revoked_through_generation, 2);

  const reloaded = readRevocation("alpha-worker");
  assert.deepEqual(reloaded.revoked_token_ids, [tokenId("a"), tokenId("b")]);
  assert.equal(identityIsRevoked("alpha-worker", 1, tokenId("a")), true);
  assert.equal(identityIsRevoked("alpha-worker", 2, tokenId("b")), true);
  assert.equal(identityIsRevoked("alpha-worker", 3, tokenId("c")), false);
  assert.throws(
    () => revokeGeneration("alpha-worker", 3, tokenId("a")),
    /token reuse is invalid/,
  );
});

test("corrupt or unsafe revocation state fails closed", () => {
  const path = join(root, "corrupt-worker.json");
  writeFileSync(path, "{not-json}\n", { mode: 0o600 });
  assert.throws(() => identityIsRevoked("corrupt-worker", 3, tokenId("c")), /E_REVOCATION_CORRUPT/);
  writeFileSync(path, JSON.stringify({
    schema_version: 1,
    seat: "corrupt-worker",
    revoked_through_generation: 1,
    revoked_token_ids: [tokenId("a")],
  }));
  chmodSync(path, 0o644);
  assert.throws(() => readRevocation("corrupt-worker"), /unsafe revocation file/);
});
