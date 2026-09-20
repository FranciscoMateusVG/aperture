import { invoke } from "@tauri-apps/api/core";
import type { BootstrapView, ArchiveView, ExecutionTuple, OwnerSummary, PreparedReplacementView, ReplacementView, RuntimeBlocker, TeamView } from "../types";
import { isExecutionTuple, sameExecutionTuple } from "./team-contract";

const invalid = (): never => { throw { code: "E_RESPONSE_INVALID", message: "Runtime response could not be confirmed" }; };
const unavailable = (): never => { throw { code: "E_RUNTIME_UNAVAILABLE", message: "Runtime capability is unavailable" }; };
const obj = (v: unknown): v is Record<string, unknown> => !!v && typeof v === "object" && !Array.isArray(v);
const number = (v: unknown): v is number => Number.isSafeInteger(v) && (v as number) >= 0;
const exact = (v: Record<string, unknown>, keys: readonly string[]) => Object.keys(v).every(k => keys.includes(k)) && keys.every(k => k in v);
const checkStates = ["pending", "verified", "blocked", "unknown"];
const phases = ["snapshot", "checkpoint_pending", "stopping", "revoking", "reconciling", "ready", "starting", "started", "model_unverified", "blocked"];
const replacementKeys = ["process_stop", "revocation", "remote_effects"];
const archiveKeys = ["reconciliation", "reviews", "metrics", ...replacementKeys, "worktrees"];
const viewKeys = ["team", "seat", "generation", "phase", "checkpoint_recovery", "checks", "owner", "blockers"];
const includes = (v: unknown, items: readonly string[]) => typeof v === "string" && items.includes(v);
const safeTuple = (v: unknown): v is ExecutionTuple => obj(v) && exact(v, ["harness", "model", "reasoning"]) && isExecutionTuple(v);
const safeId = (v: unknown) => typeof v === "string" && /^[A-Za-z0-9][A-Za-z0-9_.:-]{0,127}$/.test(v);
function checks(v: unknown, keys: string[]): boolean {
  return obj(v) && exact(v, keys) && keys.every(k => includes(v[k], checkStates));
}
function blockers(v: unknown): v is RuntimeBlocker[] {
  return Array.isArray(v) && v.length <= 256 && v.every(b => obj(b) && exact(b, ["code", "reference"]) && typeof b.code === "string" && /^E_[A-Z0-9_]{1,80}$/.test(b.code) && safeId(b.reference));
}
export function authorizedSelections(team: TeamView, seat: string): ExecutionTuple[] {
  const snapshot = team.snapshot.seats.find(s => s.name === seat);
  if (!snapshot) return [];
  const choices: ExecutionTuple[] = [];
  for (const tuple of [snapshot, ...team.snapshot.fallbacks]) if (!choices.some(t => sameExecutionTuple(t, tuple))) {
    choices.push({ harness: tuple.harness, model: tuple.model, reasoning: tuple.reasoning });
  }
  return choices;
}
function safeOwner(v: unknown, team: TeamView, seat: string): v is OwnerSummary | null {
  if (v === null) return true;
  if (!obj(v) || !exact(v, ["generation", "state", "since", "configured", "actual", "process_count", "thread_bound"]) ||
    !number(v.generation) || !includes(v.state, ["starting", "active", "stale", "quarantined"]) || typeof v.since !== "string" ||
    !number(v.process_count) || typeof v.thread_bound !== "boolean" || !safeTuple(v.configured) ||
    !(v.actual === null || safeTuple(v.actual))) return false;
  const allowed = authorizedSelections(team, seat);
  return allowed.length > 0 && (v.generation === 0 ? sameExecutionTuple(v.configured, allowed[0]) : allowed.some(t => sameExecutionTuple(t, v.configured as ExecutionTuple))) &&
    (v.state !== "active" || (!!v.actual && sameExecutionTuple(v.configured, v.actual as ExecutionTuple)));
}
export function canBootstrapSeat(team: TeamView, seat: string): boolean {
  const snapshot = team.snapshot.seats.find(s => s.name === seat);
  const owner = team.seats.find(s => s.configured.name === seat)?.observed_owner;
  return team.state.state === "active" && team.capabilities?.start === true && !!snapshot && !!owner &&
    owner.state === "stale" && owner.generation === 0 && sameExecutionTuple(owner.configured, snapshot);
}
export function parseBootstrap(v: unknown, team: TeamView, seat: string): BootstrapView {
  const snapshot = team.snapshot.seats.find(s => s.name === seat);
  if (!snapshot || !obj(v) || !exact(v, ["team", "seat", "generation", "phase", "owner", "blockers"]) ||
    v.team !== team.snapshot.team || v.seat !== seat || !number(v.generation) ||
    !includes(v.phase, ["starting", "started", "blocked"]) || !blockers(v.blockers) || !safeOwner(v.owner, team, seat)) return invalid();
  if (v.owner && (v.owner.generation !== v.generation || !sameExecutionTuple(v.owner.configured, snapshot))) return invalid();
  if (v.phase === "started" && (!v.owner || v.owner.state !== "active" || v.generation === 0 ||
    !v.owner.actual || !sameExecutionTuple(v.owner.actual, snapshot) || v.blockers.length)) return invalid();
  return v as unknown as BootstrapView;
}
export function ownerGeneration(team: TeamView, seat: string): number | null {
  return team.seats.find(s => s.configured.name === seat)?.observed_owner?.generation ?? null;
}
function canInvoke(team: TeamView, capability: "replace" | "archive") {
  return team.state.state === "active" && team.capabilities?.[capability] === true;
}
export function parseReplacement(v: unknown, team: TeamView, seat: string, prepared: boolean): ReplacementView | PreparedReplacementView {
  if (!obj(v) || !exact(v, prepared ? [...viewKeys, "preparation_id"] : viewKeys) || v.team !== team.snapshot.team || v.seat !== seat ||
    !number(v.generation) || !includes(v.phase, phases) || !includes(v.checkpoint_recovery, ["valid", "stale", "none"]) ||
    !checks(v.checks, replacementKeys) || !blockers(v.blockers) || !safeOwner(v.owner, team, seat)) return invalid();
  if (v.owner && v.owner.generation !== v.generation) return invalid();
  if (prepared && !(v.preparation_id === null || (typeof v.preparation_id === "string" && v.preparation_id.length > 0))) return invalid();
  if (prepared && v.phase !== "ready" && v.preparation_id !== null) return invalid();
  if (v.phase === "started" && (!v.owner || v.owner.state !== "active" || !v.owner.actual || v.blockers.length || !Object.values(v.checks as object).every(x => x === "verified"))) return invalid();
  return v as unknown as ReplacementView | PreparedReplacementView;
}
export function canStartReplacement(team: TeamView, seat: string, prepared: PreparedReplacementView | null, selection: ExecutionTuple): boolean {
  return canInvoke(team, "replace") && (ownerGeneration(team, seat) ?? 0) > 0 && !!prepared && prepared.team === team.snapshot.team && prepared.seat === seat &&
    prepared.generation === ownerGeneration(team, seat) && prepared.phase === "ready" && !!prepared.preparation_id &&
    prepared.blockers.length === 0 && Object.values(prepared.checks).every(v => v === "verified") &&
    authorizedSelections(team, seat).some(t => sameExecutionTuple(t, selection));
}
export function parseArchive(v: unknown, team: TeamView): ArchiveView {
  if (!obj(v) || !exact(v, ["team", "generation", "state", "checks", "blockers"]) || v.team !== team.snapshot.team || !number(v.generation) ||
    !includes(v.state, ["pending", "blocked", "archived", "unknown"]) || !checks(v.checks, archiveKeys) || !blockers(v.blockers)) return invalid();
  if (v.state === "archived" && (v.blockers.length > 0 || !Object.values(v.checks as object).every(x => x === "verified"))) return invalid();
  return v as unknown as ArchiveView;
}

