// Source-only synthetic native replies. No fixture is wired into production capabilities.
import test from "node:test";
import assert from "node:assert/strict";
import { createServer } from "vite";
import { team, clone, preset } from "./fixtures/team-ui.mjs";
const vite = await createServer({ appType: "custom", logLevel: "silent", server: { middlewareMode: true } });
const r = await vite.ssrLoadModule("/src/services/team-runtime.ts");
const { renderReplacementEvidence, renderArchiveEvidence } = await vite.ssrLoadModule("/src/components/TeamLifecycle.ts");
await vite.close();
const tuple = () => ({ harness: "codex", model: "gpt-6-astra", reasoning: "high" });
function current() {
 const t = team("active"); t.capabilities.replace = true; t.capabilities.archive = true;
 t.seats[0].observed_owner = { generation: 4, state: "active", since: "fixture-time", configured: tuple(), actual: tuple(), process_count: 2, thread_bound: true }; return t;
}
function prepared(t = current()) { return { team: "t1", seat: "t1-backend", generation: 4, phase: "ready", checkpoint_recovery: "stale", checks: { process_stop: "verified", revocation: "verified", remote_effects: "verified" }, owner: { ...clone(t.seats[0].observed_owner), state: "stale" }, blockers: [], preparation_id: "fixture-preparation" }; }
function archive(state = "blocked") { return { team: "t1", generation: 1, state, checks: { reconciliation: "verified", reviews: "unknown", metrics: "verified", process_stop: "verified", revocation: "verified", remote_effects: "unknown", worktrees: "verified" }, blockers: [{ code: "E_REVIEW_MISSING", reference: "fixture-review" }] }; }
const rejected = fn => assert.throws(fn, e => e.code === "E_RESPONSE_INVALID");

