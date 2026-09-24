// Existing FakeElement handler-contract seam: no layout/native focus evidence.
import test from "node:test";
import assert from "node:assert/strict";
import { createServer } from "vite";
import { readFile } from "node:fs/promises";
import { preset, catalog, team, clone } from "./fixtures/team-ui.mjs";
const vite = await createServer({ appType: "custom", logLevel: "silent", server: { middlewareMode: true } });
const { createTeamCommands } = await vite.ssrLoadModule("/src/services/team-commands.ts");
const { createTeamsArea, seatStatus, renderTeamGroup } = await vite.ssrLoadModule("/src/components/TeamsArea.ts"); await vite.close();
const tick = () => new Promise(r => setImmediate(r));
async function setup(options, fn) {
 class El {
  innerHTML = ""; textContent = ""; attrs = {}; dataset = {}; disabled = false; hidden = false; handlers = {}; childButtons = []; tabIndex = 0;
  setAttribute(k, v) { this.attrs[k] = v; } hasAttribute(k) { return k in this.attrs; }
  addEventListener(k, fn) { this.handlers[k] = fn; } closest() { return this; }
  appendChild(el) { this.child = el; } focus() { doc.activeElement = this; }
  classes = new Set(); classList = { add: c => this.classes.add(c), remove: c => this.classes.delete(c), contains: c => this.classes.has(c) };
  detailsEls = []; contains(el) { return el === this || this.childButtons.includes(el) || this.detailsEls.includes(el); }
  querySelectorAll(s) { return s === "button" ? this.childButtons : s === "details[open]" ? this.detailsEls.filter(d => d.open) : s === "details" ? this.detailsEls : []; }
 }
 const root = new El(), status = new El(), teams = new El(), coordination = new El(), refresh = new El(), coordPanel = new El(), teamsPanel = new El(), tabs = new El(), legacy = new El();
 const coordinationTab = new El(), teamsTab = new El(); coordinationTab.dataset.tab = "coordination"; teamsTab.dataset.tab = "teams"; coordinationTab.attrs["aria-selected"] = "true"; teamsTab.attrs["aria-selected"] = "false";
 root.querySelector = s => ({ "[data-global-status]": status, "[data-teams]": teams, "[data-coordination]": coordination, "[data-refresh]": refresh, "#v4-coordination": coordPanel, "#v4-teams": teamsPanel, '[role="tablist"]': tabs })[s] ?? [coordinationTab, teamsTab].find(t => t.attrs["aria-selected"] === "true");
 root.querySelectorAll = () => [coordinationTab, teamsTab];
 teams.childButtons.push(new El());
 const prior = globalThis.document; const doc = { createElement: () => root, hidden: false, activeElement: null }; globalThis.document = doc;
 const priorInterval = globalThis.setInterval, priorClear = globalThis.clearInterval; const timers = [];
 globalThis.setInterval = (fn, ms) => { timers.push({ fn, ms, cleared: false }); return timers.length; };
 globalThis.clearInterval = id => { if (timers[id - 1]) timers[id - 1].cleared = true; };
 const known = []; const forbidden = name => async () => { throw new Error(`${name} must not be read by the launcher`); };
 // presets/catalog are deliberately present and throwing: a refresh that touches them fails the test.
 const api = { list: async () => [team()], presets: forbidden("presets"), catalog: forbidden("catalog"), ...options.api };
 const instance = createTeamsArea(new El(), legacy, { api, listAgents: async () => [{ name: "glados" }], openAgent: async () => {}, onTeamSeats: n => known.push(n), ...options, api });
 const auto = async () => { for (const t of timers) if (!t.cleared) t.fn(); await tick(); await tick(); };
 try { await tick(); await fn({ root, status, teams, coordination, refresh, coordPanel, teamsPanel, legacy, tabs, doc, known, api, instance, El, timers, auto }); }
 finally { globalThis.setInterval = priorInterval; globalThis.clearInterval = priorClear; if (prior === undefined) delete globalThis.document; else globalThis.document = prior; }
}
test("loads authoritative teams only and mounts the standing roster inside Coordination", async () => {
 await setup({}, async f => {
  assert.equal(f.coordination.child, f.legacy); assert.deepEqual(f.known[0], ["t1-backend", "t1-qa"]);
  assert.match(f.teams.innerHTML, /Awaiting registration and approval/); assert.match(f.status.textContent, /Atualizado \d\d:\d\d:\d\d/);
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
   await f.root.handlers.click({ target: button }); assert.equal(f.doc.activeElement == null, true);
  }
  assert.match(f.status.textContent, /Atualizado \d\d:\d\d:\d\d/);
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
  assert.equal((html.match(/<details class="v4-details" data-details="/g) ?? []).length, t.seats.length + 1, "one details per seat plus one for mission/actions");
  assert.doesNotMatch(html, /<details[^>]*\sopen/);
  for (const inside of ["Thread binding", "Checkpoint / context", "Replace worker", "Archive checklist", "Acceptance:"]) {
   const at = html.indexOf(inside); assert.ok(at > 0, inside);
   assert.ok(html.lastIndexOf("<details", at) > html.lastIndexOf("</details>", at), `${inside} sits inside a details element`);
  }
  assert.match(html, /2 seats · 0 trabalhando · 0 aguardando/); assert.match(html, /t1-backend · LEAD/); assert.match(html, /data-action="open-seat" data-team="t1" data-seat="t1-backend"/);
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
  assert.match(f.status.textContent,/Atualizado \d\d:\d\d:\d\d/);
  assert.match(f.teams.innerHTML,/t1-backend/);
  assert.match(f.teams.innerHTML,/coordinated by GLaDOS/); assert.doesNotMatch(f.teams.innerHTML,/Bootstrap worker/);
  assert.doesNotMatch(f.teams.innerHTML,/No teams registered/);
 });
});

function ownerIn(state, generation = 0) { return { generation, state, since: "fixture", configured: { harness: "codex", model: "gpt-6-astra", reasoning: "high" }, actual: null, process_count: 0, thread_bound: false }; }
test("seat status is derived only from exact owner and turn observations; active is never inferred as working", () => {
 assert.deepEqual(seatStatus(null, "busy"), { kind: "unknown", label: "Sem informação" });
 assert.deepEqual(seatStatus(ownerIn("stale", 0), "busy"), { kind: "unstarted", label: "Não iniciado" });
 assert.deepEqual(seatStatus(ownerIn("stale", 2), "busy"), { kind: "stopped", label: "Parado" });
 assert.deepEqual(seatStatus(ownerIn("starting"), "busy"), { kind: "starting", label: "Inicializando" });
 assert.deepEqual(seatStatus(ownerIn("quarantined"), "idle"), { kind: "quarantined", label: "Quarentena" });
 assert.deepEqual(seatStatus(ownerIn("active", 1), "busy"), { kind: "working", label: "Trabalhando" });
 assert.deepEqual(seatStatus(ownerIn("active", 1), "idle"), { kind: "waiting", label: "Aguardando" });
 for (const turn of [undefined, null, "stopped", "running"]) assert.deepEqual(seatStatus(ownerIn("active", 1), turn), { kind: "unknown", label: "Sem informação" }, String(turn));
});
test("rendered badges carry text labels, and the seat count follows human status not owner state", () => {
 const t = team("active"); t.seats[0].observed_owner = ownerIn("active", 1); t.seats[1].observed_owner = ownerIn("active", 1);
 const html = renderTeamGroup(t, [{ name: "t1-backend", turn_state: "busy" }, { name: "t1-qa", status: "running" }]);
 assert.match(html, /data-status="working">Trabalhando</); assert.match(html, /data-status="unknown">Sem informação</);
 assert.match(html, /2 seats · 1 trabalhando · 0 aguardando/); assert.doesNotMatch(html, /2 active/);
 const idle = renderTeamGroup(t, [{ name: "t1-backend", turn_state: "idle" }]); assert.match(idle, /data-status="waiting">Aguardando</);
 assert.match(renderTeamGroup(team("active")), /data-status="unknown">Sem informação</);
});
test("auto refresh is bounded, never overlaps an in-flight read, and pauses while the Teams tab or page is hidden", async () => {
 let reads = 0, release; const t = team("active");
 await setup({ api: { list: () => { reads++; return new Promise(r => { release = () => r([t]); }); } } }, async f => {
  release(); await tick(); await tick(); assert.equal(reads, 1);
  assert.equal(f.timers.length, 1); assert.ok(f.timers[0].ms >= 3000 && f.timers[0].ms <= 5000, `bounded interval ${f.timers[0].ms}`);
  f.teamsPanel.hidden = true; await f.auto(); assert.equal(reads, 1, "hidden tab: no read");
  f.teamsPanel.hidden = false; f.doc.hidden = true; await f.auto(); assert.equal(reads, 1, "hidden page: no read");
  f.doc.hidden = false; await f.auto(); assert.equal(reads, 2, "visible: one read, now in flight");
  await f.auto(); assert.equal(reads, 2, "no overlap while that read is still pending");
  release(); await tick(); await tick(); await f.auto(); assert.equal(reads, 3, "settled: next tick reads again");
  release(); await tick(); await tick();
  f.instance.dispose(); assert.equal(f.timers[0].cleared, true); await f.auto(); assert.equal(reads, 3, "disposed: no read");
 });
});
test("auto refresh does not repaint unchanged data and preserves open details and focus across a changed repaint", async () => {
 const t = team("active"); t.seats[0].observed_owner = ownerIn("active", 1); let agents = [{ name: "t1-backend", turn_state: "idle" }];
 await setup({ api: { list: async () => [structuredClone(t)] }, listAgents: async () => agents }, async f => {
  let paints = 0; const inner = { v: f.teams.innerHTML }; Object.defineProperty(f.teams, "innerHTML", { get: () => inner.v, set: v => { inner.v = v; paints++; } });
  await f.auto(); assert.equal(paints, 0, "identical markup is not reassigned");
  const d = new f.El(); d.dataset.details = "seat:t1:t1-backend"; d.open = true; f.teams.detailsEls = [d];
  const btn = new f.El(); btn.dataset = { action: "open-seat", team: "t1", seat: "t1-backend" }; f.teams.childButtons = [btn]; f.doc.activeElement = btn;
  let focused = 0; btn.focus = () => { focused++; }; f.teams.querySelector = sel => sel.includes('data-seat="t1-backend"') ? btn : null;
  const fresh = new f.El(); fresh.dataset.details = "seat:t1:t1-backend"; fresh.open = false;
  Object.defineProperty(f.teams, "innerHTML", { get: () => inner.v, set: v => { inner.v = v; paints++; f.teams.detailsEls = [fresh]; } });
  agents = [{ name: "t1-backend", turn_state: "busy" }]; await f.auto();
  assert.equal(paints, 1, "changed status repaints once"); assert.match(inner.v, /Trabalhando/);
  assert.equal(fresh.open, true, "previously open details reopened by key"); assert.equal(focused, 1, "focused control refocused by key");
 });
});
test("auto refresh error marks the last-known data stale with a timestamp and disables actions; a later success clears it", async () => {
 let fail = false; const t = team("active");
 await setup({ api: { list: async () => { if (fail) throw new Error("SENTINEL_SECRET"); return [t]; } } }, async f => {
  const before = f.teams.innerHTML; fail = true; await f.auto();
  assert.equal(f.teams.innerHTML, before); assert.ok(f.teams.classList.contains("v4-stale"));
  assert.match(f.status.textContent, /Sem atualização desde \d\d:\d\d:\d\d/); assert.doesNotMatch(f.status.textContent, /SENTINEL_SECRET/);
  assert.equal(f.teams.childButtons[0].disabled, true);
  fail = false; await f.auto(); assert.equal(f.teams.classList.contains("v4-stale"), false); assert.match(f.status.textContent, /Atualizado/);
 });
});

test("Open attaches a verified active managed seat even without a preexisting tmux window", async () => {
 const t = team("active"); t.seats[0].observed_owner = {...ownerIn("active",1),actual:{harness:"codex",model:"gpt-6-astra",reasoning:"high"},process_count:1,thread_bound:true};
 let opened = 0;
 await setup({ api: { list: async () => [t] }, openAgent: async (current, name) => { assert.equal(current.snapshot.team, "t1"); assert.equal(name,"t1-backend"); opened++; }, listAgents: async () => [] }, async f => {
  assert.match(f.teams.innerHTML, /data-seat="t1-backend">Abrir</);
  assert.match(f.teams.innerHTML, /data-seat="t1-qa" disabled/);
  const live = new f.El(); live.dataset = { action:"open-seat",team:"t1",seat:"t1-backend" };
  await f.root.handlers.click({target:live}); assert.equal(opened,1);
 });
});
test("Open cannot use a stale tmux window to bypass current owner eligibility", async () => {
 const t = team("active"); t.seats[0].observed_owner = ownerIn("quarantined", 1); let opened=0;
 await setup({api:{list:async()=>[t]},openAgent:async()=>{opened++},listAgents:async()=>[{name:"t1-backend",tmux_window_id:"@7"}]},async f=>{
  assert.match(f.teams.innerHTML,/data-seat="t1-backend" disabled/);
  const button=new f.El();button.dataset={action:"open-seat",team:"t1",seat:"t1-backend"};
  await f.root.handlers.click({target:button});assert.equal(opened,0);
 });
});
test("double Open while attach is pending is locked without starting a worker", async()=>{
 const t=team("active");t.seats[0].observed_owner={...ownerIn("active",1),actual:{harness:"codex",model:"gpt-6-astra",reasoning:"high"},process_count:1,thread_bound:true};let finish,calls=0;
 await setup({api:{list:async()=>[t]},openAgent:()=>{calls++;return new Promise(r=>finish=r)}},async f=>{
  const button=new f.El();button.dataset={action:"open-seat",team:"t1",seat:"t1-backend"};
  const first=f.root.handlers.click({target:button});await f.root.handlers.click({target:button});assert.equal(calls,1);finish();await first;
 });
});

test("error then identical success re-enables valid Open and keeps ineligible Open disabled", async () => {
 let fail = false; const t = team("active"); t.seats[0].observed_owner = {...ownerIn("active",1),actual:{harness:"codex",model:"gpt-6-astra",reasoning:"high"},process_count:1,thread_bound:true}; t.seats[1].observed_owner = ownerIn("stale", 0);
 await setup({ api: { list: async () => { if (fail) throw new Error("boom"); return [structuredClone(t)]; } }, listAgents: async () => [{ name: "t1-backend", turn_state: "idle", tmux_window_id: "@7" }, { name: "t1-qa", turn_state: "idle", tmux_window_id: null }] }, async f => {
  let paints = 0; const inner = { v: f.teams.innerHTML }; Object.defineProperty(f.teams, "innerHTML", { get: () => inner.v, set: v => { inner.v = v; paints++; } });
  const live = new f.El(); live.dataset = { action: "open-seat", team: "t1", seat: "t1-backend" }; f.teams.childButtons = [live];
  fail = true; await f.auto(); assert.equal(live.disabled, true, "error disables actions"); assert.equal(paints, 0);
  fail = false; await f.auto();
  assert.equal(paints, 1, "identical data after an error still repaints"); assert.equal(f.teams.classList.contains("v4-stale"), false);
  assert.match(inner.v, /data-seat="t1-backend">Abrir</, "Open with a window is live again");
  assert.match(inner.v, /data-seat="t1-qa" disabled title="Agente não disponível para abrir"/, "Ineligible Open stays disabled");
  await f.auto(); assert.equal(paints, 1, "steady state: no further repaint");
 });
});
test("stale copy names the last successful read and does not drift across consecutive failures", async () => {
 let fail = false; const t = team("active");
 await setup({ api: { list: async () => { if (fail) throw new Error("boom"); return [t]; } } }, async f => {
  const good = /Atualizado (\d\d:\d\d:\d\d)/.exec(f.status.textContent)[1];
  fail = true; await f.auto(); const first = /desde (\S+)/.exec(f.status.textContent)[1]; assert.equal(first, good, "uses the last good read, not the error time");
  await new Promise(r => setTimeout(r, 1100)); await f.auto(); const second = /desde (\S+)/.exec(f.status.textContent)[1];
  assert.equal(second, first, "a second failure keeps the same timestamp");
 });
});
test("stale copy before any successful read says 'nunca'", async () => {
 await setup({ api: { list: async () => { throw new Error("boom"); } } }, async f => {
  await f.auto(); assert.match(f.status.textContent, /Sem atualização desde nunca/);
 });
});
test("focus on an open details summary is restored by details key across a changed repaint", async () => {
 const t = team("active"); t.seats[0].observed_owner = {...ownerIn("active",1),actual:{harness:"codex",model:"gpt-6-astra",reasoning:"high"},process_count:1,thread_bound:true}; let agents = [{ name: "t1-backend", turn_state: "idle" }];
 await setup({ api: { list: async () => [structuredClone(t)] }, listAgents: async () => agents }, async f => {
  const details = new f.El(); details.dataset.details = "seat:t1:t1-backend"; details.open = true;
  const summary = new f.El(); summary.dataset = {}; summary.closest = sel => sel === "details" ? details : null;
  f.teams.detailsEls = [details]; f.teams.contains = el => el === summary || el === details; f.doc.activeElement = summary;
  const fresh = new f.El(); fresh.dataset.details = "seat:t1:t1-backend"; fresh.open = false; const freshSummary = new f.El(); let focused = 0; freshSummary.focus = () => { focused++; };
  const inner = { v: f.teams.innerHTML }; Object.defineProperty(f.teams, "innerHTML", { get: () => inner.v, set: v => { inner.v = v; f.teams.detailsEls = [fresh]; } });
  f.teams.querySelector = sel => sel === 'details[data-details="seat:t1:t1-backend"] > summary' ? freshSummary : null;
  agents = [{ name: "t1-backend", turn_state: "busy" }]; await f.auto();
  assert.match(inner.v, /Trabalhando/); assert.equal(fresh.open, true, "details reopened"); assert.equal(focused, 1, "summary refocused by details key");
 });
});

test("Claude seat Open is enabled only for an exact observed Sonnet 5/None owner and never without actual", async () => {
 const sonnet = { harness: "claude", model: "claude-sonnet-5", reasoning: null };
 const t = team("active"); t.seats[1].observed_owner = { ...ownerIn("active", 1), configured: { ...sonnet }, actual: { ...sonnet }, process_count: 1, thread_bound: true };
 let opened = 0;
 await setup({ api: { list: async () => [t] }, openAgent: async (current, name) => { assert.equal(current.snapshot.team, "t1"); assert.equal(name, "t1-qa"); opened++; }, listAgents: async () => [] }, async f => {
  assert.match(f.teams.innerHTML, /data-seat="t1-qa">Abrir</);
  const live = new f.El(); live.dataset = { action: "open-seat", team: "t1", seat: "t1-qa" };
  await f.root.handlers.click({ target: live }); assert.equal(opened, 1);
 });
 for (const patch of [{ actual: null }, { actual: { ...sonnet, model: "sonnet" } }, { configured: { ...sonnet, reasoning: "high" }, actual: { ...sonnet, reasoning: "high" } }]) {
  const c = team("active"); c.seats[1].observed_owner = { ...ownerIn("active", 1), configured: { ...sonnet }, actual: { ...sonnet }, process_count: 1, thread_bound: true, ...patch };
  let calls = 0;
  await setup({ api: { list: async () => [c] }, openAgent: async () => { calls++; }, listAgents: async () => [] }, async f => {
   assert.match(f.teams.innerHTML, /data-seat="t1-qa" disabled/);
   const button = new f.El(); button.dataset = { action: "open-seat", team: "t1", seat: "t1-qa" };
   await f.root.handlers.click({ target: button }); assert.equal(calls, 0);
  });
 }
});

test("seat row is clean: status badge, name, role and Abrir only; model, generation, repo and epic live inside details", () => {
 const t = team("active"); t.state.epic_id = "aperture-epic1";
 t.seats[0].observed_owner = { ...ownerIn("active", 1), actual: { harness: "codex", model: "gpt-6-astra", reasoning: "high" }, process_count: 1, thread_bound: true };
 t.seats[1].observed_owner = ownerIn("active", 1);
 const html = renderTeamGroup(t, [{ name: "t1-backend", turn_state: "busy" }]);
 const rows = [...html.matchAll(/<div class="v4-seat__row">(.*?)<\/div>/g)].map(m => m[1]);
 assert.equal(rows.length, 2, "one visible row per seat");
 assert.match(rows[0], /^<span class="v4-badge v4-seat-status v4-seat-status--working" data-status="working">Trabalhando<\/span><span class="v4-seat__name" title="t1-backend">t1-backend · LEAD<\/span><span class="v4-seat__role">backend<\/span><button class="v4-button v4-button--small" data-action="open-seat" data-team="t1" data-seat="t1-backend">Abrir<\/button>$/);
 assert.match(rows[1], /<span class="v4-seat__role">qa<\/span><button[^>]*data-seat="t1-qa" disabled title="Agente não disponível para abrir">Abrir<\/button>$/);
 for (const row of rows) assert.doesNotMatch(row, /gpt-6-astra|sonnet|codex|claude|· g1|v4-meta|repository|epic|v4-seat__noterm/, "row carries no technical metadata");
 const head = html.slice(0, html.indexOf("</div></div>"));
 assert.match(head, /<h2 title="t1">t1<\/h2><p class="v4-meta">2 seats · 1 trabalhando · 0 aguardando<\/p>/);
 assert.match(head, /v4-status">active<\/span>/); assert.doesNotMatch(head, /repository|epic|lead:|· g1|gpt-6-astra/, "header is team + count + lifecycle only");
 for (const inside of ["repository: aperture (immutable)", "lead: t1-backend", "generation: g1", "epic: aperture-epic1", "codex · gpt-6-astra · high", "active · g1", "<dt>Terminal</dt><dd>Agente não disponível para abrir</dd>"]) {
  const at = html.indexOf(inside); assert.ok(at > 0, inside);
  assert.ok(html.lastIndexOf("<details", at) > html.lastIndexOf("</details>", at), `${inside} sits inside a details element`);
 }
 assert.equal((html.match(/<details class="v4-details" data-details="/g) ?? []).length, 3); assert.doesNotMatch(html, /<details[^>]*\sopen/);
});
test("long seat names keep a title for the ellipsis and the stylesheet forbids the wrapped metadata column", async () => {
 const t = team("active"); const long = "t1-" + "x".repeat(120);
 t.snapshot.seats[0].name = long; t.seats[0].configured.name = long; t.snapshot.lead = long;
 assert.match(renderTeamGroup(t), new RegExp(`<span class="v4-seat__name" title="${long}">${long} · LEAD</span><span class="v4-seat__role">backend</span>`));
 const css = await readFile(new URL("../src/teams.css", import.meta.url), "utf8");
 const rule = sel => { const at = css.indexOf(`\n${sel} {`); assert.ok(at >= 0, sel); return css.slice(at, css.indexOf("}", at)); };
 for (const sel of [".v4-seat__name", ".v4-seat__role", ".v4-team__title h2, .v4-team__title .v4-meta"]) { assert.match(rule(sel), /text-overflow: ellipsis/, sel); assert.match(rule(sel), /white-space: nowrap/, sel); assert.match(rule(sel), /overflow-wrap: normal/, sel); }
 assert.match(rule(".v4-seat__name"), /min-width: 0/); assert.match(rule(".v4-seat__row"), /flex-wrap: nowrap/);
 assert.doesNotMatch(css, /\.v4-seat__row \.v4-meta/, "no flexible metadata span in the row");
 assert.match(rule(".v4-button--small"), /min-height: var\(--v4-target\)/); assert.match(css, /--v4-target: 44px/);
 const narrow = css.slice(css.indexOf("@media (max-width: 700px)")); assert.match(narrow, /\.v4-seat__name \{ flex: 1 1 100%; \}/); assert.match(narrow, /\.v4-seat__row \{ flex-wrap: wrap/);
});
