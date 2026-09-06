// aperture-3kavd P0 — the tracked .claude/settings.json hooks must resolve through the launch-selected
// runtime root, never a build-machine absolute path. A fresh Peppy session on the candidate worktree
// (2026-09-06) ran hooks pointing at /Users/.../projects/aperture (master, files absent): exit 127 / 1,
// no index injection, no recall, no busy/idle. These tests render the REAL commands and run them.
import { test } from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { readFileSync, mkdtempSync, mkdirSync, symlinkSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const ROOT = resolve(here, "..", "..");
const settings = JSON.parse(readFileSync(join(ROOT, ".claude", "settings.json"), "utf8"));
const hooks = Object.entries(settings.hooks).flatMap(([ev, lst]) => lst.flatMap((h) => (h.hooks ?? []).map((c) => ({ ev, command: c.command }))));

const runHook = (ev, command, env, payloadPrompt = "why does the hub replay messages after a reconnect?") => {
  const payload = JSON.stringify({ hook_event_name: ev, prompt: payloadPrompt, session_id: "t", cwd: env.APERTURE_PROJECT_DIR ?? ROOT });
  return spawnSync("sh", ["-c", command], { input: payload, encoding: "utf8", timeout: 30000, env });
};
const baseEnv = () => {
  const dir = mkdtempSync(join(tmpdir(), "hooks-"));
  const tok = join(dir, "hub-tokens", "gate.token"); mkdirSync(dirname(tok), { recursive: true }); writeFileSync(tok, "dummy", { mode: 0o600 });
  const env = { ...process.env, APERTURE_HUB_TOKEN_FILE: tok, APERTURE_RECALL_TIMEOUT_MS: "5000" };
  delete env.APERTURE_PROJECT_DIR; delete env.CLAUDE_PROJECT_DIR;
  return { env, dir };
};

test("tracked settings.json: no hook command carries a build-machine absolute path; all resolve via APERTURE_PROJECT_DIR", () => {
  assert.ok(hooks.length >= 6, `expected the 6 diet/presence hooks, got ${hooks.length}`);
  for (const h of hooks) {
    assert.doesNotMatch(h.command, /\/Users\/|\/home\/[a-z]/, `${h.ev}: absolute build-machine path in ${h.command}`);
    assert.match(h.command, /\$\{APERTURE_PROJECT_DIR:-\$\{CLAUDE_PROJECT_DIR:\?[^}]+\}\}/, `${h.ev}: must resolve through the launch-selected root and fail loudly when unset: ${h.command}`);
    assert.doesNotMatch(h.command, /\$HOME\/projects\/aperture/, `${h.ev}: no silent fallback to a fixed checkout`);
  }
});

// Claude Code shows the model at most 10,000 chars of a hook command's stdout (measured 2026-09-06 on
// 2.1.263; hooks reference) — commands on one event are capped SEPARATELY. So visibility, not emission,
// is the property: every command under the cap, and the standing rules in the UNION (aperture-g4hku).
const HOOK_VISIBLE_MAX = 9800;
const byEvent = (ev) => hooks.filter((h) => h.ev === ev);

test("tracked settings.json: SessionStart = preamble + standing 1 + standing 2 + pointer (4 commands); PreCompact = standing 1 + standing 2 + pointer (3)", () => {
  const ss = byEvent("SessionStart").map((h) => h.command);
  const pc = byEvent("PreCompact").map((h) => h.command);
  assert.equal(ss.length, 4, `SessionStart commands:\n${ss.join("\n")}`);
  assert.equal(pc.length, 3, `PreCompact commands:\n${pc.join("\n")}`);
  const tail = (c) => c.replace(/^.*aperture-prime\.sh\s+/, "").trim();
  assert.deepEqual(ss.map(tail), ["preamble", "standing 1", "standing 2", "pointer"]);
  assert.deepEqual(pc.map(tail), ["standing 1", "standing 2", "pointer"]);
  for (const c of [...ss, ...pc]) assert.doesNotMatch(c, /aperture-prime\.sh\s+(boot|precompact)\b/, `Codex-only whole-block modes must not be Claude hooks: ${c}`);
});

