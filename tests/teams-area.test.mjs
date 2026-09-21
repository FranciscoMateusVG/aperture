// Existing FakeElement handler-contract seam: no layout/native focus evidence.
import test from "node:test";
import assert from "node:assert/strict";
import { createServer } from "vite";
import { preset, catalog, team, clone } from "./fixtures/team-ui.mjs";
const vite = await createServer({ appType: "custom", logLevel: "silent", server: { middlewareMode: true } });
const { createTeamCommands } = await vite.ssrLoadModule("/src/services/team-commands.ts");
const { createTeamsArea } = await vite.ssrLoadModule("/src/components/TeamsArea.ts"); await vite.close();
const tick = () => new Promise(r => setImmediate(r));
async function setup(options, fn) {
 class El {
  innerHTML = ""; textContent = ""; attrs = {}; dataset = {}; disabled = false; hidden = false; handlers = {}; childButtons = []; tabIndex = 0;
  setAttribute(k, v) { this.attrs[k] = v; } hasAttribute(k) { return k in this.attrs; }
  addEventListener(k, fn) { this.handlers[k] = fn; } closest() { return this; }
  appendChild(el) { this.child = el; } focus() { doc.activeElement = this; }
  querySelectorAll(s) { return s === "button" ? this.childButtons : []; }
 }
 const root = new El(), status = new El(), teams = new El(), coordination = new El(), refresh = new El(), coordPanel = new El(), teamsPanel = new El(), tabs = new El(), legacy = new El();
 const coordinationTab = new El(), teamsTab = new El(); coordinationTab.dataset.tab = "coordination"; teamsTab.dataset.tab = "teams"; coordinationTab.attrs["aria-selected"] = "true"; teamsTab.attrs["aria-selected"] = "false";
 root.querySelector = s => ({ "[data-global-status]": status, "[data-teams]": teams, "[data-coordination]": coordination, "[data-refresh]": refresh, "#v4-coordination": coordPanel, "#v4-teams": teamsPanel, '[role="tablist"]': tabs })[s] ?? [coordinationTab, teamsTab].find(t => t.attrs["aria-selected"] === "true");
 root.querySelectorAll = () => [coordinationTab, teamsTab];
 teams.childButtons.push(new El());
 const prior = globalThis.document; const doc = { createElement: () => root }; globalThis.document = doc;
 const known = []; const forbidden = name => async () => { throw new Error(`${name} must not be read by the launcher`); };
 // presets/catalog are deliberately present and throwing: a refresh that touches them fails the test.
 const api = { list: async () => [team()], presets: forbidden("presets"), catalog: forbidden("catalog"), ...options.api };
 const instance = createTeamsArea(new El(), legacy, { api, listAgents: async () => [{ name: "glados" }], openAgent: async () => {}, onTeamSeats: n => known.push(n), ...options, api });
 try { await tick(); await fn({ root, status, teams, coordination, refresh, coordPanel, teamsPanel, legacy, tabs, doc, known, api, instance, El }); }
 finally { if (prior === undefined) delete globalThis.document; else globalThis.document = prior; }
}
test("loads authoritative teams only and mounts the standing roster inside Coordination", async () => {
 await setup({}, async f => {
  assert.equal(f.coordination.child, f.legacy); assert.deepEqual(f.known[0], ["t1-backend", "t1-qa"]);
  assert.match(f.teams.innerHTML, /Awaiting registration and approval/); assert.match(f.status.textContent, /Team state loaded/);
  // Initial visibility is declared in the component markup (the fake DOM does not parse innerHTML into element state).
  assert.match(f.root.innerHTML, /<section id="v4-teams"[^>]*\bhidden>/); assert.doesNotMatch(f.root.innerHTML, /<section id="v4-coordination"[^>]*\bhidden/);
 });
});
test("no preset or creation controls are reachable from the launcher", async () => {
 await setup({}, async f => {
  assert.doesNotMatch(f.root.innerHTML, /data-presets|data-action="(blank|create|edit|duplicate)"|Presets|preset/i);
  assert.match(f.root.innerHTML, /data-tab="coordination"/); assert.match(f.root.innerHTML, /data-tab="teams"/);
  assert.doesNotMatch(f.teams.innerHTML, /data-action="(blank|create|edit|duplicate)"/);
  for (const button of [new f.El(), new f.El(), new f.El(), new f.El()].map((b, i) => { b.dataset = { action: ["blank", "create", "edit", "duplicate"][i], preset: "fullstack" }; return b; })) {
   await f.root.handlers.click({ target: button }); assert.equal(f.doc.activeElement, undefined);
  }
  assert.match(f.status.textContent, /Team state loaded/);
 });
});
test("backend unavailable is not empty success; last-known markup remains, actions disabled, roster untouched", async () => {
 await setup({}, async f => {
  const before = f.teams.innerHTML; f.api.list = async () => { throw new Error("SENTINEL_SECRET"); };
  await f.instance.refresh(); assert.equal(f.teams.innerHTML, before); assert.match(f.status.textContent, /last known/);
  assert.doesNotMatch(f.status.textContent, /SENTINEL_SECRET|No teams registered/);
  assert.equal(f.teams.childButtons[0].disabled, true); assert.equal(f.coordination.child, f.legacy); assert.equal(f.coordPanel.hidden, false);
 });
});
test("initial missing command leaves teams unavailable, not a fabricated empty list", async () => {
 await setup({ api: { list: async () => { throw new Error("command not found"); } } }, async f => {
  assert.equal(f.teams.innerHTML, ""); assert.equal(f.known.length, 0); assert.match(f.status.textContent, /unavailable/); assert.equal(f.coordination.child, f.legacy);
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
test("tab handler follows arrow/home/end semantics between Coordination and Teams", async () => {
 await setup({}, async f => {
  let prevented = 0; const key = k => f.tabs.handlers.keydown({ key: k, preventDefault() { prevented++; } });
  key("ArrowRight"); assert.equal(f.coordPanel.hidden, true); assert.equal(f.teamsPanel.hidden, false); assert.equal(f.doc.activeElement.dataset.tab, "teams");
  key("Home"); assert.equal(f.coordPanel.hidden, false); assert.equal(f.teamsPanel.hidden, true); assert.equal(f.doc.activeElement.dataset.tab, "coordination");
  key("End"); assert.equal(f.coordPanel.hidden, true); assert.equal(prevented, 3);
 });
});

function startableTeam() {
 const t = team("active"); t.capabilities.start = true;
 for (const seat of t.seats) seat.observed_owner = { generation: 0, state: "stale", since: "fixture", configured: { harness: seat.configured.harness, model: seat.configured.model, reasoning: seat.configured.reasoning }, actual: null, process_count: 0, thread_bound: false };
 return t;
}
test("no manual start control exists for any seat state; start is coordinated by GLaDOS", async () => {
 for (const mutate of [() => {}, t => { t.capabilities.start = false; }, t => { t.seats[0].observed_owner.generation = 1; }, t => { t.state.state = "pending"; }]) {
  const t = startableTeam(); mutate(t);
  await setup({ api: { list: async () => [t] } }, async f => {
   assert.doesNotMatch(f.teams.innerHTML, /data-action="bootstrap"|Bootstrap worker/);
   assert.match(f.teams.innerHTML, /coordinated by GLaDOS/);
   assert.doesNotMatch(f.teams.innerHTML, /click .*start|start (the|a) worker|press .*start/i);
  });
 }
});
test("a synthetic bootstrap click is inert: no command, no status change, no lock", async () => {
 let reads = 0; const t = startableTeam();
 await setup({ api: { list: async () => { reads++; return [t]; } } }, async f => {
  const before = f.status.textContent; const button = new f.El(); button.dataset = { action: "bootstrap", team: "t1", seat: "t1-backend" };
  await f.root.handlers.click({ target: button });
  assert.equal(reads, 1); assert.equal(f.status.textContent, before); assert.equal(f.refresh.disabled, false); assert.equal(button.disabled, false);
 });
});
test("active card is compact by default: one row per seat, diagnostics and advanced actions only inside closed details", async () => {
 const t = startableTeam();
 await setup({ api: { list: async () => [t] } }, async f => {
  const html = f.teams.innerHTML;
  assert.equal((html.match(/<details class="v4-details">/g) ?? []).length, t.seats.length + 1, "one details per seat plus one for mission/actions");
  assert.doesNotMatch(html, /<details[^>]*\sopen/);
  for (const inside of ["Thread binding", "Checkpoint / context", "Replace worker", "Archive checklist", "Acceptance:"]) {
   const at = html.indexOf(inside); assert.ok(at > 0, inside);
   assert.ok(html.lastIndexOf("<details", at) > html.lastIndexOf("</details>", at), `${inside} sits inside a details element`);
  }
  assert.match(html, /2 seats · 0 active/); assert.match(html, /t1-backend · LEAD/); assert.match(html, /data-action="open-seat" data-seat="t1-backend"/);
  const openAt = html.indexOf('data-action="open-seat"'); assert.ok(html.lastIndexOf("<details", openAt) < html.lastIndexOf("</details>", openAt) || html.lastIndexOf("<details", openAt) === -1, "Open stays on the seat row, outside details");
 });
});
test("pending card keeps cancel visible and hides worker rows and worker actions", async () => {
 await setup({}, async f => {
  const html = f.teams.innerHTML;
  assert.match(html, /data-action="cancel-pending"/); assert.match(html, /1 seats|2 seats/);
  assert.doesNotMatch(html, /data-action="open-seat"|data-action="replace"|Bootstrap/);
  assert.match(html, /start is coordinated by GLaDOS after approval/);
 });
});

test("Refresh renders a presetless native active team through the real command parser", async () => {
 const native = team("active"); native.snapshot.preset = {id:null,sha256:null};
 native.capabilities.start = true;
 native.seats = native.snapshot.seats.map(configured => ({configured, observed_owner:{generation:0,state:"stale",since:"fixture",configured:{harness:configured.harness,model:configured.model,reasoning:configured.reasoning},actual:null,process_count:0,thread_bound:false}}));
 const api = createTeamCommands(async command => { assert.equal(command,"team_list"); return [native]; });
 await setup({api}, async f => {
  assert.match(f.status.textContent,/Team state loaded/);
  assert.match(f.teams.innerHTML,/t1-backend/);
  assert.match(f.teams.innerHTML,/coordinated by GLaDOS/); assert.doesNotMatch(f.teams.innerHTML,/Bootstrap worker/);
  assert.doesNotMatch(f.teams.innerHTML,/No teams registered/);
 });
});
