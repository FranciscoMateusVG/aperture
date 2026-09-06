// aperture-prime.sh failure-shape receipts (GLaDOS holds #1–#3, aperture-trgpo).
// Spawns the real script with stub `bd` binaries on PATH and an empty MCP dist dir so the
// index half prints its own unavailable line and never masks the preamble behaviour.
import { test } from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtempSync, writeFileSync, chmodSync, mkdirSync, readFileSync, existsSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const SCRIPT = resolve(here, "..", "..", "scripts", "aperture-prime.sh");

const GOOD_PREAMBLE = `# Beads Workflow Context

> **Context Recovery**: Run \`bd prime\` after compaction

## Core Rules
- Default: Use beads for ALL task tracking

## Essential Commands
### Finding Work
- \`bd ready\`
`;
const BANK = `## Persistent Memories (2)
### secret-ish-memory-2026-09-01
password=hunter2 lives in the drawer SENTINEL_BANK_LINE_A
### another-memory-2026-09-02
SENTINEL_BANK_LINE_B
`;

function stubBd(dir, body) {
  const bin = join(dir, "bin"); mkdirSync(bin, { recursive: true });
  const f = join(bin, "bd");
  writeFileSync(f, body); chmodSync(f, 0o755);
  return bin;
}
function run(bin, mode = "boot", extraEnv = {}) {
  const env = { ...process.env, PATH: `${bin}:${process.env.PATH}`, APERTURE_MCP_DIST: join(bin, "no-dist") };
  delete env.APERTURE_HUB_TOKEN_FILE; // operator-style session → preamble path is exercised
  Object.assign(env, extraEnv);
  const t0 = Date.now();
  const r = spawnSync("bash", [SCRIPT, mode], { env, encoding: "utf8", timeout: 30000 });
  return { ...r, ms: Date.now() - t0 };
}

test("hold #1: a hanging bd is killed at the deadline — child gone, wrapper returns in ~5s, exit 0", () => {
  const dir = mkdtempSync(join(tmpdir(), "prime-hang-"));
  const pidFile = join(dir, "bd.pid");
  const bin = stubBd(dir, `#!/bin/bash\necho $$ > "${pidFile}"\nexec sleep 60\n`);
  const r = run(bin);
  assert.equal(r.status, 0);
  assert.ok(r.ms >= 4500 && r.ms < 12000, `returned in ${r.ms} ms (deadline is 5 s)`);
  assert.ok(existsSync(pidFile), "stub recorded its pid");
  const pid = Number(readFileSync(pidFile, "utf8").trim());
  let alive = true;
  try { process.kill(pid, 0); } catch { alive = false; }
  assert.equal(alive, false, `hung bd (pid ${pid}) must be dead, not merely abandoned`);
  assert.ok(r.stdout.includes("[bd workflow preamble unavailable"), r.stdout);
  assert.ok(!r.stdout.includes("SENTINEL"), "no bank text");
  rmSync(dir, { recursive: true, force: true });
});