test("false or missing runtime capabilities produce zero invokes", async () => {
 for (const value of [false, undefined]) {
  const t = current(); t.capabilities.replace = value; t.capabilities.archive = value;
  let calls = 0; const api = r.createRuntimeCommands(async () => { calls++; throw Error("must not invoke"); });
  for (const p of [api.prepare(t, "t1-backend"), api.start(t, "t1-backend", prepared(t), tuple()), api.archive(t)]) await assert.rejects(p, e => e.code === "E_RUNTIME_UNAVAILABLE");
  assert.equal(calls, 0);
 }
});
test("missing owner generation cannot borrow team generation or infer stopped", async () => {
 const t = current(); t.seats[0].observed_owner = null; let calls = 0;
 await assert.rejects(r.createRuntimeCommands(async () => { calls++; }).prepare(t, "t1-backend"), e => e.code === "E_RUNTIME_UNAVAILABLE");
 assert.equal(calls, 0); assert.equal(r.ownerGeneration(t, "t1-backend"), null);
});
test("aggregated replace availability never enables an ineligible or Claude seat", async () => {
 const states = ["starting", "stale", "quarantined"];
 for (const state of states) {
  const t = current(); t.seats[0].observed_owner.state = state; let calls = 0;
  assert.equal(r.canPrepareReplacement(t, "t1-backend"), false);
  await assert.rejects(r.createRuntimeCommands(async () => { calls++; }).prepare(t, "t1-backend"), e => e.code === "E_RUNTIME_UNAVAILABLE");
  assert.equal(calls, 0);
 }
 const t = current();
 const claude = t.snapshot.seats[1];
 t.seats[1].observed_owner = { generation: 2, state: "active", since: "fixture-time",
  configured: { harness: claude.harness, model: claude.model, reasoning: claude.reasoning },
  actual: { harness: claude.harness, model: claude.model, reasoning: claude.reasoning }, process_count: 1, thread_bound: true };
 let calls = 0;
 assert.equal(r.canPrepareReplacement(t, claude.name), false);
 await assert.rejects(r.createRuntimeCommands(async () => { calls++; }).prepare(t, claude.name), e => e.code === "E_RUNTIME_UNAVAILABLE");
 assert.equal(calls, 0);
});
test("observed Codex fallback remains prepare-eligible without matching the initial snapshot tuple", async () => {
 const t = current();
 t.seats[0].observed_owner.configured = clone(preset.fallbacks[0]);
 t.seats[0].observed_owner.actual = clone(preset.fallbacks[0]);
 let calls = 0;
 const api = r.createRuntimeCommands(async () => { calls++; return prepared(t); });
 assert.equal(r.canPrepareReplacement(t, "t1-backend"), true);
 await api.prepare(t, "t1-backend"); assert.equal(calls, 1);
});
test("prepare is separate; inputs have no proof or actors and use owner g4, not team g1", async () => {
 const t = current(), calls = []; const api = r.createRuntimeCommands(async (...args) => { calls.push(args); return prepared(t); });
 await api.prepare(t, "t1-backend"); assert.deepEqual(calls, [["team_prepare_replacement", { input: { team: "t1", seat: "t1-backend", expected_generation: 4 } }]]);
});
test("prepare permit remains startable after an authoritative same-generation refresh", async () => {
 const initial = current(); let calls = 0;
 const api = r.createRuntimeCommands(async name => {
  calls++;
  if (name === "team_prepare_replacement") return prepared(initial);
  const result = prepared(initial); delete result.preparation_id;
  return { ...result, generation: 5, phase: "started", owner: { ...result.owner, generation: 5, state: "active", actual: tuple() } };
 });
 const permit = await api.prepare(initial, "t1-backend");
 const refreshed = clone(initial);
 await api.start(refreshed, "t1-backend", permit, tuple());
 assert.equal(calls, 2);
});
test("Claude replacement selection remains zero-invoke even when team replace is available", async () => {
 const t = current(), p = prepared(t), selection = { harness: "claude", model: "sonnet", reasoning: null }; let calls = 0;
 assert.equal(r.canStartReplacement(t, "t1-backend", p, selection), false);
 await assert.rejects(r.createRuntimeCommands(async () => { calls++; }).start(t, "t1-backend", p, selection), e => e.code === "E_RUNTIME_UNAVAILABLE");
 assert.equal(calls, 0);
});
for (const [label, mutate] of [
 ["no permit", p => p.preparation_id = null], ["unknown stop", p => p.checks.process_stop = "unknown"],
 ["pending revocation", p => p.checks.revocation = "pending"], ["unknown remote effects", p => p.checks.remote_effects = "unknown"],
 ["blocked phase", p => p.phase = "blocked"], ["different generation", p => p.generation = 3],
 ["wrong seat", p => p.seat = "t1-qa"], ["wrong team", p => p.team = "other"],
 ["blocker exists", p => p.blockers = [{ code: "E_REMOTE_UNCERTAIN", reference: "t1-backend" }]],
]) test(`start stays disabled and never invokes: ${label}`, async () => {
 const t = current(), p = prepared(t); mutate(p); let calls = 0;
 assert.equal(r.canStartReplacement(t, "t1-backend", p, tuple()), false);
 await assert.rejects(r.createRuntimeCommands(async () => { calls++; }).start(t, "t1-backend", p, tuple())); assert.equal(calls, 0);
});
test("stale/none checkpoint is a warning, not a forged valid checkpoint or automatic blocker", () => {
 for (const checkpoint_recovery of ["stale", "none"]) {
  const t = current(), p = { ...prepared(t), checkpoint_recovery };
  assert.equal(r.canStartReplacement(t, "t1-backend", p, tuple()), true);
  assert.match(renderReplacementEvidence(p), new RegExp(`Checkpoint recovery: ${checkpoint_recovery}`));
  assert.match(renderReplacementEvidence(p), /warning/);
 }
});
test("catalog membership cannot authorize off-policy tuple", async () => {
 const t = current(); const off = { ...tuple(), reasoning: "low" }; let calls = 0;
 assert.equal(r.canStartReplacement(t, "t1-backend", prepared(t), off), false);
 await assert.rejects(r.createRuntimeCommands(async () => { calls++; }).start(t, "t1-backend", prepared(t), off)); assert.equal(calls, 0);
});
test("start sends only exact selectors and selection, returns authoritative generation without client increment", async () => {
 const t = current(); const p = prepared(t); const selection = { ...preset.fallbacks[0], actor: "glados", pid: 100, proof: true };
 const response = { ...p, phase: "started", generation: 7, owner: { ...p.owner, generation: 7, state: "active", configured: clone(preset.fallbacks[0]), actual: clone(preset.fallbacks[0]) } }; delete response.preparation_id;
 let sent; const api = r.createRuntimeCommands(async (...args) => { sent = args; return response; });
 const result = await api.start(t, "t1-backend", p, selection);
 assert.equal(result.generation, 7); assert.equal(t.seats[0].observed_owner.generation, 4);
 assert.deepEqual(sent, ["team_start_replacement", { input: { team: "t1", seat: "t1-backend", expected_generation: 4, preparation_id: p.preparation_id, selection: preset.fallbacks[0] } }]);
});
test("model failure/transport loss never retries or increments owner generation", async () => {
 const t = current(); let calls = 0;
 const api = r.createRuntimeCommands(async () => { calls++; throw { code: "E_START_CLEANUP_UNVERIFIED", message: "SENTINEL_SECRET" }; });
 await assert.rejects(api.start(t, "t1-backend", prepared(t), tuple())); assert.equal(calls, 1); assert.equal(t.seats[0].observed_owner.generation, 4);
 assert.doesNotMatch(r.runtimeErrorCopy({ code: "E_START_CLEANUP_UNVERIFIED", message: "SENTINEL_SECRET" }), /SENTINEL_SECRET/);
 const failed = { ...prepared(t), phase: "model_unverified", preparation_id: null, owner: { ...prepared(t).owner, state: "quarantined", actual: null } };
 assert.match(renderReplacementEvidence(failed), /No successful replacement is confirmed/); assert.match(renderReplacementEvidence(failed), /Unknown — actual model not observed/);
});
test("runtime data boundary refuses private owner fields and forged top-level proof", () => {
 for (const [path, key, value] of [["owner", "pid", 100], ["owner", "thread_id", "fixture-thread"], ["owner", "token_id", "fixture-token"], ["", "proof", true], ["checks", "authorized", true]]) {
  const p = prepared(); (path ? p[path] : p)[key] = value; rejected(() => r.parseReplacement(p, current(), "t1-backend", true));
 }
 const p = prepared(); p.owner.configured.token = "SENTINEL_SECRET"; rejected(() => r.parseReplacement(p, current(), "t1-backend", true));
});
test("malformed identities and started-but-unverified replies are not successful", () => {
 for (const mutate of [p => p.team = "other", p => p.seat = "other", p => p.owner.generation = 12, p => p.checks = {}, p => p.blockers = [{ code: "E_REMOTE_UNCERTAIN", reference: "/tmp/private" }]]) {
  const p = prepared(); mutate(p); rejected(() => r.parseReplacement(p, current(), "t1-backend", true));
 }
 const p = prepared(); delete p.preparation_id; p.phase = "started"; p.owner.state = "active"; p.owner.actual = null;
 rejected(() => r.parseReplacement(p, current(), "t1-backend", false));
});
test("archive uses team g1, sends no evidence/approval/task lists", async () => {
 let sent; const t = current(); const api = r.createRuntimeCommands(async (...args) => { sent = args; return archive(); });
 const result = await api.archive(t); assert.equal(result.state, "blocked");
 assert.deepEqual(sent, ["team_archive", { input: { team: "t1", expected_generation: 1 } }]);
});
test("archive missing reviews/worktree/remote checks and journal ambiguity cannot display archived", () => {
 for (const state of ["pending", "blocked", "unknown"]) assert.match(renderArchiveEvidence(r.parseArchive(archive(state), current())), new RegExp(`Backend archive state: ${state}`));
 for (const key of ["reviews", "worktrees", "remote_effects"]) {
  const a = archive("archived"); a.blockers = []; for (const k in a.checks) a.checks[k] = "verified"; a.checks[key] = "unknown";
  rejected(() => r.parseArchive(a, current()));
 }
 assert.match(r.runtimeErrorCopy({ code: "E_JOURNAL_INCONSISTENT" }), /No rollback or completion/);
 assert.doesNotMatch(renderArchiveEvidence(null), /Backend archive state: archived/);
});
test("no remote observation stays unknown, never 0-in-flight or verified by empty array", () => {
 assert.match(renderReplacementEvidence(null), /remote effects.*unknown/); assert.doesNotMatch(renderReplacementEvidence(null), /0.in.flight|remote effects.*verified/);
 const p = prepared(); p.checks.remote_effects = []; rejected(() => r.parseReplacement(p, current(), "t1-backend", true));
});

