import test from "node:test";
import assert from "node:assert/strict";
import { createServer } from "vite";
import { preset, catalog, team, created, clone } from "./fixtures/team-ui.mjs";
const vite = await createServer({ appType: "custom", logLevel: "silent", server: { middlewareMode: true } });
const c = await vite.ssrLoadModule("/src/services/team-contract.ts");
const { createTeamCommands } = await vite.ssrLoadModule("/src/services/team-commands.ts");
const { renderTeamGroup } = await vite.ssrLoadModule("/src/components/TeamsArea.ts");
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
 const injected = '<img src=x onerror="boom">';
 const t = team("active"); t.snapshot.team = injected; t.snapshot.mission = injected; t.seats[0].configured.name = injected;
 const html = renderTeamGroup(t); assert.equal(html.includes("<img"), false); assert.match(html, /&lt;img/);
});
test("preset drafting is detached and catalog membership is not inferred", () => {
 const p = clone(preset), draft = createInitialTeamDraft(p); draft.seats[0].model = "unavailable-model";
 assert.equal(p.seats[0].model, "gpt-6-astra"); assert.ok(validateCatalogDraft(draft, catalog).some(x => x.field === "seats.0.execution"));
 draft.seats[0] = { ...p.seats[0], role: "unlisted" }; assert.ok(validateCatalogDraft(draft, catalog).some(x => x.field === "seats.0.role"));
});
test("launcher wrappers expose read and cancel only: exact names, envelopes, no creation/presets/authority inputs", async () => {
 const calls = []; const api = createTeamCommands(async (cmd, args) => { calls.push([cmd, args]); return cmd === "team_list" ? [team()] : { team: "t1", cancelled: true, rejected_snapshot_id: "fixture" }; });
 assert.deepEqual(await api.list(), [team()]); assert.deepEqual(calls[0], ["team_list", undefined]);
 await api.cancel({ team: "t1", expected_generation: 0, creation_request_id: "fixture-request", actor: "glados", grants: ["forged"] });
 assert.deepEqual(calls[1], ["team_cancel_pending", { input: { team: "t1", expected_generation: 0, creation_request_id: "fixture-request" } }]);
 for (const removed of ["create", "savePreset", "presets", "catalog", "activate"]) assert.equal(removed in api, false, removed);
 assert.deepEqual(Object.keys(api).sort(), ["cancel", "list"]);
});
test("write failures do not auto-retry and malformed success is rejected", async () => {
 let calls = 0; const api = createTeamCommands(async () => { calls++; return { success: true }; });
 await assert.rejects(api.cancel({ team: "t1", expected_generation: 0, creation_request_id: "fixture-request" }), e => e.code === "E_RESPONSE_INVALID"); assert.equal(calls, 1);
 const rejected = createTeamCommands(async () => { throw { code: "E_CAS_CONFLICT", message: "fixed" }; });
 await assert.rejects(rejected.cancel({ team: "t1", expected_generation: 0, creation_request_id: "fixture-request" }), e => e.code === "E_CAS_CONFLICT");
});
test("successful cancel response for another team is not accepted", async () => {
 const api = createTeamCommands(async () => ({ team: "other", cancelled: true, rejected_snapshot_id: "fixture" }));
 await assert.rejects(api.cancel({ team: "t1", expected_generation: 0, creation_request_id: "fixture-request" }), e => e.code === "E_RESPONSE_INVALID");
});

test("turn state uses observed hub facts only, not requested model or owner state", () => {
 assert.match(renderTeamGroup(team("active"), [{ name: "t1-backend", turn_state: "busy" }]), /Current turn<\/dt><dd>busy/);
 assert.match(renderTeamGroup(team("active"), [{ name: "t1-backend", status: "running", model: "gpt-6-astra" }]), /Current turn<\/dt><dd>Unknown/);
});


const createInput = () => ({ ...createInitialTeamDraft(preset, catalog), team: "t1" });
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

test("repository catalog is mandatory, shaped and unambiguous; empty is legitimate", () => {
 for (const mutate of [
  v => delete v.repositories,
  v => v.repositories[0].available = "true",
  v => v.repositories[0].repo = "/tmp/arbitrary",
  v => v.repositories[0].repo = "../aperture",
  v => v.repositories[0].display_name = "",
  v => v.repositories.push(clone(v.repositories[0])),
 ]) { const v = clone(catalog); mutate(v); rejects(() => c.parseTeamCatalog(v)); }
 assert.deepEqual(c.parseTeamCatalog({ ...catalog, repositories: [] }).repositories, []);
});
test("repository choice uses served catalog only, never preset or project inference", () => {
 const p = { ...clone(preset), repo: "forged-from-preset" };
 assert.equal(createInitialTeamDraft(p).repo, "");
 assert.equal(createInitialTeamDraft(p, catalog).repo, "aperture");
 const changed = { ...catalog, repositories: [{ project: "project:aperture", repo: "native-new-key", display_name: "Native", available: true }] };
 assert.equal(createInitialTeamDraft(p, changed).repo, "native-new-key");
 assert.deepEqual(validateCatalogDraft({ ...createInitialTeamDraft(p, changed), team: "t1" }, changed), []);
 for (const [project, repo, pattern] of [
  ["project:incluir", "", /explicitly/],
  ["project:incluir", "aperture", /backend catalog/],
  ["project:aperture", "/tmp/aperture", /backend catalog/],
  ["project:mempalace", "", /No repositories/],
 ]) assert.match(validateCatalogDraft({ ...createInput(), project, repo }, catalog).find(i => i.field === "repo").message, pattern);
 const absent = clone(catalog); absent.repositories[0].available = false;
 assert.match(validateCatalogDraft(createInput(), absent).find(i => i.field === "repo").message, /unavailable locally/);
});
test("immutable repository is visible in pending and active approval context", () => {
 for (const state of ["pending", "active"]) assert.match(renderTeamGroup(team(state)), /repository: aperture \(immutable\)/);
});