type Invoke = (command: string, args: Record<string, unknown>) => Promise<unknown>;
/** Reserved calls. Capability false/missing fails before invoking, including in fixtures. */
export function createRuntimeCommands(call: Invoke) {
  return {
    bootstrap: async (team: TeamView, seat: string): Promise<BootstrapView> => {
      if (!canBootstrapSeat(team, seat)) return unavailable();
      return parseBootstrap(await call("team_bootstrap_seat", {
        input: { team: team.snapshot.team, seat, expected_generation: 0 },
      }), team, seat);
    },
    prepare: async (team: TeamView, seat: string): Promise<PreparedReplacementView> => {
      const generation = ownerGeneration(team, seat);
      if (!canInvoke(team, "replace") || (generation === null || generation === 0)) return unavailable();
      const result = parseReplacement(await call("team_prepare_replacement", { input: { team: team.snapshot.team, seat, expected_generation: generation } }), team, seat, true) as PreparedReplacementView;
      if (result.generation !== generation) return invalid();
      return result;
    },
    start: async (team: TeamView, seat: string, prepared: PreparedReplacementView, selection: ExecutionTuple): Promise<ReplacementView> => {
      if (!canStartReplacement(team, seat, prepared, selection)) return unavailable();
      const result = parseReplacement(await call("team_start_replacement", { input: { team: team.snapshot.team, seat,
        expected_generation: prepared.generation, preparation_id: prepared.preparation_id,
        selection: { harness: selection.harness, model: selection.model, reasoning: selection.reasoning },
      } }), team, seat, false);
      if (result.phase === "started" && (result.generation <= prepared.generation || !result.owner || !sameExecutionTuple(result.owner.configured, selection))) return invalid();
      return result;
    },
    archive: async (team: TeamView): Promise<ArchiveView> => {
      if (!canInvoke(team, "archive")) return unavailable();
      return parseArchive(await call("team_archive", { input: { team: team.snapshot.team, expected_generation: team.state.generation } }), team);
    },
  };
}
export const runtimeCommands = createRuntimeCommands(invoke);
export type RuntimeCommands = ReturnType<typeof createRuntimeCommands>;

