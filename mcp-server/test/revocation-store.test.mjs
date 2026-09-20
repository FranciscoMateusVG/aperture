import { test } from "node:test";
import assert from "node:assert/strict";
import { chmodSync, mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  identityIsRevoked,
  readRevocation,
  revokeGeneration,
} from "../dist/revocation-store.js";

const tokenId = (byte) => byte.repeat(64);

test("revocation floor survives reload and never forgets earlier generations", () => {
  const parent = mkdtempSync(join(tmpdir(), "aperture-revocation-"));
  chmodSync(parent, 0o700);
  const root = join(parent, "store");
  mkdirSync(root, { mode: 0o700 });

  const first = revokeGeneration("alpha-worker", 1, tokenId("a"), root);
  assert.equal(first.revoked_through_generation, 1);
  const second = revokeGeneration("alpha-worker", 2, tokenId("b"), root);
  assert.equal(second.revoked_through_generation, 2);

  const reloaded = readRevocation("alpha-worker", root);
  assert.deepEqual(reloaded.revoked_token_ids, [tokenId("a"), tokenId("b")]);
  assert.equal(identityIsRevoked("alpha-worker", 1, tokenId("a"), root), true);
  assert.equal(identityIsRevoked("alpha-worker", 2, tokenId("b"), root), true);
  assert.equal(identityIsRevoked("alpha-worker", 3, tokenId("c"), root), false);
  assert.throws(
    () => revokeGeneration("alpha-worker", 3, tokenId("a"), root),
    /token reuse is invalid/,
  );
});

test("corrupt or unsafe revocation state fails closed", () => {
  const parent = mkdtempSync(join(tmpdir(), "aperture-revocation-corrupt-"));
  chmodSync(parent, 0o700);
  const root = join(parent, "store");
  mkdirSync(root, { mode: 0o700 });
  const path = join(root, "alpha-worker.json");
  writeFileSync(path, "{not-json}\n", { mode: 0o600 });
  assert.throws(() => identityIsRevoked("alpha-worker", 3, tokenId("c"), root), /E_REVOCATION_CORRUPT/);
  writeFileSync(path, JSON.stringify({
    schema_version: 1,
    seat: "alpha-worker",
    revoked_through_generation: 1,
    revoked_token_ids: [tokenId("a")],
  }));
  chmodSync(path, 0o644);
  assert.throws(() => readRevocation("alpha-worker", root), /unsafe revocation file/);
});
