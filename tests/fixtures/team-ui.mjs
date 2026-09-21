export const execution = { harness: "codex", model: "gpt-6-astra", reasoning: "high" };
export const preset = {
 schema_version: 1, id: "fullstack", display_name: "Fullstack", mission_placeholder: "Build a scoped slice", acceptance_placeholder: "Reviewed evidence",
 seats: [{ role: "backend", ...execution }, { role: "qa", harness: "claude", model: "sonnet", reasoning: null }],
 lead_index: 0, fallbacks: [{ harness: "codex", model: "gpt-5.6-sol", reasoning: "high" }], source: "shipped", sha256: "a".repeat(64),
};
export const catalog = {
 repositories: [
  { project: "project:aperture", repo: "aperture", display_name: "Aperture", available: true },
  { project: "project:incluir", repo: "monorepo-incluir", display_name: "Incluir", available: true },
  { project: "project:incluir", repo: "eunenem", display_name: "EuNeném", available: true },
 ],
 roles: [{ id: "backend", display_name: "Backend" }, { id: "qa", display_name: "QA" }],
 execution_tuples: [execution, preset.seats[1], preset.fallbacks[0]].map(({ harness, model, reasoning }) => ({ harness, model, reasoning })),
 limits: { max_seats: 99, max_fallbacks: 16, max_role_skills: 64, max_preset_bytes: 262144, max_template_bytes: 131072, max_rendered_seat_bytes: 262144, max_rendered_team_bytes: 8388608, max_display_scalars: 80, max_display_bytes: 320, max_mission_scalars: 2000, max_mission_bytes: 8000 },
};
export function team(lifecycle = "pending") {
 const snapshot = { schema_version: 1, team: "t1", project: "project:aperture", repo: "aperture", mission: "Scoped mission", acceptance: "Reviewed evidence", preset: { id: "fullstack", sha256: preset.sha256 }, lead: "t1-backend", seats: preset.seats.map(s => ({ ...s, name: `t1-${s.role}` })), fallbacks: preset.fallbacks, grants: [], created_at: "2026-09-20T00:00:00Z", creation_request_id: "fixture-request", staging_uuid: "fixture-staging" };
 return { snapshot, state: { schema_version: 1, state: lifecycle, generation: lifecycle === "pending" ? 0 : 1, epic_id: null, failure: null, updated_at: snapshot.created_at }, seats: snapshot.seats.map(configured => ({ configured, observed_owner: null })), capabilities: { cancel: lifecycle === "pending", activate: false, start: false, checkpoint: false, replace: false, archive: false } };
}
export function created(input) {
 const t = team();
 if (input) {
  const counts = new Map();
  const seats = input.seats.map(s => { const n = (counts.get(s.role) ?? 0) + 1; counts.set(s.role, n); return { ...s, name: `${input.team}-${s.role}${n === 1 ? "" : `-${n}`}` }; });
  Object.assign(t.snapshot, { team: input.team, project: input.project, repo: input.repo, mission: input.mission, acceptance: input.acceptance, preset: { id: input.preset_id, sha256: input.preset_id === null ? null : preset.sha256 }, seats, lead: seats[input.lead_index]?.name, fallbacks: clone(input.fallbacks) });
  t.seats = seats.map(configured => ({ configured, observed_owner: null }));
 }
 return { team: t, creation_request: { schema_version: 1, request_id: t.snapshot.creation_request_id, team: t.snapshot.team, project: t.snapshot.project, repo: t.snapshot.repo, snapshot_sha256: "b".repeat(64), expected_generation: 0, created_at: t.snapshot.created_at } }; }
export const clone = v => structuredClone(v);
