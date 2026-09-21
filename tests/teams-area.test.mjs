// Existing FakeElement handler-contract seam: no layout/native focus evidence.
import test from "node:test";
import assert from "node:assert/strict";
import { createServer } from "vite";
import { preset, catalog, team, clone } from "./fixtures/team-ui.mjs";
const vite = await createServer({ appType: "custom", logLevel: "silent", server: { middlewareMode: true } });
const { createTeamsArea } = await vite.ssrLoadModule("/src/components/TeamsArea.ts"); const { createRuntimeCommands } = await vite.ssrLoadModule("/src/services/team-runtime.ts"); await vite.close();
const tick = () => new Promise(r => setImmediate(r));
async function setup(options, fn) {
 class El {
  innerHTML = ""; textContent = ""; attrs = {}; dataset = {}; disabled = false; hidden = false; handlers = {}; childButtons = []; tabIndex = 0;
  setAttribute(k, v) { this.attrs[k] = v; } hasAttribute(k) { return k in this.attrs; }
  addEventListener(k, fn) { this.handlers[k] = fn; } closest() { return this; } before(el) { this.beforeElement = el; }
  appendChild(el) { this.child = el; } focus() { doc.activeElement = this; }
  querySelectorAll(s) { return s === "button" ? this.childButtons : []; }
 }
 const root = new El(), status = new El(), teams = new El(), presets = new El(), blank = new El(), refresh = new El(), sessions = new El(), library = new El(), tabs = new El(), legacy = new El();
 const sessionTab = new El(), presetTab = new El(); sessionTab.dataset.tab = "sessions"; presetTab.dataset.tab = "presets"; sessionTab.attrs["aria-selected"] = "true"; presetTab.attrs["aria-selected"] = "false";
 root.querySelector = s => ({ "[data-global-status]": status, "[data-teams]": teams, "[data-presets]": presets, '[data-action="blank"]': blank, "[data-refresh]": refresh, "#v4-sessions": sessions, "#v4-presets": library, '[role="tablist"]': tabs })[s] ?? [sessionTab, presetTab].find(t => t.attrs["aria-selected"] === "true");
 root.querySelectorAll = () => [sessionTab, presetTab];
 teams.childButtons.push(new El()); presets.childButtons.push(new El());
 const prior = globalThis.document; const doc = { createElement: () => root }; globalThis.document = doc;
 const known = []; const api = { list: async () => [team()], presets: async () => [clone(preset)], catalog: async () => clone(catalog), ...options.api };
 const instance = createTeamsArea(new El(), legacy, { api, listAgents: async () => [{ name: "glados" }], openAgent: async () => {}, onTeamSeats: n => known.push(n), ...options, api });
 try { await tick(); await fn({ root, status, teams, presets, blank, refresh, sessions, library, legacy, tabs, doc, known, api, instance, El }); }
 finally { if (prior === undefined) delete globalThis.document; else globalThis.document = prior; }
}
test("loads authoritative teams/presets and preserves actual standing roster before groups", async () => {
 await setup({}, async f => {
  assert.equal(f.teams.beforeElement, f.legacy); assert.deepEqual(f.known[0], ["t1-backend", "t1-qa"]);
  assert.match(f.teams.innerHTML, /Awaiting registration and approval/); assert.match(f.presets.innerHTML, /Fullstack/); assert.equal(f.blank.disabled, false);
 });
});
test("backend unavailable is not empty success; last-known markup remains, actions disabled", async () => {
 await setup({}, async f => {
  const before = f.teams.innerHTML; f.api.list = async () => { throw new Error("SENTINEL_SECRET"); };
  await f.instance.refresh(); assert.equal(f.teams.innerHTML, before); assert.match(f.status.textContent, /last known/);
  assert.doesNotMatch(f.status.textContent, /SENTINEL_SECRET|No teams registered/); assert.equal(f.blank.disabled, true);
  assert.equal(f.teams.childButtons[0].disabled, true); assert.equal(f.presets.childButtons[0].disabled, true);
 });
});
test("initial missing command leaves teams unavailable, not fabricated presets or empty list", async () => {
 await setup({ api: { catalog: async () => { throw new Error("command not found"); } } }, async f => {
  assert.equal(f.teams.innerHTML, ""); assert.equal(f.presets.innerHTML, ""); assert.equal(f.known.length, 0); assert.equal(f.blank.disabled, true); assert.match(f.status.textContent, /unavailable/);
 });
});
test("pending cancellation sends exact generation/request and locks duplicate clicks", async () => {
 let resolve, calls = 0, input;
 await setup({ api: { cancel: v => { calls++; input = v; return new Promise(r => { resolve = r; }); } } }, async f => {
  const button = new f.El(); button.dataset = { action: "cancel-pending", team: "t1" };
  const pending = f.root.handlers.click({ target: button }); await f.root.handlers.click({ target: button });
  assert.equal(calls, 1); assert.deepEqual(input, { team: "t1", expected_generation: 0, creation_request_id: "fixture-request" }); assert.equal(button.disabled, true);
  f.api.list = async () => []; resolve({ team: "t1", cancelled: true, rejected_snapshot_id: "fixture" }); await pending; assert.match(f.teams.innerHTML, /No teams registered/);
 });
});
test("tab handler follows arrow/home/end semantics and preserves roster visibility", async () => {
 await setup({}, async f => {
  let prevented = 0; const key = k => f.tabs.handlers.keydown({ key: k, preventDefault() { prevented++; } });
  key("ArrowRight"); assert.equal(f.legacy.hidden, true); assert.equal(f.library.hidden, false); assert.equal(f.doc.activeElement.dataset.tab, "presets");
  key("Home"); assert.equal(f.legacy.hidden, false); assert.equal(f.sessions.hidden, false); assert.equal(f.doc.activeElement.dataset.tab, "sessions");
  key("End"); assert.equal(f.legacy.hidden, true); assert.equal(prevented, 3);
 });
});

