import type { CreateTeamInput, ExecutionTuple, PresetSeat, RepositoryCatalogEntry } from "../types";
import { isValidSeatName } from "./seat-name";

export interface DraftIssue { field: string; message: string }
const TEAM_RE = /^[a-z0-9][a-z0-9_-]{0,15}$/;
const ROLE_RE = /^[a-z0-9][a-z0-9_-]{0,9}$/;
const PROJECT = /^project:[a-z][a-z0-9._-]{0,63}$/;
const HUMAN_CONTROLS = /[\u0000-\u001f\u007f-\u009f\u061c\u200e\u200f\u202a-\u202e\u2066-\u2069]/;
const MODEL_RE = /^[A-Za-z0-9][A-Za-z0-9._:/-]{0,79}$/;

export function validHumanText(value: string, maxScalars: number, maxBytes: number): boolean {
  return !!value.trim() && !HUMAN_CONTROLS.test(value) &&
    [...value].length <= maxScalars && new TextEncoder().encode(value).length <= maxBytes;
}
const RESERVED = new Set(["glados", "wheatley", "peppy", "operator", "watchdog", "shared"]);

/** Preview only. Existing/archive collisions and authorization are rechecked by Rust. */
export function deriveTeamSeatNames(team: string, seats: readonly Pick<PresetSeat, "role">[]): string[] {
  const counts = new Map<string, number>();
  return seats.map(({ role }) => {
    const n = (counts.get(role) ?? 0) + 1;
    counts.set(role, n);
    return `${team}-${role}${n === 1 ? "" : `-${n}`}`;
  });
}

function validTuple(tuple: ExecutionTuple): boolean {
  return typeof tuple.model === "string" && MODEL_RE.test(tuple.model) &&
    ((tuple.harness === "claude" && tuple.reasoning === null) ||
      (tuple.harness === "codex" && typeof tuple.reasoning === "string" && tuple.reasoning.trim().length > 0));
}

export const isRepositoryKey = (value: unknown): value is string =>
  typeof value === "string" && /^[a-z0-9][a-z0-9._-]{0,63}$/.test(value);

/** Choices are native read facts, never a TypeScript repository authority map. */
export function initialRepository(project: string, repositories: readonly RepositoryCatalogEntry[]): string {
  const choices = repositories.filter(r => r.project === project);
  return choices.length === 1 ? choices[0].repo : "";
}
export function validateRepositoryDraft(input: Pick<CreateTeamInput, "project" | "repo">, repositories: readonly RepositoryCatalogEntry[]): DraftIssue[] {
  const choices = repositories.filter(r => r.project === input.project);
  const entry = choices.find(r => r.repo === input.repo);
  const message = !choices.length ? "No repositories are configured for this project. Team creation is unavailable."
    : !input.repo ? "Choose a repository explicitly before creating this team."
    : !isRepositoryKey(input.repo) || !entry ? "Choose a repository from this project's backend catalog."
    : !entry.available ? "This repository is unavailable locally. Creation is blocked until the backend can validate it."
    : null;
  return message ? [{ field: "repo", message }] : [];
}

