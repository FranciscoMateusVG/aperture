import test from "node:test";
import assert from "node:assert/strict";
import { createServer } from "vite";
import { preset, catalog, team, created, clone } from "./fixtures/team-ui.mjs";
const vite = await createServer({ appType: "custom", logLevel: "silent", server: { middlewareMode: true } });
const c = await vite.ssrLoadModule("/src/services/team-contract.ts");
const { createTeamCommands } = await vite.ssrLoadModule("/src/services/team-commands.ts");
const { renderPresetCard, renderTeamGroup } = await vite.ssrLoadModule("/src/components/TeamsArea.ts");
const { createInitialTeamDraft, validateCatalogDraft } = await vite.ssrLoadModule("/src/components/TeamEditor.ts");
await vite.close();
const rejects = f => assert.throws(f, e => e.code === "E_RESPONSE_INVALID");

test("v3 catalog/preset/team/creation receipt parse exact fixtures", () => {
 assert.deepEqual(c.parseTeamCatalog(catalog), catalog); assert.deepEqual(c.parseTeamPresets([preset]), [preset]);
 assert.deepEqual(c.parseTeams([team()]), [team()]); assert.deepEqual(c.parseTeamCreateResult(created()), created());
});
for (const [name, mutate] of [
 ["missing owner is not null", t => delete t.seats[0].observed_owner],
 ["string capability", t => t.capabilities.start = "true"],
 ["missing capability", t => delete t.capabilities.replace],
 ["unknown lifecycle", t => t.state.state = "ready"],
 ["negative generation", t => t.state.generation = -1],
 ["duplicate configured seat", t => t.seats[1] = t.seats[0]],
 ["configured snapshot mismatch", t => t.seats[0] = { ...t.seats[0], configured: { ...t.seats[0].configured, model: "different" } }],
 ["lead outside team", t => t.snapshot.lead = "glados"],
 ["bad preset digest", t => t.snapshot.preset.sha256 = "not-a-hash"],
 ["malformed observed model", t => t.seats[0].observed_owner = { actual: "gpt-6-astra" }],
]) test(`malformed team fails closed: ${name}`, () => { const t = team(); mutate(t); rejects(() => c.parseTeamView(t)); });
test("duplicate team and preset responses refused", () => { rejects(() => c.parseTeams([team(), team()])); rejects(() => c.parseTeamPresets([preset, preset])); });
test("inconsistent create receipt never looks like successful pending registration", () => {
 for (const field of ["request_id", "team", "project", "expected_generation"]) {
  const v = created(); v.creation_request[field] = field === "expected_generation" ? 9 : "wrong"; rejects(() => c.parseTeamCreateResult(v));
 }
});
test("catalog bounds must be finite numbers and preset CAS digest mandatory", () => {
 const v = clone(catalog); v.limits.max_seats = "99"; rejects(() => c.parseTeamCatalog(v));
 const p = clone(preset); delete p.sha256; rejects(() => c.parseTeamPreset(p));
});
test("configured values cannot stand in for observation or checkpoint", () => {
 const html = renderTeamGroup(team("active"));
 assert.match(html, /Snapshot/); assert.match(html, /Unknown — not observed/); assert.match(html, /Not available from this backend/);
 assert.doesNotMatch(html, /verified model|healthy|0%|checkpoint valid/i);
});
test("pending hides worker controls and never claims notification/boot", () => {
 const html = renderTeamGroup(team()); assert.match(html, /Awaiting registration and approval by GLaDOS/);
 assert.doesNotMatch(html, /data-action="open-seat"|data-action="replace"|notification sent|workers running/);
});
test("all backend-provided content is escaped in render templates", () => {
 const injected = '<img src=x onerror="boom">'; const p = clone(preset); p.display_name = injected; p.id = injected; p.seats[0].model = injected;
 const t = team("active"); t.snapshot.team = injected; t.snapshot.mission = injected; t.seats[0].configured.name = injected;
 for (const html of [renderPresetCard(p), renderTeamGroup(t)]) { assert.equal(html.includes("<img"), false); assert.match(html, /&lt;img/); }
});
test("preset drafting is detached and catalog membership is not inferred", () => {
 const p = clone(preset), draft = createInitialTeamDraft(p); draft.seats[0].model = "unavailable-model";
 assert.equal(p.seats[0].model, "gpt-6-astra"); assert.ok(validateCatalogDraft(draft, catalog).some(x => x.field === "seats.0.execution"));
 draft.seats[0] = { ...p.seats[0], role: "unlisted" }; assert.ok(validateCatalogDraft(draft, catalog).some(x => x.field === "seats.0.role"));
});
test("command wrappers pin exact names, envelopes, CAS, no authority inputs", async () => {
 const calls = []; const api = createTeamCommands(async (cmd, args) => { calls.push([cmd, args]); return cmd === "team_create" ? created(args.input) : cmd === "team_save_preset" ? { ...preset, source: "local" } : { team: "t1", cancelled: true, rejected_snapshot_id: "fixture-rejected" }; });
 const input = { ...createInitialTeamDraft(preset), team: "t1", actor: "glados", grants: ["forged"], creation_request_id: "forged" };
 await api.create(input); const create = calls[0]; assert.equal(create[0], "team_create");
 assert.deepEqual(Object.keys(create[1].input).sort(), ["team", "project", "mission", "acceptance", "preset_id", "seats", "lead_index", "fallbacks"].sort());
 await api.savePreset(preset, preset.sha256); const save = calls[1][1].input;
 assert.equal(save.expected_sha256, preset.sha256); assert.equal("source" in save.preset_without_source, false); assert.equal("sha256" in save.preset_without_source, false);
 await api.cancel({ team: "t1", expected_generation: 0, creation_request_id: "fixture-request", actor: "glados" });
 assert.deepEqual(calls[2], ["team_cancel_pending", { input: { team: "t1", expected_generation: 0, creation_request_id: "fixture-request" } }]);
 assert.equal("activate" in api, false);
});
test("write failures do not auto-retry and malformed success is rejected", async () => {
 let calls = 0; const api = createTeamCommands(async () => { calls++; return { success: true }; });
 await assert.rejects(api.create({ ...createInitialTeamDraft(preset), team: "t1" }), e => e.code === "E_RESPONSE_INVALID"); assert.equal(calls, 1);
 const rejected = createTeamCommands(async () => { throw { code: "E_PRESET_CONFLICT", message: "fixed" }; });
 await assert.rejects(rejected.savePreset(preset, preset.sha256), e => e.code === "E_PRESET_CONFLICT");
});