test("hold #2: no output path ever advises re-running bd prime", () => {
  const src = readFileSync(SCRIPT, "utf8");
  assert.ok(!/manually/i.test(src), "script must not contain 'manually'");
  assert.ok(!/run bd prime/i.test(src.replace(/^#.*$/gm, "")), "no non-comment line says 'run bd prime'");
});

test("hold #3a: a recognised preamble is emitted with the bank stripped — no memory line survives", () => {
  const dir = mkdtempSync(join(tmpdir(), "prime-good-"));
  const bin = stubBd(dir, `#!/bin/bash\ncat <<'EOF'\n${GOOD_PREAMBLE}${BANK}EOF\n`);
  const r = run(bin);
  assert.equal(r.status, 0);
  assert.ok(r.stdout.startsWith("# Beads Workflow Context"), r.stdout.slice(0, 80));
  assert.ok(r.stdout.includes("## Core Rules"));
  assert.ok(!r.stdout.includes("Persistent Memories"));
  assert.ok(!r.stdout.includes("SENTINEL") && !r.stdout.includes("hunter2") && !r.stdout.includes("### secret-ish"));
  rmSync(dir, { recursive: true, force: true });
});

test("hold #3b: renamed memory header (no recognised structure) → preamble suppressed, nothing leaks", () => {
  const dir = mkdtempSync(join(tmpdir(), "prime-renamed-"));
  const bin = stubBd(dir, `#!/bin/bash\ncat <<'EOF'\n${GOOD_PREAMBLE}${BANK.replace("## Persistent Memories (2)", "## Memories")}EOF\n`);
  const r = run(bin);
  assert.equal(r.status, 0);
  assert.ok(r.stdout.includes("[bd workflow preamble suppressed"), r.stdout);
  assert.ok(!r.stdout.includes("SENTINEL") && !r.stdout.includes("hunter2"));
  rmSync(dir, { recursive: true, force: true });
});

test("hold #3c: bank-only output (no workflow header) → suppressed; small bank cannot pass on size", () => {
  const dir = mkdtempSync(join(tmpdir(), "prime-bankonly-"));
  const bin = stubBd(dir, `#!/bin/bash\ncat <<'EOF'\n${BANK}EOF\n`);
  const r = run(bin);
  assert.equal(r.status, 0);
  assert.ok(r.stdout.includes("[bd workflow preamble unavailable") || r.stdout.includes("[bd workflow preamble suppressed"), r.stdout);
  assert.ok(!r.stdout.includes("SENTINEL"));
  rmSync(dir, { recursive: true, force: true });
});

test("hold #3d: memory-entry headings smuggled ABOVE the header → suppressed", () => {
  const dir = mkdtempSync(join(tmpdir(), "prime-smuggle-"));
  const bin = stubBd(dir, `#!/bin/bash\ncat <<'EOF'\n# Beads Workflow Context\n### leaked-memory-2026-09-06\nSENTINEL_SMUGGLED\n## Core Rules\n${BANK}EOF\n`);
  const r = run(bin);
  assert.ok(r.stdout.includes("[bd workflow preamble suppressed"), r.stdout);
  assert.ok(!r.stdout.includes("SENTINEL"));
  rmSync(dir, { recursive: true, force: true });
});

test("agent session (APERTURE_HUB_TOKEN_FILE set): preamble omitted entirely, bd never consulted", () => {
  const dir = mkdtempSync(join(tmpdir(), "prime-agent-"));
  const marker = join(dir, "called");
  const bin = stubBd(dir, `#!/bin/bash\ntouch "${marker}"\necho x\n`);
  const r = run(bin, "boot", { APERTURE_HUB_TOKEN_FILE: "/nonexistent/wheatley.token" });
  assert.equal(r.status, 0);
  assert.ok(r.stdout.includes("[bd workflow preamble omitted for Aperture agents"));
  assert.ok(!existsSync(marker), "bd must not be invoked for agent sessions");
  rmSync(dir, { recursive: true, force: true });
});

test("precompact mode never touches bd", () => {
  const dir = mkdtempSync(join(tmpdir(), "prime-pre-"));
  const marker = join(dir, "called");
  const bin = stubBd(dir, `#!/bin/bash\ntouch "${marker}"\n`);
  const r = run(bin, "precompact");
  assert.equal(r.status, 0);
  assert.ok(!existsSync(marker));
  assert.ok(r.stdout.includes("[memory index unavailable"));
  rmSync(dir, { recursive: true, force: true });
});

// ── Claude hook transport (aperture-g4hku) ───────────────────────────────────────────────────────
// Claude Code shows a hook command's stdout to the model only up to 10,000 chars (measured 2026-09-06
// on 2.1.263; hooks reference); commands on one event are capped separately. So the Claude path is
// four SessionStart commands — `preamble`, `standing 1`, `standing 2`, `pointer` — each under the cap.
// These run the REAL dist against a stub bank so the chunking is exercised end to end.

const REAL_DIST = resolve(here, "..", "dist");
const STANDING_PART_CAP = 9500;
const POINTER_CAP = 1000;

/** 9 standing entries × ~1.2 KB → a ~11 KB block: splits into 2 parts at the 9,000 B default, under the 16 KiB block cap. */
function standingFixture() {
  const bank = {}; const meta = {};
  for (let i = 1; i <= 9; i++) {
    const key = `stand-rule-${i}-2026-09-0${i}`;
    bank[key] = `DECISION-S${i}: standing rule ${i} body. ${`Sentence ${i} of the standing rule that must never be split across hook parts. `.repeat(13)}`.trim();
    meta[key] = { standing: true, project: "aperture" };
  }
  bank["plain-note-2026-09-05"] = "A non-standing note about the ws-hub reconnect path.";
  return { bank, meta };
}

/** Real dist + stub bd serving `memories --json` (and `prime` for the operator preamble). */
function realEnv(extraEnv = {}) {
  const dir = mkdtempSync(join(tmpdir(), "prime-real-"));
  const { bank, meta } = standingFixture();
  const bin = stubBd(dir, `#!/bin/bash
if [ "$1" = "memories" ] && [ "$2" = "--json" ]; then cat <<'JSON'
${JSON.stringify(bank)}
JSON
exit 0; fi
if [ "$1" = "prime" ]; then cat <<'PRIME'
${GOOD_PREAMBLE}${BANK}PRIME
exit 0; fi
echo "stub bd: unexpected argv $*" >&2; exit 2
`);
  mkdirSync(join(dir, "run"), { recursive: true });
  writeFileSync(join(dir, "meta.json"), JSON.stringify(meta));
  const env = {
    ...process.env,
    PATH: `${bin}:${process.env.PATH}`,
    BD_PATH: join(bin, "bd"),
    APERTURE_MCP_DIST: REAL_DIST,
    APERTURE_MEMORY_META: join(dir, "meta.json"),
    APERTURE_MEMORY_CACHE: join(dir, "cache.json"),
    APERTURE_RUN_DIR: join(dir, "run"),
    APERTURE_STANDING_CACHE: join(dir, "run", "standing.md"),
    APERTURE_HUB_TOKEN_FILE: join(dir, "agent.token"), // agent session by default
  };
  Object.assign(env, extraEnv);
  return { dir, env, bin };
}
const runArgs = (env, args) => spawnSync("bash", [SCRIPT, ...args], { env, encoding: "utf8", timeout: 60000 });
const bytes = (s) => Buffer.byteLength(s, "utf8");
const entryLines = (s) => s.split("\n").filter((l) => l.startsWith("- **"));

test("preamble (agent session, APERTURE_HUB_TOKEN_FILE set): the one-line omitted notice, bd never consulted", () => {
  const dir = mkdtempSync(join(tmpdir(), "prime-preamble-agent-"));
  const marker = join(dir, "called");
  const bin = stubBd(dir, `#!/bin/bash\ntouch "${marker}"\necho x\n`);
  const r = run(bin, "preamble", { APERTURE_HUB_TOKEN_FILE: "/nonexistent/wheatley.token" });
  assert.equal(r.status, 0, r.stderr);
  assert.match(r.stdout, /^\[bd workflow preamble omitted for Aperture agents[^\]]*\]$/m, r.stdout);
  assert.equal(r.stdout.trim().split("\n").length, 1, "exactly one line");
  assert.ok(!existsSync(marker), "bd must not be invoked for agent sessions");
  assert.ok(bytes(r.stdout) < STANDING_PART_CAP);
  rmSync(dir, { recursive: true, force: true });
});

test("preamble (operator session, stub bd): the bank-stripped bd workflow preamble and nothing else", () => {
  const dir = mkdtempSync(join(tmpdir(), "prime-preamble-op-"));
  const bin = stubBd(dir, `#!/bin/bash\ncat <<'EOF'\n${GOOD_PREAMBLE}${BANK}EOF\n`);
  const r = run(bin, "preamble");
  assert.equal(r.status, 0, r.stderr);
  assert.ok(r.stdout.startsWith("# Beads Workflow Context"), r.stdout.slice(0, 80));
  assert.ok(r.stdout.includes("## Core Rules") && r.stdout.includes("## Essential Commands"));
  assert.ok(!r.stdout.includes("Persistent Memories") && !r.stdout.includes("SENTINEL") && !r.stdout.includes("hunter2"));
  assert.ok(!r.stdout.includes("memory-index") && !r.stdout.includes("## Standing decisions"), "preamble carries no index/standing block — those are their own commands");
  assert.ok(bytes(r.stdout) < STANDING_PART_CAP);
  rmSync(dir, { recursive: true, force: true });
});

test("standing 1 / standing 2 (real dist, stub bank): each ≤ 9,500 B; the concatenation carries every entry exactly once; a part beyond M is empty", () => {
  const { dir, env } = realEnv();
  const full = spawnSync(process.execPath, [join(REAL_DIST, "memory-index.js"), "--mode", "boot"], { env, encoding: "utf8", timeout: 60000 });
  assert.equal(full.status, 0, full.stderr);
  const expected = entryLines(full.stdout);
  assert.equal(expected.length, 9, `fixture renders 9 standing entries, got ${expected.length}:\n${full.stdout.slice(0, 400)}`);
  assert.ok(bytes(full.stdout) > STANDING_PART_CAP, "fixture block must exceed one part so the split is exercised");

  const p1 = runArgs(env, ["standing", "1"]);
  const p2 = runArgs(env, ["standing", "2"]);
  for (const [n, p] of [[1, p1], [2, p2]]) {
    assert.equal(p.status, 0, p.stderr);
    assert.ok(bytes(p.stdout) <= STANDING_PART_CAP, `standing ${n} is ${bytes(p.stdout)} B > ${STANDING_PART_CAP}`);
    assert.match(p.stdout, new RegExp(`^<!-- memory-standing part ${n}/2 built=\\S+ hash=[0-9a-f]{12} -->$`, "m"), p.stdout.slice(0, 200));
    assert.match(p.stdout, new RegExp(`^## Standing decisions \\(9 total; part ${n}/2\\)$`, "m"));
    assert.doesNotMatch(p.stdout, /unavailable|STANDING BLOCK OVER BUDGET/, p.stdout);
    for (const line of entryLines(p.stdout)) assert.ok(expected.includes(line), `entry split or altered in part ${n}: ${line.slice(0, 80)}`);
  }
  assert.ok(entryLines(p1.stdout).length >= 1 && entryLines(p2.stdout).length >= 1, "both parts carry entries");
  const union = `${p1.stdout}${p2.stdout}`;
  for (const line of expected) {
    const n = union.split(line).length - 1;
    assert.equal(n, 1, `entry must appear exactly once across parts (saw ${n}): ${line.slice(0, 80)}`);
  }
  assert.ok(!union.includes("## Memory index") && !union.includes("plain-note-2026-09-05"), "standing parts never carry the index or the bank");

  const p3 = runArgs(env, ["standing", "3"]);
  assert.equal(p3.status, 0, p3.stderr);
  assert.equal(p3.stdout.trim(), "", `a part beyond M must print nothing, got: ${p3.stdout.slice(0, 120)}`);
  rmSync(dir, { recursive: true, force: true });
});

test("pointer (real dist, stub bank): ≤ 1,000 B, names recall_full, never the index lines or the bank", () => {
  const { dir, env } = realEnv();
  const r = runArgs(env, ["pointer"]);
  assert.equal(r.status, 0, r.stderr);
  assert.ok(bytes(r.stdout) <= POINTER_CAP, `pointer is ${bytes(r.stdout)} B > ${POINTER_CAP}`);
  assert.match(r.stdout, /^<!-- memory-index mode=pointer built=\S+ hash=[0-9a-f]{12} -->$/m, r.stdout);
  assert.match(r.stdout, /^## Memory index: \d+ live, \d+ superseded hidden, \d+ secret excluded — index not injected/m, r.stdout);
  assert.ok(r.stdout.includes("recall_full"));
  assert.ok(!r.stdout.includes("- **") && !r.stdout.includes("plain-note-2026-09-05"), "pointer carries no entries");
  rmSync(dir, { recursive: true, force: true });
});

test("standing 1 with no dist → exit 0 and one unavailable line, never a stack trace or the bank", () => {
  const dir = mkdtempSync(join(tmpdir(), "prime-standing-nodist-"));
  const bin = stubBd(dir, `#!/bin/bash\ncat <<'EOF'\n${GOOD_PREAMBLE}${BANK}EOF\n`);
  // same shape as `run` (APERTURE_MCP_DIST=<bin>/no-dist) plus the part number
  const r1 = spawnSync("bash", [SCRIPT, "standing", "1"], { env: { ...process.env, PATH: `${bin}:${process.env.PATH}`, APERTURE_MCP_DIST: join(bin, "no-dist"), APERTURE_HUB_TOKEN_FILE: "/nonexistent/wheatley.token" }, encoding: "utf8", timeout: 30000 });
  assert.equal(r1.status, 0, r1.stderr);
  assert.match(r1.stdout, /^\[[^\]]*unavailable[^\]]*\]$/m, r1.stdout);
  assert.equal(r1.stdout.trim().split("\n").length, 1, "exactly one line");
  assert.ok(!r1.stdout.includes("SENTINEL") && !/at .*\.js:\d+/.test(r1.stdout));
  rmSync(dir, { recursive: true, force: true });
});
