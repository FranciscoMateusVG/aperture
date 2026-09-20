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
 assert.match(html, /Configured/); assert.match(html, /Unknown — not observed/); assert.match(html, /Not available from this backend/);
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
 const calls = []; const api = createTeamCommands(async (cmd, args) => { calls.push([cmd, args]); return cmd === "team_create" ? created() : cmd === "team_save_preset" ? preset : { team: "t1", cancelled: true, rejected_snapshot_id: "fixture-rejected" }; });
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