test("rendered commands run from THIS checkout root (a non-master worktree in CI/dev): every hook exits 0; each SessionStart/PreCompact command ≤ 9,800 B; the union carries the standing block, never the bank", () => {
  const { env, dir } = baseEnv(); env.APERTURE_PROJECT_DIR = ROOT;
  const union = { SessionStart: "", PreCompact: "" };
  const counts = { SessionStart: 0, PreCompact: 0 };
  for (const h of hooks) {
    const r = runHook(h.ev, h.command, env);
    assert.equal(r.status, 0, `${h.ev} ${h.command}\nstderr=${r.stderr}`);
    if (h.ev === "SessionStart" || h.ev === "PreCompact") {
      const b = Buffer.byteLength(r.stdout, "utf8");
      assert.ok(b <= HOOK_VISIBLE_MAX, `${h.ev}#${counts[h.ev] + 1} prints ${b} B > ${HOOK_VISIBLE_MAX} — invisible to the model (10 KB hook cap): ${h.command}`);
      assert.doesNotMatch(r.stdout, /## Persistent Memories/, "never the full bank");
      assert.doesNotMatch(r.stdout, /standing part \d+ unavailable|STANDING BLOCK OVER BUDGET/, r.stdout.slice(0, 300));
      union[h.ev] += `${r.stdout}\n`; counts[h.ev] += 1;
    }
    if (h.command.includes("memory-recall.js")) assert.match(r.stdout, /^\[memory recall\]|^\[recall unavailable/m);
  }
  assert.equal(counts.SessionStart, 4); assert.equal(counts.PreCompact, 3);
  for (const ev of ["SessionStart", "PreCompact"]) {
    assert.match(union[ev], /^## Standing decisions/m, `${ev} union must carry the standing block`);
    assert.match(union[ev], /^- \*\*[^*]+\*\*/m, `${ev} union must carry standing entries`);
    assert.match(union[ev], /memory-index mode=pointer|## Memory index/, `${ev} union must carry the index pointer`);
    assert.doesNotMatch(union[ev], /## Persistent Memories/, `${ev}: never the full bank`);
  }
  assert.match(union.SessionStart, /^\[bd workflow preamble omitted for Aperture agents/m, "agent session: the preamble command prints its one-line notice");
  rmSync(dir, { recursive: true, force: true });
});

test("resolution follows the runtime root: a second root with the same layout works; a root without the files fails loudly (no silent master fallback)", () => {
  const { env, dir } = baseEnv();
  // canonical-vs-worktree parity: a different root that carries scripts/ + mcp-server/dist behaves identically
  const alt = join(dir, "alt root with spaces"); mkdirSync(join(alt, "mcp-server"), { recursive: true });
  symlinkSync(join(ROOT, "scripts"), join(alt, "scripts")); symlinkSync(join(ROOT, "mcp-server", "dist"), join(alt, "mcp-server", "dist"));
  symlinkSync(join(ROOT, "mcp-server", "node_modules"), join(alt, "mcp-server", "node_modules"));
  symlinkSync(join(ROOT, "docs"), join(alt, "docs")); // sidecar seed lives under <root>/docs
  let altUnion = "";
  for (const h of byEvent("SessionStart")) {
    const ok = runHook("SessionStart", h.command, { ...env, APERTURE_PROJECT_DIR: alt });
    assert.equal(ok.status, 0, ok.stderr);
    altUnion += ok.stdout;
  }
  assert.match(altUnion, /^## Standing decisions/m);
  assert.match(altUnion, /memory-index mode=pointer|## Memory index/);
  // a root that lacks the files must NOT be papered over by master: the command fails and prints nothing
  const empty = join(dir, "empty-root"); mkdirSync(empty);
  for (const h of byEvent("SessionStart")) {
    const bad = runHook("SessionStart", h.command, { ...env, APERTURE_PROJECT_DIR: empty });
    assert.notEqual(bad.status, 0, "missing script under the selected root must fail, not fall back");
    assert.equal(bad.stdout, "");
  }
  // CLAUDE_PROJECT_DIR (set by Claude Code itself) is honoured when the launcher var is absent
  const viaClaude = runHook("SessionStart", byEvent("SessionStart")[0].command, { ...env, CLAUDE_PROJECT_DIR: ROOT });
  assert.equal(viaClaude.status, 0, viaClaude.stderr);
  // BOTH unset → explicit failure, never a silent fixed checkout (the old master fallback is gone)
  for (const h of hooks) {
    const r = runHook(h.ev, h.command, env);
    assert.notEqual(r.status, 0, `${h.ev}: must fail loudly with no runtime root`);
    assert.match(r.stderr, /no runtime root/, `${h.ev}: stderr must say why: ${r.stderr}`);
    assert.equal(r.stdout, "", `${h.ev}: nothing on stdout without a root`);
  }
  rmSync(dir, { recursive: true, force: true });
});
