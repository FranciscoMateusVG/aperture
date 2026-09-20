import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { isValidSeatName } from "../src/services/seat-name.ts";
import { escapeHtml } from "../src/utils/html.ts";

const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = resolve(HERE, "..");
const CASES = JSON.parse(readFileSync(resolve(HERE, "fixtures/seat-name-cases.json"), "utf8"));

test("UI validator consumes the shared canonical boundary fixture", () => {
  for (const name of CASES.valid) assert.equal(isValidSeatName(name), true, name);
  for (const name of CASES.invalid) assert.equal(isValidSeatName(name), false, name);
});

test("below-loader invalid identity is escaped at the actual AgentCard interpolation seam", () => {
  const injected = '<img src=x onerror="globalThis.pwned=1">';
  const rendered = `<span class="agent-mini__name">${escapeHtml(injected)}</span>`;
  assert.equal(rendered.includes("<img"), false);
  assert.match(rendered, /&lt;img/);

  const source = readFileSync(resolve(ROOT, "src/components/AgentCard.ts"), "utf8");
  assert.match(source, /agent-mini__name[^\n]*\$\{escapeHtml\(agent\.name\)\}/);
  assert.match(source, /agent-mini__model[^\n]*\$\{escapeHtml\(agent\.model\)\}/);
  assert.match(source, /agent-mini__icon[^\n]*\$\{escapeHtml\(icon\)\}/);
});
