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

test("rendered commands run from THIS checkout root (a non-master worktree in CI/dev): every hook exits 0; SessionStart injects the index block, not the bank", () => {
  const { env, dir } = baseEnv(); env.APERTURE_PROJECT_DIR = ROOT;
  for (const h of hooks) {
    const r = runHook(h.ev, h.command, env);
    assert.equal(r.status, 0, `${h.ev} ${h.command}\nstderr=${r.stderr}`);
    if (h.ev === "SessionStart") {
      assert.match(r.stdout, /memory-index mode=boot|## Memory index/, "SessionStart must inject the memory index block");
      assert.doesNotMatch(r.stdout, /## Persistent Memories/, "never the full bank");
      assert.ok(Buffer.byteLength(r.stdout) < 40 * 1024, `boot block ${Buffer.byteLength(r.stdout)} B exceeds 40 KiB`);
    }
    if (h.ev === "PreCompact") assert.match(r.stdout, /memory-index mode=precompact/);
    if (h.command.includes("memory-recall.js")) assert.match(r.stdout, /^\[memory recall\]|^\[recall unavailable/m);
  }
  rmSync(dir, { recursive: true, force: true });
});

test("resolution follows the runtime root: a second root with the same layout works; a root without the files fails loudly (no silent master fallback)", () => {
  const { env, dir } = baseEnv();
  // canonical-vs-worktree parity: a different root that carries scripts/ + mcp-server/dist behaves identically
  const alt = join(dir, "alt root with spaces"); mkdirSync(join(alt, "mcp-server"), { recursive: true });
  symlinkSync(join(ROOT, "scripts"), join(alt, "scripts")); symlinkSync(join(ROOT, "mcp-server", "dist"), join(alt, "mcp-server", "dist"));
  symlinkSync(join(ROOT, "mcp-server", "node_modules"), join(alt, "mcp-server", "node_modules"));
  symlinkSync(join(ROOT, "docs"), join(alt, "docs")); // sidecar seed lives under <root>/docs
  const boot = hooks.find((h) => h.ev === "SessionStart");
  const ok = runHook("SessionStart", boot.command, { ...env, APERTURE_PROJECT_DIR: alt });
  assert.equal(ok.status, 0, ok.stderr);
  assert.match(ok.stdout, /memory-index mode=boot|## Memory index/);
  // a root that lacks the files must NOT be papered over by master: the command fails and prints nothing
  const empty = join(dir, "empty-root"); mkdirSync(empty);
  const bad = runHook("SessionStart", boot.command, { ...env, APERTURE_PROJECT_DIR: empty });
  assert.notEqual(bad.status, 0, "missing script under the selected root must fail, not fall back");
  assert.equal(bad.stdout, "");
  // CLAUDE_PROJECT_DIR (set by Claude Code itself) is honoured when the launcher var is absent
  const viaClaude = runHook("SessionStart", boot.command, { ...env, CLAUDE_PROJECT_DIR: ROOT });
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