function bootstrapTeam() {
 const t = team("active"); t.capabilities.start = true;
 const { harness, model, reasoning } = t.snapshot.seats[0];
 t.seats[0].observed_owner = { generation: 0, state: "stale", since: "fixture", configured: { harness, model, reasoning }, actual: null, process_count: 0, thread_bound: false };
 const claude = t.snapshot.seats[1];
 t.seats[1].observed_owner = { generation: 0, state: "stale", since: "fixture", configured: { harness: claude.harness, model: claude.model, reasoning: claude.reasoning }, actual: null, process_count: 0, thread_bound: false };
 return t;
}
test("bootstrap CTA exists only for active/stale g0 enabled seat, not other seats", async () => {
 for (const [enabled, mutate] of [[true, () => {}], [false, t => t.capabilities.start = false], [false, t => t.seats[0].observed_owner.generation = 1], [false, t => t.state.state = "pending"]]) {
  const t = bootstrapTeam(); mutate(t);
  await setup({ api: { list: async () => [t] } }, async f => {
   assert.equal(f.teams.innerHTML.includes('data-action="bootstrap"'), enabled);
   if (enabled) assert.equal((f.teams.innerHTML.match(/data-action="bootstrap"/g) ?? []).length, 1);
  });
 }
});
test("bootstrap click handler-contract locks duplicate start, avoids legacy and refreshes authoritative state", async () => {
 const t = bootstrapTeam(); let resolve, calls = 0, reads = 0, legacy = 0;
 await setup({ api: { list: async () => { reads++; return [t]; } }, openAgent: async () => { legacy++; },
  runtime: { bootstrap: (input, seat) => { calls++; assert.equal(input, t); assert.equal(seat, "t1-backend"); return new Promise(r => { resolve = r; }); } } }, async f => {
  const button = new f.El(); button.dataset = { action: "bootstrap", team: "t1", seat: "t1-backend" };
  const pending = f.root.handlers.click({ target: button }); await f.root.handlers.click({ target: button });
  assert.equal(calls, 1); assert.equal(reads, 1); assert.equal(legacy, 0); assert.equal(f.refresh.disabled, true);
  assert.match(f.status.textContent, /pending.*awaiting native evidence/);
  const next = clone(t); next.seats[0].observed_owner.generation = 3; next.seats[0].observed_owner.state = "active"; next.seats[0].observed_owner.actual = next.seats[0].observed_owner.configured;
  f.api.list = async () => { reads++; return [next]; };
  resolve({ phase: "started", blockers: [] }); await pending;
  assert.equal(reads, 2); assert.match(f.teams.innerHTML, /active · g3/); assert.equal(t.seats[0].observed_owner.generation, 0);
  assert.doesNotMatch(f.teams.innerHTML, /data-action="bootstrap"/); assert.match(f.status.textContent, /reported started/);
 });
});
test("bootstrap unknown outcome requires explicit read refresh, no retry or local generation reset", async () => {
 let calls = 0, reads = 0; const t = bootstrapTeam();
 await setup({ api: { list: async () => { reads++; return [t]; } }, runtime: { bootstrap: async () => { calls++; throw { code: "E_RUNTIME_DEADLINE", message: "SECRET_SENTINEL" }; } } }, async f => {
  const button = new f.El(); button.dataset = { action: "bootstrap", team: "t1", seat: "t1-backend" };
  await f.root.handlers.click({ target: button });
  assert.equal(calls, 1); assert.equal(reads, 1); assert.match(f.status.textContent, /unknown.*[Rr]efresh/); assert.doesNotMatch(f.status.textContent, /SECRET_SENTINEL/);
  const syntheticSecond = new f.El(); syntheticSecond.dataset = button.dataset;
  await f.root.handlers.click({ target: syntheticSecond }); assert.equal(calls, 1);
  assert.equal(f.refresh.disabled, false); assert.equal(t.seats[0].observed_owner.generation, 0);
  await f.instance.refresh(); assert.equal(reads, 2); assert.equal(calls, 1);
 });
});
test("bootstrap starting and blocked responses refresh but do not claim success", async () => {
 for (const phase of ["starting", "blocked"]) {
  let reads = 0;
  await setup({ api: { list: async () => { reads++; return [bootstrapTeam()]; } }, runtime: { bootstrap: async () => ({ phase, blockers: phase === "blocked" ? [{ code: "E_LAUNCH_UNAVAILABLE", reference: "fixture" }] : [] }) } }, async f => {
   const button = new f.El(); button.dataset = { action: "bootstrap", team: "t1", seat: "t1-backend" };
   await f.root.handlers.click({ target: button });
   assert.equal(reads, 2); assert.match(f.status.textContent, /successful launch is not confirmed/); assert.doesNotMatch(f.status.textContent, /reported started/);
  });
 }
});


