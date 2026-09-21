import { isRepositoryKey } from "./team-draft";
import type { TeamCatalog, TeamPreset, TeamView, TeamCreateResult, CancelPendingResult, ExecutionTuple, PresetSeat, TeamPresetInput } from "../types";

const record = (v: unknown): v is Record<string, unknown> => !!v && typeof v === "object" && !Array.isArray(v);
const str = (v: unknown): v is string => typeof v === "string";
const int = (v: unknown): v is number => Number.isSafeInteger(v) && (v as number) >= 0;
const hash = (v: unknown) => str(v) && /^[a-f0-9]{64}$/.test(v);
const oneOf = (v: unknown, values: readonly string[]) => str(v) && values.includes(v);
const arr = (v: unknown, guard: (item: unknown) => boolean): boolean => Array.isArray(v) && v.every(guard);
const invalid = (): never => { throw { code: "E_RESPONSE_INVALID", message: "Invalid team response" }; };
export function isExecutionTuple(v: unknown): v is ExecutionTuple {
  return record(v) && str(v.model) && v.model.length > 0 &&
    ((v.harness === "claude" && v.reasoning === null) || (v.harness === "codex" && str(v.reasoning) && v.reasoning.length > 0));
}
export function sameExecutionTuple(a: ExecutionTuple, b: ExecutionTuple): boolean {
  return a.harness === b.harness && a.model === b.model && a.reasoning === b.reasoning;
}
export function sameSeats(a: readonly PresetSeat[], b: readonly PresetSeat[]): boolean {
  return a.length === b.length && a.every((seat, i) => seat.role === b[i].role && sameExecutionTuple(seat, b[i]));
}
export function sameFallbacks(a: readonly ExecutionTuple[], b: readonly ExecutionTuple[]): boolean {
  return a.length === b.length && a.every((tuple, i) => sameExecutionTuple(tuple, b[i]));
}
export function samePresetContent(a: TeamPresetInput, b: TeamPresetInput): boolean {
  return a.schema_version === b.schema_version && a.id === b.id && a.display_name === b.display_name &&
    a.mission_placeholder === b.mission_placeholder && a.acceptance_placeholder === b.acceptance_placeholder &&
    a.lead_index === b.lead_index && sameSeats(a.seats, b.seats) && sameFallbacks(a.fallbacks, b.fallbacks);
}
const seat = (v: unknown) => record(v) && str(v.role) && isExecutionTuple(v);
const teamSeat = (v: unknown) => record(v) && str(v.name) && seat(v);
const preset = (v: unknown): v is TeamPreset => record(v) && v.schema_version === 1 &&
  [v.id, v.display_name, v.mission_placeholder, v.acceptance_placeholder].every(str) &&
  arr(v.seats, seat) && (v.seats as unknown[]).length > 0 && int(v.lead_index) && v.lead_index < (v.seats as unknown[]).length &&
  arr(v.fallbacks, isExecutionTuple) && oneOf(v.source, ["shipped", "local"]) && hash(v.sha256);
export function parseTeamPreset(v: unknown): TeamPreset { return preset(v) ? v : invalid(); }
export function parseTeamPresets(v: unknown): TeamPreset[] {
  if (!Array.isArray(v) || !v.every(preset) || new Set(v.map(p => p.id)).size !== v.length) return invalid();
  return v;
}
const LIMITS = ["max_seats", "max_fallbacks", "max_role_skills", "max_preset_bytes", "max_template_bytes", "max_rendered_seat_bytes", "max_rendered_team_bytes", "max_display_scalars", "max_display_bytes", "max_mission_scalars", "max_mission_bytes"];
export function parseTeamCatalog(v: unknown): TeamCatalog {
  if (!record(v) || !arr(v.roles, r => record(r) && str(r.id) && str(r.display_name)) ||
    !arr(v.execution_tuples, isExecutionTuple) || !record(v.limits) || !LIMITS.every(k => int((v.limits as Record<string, unknown>)[k]))) return invalid();
  if (!arr(v.repositories, r => record(r) && str(r.project) && isRepositoryKey(r.repo) &&
    str(r.display_name) && r.display_name.trim().length > 0 && typeof r.available === "boolean")) return invalid();
  const repos = v.repositories as TeamCatalog["repositories"];
  if (new Set(repos.map(r => JSON.stringify([r.project, r.repo]))).size !== repos.length) return invalid();
  return v as unknown as TeamCatalog;
}
// Rust PresetSnapshotRef uses None/None for conversation-created teams.
// Keep named presets hash-bound; a mixed/missing pair is still invalid.
const presetReference = (v: unknown): boolean => record(v) &&
  ((v.id === null && v.sha256 === null) || (str(v.id) && v.id.length > 0 && hash(v.sha256)));