test("successful response for another team or preset is not accepted", async () => {
 const api = createTeamCommands(async cmd => cmd === "team_create" ? created() : { ...preset, id: "other" });
 await assert.rejects(api.create({ ...createInitialTeamDraft(preset), team: "another" }), e => e.code === "E_RESPONSE_INVALID");
 await assert.rejects(api.savePreset(preset, preset.sha256), e => e.code === "E_RESPONSE_INVALID");
});

test("turn state uses observed hub facts only, not requested model or owner state", () => {
 assert.match(renderTeamGroup(team("active"), [{ name: "t1-backend", turn_state: "busy" }]), /Current turn<\/dt><dd>busy/);
 assert.match(renderTeamGroup(team("active"), [{ name: "t1-backend", status: "running", model: "gpt-6-astra" }]), /Current turn<\/dt><dd>Unknown/);
});


const createInput = () => ({ ...createInitialTeamDraft(preset), team: "t1" });
for (const [name, mutate] of [
 ["active state/generation one", r => { r.team.state.state = "active"; r.team.state.generation = 1; r.creation_request.expected_generation = 1; }],
 ["altered immutable mission", r => r.team.snapshot.mission = "Different mission"],
 ["altered acceptance", r => r.team.snapshot.acceptance = "Different evidence"],
 ["altered preset reference", r => r.team.snapshot.preset.id = "different-preset"],
 ["altered seat tuple", r => r.team.snapshot.seats[0].model = "gpt-5.6-sol"],
 ["altered derived seat name", r => { r.team.snapshot.seats[1].name = "t1-other"; }],
 ["altered lead", r => r.team.snapshot.lead = r.team.snapshot.seats[1].name],
 ["altered fallback", r => r.team.snapshot.fallbacks[0].reasoning = "low"],
 ["reordered seats", r => r.team.snapshot.seats.reverse()],
]) test(`create rejects non-corresponding durable response: ${name}`, async () => {
 const request = createInput(); const response = created(request); mutate(response);
 const api = createTeamCommands(async () => response);
 await assert.rejects(api.create(request), e => e.code === "E_RESPONSE_INVALID");
});
for (const [name, mutate] of [
 ["shipped source", p => p.source = "shipped"],
 ["altered display name", p => p.display_name = "Different title"],
 ["altered mission placeholder", p => p.mission_placeholder = "Different mission"],
 ["altered acceptance placeholder", p => p.acceptance_placeholder = "Different evidence"],
 ["altered seat tuple", p => p.seats[0].model = "gpt-5.6-sol"],
 ["altered lead", p => p.lead_index = 1],
 ["altered fallbacks", p => p.fallbacks = []],
 ["invalid derived digest", p => p.sha256 = "invalid"],
]) test(`save rejects non-corresponding durable response: ${name}`, async () => {
 const response = { ...clone(preset), source: "local" }; mutate(response);
 const api = createTeamCommands(async () => response);
 await assert.rejects(api.savePreset(preset, preset.sha256), e => e.code === "E_RESPONSE_INVALID");
});
test("exact pending g0 echo accepts repeated role names and selected lead without normalizing text", async () => {
 const request = createInput(); request.mission = "  Exact immutable mission  "; request.seats.push(clone(request.seats[0])); request.lead_index = 2;
 const api = createTeamCommands(async () => created(request)); const result = await api.create(request);
 assert.equal(result.team.snapshot.lead, "t1-backend-2"); assert.equal(result.team.snapshot.mission, request.mission);
});