test("malformed bootstrap response keeps unknown state and cannot trigger success refresh", async () => {
 let calls = 0, reads = 0;
 const runtime = createRuntimeCommands(async () => { calls++; return { phase: "started", owner: null }; });
 await setup({ api: { list: async () => { reads++; return [bootstrapTeam()]; } }, runtime }, async f => {
  const button = new f.El(); button.dataset = { action: "bootstrap", team: "t1", seat: "t1-backend" };
  await f.root.handlers.click({ target: button }); assert.equal(calls, 1); assert.equal(reads, 1);
  assert.match(f.status.textContent, /could not be confirmed/); assert.doesNotMatch(f.status.textContent, /reported started/);
 });
});
test("stale rendered bootstrap actions cannot bypass current capability or failed refresh", async () => {
 let calls = 0;
 const t = bootstrapTeam(); t.capabilities.start = false;
 await setup({ api: { list: async () => [t] }, runtime: { bootstrap: async () => { calls++; } } }, async f => {
  const button = new f.El(); button.dataset = { action: "bootstrap", team: "t1", seat: "t1-backend" };
  await f.root.handlers.click({ target: button }); assert.equal(calls, 0);
  f.api.list = async () => { throw new Error("unavailable"); }; await f.instance.refresh();
  t.capabilities.start = true; await f.root.handlers.click({ target: button }); assert.equal(calls, 0);
 });
});