const errorMessages: Record<string, string> = {
 E_PREPARATION_EXPIRED: "Start blocked: the preparation permit expired or is no longer valid. The previous worker remains stopped and revoked. Refresh state and prepare again; no rollback or automatic retry.",
 E_LAUNCH_UNAVAILABLE: "The native launch capability is unavailable. No start is confirmed.",
 E_RUNTIME_DEADLINE: "The runtime deadline elapsed. Outcome is unknown; refresh state, do not assume rollback.",
 E_CONTROL_UNKNOWN: "Native control outcome is unknown. Refresh authoritative state before another operation.",
 E_RUNTIME_UNAVAILABLE: "Not available: this backend has not enabled the runtime capability.",
 E_GENERATION_MISMATCH: "Generation changed. Refresh authoritative state before another lifecycle operation.",
 E_STOP_UNVERIFIED: "Owned-process stop could not be verified.",
 E_UNOWNED_PROCESS: "An unowned process blocks this operation; it was not authorized for termination.",
 E_REVOCATION_UNVERIFIED: "Hub/message authority revocation could not be verified.",
 E_REMOTE_UNCERTAIN: "Remote effects remain uncertain; no safe completion is confirmed.",
 E_REPLACEMENT_AUTHORIZATION: "This exact replacement tuple is not authorized by the backend.",
 E_FRESH_THREAD_UNVERIFIED: "A fresh thread could not be verified.",
 E_MODEL_UNVERIFIED: "The requested model was not verified. No successful start is confirmed.",
 E_START_CLEANUP_UNVERIFIED: "Failed-start cleanup could not be verified. Do not assume rollback.",
 E_RUNTIME_IO: "Runtime outcome is unknown. Refresh before another operation.",
 E_REVIEW_MISSING: "A required review is missing.",
 E_COMPLETED_WITHOUT_EVIDENCE: "A completed item lacks required evidence.",
 E_TRANSFER_UNACCEPTED: "A transfer lacks recorded acceptance.",
 E_WORKTREE_UNPROTECTED: "Worktree preservation has not been verified.",
 E_JOURNAL_INCONSISTENT: "The lifecycle journal requires reconciliation. No rollback or completion is confirmed.",
};
export function runtimeErrorCopy(error: unknown): string {
  return obj(error) && typeof error.code === "string" && errorMessages[error.code] || "The runtime outcome could not be confirmed. No automatic retry was made.";
}