test("started must return a strictly newer authoritative owner generation, no assumed plus-one", async () => {
 const t = current(), p = prepared(t);
 const response = { ...p, phase: "started", owner: { ...p.owner, state: "active", actual: tuple() } }; delete response.preparation_id;
 const api = r.createRuntimeCommands(async () => response);
 await assert.rejects(api.start(t, "t1-backend", p, tuple()), e => e.code === "E_RESPONSE_INVALID");
});
test("archive generation is canonical readback, never computed or equated to request", async () => {
 const a = archive("archived"); a.generation = 9; a.blockers = []; for (const k in a.checks) a.checks[k] = "verified";
 const t = current(); const result = await r.createRuntimeCommands(async () => a).archive(t);
 assert.equal(result.generation, 9); assert.equal(t.state.generation, 1);
});

function bootable() {
 const t = current(); t.capabilities.start = true;
 t.seats[0].observed_owner = { ...t.seats[0].observed_owner, generation: 0, state: "stale", actual: null, process_count: 0, thread_bound: false };
 return t;
}
function booted(t = bootable()) {
 return { team: "t1", seat: "t1-backend", generation: 1, phase: "started",
  owner: { ...clone(t.seats[0].observed_owner), generation: 1, state: "active", actual: tuple(), process_count: 1, thread_bound: true }, blockers: [] };
}
test("bootstrap input is exact selectors/g0 only; snapshot implicit, no picker/prepare/actor", async () => {
 const t = bootable(); t.state.generation = 27; let sent;
 const api = r.createRuntimeCommands(async (...args) => { sent = args; return booted(t); });
 const result = await api.bootstrap(t, "t1-backend");
 assert.deepEqual(sent, ["team_bootstrap_seat", { input: { team: "t1", seat: "t1-backend", expected_generation: 0 } }]);
 assert.equal(result.generation, 1); assert.equal(t.seats[0].observed_owner.generation, 0); assert.equal(t.state.generation, 27);
});
for (const [label, mutate] of [
 ["false capability", t => t.capabilities.start = false], ["missing capability", t => delete t.capabilities.start],
 ["pending team", t => t.state.state = "pending"], ["missing owner", t => t.seats[0].observed_owner = null],
 ["starting owner", t => t.seats[0].observed_owner.state = "starting"], ["nonzero owner", t => t.seats[0].observed_owner.generation = 1],
 ["wrong configured tuple", t => t.seats[0].observed_owner.configured = preset.fallbacks[0]],
 ["Claude configured seat", t => { const seat = t.snapshot.seats[0]; seat.harness = "claude"; seat.model = "sonnet"; seat.reasoning = null; t.seats[0].configured = clone(seat); t.seats[0].observed_owner.configured = { harness: "claude", model: "sonnet", reasoning: null }; }],
 ["unexpected observed tuple", t => t.seats[0].observed_owner.actual = tuple()],
 ["unexpected process evidence", t => t.seats[0].observed_owner.process_count = 1],
 ["unexpected thread evidence", t => t.seats[0].observed_owner.thread_bound = true],
]) test(`bootstrap zero-invoke gate: ${label}`, async () => {
 const t = bootable(); mutate(t); let calls = 0;
 assert.equal(r.canBootstrapSeat(t, "t1-backend"), false);
 await assert.rejects(r.createRuntimeCommands(async () => { calls++; }).bootstrap(t, "t1-backend"), e => e.code === "E_RUNTIME_UNAVAILABLE");
 assert.equal(calls, 0);
});
test("bootstrap unknown seat and g0 replacement preparation never invoke", async () => {
 const t = bootable(); let calls = 0; const api = r.createRuntimeCommands(async () => { calls++; });
 await assert.rejects(api.prepare(t, "t1-backend"));
 assert.equal(calls, 0);
 await assert.rejects(api.bootstrap(t, "other"));
 assert.equal(r.canStartReplacement(t, "t1-backend", { ...prepared(t), generation: 0 }, tuple()), false);
 assert.equal(calls, 0);
});
for (const [label, mutate] of [
 ["wrong team", v => v.team = "other"], ["wrong seat", v => v.seat = "other"],
 ["zero started generation", v => { v.generation = 0; v.owner.generation = 0; }],
 ["generation mismatch", v => v.owner.generation = 2], ["missing active owner", v => v.owner = null],
 ["missing actual", v => v.owner.actual = null], ["different actual", v => v.owner.actual = preset.fallbacks[0]],
 ["fallback used for first start", v => { v.owner.configured = preset.fallbacks[0]; v.owner.actual = preset.fallbacks[0]; }],
 ["started with blocker", v => v.blockers = [{ code: "E_MODEL_UNVERIFIED", reference: "fixture" }]],
 ["raw PID", v => v.owner.pid = 123], ["raw thread", v => v.owner.thread_id = "fixture"],
 ["invented verified checks", v => v.checks = { process_stop: "verified" }],
]) test(`bootstrap rejects false success/private reply: ${label}`, () => {
 const v = booted(); mutate(v); rejected(() => r.parseBootstrap(v, bootable(), "t1-backend"));
});
test("bootstrap pending/blocked are evidence, not fabricated completed checks", () => {
 for (const phase of ["starting", "blocked"]) {
  const v = { ...booted(), phase, owner: null };
  assert.equal(r.parseBootstrap(v, bootable(), "t1-backend").phase, phase);
 }
});
test("bootstrap transport/deadline errors never retry or change generation", async () => {
 for (const code of ["E_LAUNCH_UNAVAILABLE", "E_MODEL_UNVERIFIED", "E_START_CLEANUP_UNVERIFIED", "E_RUNTIME_DEADLINE", "E_CONTROL_UNKNOWN"]) {
  const t = bootable(); let calls = 0;
  const api = r.createRuntimeCommands(async () => { calls++; throw { code, message: "SECRET_SENTINEL" }; });
  await assert.rejects(api.bootstrap(t, "t1-backend")); assert.equal(calls, 1); assert.equal(t.seats[0].observed_owner.generation, 0);
  assert.doesNotMatch(r.runtimeErrorCopy({ code, message: "SECRET_SENTINEL" }), /SECRET_SENTINEL/);
 }
});
