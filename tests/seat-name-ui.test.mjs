import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { createServer } from "vite";

import { isValidSeatName } from "../src/services/seat-name.ts";

const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = resolve(HERE, "..");
const CASES = JSON.parse(readFileSync(resolve(HERE, "fixtures/seat-name-cases.json"), "utf8"));

test("UI validator consumes the shared canonical boundary fixture", () => {
  for (const name of CASES.valid) assert.equal(isValidSeatName(name), true, name);
  for (const name of CASES.invalid) assert.equal(isValidSeatName(name), false, name);
});

test("below-loader invalid identity is escaped by the actual AgentCard component", async () => {
  const injected = '<img src=x onerror="globalThis.pwned=1">';
  class FakeElement {
    className = "";
    dataset = {};
    innerHTML = "";
    listeners = [];
    style = { setProperty() {} };
    addEventListener(type, handler) { this.listeners.push([type, handler]); }
    querySelector(selector) {
      if (![".agent-mini__config", ".agent-mini__restart", ".agent-mini__toggle"].includes(selector)) return null;
      return { addEventListener: (type, handler) => this.listeners.push([`${selector}:${type}`, handler]) };
    }
  }
  const priorDocument = globalThis.document;
  const priorPwned = globalThis.pwned;
  globalThis.document = { createElement: () => new FakeElement() };
  const vite = await createServer({ root: ROOT, appType: "custom", logLevel: "silent", server: { middlewareMode: true } });
  try {
    const { createAgentCard } = await vite.ssrLoadModule("/src/components/AgentCard.ts");
    const card = createAgentCard(
      {
        name: injected,
        model: `${injected}-model`,
        role: "fixture",
        status: "stopped",
        emoji: injected,
        tmux_window_id: null,
        attention: false,
      },
      { open() {} },
      () => {},
      { start() {}, stop() {}, restart() {} },
    );
    assert.equal(card.innerHTML.includes("<img"), false, "no executable image element is created");
    assert.equal(card.innerHTML.includes("onerror=\""), false, "attribute text cannot escape into a handler");
    assert.match(card.innerHTML, /&lt;img src=x onerror=&quot;globalThis\.pwned=1&quot;&gt;/);
    assert.equal(globalThis.pwned, priorPwned, "injected handler never executes");
    assert.deepEqual(
      card.listeners.map(([type]) => type),
      ["click", ".agent-mini__config:click", ".agent-mini__restart:click", ".agent-mini__toggle:click"],
      "only the component's fixed handlers are registered",
    );
  } finally {
    await vite.close();
    if (priorDocument === undefined) delete globalThis.document;
    else globalThis.document = priorDocument;
    if (priorPwned === undefined) delete globalThis.pwned;
    else globalThis.pwned = priorPwned;
  }
});