function owner(v: unknown): boolean {
  return v === null || (record(v) && int(v.generation) && oneOf(v.state, ["starting", "active", "stale", "quarantined"]) && str(v.since) &&
    isExecutionTuple(v.configured) && (v.actual === null || isExecutionTuple(v.actual)) && int(v.process_count) && typeof v.thread_bound === "boolean");
}
export function parseTeamView(v: unknown): TeamView {
  if (!record(v) || !record(v.snapshot) || !record(v.state) || !record(v.capabilities)) return invalid();
  const s = v.snapshot, state = v.state;
  if (s.schema_version !== 1 || !isRepositoryKey(s.repo) || ![s.team, s.project, s.mission, s.acceptance, s.lead, s.created_at, s.creation_request_id, s.staging_uuid].every(str) ||
    !presetReference(s.preset) ||
    !arr(s.seats, teamSeat) || !arr(s.fallbacks, isExecutionTuple) || !Array.isArray(s.grants)) return invalid();
  if (state.schema_version !== 1 || !oneOf(state.state, ["pending", "active", "failed", "archived"]) || !int(state.generation) ||
    !(state.epic_id === null || str(state.epic_id)) || !str(state.updated_at) ||
    !(state.failure === null || (record(state.failure) && str(state.failure.code) && int(state.failure.completed_moves)))) return invalid();
  if (!["cancel", "activate", "start", "checkpoint", "replace", "archive"].every(k => typeof (v.capabilities as Record<string, unknown>)[k] === "boolean") ||
    !arr(v.seats, s => record(s) && teamSeat(s.configured) && owner(s.observed_owner))) return invalid();
  const result = v as unknown as TeamView;
  const names = result.snapshot.seats.map(s => s.name);
  if (!names.length || new Set(names).size !== names.length || !names.includes(result.snapshot.lead) ||
    result.seats.length !== names.length || new Set(result.seats.map(s => s.configured.name)).size !== names.length ||
    result.seats.some(s => {
      const snap = result.snapshot.seats.find(p => p.name === s.configured.name);
      return !snap || ["role", "harness", "model", "reasoning"].some(k => snap[k as keyof typeof snap] !== s.configured[k as keyof typeof s.configured]);
    })) return invalid();
  for (const { configured, observed_owner: observed } of result.seats) {
    if (!observed) continue;
    // Team lifecycle and seat incarnation generations are independent CAS counters.
    // owner.configured is the incarnation request, not the immutable seat snapshot.
    const allowed = observed.generation === 0
      ? sameExecutionTuple(observed.configured, configured)
      : [configured, ...result.snapshot.fallbacks].some(tuple => sameExecutionTuple(observed.configured, tuple));
    if (!allowed || (observed.state === "active" && (!observed.actual || !sameExecutionTuple(observed.actual, observed.configured)))) return invalid();
  }
  return result;
}
export function parseTeams(v: unknown): TeamView[] {
  if (!Array.isArray(v)) return invalid();
  const teams = v.map(parseTeamView);
  if (new Set(teams.map(t => t.snapshot.team)).size !== teams.length) return invalid();
  return teams;
}
export function parseTeamCreateResult(v: unknown): TeamCreateResult {
  if (!record(v) || !record(v.creation_request)) return invalid();
  const team = parseTeamView(v.team), r = v.creation_request;
  if (team.state.state !== "pending" || team.state.generation !== 0 || team.state.epic_id !== null || team.state.failure !== null) return invalid();
  if (r.schema_version !== 1 || ![r.request_id, r.team, r.project, r.created_at].every(str) || !hash(r.snapshot_sha256) || !int(r.expected_generation) ||
    !isRepositoryKey(r.repo) || r.repo !== team.snapshot.repo || r.team !== team.snapshot.team || r.project !== team.snapshot.project || r.request_id !== team.snapshot.creation_request_id || r.expected_generation !== team.state.generation) return invalid();
  return v as unknown as TeamCreateResult;
}
export function parseCancelled(v: unknown, team: string): CancelPendingResult {
  if (!record(v) || v.team !== team || v.cancelled !== true || !str(v.rejected_snapshot_id)) return invalid();
  return v as unknown as CancelPendingResult;
}
