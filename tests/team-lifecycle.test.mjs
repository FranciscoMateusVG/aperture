// Synthetic handler-contract fixture only. No browser, native invocation or installation.
import test from "node:test";
import assert from "node:assert/strict";
import { createServer } from "vite";
import { team, clone } from "./fixtures/team-ui.mjs";
const vite = await createServer({ appType: "custom", logLevel: "silent", server: { middlewareMode: true } });
const { openTeamLifecycle } = await vite.ssrLoadModule("/src/components/TeamLifecycle.ts");
const { createRuntimeCommands } = await vite.ssrLoadModule("/src/services/team-runtime.ts"); await vite.close();
const tuple = { harness: "codex", model: "gpt-6-astra", reasoning: "high" };
function current(enabled = true) { const t = team("active"); t.capabilities.replace = enabled; t.capabilities.archive = enabled; t.seats[0].observed_owner = { generation: 4, state: "active", since: "fixture-time", configured: clone(tuple), actual: clone(tuple), process_count: 2, thread_bound: true }; return t; }
function ready(t) { return { team: "t1", seat: "t1-backend", generation: 4, phase: "ready", checkpoint_recovery: "none", checks: { process_stop: "verified", revocation: "verified", remote_effects: "verified" }, owner: { ...clone(t.seats[0].observed_owner), state: "stale" }, blockers: [], preparation_id: "fixture-only-selector" }; }
async function run({ enabled = true, kind = "replace", call }, verify) {
 class El {
  disabled = false; checked = false; hidden = false; value = ""; textContent = ""; dataset = {}; attrs = {}; handlers = {}; isConnected = true;
  setAttribute(k,v) { this.attrs[k] = v; } addEventListener(k,fn) { this.handlers[k] = fn; }
  closest() { return this; } focus() { doc.activeElement = this; }
 }
 const t = current(enabled), origin = new El(), dialog = new El(), evidence = new El(), status = new El(), error = new El(), selection = new El(), confirm = new El(); confirm.parentElement = new El();
 Object.defineProperty(selection, "innerHTML", { set(html) { selection.html = html; const opts = [...html.matchAll(/<option value="([^"]*)"([^>]*)>/g)]; selection.value = (opts.find(x => /selected/.test(x[2])) ?? opts[0])?.[1] ?? ""; } });
 const buttons = Object.fromEntries(["refresh", "prepare", "start", "archive", "close"].map(name => { const e = new El(); e.dataset.action = name; return [name, e]; }));
 dialog.querySelector = s => ({ "[data-evidence]": evidence, "[data-status]": status, "[data-error]": error, "#v4-runtime-selection": kind === "replace" ? selection : null, "[data-confirm]": kind === "replace" ? confirm : null })[s] ?? buttons[/data-action="([^"]+)"/.exec(s)?.[1]] ?? null;
 dialog.showModal = () => { dialog.open = true; }; dialog.close = () => { dialog.open = false; dialog.handlers.close(); }; dialog.remove = () => { dialog.isConnected = false; };
 const doc = { activeElement: origin, createElement: () => dialog, body: { appendChild() {} } };
 const prior = { document: globalThis.document, HTMLElement: globalThis.HTMLElement }; globalThis.document = doc; globalThis.HTMLElement = El;
 let live = t; const calls = []; let reads = 0;
 const api = createRuntimeCommands(async (...args) => { calls.push(args); return call(...args, t); });
 openTeamLifecycle({ team: t, seat: "t1-backend", kind, api, current: () => live, refresh: async () => { reads++; return live; } });
 try { await verify({ t, calls, dialog, evidence, status, error, selection, confirm, buttons, doc, origin, setLive: v => { live = v; }, reads: () => reads, click: name => dialog.handlers.click({ target: buttons[name] }), change: index => { selection.value = String(index); selection.handlers.change(); }, confirmChange: () => { confirm.checked = true; confirm.handlers.change(); } }); }
 finally { for (const [k,v] of Object.entries(prior)) if (v === undefined) delete globalThis[k]; else globalThis[k] = v; }
}
test("unavailable dialog is read-only: disabled prepare/start/archive, no invoke even forged click", async () => {
 for (const kind of ["replace", "archive"]) await run({ enabled: false, kind, call: () => { throw Error("no invoke"); } }, async f => {
  assert.match(f.status.textContent, /Not available/); assert.equal(f.buttons.prepare.disabled, true); assert.equal(f.buttons.start.disabled, true); assert.equal(f.buttons.archive.disabled, true);
  f.buttons.prepare.disabled = false; await f.click("prepare"); assert.equal(f.calls.length, 0);
  await f.click("close"); assert.equal(f.doc.activeElement, f.origin);
 });
});
test("prepare promise shows pending, never auto-starts; ready-none checkpoint remains warning", async () => {
 let resolve;
 await run({ call: (_name, _args, t) => new Promise(r => { resolve = () => r(ready(t)); }) }, async f => {
  const promise = f.click("prepare"); assert.equal(f.calls.length, 1); assert.match(f.evidence.innerHTML, /Operation pending/); assert.doesNotMatch(f.evidence.innerHTML, /<strong>verified/);
  assert.equal(f.buttons.start.disabled, true); await f.click("prepare"); assert.equal(f.calls.length, 1);
  resolve(); await promise; assert.equal(f.calls.length, 1); assert.equal(f.buttons.start.disabled, false); assert.match(f.evidence.innerHTML, /none — warning/);
  assert.equal(f.evidence.innerHTML.includes("fixture-only-selector"), false);
 });
});
test("changing prepared selection invalidates local permit, needs refresh and never auto-starts", async () => {
 await run({ call: (_name, _args, t) => ready(t) }, async f => {
  await f.click("prepare"); f.change(1); assert.equal(f.buttons.start.disabled, true); assert.equal(f.buttons.prepare.disabled, true); assert.match(f.status.textContent, /Refresh authoritative/);
  assert.equal(f.calls.length, 1); await f.click("refresh"); assert.equal(f.reads(), 1); assert.equal(f.buttons.prepare.disabled, false); assert.equal(f.buttons.start.disabled, true);
 });
});
test("configuration change needs explicit intent confirmation; start once, consume permit, use returned generation", async () => {
 await run({ call: (name, args, t) => {
  const p = ready(t); if (name === "team_prepare_replacement") return p;
  delete p.preparation_id; return { ...p, phase: "started", generation: 8, owner: { ...p.owner, generation: 8, state: "active", configured: args.input.selection, actual: args.input.selection } };
 } }, async f => {
  f.change(1); await f.click("prepare"); assert.equal(f.buttons.start.disabled, true); f.confirmChange(); assert.equal(f.buttons.start.disabled, false);
  await f.click("start"); assert.equal(f.calls.length, 2); assert.equal(f.buttons.start.disabled, true); assert.match(f.evidence.innerHTML, /g8/); assert.equal(f.t.seats[0].observed_owner.generation, 4);
  assert.equal("confirmed" in f.calls[1][1].input, false); await f.click("start"); assert.equal(f.calls.length, 2);
 });
});
test("transport failure is unknown and requires refresh, not rollback/success/retry", async () => {
 await run({ call: () => { throw { code: "E_RUNTIME_IO", message: "SENTINEL_PRIVATE" }; } }, async f => {
  await f.click("prepare"); assert.match(f.evidence.innerHTML, /outcome: unknown/); assert.match(f.error.textContent, /outcome is unknown/);
  assert.doesNotMatch(f.error.textContent, /SENTINEL_PRIVATE/); assert.equal(f.buttons.prepare.disabled, true); assert.equal(f.calls.length, 1);
 });
});
test("closed in-flight operation is not cancelled; no second invoke, detached UI is not repainted", async () => {
 let resolve;
 await run({ call: (_name, _args, t) => new Promise(r => { resolve = () => r(ready(t)); }) }, async f => {
  const promise = f.click("prepare"); await f.click("close"); const prior = f.evidence.innerHTML; resolve(); await promise;
  assert.equal(f.dialog.open, false); assert.equal(f.calls.length, 1); assert.equal(f.evidence.innerHTML, prior); assert.equal(f.reads(), 1);
 });
});
test("generation changes and lost capability disable start before invocation", async () => {
 await run({ call: (_name, _args, t) => ready(t) }, async f => {
  await f.click("prepare"); const next = clone(f.t); next.seats[0].observed_owner.generation = 5; f.setLive(next);
  await f.click("start"); assert.equal(f.calls.length, 1); assert.equal(f.buttons.start.disabled, true);
 });
});
test("archive CTA is explicitly effectful, unknown review blocks completion, no local rollback", async () => {
 await run({ kind: "archive", call: () => ({ team: "t1", generation: 1, state: "blocked", checks: { reconciliation: "verified", reviews: "unknown", metrics: "verified", process_stop: "verified", revocation: "verified", remote_effects: "unknown", worktrees: "unknown" }, blockers: [{ code: "E_REVIEW_MISSING", reference: "fixture-review" }] }) }, async f => {
  assert.match(f.dialog.innerHTML, /not a read-only check/); assert.match(f.dialog.innerHTML, /Verify and archive/); await f.click("archive");
  assert.match(f.evidence.innerHTML, /Backend archive state: blocked/); assert.doesNotMatch(f.evidence.innerHTML, /Backend archive state: archived/); assert.equal(f.buttons.archive.disabled, true); assert.equal(f.calls.length, 1);
 });
});