/** Local affordance checks, not an independent permission/policy engine. */
export function validateTeamDraft(
  input: CreateTeamInput,
  knownSeatNames: readonly string[] = [],
  knownTeamNames: readonly string[] = [],
): DraftIssue[] {
  const issues: DraftIssue[] = [];
  const add = (field: string, message: string) => issues.push({ field, message });
  if (!TEAM_RE.test(input.team)) add("team", "Use 1–16 lowercase letters, digits, hyphens or underscores; start with a letter or digit.");
  if (knownTeamNames.includes(input.team) || RESERVED.has(input.team)) add("team", "This team name is reserved or already in use, including archived teams.");
  if (!PROJECT.test(input.project)) add("project", "Enter the registered project label, normally project:<repository-key>.");
  if (!validHumanText(input.mission, 2000, 8000)) add("mission", "Describe a mission of up to 2,000 characters without control characters.");
  if (!validHumanText(input.acceptance, 2000, 8000)) add("acceptance", "Describe acceptance in up to 2,000 characters without control characters.");
  if (!input.seats.length || input.seats.length > 99) add("seats", "Use between 1 and 99 seats.");
  if (input.fallbacks.length > 16) add("fallbacks", "Use at most 16 approved fallback tuples.");
  if (!Number.isInteger(input.lead_index) || input.lead_index < 0 || input.lead_index >= input.seats.length) add("lead_index", "Choose exactly one lead from this team.");
  const names = deriveTeamSeatNames(input.team, input.seats);
  const used = new Set([...knownSeatNames, ...RESERVED]);
  const counts = new Map<string, number>();
  input.seats.forEach((seat, index) => {
    if (!ROLE_RE.test(seat.role)) add(`seats.${index}.role`, "Use a valid role identifier of 1–10 characters.");
    if (!validTuple(seat)) add(`seats.${index}.execution`, "Choose a model and harness; Codex requires reasoning and Claude uses none.");
    const count = (counts.get(seat.role) ?? 0) + 1;
    counts.set(seat.role, count);
    if (count > 99 || !isValidSeatName(names[index])) add(`seats.${index}.name`, "The derived seat name exceeds the supported name or duplicate limit.");
    if (used.has(names[index])) add(`seats.${index}.name`, "This seat name is already reserved, active or archived.");
    used.add(names[index]);
  });
  input.fallbacks.forEach((tuple, index) => {
    if (!validTuple(tuple)) add(`fallbacks.${index}`, "Each fallback needs its exact harness, model and reasoning tuple.");
  });
  return issues;
}

/** Copy at submission. Editing the form later must not mutate the in-flight request. */
export function snapshotTeamDraft(input: CreateTeamInput): CreateTeamInput {
  return {
    team: input.team,
    project: input.project,
    repo: input.repo,
    preset_id: input.preset_id,
    lead_index: input.lead_index,
    mission: input.mission,
    acceptance: input.acceptance,
    seats: input.seats.map(({ role, harness, model, reasoning }) => ({ role, harness, model, reasoning })),
    fallbacks: input.fallbacks.map(({ harness, model, reasoning }) => ({ harness, model, reasoning })),
  };
}

const ERROR_COPY: Record<string, string> = {
  E_REPO_REQUIRED: "Choose a repository before creating this team.",
  E_REPO_NOT_IN_CATALOG: "The repository does not belong to this project's backend catalog. Refresh and choose again.",
  E_REPO_UNAVAILABLE: "The repository is unavailable locally. No team creation is confirmed; refresh before retrying.",
  E_NAME_INVALID: "A team or seat name is invalid. Check the highlighted name rules.",
  E_NAME_COLLISION: "A name is already reserved, active or archived. Choose another name.",
  E_PRESET_CONFLICT: "The preset changed elsewhere. Reload it before saving; your draft has not been applied.",
  E_PRESET_INVALID: "The preset or execution configuration is not supported. Review it before retrying.",
  E_BUDGET_EXCEEDED: "This team exceeds an approved limit. Review its seats and configuration.",
  E_PATH_UNSAFE: "The backend rejected an unsafe storage path. No success is confirmed.",
  E_PERMISSION_UNSAFE: "Storage permissions do not meet the backend requirements.",
  E_STAGING_IO: "The backend could not finish staging. Refresh to check the current team state.",
  E_CREATION_GATE_PENDING: "Awaiting registration and approval by GLaDOS. No workers have been started.",
  E_LOCK_HELD: "Another operation owns this team. Refresh its state before retrying.",
  E_GENERATION_MISMATCH: "The generation changed. Refresh before taking another action.",
  E_JOURNAL_INCONSISTENT: "Recovery needs attention. Do not retry a lifecycle operation until reconciled.",
  E_TEAM_NOT_FOUND: "This team is no longer available. Refresh the team list.",
  E_STATE_CONFLICT: "The team state changed. Refresh before taking another action.",
};

/** Fixed copy only: never echo a rejected command's raw message/path/payload. */
export function teamErrorCopy(error: unknown): string {
  if (error && typeof error === "object" && "code" in error && typeof error.code === "string") {
    return ERROR_COPY[error.code] ?? "The operation could not be confirmed. Refresh to check its current state.";
  }
  return "The operation could not be confirmed. Refresh to check its current state.";
}
