import { invoke } from "@tauri-apps/api/core";
import type { CancelPendingInput } from "../types";
import { parseCancelled, parseTeams } from "./team-contract";

type Invoke = (command: string, args?: Record<string, unknown>) => Promise<unknown>;
/**
 * Explicit launcher allowlist: read teams and cancel a pending registration.
 * Team creation and preset editing are not launcher actions: GLaDOS creates
 * teams through the authenticated control path, and shipped presets remain
 * backend-only templates. No activation/actor/grants/path inputs.
 * Injectable only for tests.
 */
export function createTeamCommands(call: Invoke) {
  return {
    list: async () => parseTeams(await call("team_list")),
    cancel: async ({ team, expected_generation, creation_request_id }: CancelPendingInput) =>
      parseCancelled(await call("team_cancel_pending", { input: { team, expected_generation, creation_request_id } }), team),
  };
}
export const teamCommands = createTeamCommands(invoke);
export type TeamCommands = ReturnType<typeof createTeamCommands>;
