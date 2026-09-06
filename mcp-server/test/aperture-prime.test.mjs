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
