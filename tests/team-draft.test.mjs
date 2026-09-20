import test from "node:test";
import assert from "node:assert/strict";
import { createServer } from "vite";

const vite = await createServer({ appType: "custom", logLevel: "silent", server: { middlewareMode: true } });
const { deriveTeamSeatNames, validateTeamDraft, snapshotTeamDraft, teamErrorCopy, validHumanText } = await vite.ssrLoadModule("/src/services/team-draft.ts");
await vite.close();
const seat = { role: "backend", harness: "codex", model: "gpt-6-astra", reasoning: "high" };
const draft = () => ({ team: "t1", project: "project:aperture", mission: "  A scoped mission  ", acceptance: "  A reviewed PR  ", preset_id: "fullstack", seats: [{ ...seat }], lead_index: 0, fallbacks: [{ harness: "codex", model: "gpt-5.6-sol", reasoning: "high" }] });

test("valid draft and stable repeated-role names", () => {
  assert.deepEqual(validateTeamDraft(draft()), []);
  assert.deepEqual(deriveTeamSeatNames("t1", [seat, { ...seat, role: "qa" }, seat, seat]), ["t1-backend", "t1-qa", "t1-backend-2", "t1-backend-3"]);
});
for (const team of ["", "T1.x", "../x", "a/b", "_t1", "t1 space", "a".repeat(17), '<img onerror="x">']) {
  test(`invalid team is refused locally: ${JSON.stringify(team)}`, () => assert.ok(validateTeamDraft({ ...draft(), team }).some(x => x.field === "team")));
}
for (const team of ["a", "1", "a".repeat(16)]) {
  test(`valid team boundary: ${team}`, () => assert.deepEqual(validateTeamDraft({ ...draft(), team }), []));
}
test("known active/archive/reserved collisions are refused, not silently renamed", () => {
  assert.ok(validateTeamDraft(draft(), ["t1-backend"]).some(x => x.field === "seats.0.name"));
  assert.ok(validateTeamDraft(draft(), [], ["t1"]).some(x => x.field === "team"));
  assert.ok(validateTeamDraft({ ...draft(), team: "operator" }).some(x => x.field === "team"));
});
test("exact tuple: Claude reasoning null, Codex reasoning nonempty, unknown harness rejected", () => {
  for (const invalid of [{ harness: "other" }, { reasoning: " " }, { model: " " }, { harness: "claude", reasoning: "high" }]) {
    assert.ok(validateTeamDraft({ ...draft(), seats: [{ ...seat, ...invalid }] }).some(x => x.field === "seats.0.execution"));
  }
  assert.deepEqual(validateTeamDraft({ ...draft(), seats: [{ ...seat, harness: "claude", model: "opus", reasoning: null }] }), []);
});
test("lead must refer to a remaining seat after removal", () => {
  for (const lead_index of [-1, 1, 0.5, NaN]) assert.ok(validateTeamDraft({ ...draft(), lead_index }).some(x => x.field === "lead_index"));
  assert.ok(validateTeamDraft({ ...draft(), seats: [] }).some(x => x.field === "seats"));
});
test("required mission, acceptance and canonical project", () => {
  for (const field of ["mission", "acceptance", "project"]) assert.ok(validateTeamDraft({ ...draft(), [field]: " " }).some(x => x.field === field));
});
test("role and duplicate bound at 99, no truncated names", () => {
  const input = { ...draft(), team: "a".repeat(16), seats: Array.from({ length: 99 }, () => ({ ...seat, role: "b".repeat(10) })) };
  assert.deepEqual(validateTeamDraft(input), []);
  assert.equal(deriveTeamSeatNames(input.team, input.seats).at(-1).length, 30);
  input.seats.push({ ...seat, role: "b".repeat(10) });
  assert.ok(validateTeamDraft(input).some(x => x.field === "seats.99.name"));
});
test("submission copy is detached; text and exact tuples stay unnormalized", () => {
  const input = { ...draft(), actor: "glados", grants: ["forged"] }; const result = snapshotTeamDraft(input);
  assert.equal(result.mission, "  A scoped mission  "); assert.equal(result.acceptance, "  A reviewed PR  ");
  assert.deepEqual(result.seats, input.seats); assert.deepEqual(result.fallbacks, input.fallbacks);
  input.seats[0].model = "changed"; input.fallbacks[0].reasoning = "low";
  assert.equal(result.seats[0].model, "gpt-6-astra"); assert.equal(result.fallbacks[0].reasoning, "high");
  assert.equal("actor" in result, false); assert.equal("grants" in result, false);
});
test("error boundary uses fixed text and does not claim notification or successful creation", () => {
  const sentinel = "SENTINEL_SECRET_PATH";
  for (const error of [new Error(sentinel), sentinel, { code: sentinel, message: sentinel }, { code: "E_STAGING_IO", message: sentinel }]) assert.equal(teamErrorCopy(error).includes(sentinel), false);
  assert.match(teamErrorCopy({ code: "E_CREATION_GATE_PENDING" }), /Awaiting registration and approval by GLaDOS/);
  assert.doesNotMatch(teamErrorCopy({ code: "E_CREATION_GATE_PENDING" }), /notified|sent|created successfully/);
});

test("v2 human-text boundaries count Unicode scalars and reject controls without normalization", () => {
  assert.equal(validHumanText("😀".repeat(80), 80, 320), true);
  assert.equal(validHumanText("😀".repeat(81), 80, 320), false);
  for (const text of ["a\0b", "a\nb", "a\u202eb", "a\u2066b", "a\u007fb"]) assert.equal(validHumanText(text, 80, 320), false);
  assert.equal(validHumanText(" leading and trailing ", 80, 320), true);
});
test("v2 taxonomy and total budgets", () => {
  assert.ok(validateTeamDraft({ ...draft(), project: "project:new-unapproved" }).some(x => x.field === "project"));
  assert.ok(validateTeamDraft({ ...draft(), mission: "a".repeat(2001) }).some(x => x.field === "mission"));
  assert.ok(validateTeamDraft({ ...draft(), fallbacks: Array.from({ length: 17 }, () => draft().fallbacks[0]) }).some(x => x.field === "fallbacks"));
  assert.ok(validateTeamDraft({ ...draft(), seats: Array.from({ length: 100 }, () => ({ ...seat })) }).some(x => x.field === "seats"));
});