function observedTeam({ generation = 2, state = "active", requested, actual } = {}) {
 const t = team("active"), snapshot = t.snapshot.seats[0], fallback = t.snapshot.fallbacks[0];
 t.seats[0].observed_owner = { generation, state, since: "2026-09-20T00:00:00Z", configured: clone(requested ?? fallback), actual: actual === undefined ? clone(requested ?? fallback) : actual, process_count: 1, thread_bound: true };
 return { t, snapshot, fallback };
}
test("authorized replacement requested tuple differs from snapshot; independent generations accepted", () => {
 const { t } = observedTeam(); assert.equal(t.state.generation, 1); assert.equal(t.seats[0].observed_owner.generation, 2);
 assert.deepEqual(c.parseTeamView(t), t);
 const html = renderTeamGroup(t); assert.match(html, /Snapshot/); assert.match(html, /Requested/); assert.match(html, /Observed/);
 assert.match(html, /gpt-6-astra/); assert.match(html, /gpt-5.6-sol/);
});
test("initial owner g0 cannot substitute a fallback for its immutable snapshot", () => {
 const { t } = observedTeam({ generation: 0, state: "stale" }); rejects(() => c.parseTeamView(t));
 t.seats[0].observed_owner.configured = { ...preset.seats[0] }; t.seats[0].observed_owner.actual = null;
 assert.deepEqual(c.parseTeamView(t), t);
});
for (const [name, delta] of [["model", { model: "unapproved" }], ["reasoning", { reasoning: "low" }], ["harness", { harness: "claude", reasoning: null }]]) {
 test(`post-initial owner requested tuple outside immutable policy is rejected: ${name}`, () => {
  const { t } = observedTeam(); Object.assign(t.seats[0].observed_owner.configured, delta); t.seats[0].observed_owner.actual = clone(t.seats[0].observed_owner.configured);
  rejects(() => c.parseTeamView(t));
 });
}
test("active owner requires actual present and exactly matching requested", () => {
 for (const actual of [null, { ...preset.fallbacks[0], reasoning: "low" }, { ...preset.seats[0] }]) {
  const { t } = observedTeam({ actual }); rejects(() => c.parseTeamView(t));
 }
});
test("non-active owners can show absent or divergent actual without a success claim", () => {
 for (const state of ["starting", "stale", "quarantined"]) for (const actual of [null, { ...preset.seats[0] }]) {
  const { t } = observedTeam({ state, actual }); assert.deepEqual(c.parseTeamView(t), t);
  assert.doesNotMatch(renderTeamGroup(t), /verified model|replacement succeeded|healthy/i);
 }
});
