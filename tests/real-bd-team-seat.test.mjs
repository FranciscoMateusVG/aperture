import test from "node:test";
import assert from "node:assert/strict";
import {
  lstatSync,
  mkdtempSync,
  mkdirSync,
  readFileSync,
  realpathSync,
  rmSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, relative, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

function isInside(root, candidate) {
  const rel = relative(realpathSync(root), realpathSync(candidate));
  return rel === "" || (rel !== ".." && !rel.startsWith(`..${sep}`));
}

function effectiveDatabasePath(rawInfo) {
  let parsed = null;
  try {
    parsed = JSON.parse(rawInfo);
  } catch {
    // bd 1.0.2 advertises --json for `info` but currently emits its text
    // report. Fall through to the documented "absolute path" line.
  }
  if (parsed !== null && typeof parsed === "object") {
    const candidates = [parsed.database_path, parsed.databasePath, parsed.database?.path]
      .filter((value) => value !== undefined);
    assert.equal(candidates.length, 1, "bd info JSON must contain exactly one effective database path field");
    assert.equal(typeof candidates[0], "string", "bd info effective database path must be a string");
    return candidates[0];
  }
  const labelled = [...rawInfo.matchAll(/^\s*Database\s*:\s*(\/.+)\s*$/gim)];
  assert.equal(labelled.length, 1, "bd info text must contain exactly one Database: absolute-path line");
  return labelled[0][1].trim();
}

function assertEffectiveDatabase(rawInfo, expectedDb, fixtureRoot) {
  const observed = effectiveDatabasePath(rawInfo);
  assert.equal(typeof observed, "string", "bd info must expose an absolute effective database path");
  assert.equal(realpathSync(observed), realpathSync(expectedDb), "bd opened a different database than the explicit fixture --db");
  assert.equal(isInside(fixtureRoot, observed), true, "effective bd database escaped the fixture root");
}

test("effective database gate rejects missing, duplicate, conflicting, and mismatched paths before create", () => {
  const root = mkdtempSync(join(tmpdir(), "aperture-v4-db-gate-"));
  const expected = join(root, "expected");
  const other = join(root, "other");
  mkdirSync(expected);
  mkdirSync(other);
  try {
    assert.throws(() => effectiveDatabasePath("Beads Database Information\nIssue Count: 0\n"), /exactly one/);
    assert.throws(
      () => effectiveDatabasePath(`Database: ${expected}\nDatabase: ${expected}\n`),
      /exactly one/,
    );
    assert.throws(
      () => effectiveDatabasePath(JSON.stringify({ database_path: expected, databasePath: other })),
      /exactly one/,
    );

    let createReached = false;
    const mismatchedAttempt = () => {
      assertEffectiveDatabase(`Database: ${other}\n`, expected, root);
      createReached = true;
    };
    assert.throws(mismatchedAttempt, /different database/);
    assert.equal(createReached, false, "a mismatched effective path stops before the create step");
  } finally {
    if (isInside(tmpdir(), root)) rmSync(root, { recursive: true, force: true });
  }
});

test("real bd fixture accepts a dynamic team assignee and message prose cannot mutate its target", () => {
  const root = mkdtempSync(join(tmpdir(), "aperture-v4-bd-fixture-"));
  const home = join(root, "home");
  const store = join(root, ".beads");
  const db = join(store, "embeddeddolt", "v4fixture");
  mkdirSync(home, { recursive: true });

  // Deliberately do NOT spread process.env. This is an allowlist, so no
  // BEADS_*, BD_*, DOLT_*, XDG config, credentials or live board routing can
  // leak into the fixture. Every bd command also receives the explicit store.
  const childEnv = {
    PATH: process.env.PATH ?? "/usr/bin:/bin:/usr/sbin:/sbin",
    HOME: home,
    TMPDIR: root,
    USER: "aperture-fixture",
    LOGNAME: "aperture-fixture",
    LANG: "C.UTF-8",
    LC_ALL: "C.UTF-8",
    TZ: "UTC",
    BD_NON_INTERACTIVE: "1",
    BEADS_DIR: store,
    BEADS_ACTOR: "fixture",
    GIT_AUTHOR_NAME: "Aperture Fixture",
    GIT_AUTHOR_EMAIL: "fixture@invalid.example",
    GIT_COMMITTER_NAME: "Aperture Fixture",
    GIT_COMMITTER_EMAIL: "fixture@invalid.example",
  };

  const run = (command, args, { actor } = {}) => {
    const result = spawnSync(command, args, {
      cwd: root,
      env: actor ? { ...childEnv, BEADS_ACTOR: actor } : childEnv,
      encoding: "utf8",
      timeout: 30_000,
    });
    assert.equal(
      result.status,
      0,
      `${command} ${args.join(" ")} failed before the fixture could continue: ${result.stderr}`,
    );
    return result.stdout.trim();
  };
  const bd = (args, actor) => run("bd", [...args, "--db", db], { actor });

  try {
    run("git", ["init", "--quiet"]);
    // Fail closed here: run() throws on a non-zero init, so no later bd
    // command can execute after an ambiguous/existing-store result.
    run("bd", ["init", "--non-interactive", "--skip-agents", "--skip-hooks", "-p", "v4fixture"]);

    const metadataPath = join(store, "metadata.json");
    const storeStat = lstatSync(store);
    const dbStat = lstatSync(db);
    assert.equal(storeStat.isDirectory() && !storeStat.isSymbolicLink(), true);
    assert.equal(dbStat.isDirectory() && !dbStat.isSymbolicLink(), true);
    assert.equal(isInside(root, store), true);
    assert.equal(isInside(root, db), true);
    const metadata = JSON.parse(readFileSync(metadataPath, "utf8"));
    assert.equal(metadata.backend, "dolt");
    assert.equal(metadata.dolt_mode, "embedded");
    assert.equal(metadata.dolt_database, "v4fixture");

    // A read-only preflight through the exact explicit --db path proves bd is
    // opening the empty fixture store before the first business write.
    const infoFixture = readFileSync(
      join(dirname(fileURLToPath(import.meta.url)), "fixtures", "bd-info-v1.0.2.txt"),
      "utf8",
    ).replace("__DATABASE_PATH__", db);
    assert.equal(effectiveDatabasePath(infoFixture), db, "installed bd 1.0.2 text interface fixture is pinned");
    const info = bd(["info", "--json"]);
    assertEffectiveDatabase(info, db, root);
    assert.deepEqual(JSON.parse(bd(["list", "--all", "--json"])), [], "explicit fixture DB is empty before writes");

    const created = JSON.parse(bd(["create", "seat claim target", "-p", "2", "--json"], "glados"));
    const targetId = created.id;
    assert.match(targetId, /^v4fixture-/);

    const claim = JSON.parse(bd(["update", targetId, "--claim", "--json"], "p1-backend"));
    const claimed = Array.isArray(claim) ? claim[0] : claim;
    assert.equal(claimed.assignee, "p1-backend");
    assert.equal(claimed.status, "in_progress");

    const targetBefore = bd(["show", targetId, "--json"]);
    const message = JSON.parse(
      bd([
        "create",
        "[glados->p1-backend] instruction",
        "-p",
        "3",
        "--type",
        "message",
        "-d",
        "reassign/cancel leaves the target bead byte-identical",
        "--json",
      ], "glados"),
    );
    assert.match(message.id, /^v4fixture-/);
    const targetAfter = bd(["show", targetId, "--json"]);
    assert.equal(targetAfter, targetBefore, "message prose does not reassign, cancel, or otherwise mutate the target bead");
  } finally {
    // This path was created by this test under the OS temp root. Never clean a
    // caller-provided or discovered BEADS directory.
    if (isInside(tmpdir(), root)) rmSync(root, { recursive: true, force: true });
  }
});
