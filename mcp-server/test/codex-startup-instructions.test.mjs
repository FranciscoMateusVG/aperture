// Rendered-startup regression (aperture-1socy): Codex sessions must never be told to start the
// Claude inbox monitor / hub-client or to hunt for a hub token; Claude sessions must keep that path.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync, readdirSync, existsSync } from "node:fs";
import { resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const repo = resolve(here, "..", "..");
const FORBIDDEN_FOR_CODEX = ["hub-client", "Monitor tool", "start your inbox monitor", "APERTURE_HUB_TOKEN"];
const agentModel = (n) => { const p = resolve(repo, "agents", n, "manifest.json"); return existsSync(p) ? JSON.parse(readFileSync(p, "utf8")).model : null; };
const agents = () => readdirSync(resolve(repo, "agents")).filter((n) => existsSync(resolve(repo, "agents", n, "manifest.json")));

test("Codex kickoff text: no monitor/hub-client/token step; keeps get_messages + mark-as-read; dist matches source", () => {
  const src = readFileSync(resolve(repo, "mcp-server", "src", "codex-bridge.ts"), "utf8");
  const m = src.match(/export const CODEX_KICKOFF_TEXT =\s*"([^"]+)"/);
  assert.ok(m, "CODEX_KICKOFF_TEXT literal present");
  const text = m[1];
  for (const bad of ["start your inbox monitor", "hub-client.js", "token file"]) assert.ok(!text.includes(bad), `kickoff must not say ${bad}`);
  assert.match(text, /do not start a monitor process or hub-client/i);
  assert.ok(text.includes("get_messages") && /mark each read/i.test(text));
  const dist = resolve(repo, "mcp-server", "dist", "codex-bridge.js");
  assert.ok(existsSync(dist), "dist/codex-bridge.js exists (run just build-mcp)");
  assert.ok(readFileSync(dist, "utf8").includes(text), "dist/codex-bridge.js carries the same kickoff text — stale dist ships the old boot");
});

test("rex (Codex) prompt: no Claude monitor / hub-client / token instructions; bridge-delivered inbox described", () => {
  assert.match(String(agentModel("rex")), /^codex\//, "rex manifest model is codex/*");
  const prompt = readFileSync(resolve(repo, "prompts", "rex.md"), "utf8");
  for (const bad of FORBIDDEN_FOR_CODEX) assert.ok(!prompt.includes(bad), `prompts/rex.md must not contain ${JSON.stringify(bad)}`);
  assert.ok(/bridge/i.test(prompt) && prompt.includes("get_messages") && prompt.includes("mark_as_read"));
  assert.match(prompt, /nothing for you to start/i);
});

test("Claude agents keep the Monitor + hub-client inbox path with their own name", () => {
  const claude = agents().filter((n) => !String(agentModel(n)).startsWith("codex/"));
  assert.ok(claude.length >= 1);
  for (const n of claude) {
    const prompt = readFileSync(resolve(repo, "prompts", `${n}.md`), "utf8");
    assert.ok(prompt.includes(`hub-client.js ${n}`), `prompts/${n}.md keeps 'hub-client.js ${n}'`);
    assert.ok(prompt.includes("Monitor tool"), `prompts/${n}.md keeps the Monitor tool instruction`);
  }
});

test("report only: other Codex agents still carrying Claude monitor instructions (fleet follow-up)", () => {
  const stale = agents().filter((n) => n !== "rex" && String(agentModel(n)).startsWith("codex/"))
    .filter((n) => FORBIDDEN_FOR_CODEX.some((bad) => readFileSync(resolve(repo, "prompts", `${n}.md`), "utf8").includes(bad)));
  console.log(`  [info] Codex prompts still carrying Claude monitor instructions (out of aperture-1socy scope): ${stale.join(", ") || "none"}`);
});
