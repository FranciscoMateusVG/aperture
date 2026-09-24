import { invoke } from "@tauri-apps/api/core";
import type { TeamView } from "../types";
import { sameExecutionTuple } from "./team-contract";
export function canOpenSeat(team: TeamView, seat: string): boolean {
  const owner = team.seats.find(s => s.configured.name === seat)?.observed_owner;
  if (!(team.state.state === "active" && !!owner && owner.state === "active" && owner.generation > 0 &&
    !!owner.actual && sameExecutionTuple(owner.configured, owner.actual) && owner.process_count > 0 && owner.thread_bound)) return false;
  // Codex: any observed exact tuple. Claude: only an approved exact literal / reasoning None
  // tuple, and only when the native observation (actual) exists and matches.
  const c = owner.configured;
  return c.harness === "codex" || (c.harness === "claude" && CLAUDE_EXACT_MODELS.includes(c.model) && c.reasoning === null);
}
/** Exact Claude literals the backend admits (mirrors Rust `CLAUDE_MODELS`, parity-tested there).
 *  No alias, `[1m]` suffix or fallback; the catalog alone never grants Open. */
export const CLAUDE_EXACT_MODELS: readonly string[] = ["claude-sonnet-5", "claude-fable-5-1", "claude-opus-5"];
export function createTerminalCommands(call: typeof invoke) {
  return { async open(team: TeamView, seat: string): Promise<void> {
    if (!canOpenSeat(team, seat)) throw {code:"E_TERMINAL_UNAVAILABLE"};
    const generation = team.seats.find(s => s.configured.name === seat)!.observed_owner!.generation;
    const result: unknown = await call("team_open_seat", {input:{team:team.snapshot.team,seat,expected_generation:generation}});
    if (!result || typeof result !== "object" || Array.isArray(result)) throw {code:"E_RESPONSE_INVALID"};
    const r = result as Record<string, unknown>;
    if (Object.keys(r).sort().join() !== ["generation","seat","team","window_id"].join() ||
      r.team !== team.snapshot.team || r.seat !== seat || r.generation !== generation ||
      typeof r.window_id !== "string" || !/^@[0-9]{1,22}$/.test(r.window_id)) throw {code:"E_RESPONSE_INVALID"};
  }};
}
export const terminalCommands = createTerminalCommands(invoke);
