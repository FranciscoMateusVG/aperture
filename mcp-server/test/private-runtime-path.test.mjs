import { test } from "node:test";
import assert from "node:assert/strict";
import { chmodSync, mkdirSync, mkdtempSync, rmSync, symlinkSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const moduleUrl = pathToFileURL(resolve(here, "..", "dist", "private-runtime-path.js")).href;

function run(home, child) {
  const runDir = join(home, ".aperture", "run");
  return spawnSync(process.execPath, [
    "--input-type=module",
    "--eval",
    `const m=await import(${JSON.stringify(moduleUrl)}); m.fixedRuntimeChild(process.env.RUNTIME_CHILD_PATH, process.env.RUNTIME_CHILD);`,
  ], {
    env: {
      ...process.env,
      HOME: home,
      APERTURE_RUN_DIR: runDir,
      RUNTIME_CHILD: child,
      RUNTIME_CHILD_PATH: join(runDir, child),
    },
    encoding: "utf8",
  });
}

function privateDir(path) {
  mkdirSync(path, { recursive: true, mode: 0o700 });
  chmodSync(path, 0o700);
}

test("fixed runtime roots reject intermediate and leaf symlinks component-by-component", () => {
  const fixture = mkdtempSync(join(tmpdir(), "aperture-runtime-path-"));
  chmodSync(fixture, 0o700);
  try {
    const positive = join(fixture, "positive");
    privateDir(join(positive, ".aperture", "run"));
    assert.equal(run(positive, "hub-tokens").status, 0, "private fixed child is accepted");
    assert.equal(run(positive, "revocations").status, 0, "both durable roots use the same component guard");

    const parentSwap = join(fixture, "parent-swap");
    const redirected = join(fixture, "redirected-parent");
    privateDir(parentSwap);
    privateDir(join(redirected, "run"));
    symlinkSync(redirected, join(parentSwap, ".aperture"));
    assert.notEqual(run(parentSwap, "hub-tokens").status, 0, "intermediate .aperture symlink fails closed");

    const runSwap = join(fixture, "run-swap");
    const redirectedRun = join(fixture, "redirected-run");
    privateDir(join(runSwap, ".aperture"));
    privateDir(redirectedRun);
    symlinkSync(redirectedRun, join(runSwap, ".aperture", "run"));
    assert.notEqual(run(runSwap, "revocations").status, 0, "intermediate run symlink fails closed");

    const leafSwap = join(fixture, "leaf-swap");
    const redirectedLeaf = join(fixture, "redirected-leaf");
    privateDir(join(leafSwap, ".aperture", "run"));
    privateDir(redirectedLeaf);
    symlinkSync(redirectedLeaf, join(leafSwap, ".aperture", "run", "hub-tokens"));
    assert.notEqual(run(leafSwap, "hub-tokens").status, 0, "leaf directory symlink fails closed");
  } finally {
    rmSync(fixture, { recursive: true, force: true });
  }
});
