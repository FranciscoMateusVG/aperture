import test from "node:test";
import assert from "node:assert/strict";
import { createServer } from "vite";

const vite = await createServer({ appType: "custom", logLevel: "silent", server: { middlewareMode: true } });
const { describeError } = await vite.ssrLoadModule("/src/services/describe-error.ts");
await vite.close();

const FIXED = "command could not be completed; refresh before retry";

test("typed backend error renders operator copy plus code, never [object Object]", () => {
  const text = describeError({ code: "E_LIFECYCLE_DESCENDANTS_UNVERIFIED", message: FIXED });
  assert.equal(text, "stop/restart blocked: child processes could not be verified (E_LIFECYCLE_DESCENDANTS_UNVERIFIED)");
  assert.ok(!text.includes("[object Object]"));
});

test("tmux failure has its own copy", () => {
  assert.equal(describeError({ code: "E_TMUX_UNVERIFIED", message: FIXED }), "tmux window could not be created or verified (E_TMUX_UNVERIFIED)");
});

test("unknown code falls back to the backend message plus code", () => {
  assert.equal(describeError({ code: "E_WEB_COMMAND_FAILED", message: FIXED }), `${FIXED} (E_WEB_COMMAND_FAILED)`);
});

test("code without message still identifies itself", () => {
  assert.equal(describeError({ code: "E_SOMETHING" }), "operation failed (E_SOMETHING)");
});

test("Error instances and strings pass through", () => {
  assert.equal(describeError(new Error("boom")), "boom");
  assert.equal(describeError("plain"), "plain");
});

test("shapeless values never render as [object Object]", () => {
  for (const v of [{}, null, undefined, 42, [], { message: 7 }]) {
    const text = describeError(v);
    assert.equal(text, "operation failed (unknown error)", JSON.stringify(v));
  }
});

test("prototype-named codes never resolve to inherited objects", () => {
  for (const code of ["__proto__", "constructor", "toString", "hasOwnProperty"]) {
    const text = describeError({ code, message: FIXED });
    assert.equal(text, `${FIXED} (${code})`, code);
    assert.ok(!text.includes("[object Object]"));
  }
});

test("operator copy names the real consequence", () => {
  assert.equal(describeError({ code: "E_COORDINATOR_SELF_STOP", message: FIXED }), "stopping this agent would also stop the Aperture server (E_COORDINATOR_SELF_STOP)");
  assert.equal(describeError({ code: "E_LIFECYCLE_OUTCOME_UNKNOWN", message: FIXED }), "lifecycle outcome unknown; inspect before retry (E_LIFECYCLE_OUTCOME_UNKNOWN)");
  assert.equal(describeError({ code: "E_TMUX_OUTCOME_UNKNOWN", message: FIXED }), "tmux outcome unknown; inspect before retry (E_TMUX_OUTCOME_UNKNOWN)");
});
