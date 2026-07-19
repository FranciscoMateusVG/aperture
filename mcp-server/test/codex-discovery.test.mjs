import test from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, dirname, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const TMP = mkdtempSync(join(tmpdir(), "codex-discovery-"));
const AGENTS = join(TMP, "agents");
const CONFIG = join(TMP, "agent-config.json");
mkdirSync(AGENTS);

function manifest(name, model) {
  const dir = join(AGENTS, name);
  mkdirSync(dir);
  writeFileSync(join(dir, "manifest.json"), JSON.stringify({ model }));
}

manifest("rex", "opus");
manifest("scout", "codex/default");
manifest("vance", "sonnet");

const here = dirname(fileURLToPath(import.meta.url));
const { discoverCodexAgents } = await import(
  pathToFileURL(resolve(here, "..", "dist", "codex-bridge.js")).href
);

test("panel overrides win over stock manifest models", () => {
  writeFileSync(CONFIG, JSON.stringify({ rex: "codex/gpt-5.6-terra", scout: "opus" }));
  assert.deepEqual(discoverCodexAgents(AGENTS, CONFIG), ["rex"]);
});

test("missing override file falls back to manifests", () => {
  assert.deepEqual(discoverCodexAgents(AGENTS, join(TMP, "missing.json")), ["scout"]);
});

test("malformed override file falls back to manifests", () => {
  writeFileSync(CONFIG, "not-json");
  assert.deepEqual(discoverCodexAgents(AGENTS, CONFIG), ["scout"]);
});

test.after(() => rmSync(TMP, { recursive: true, force: true }));
