import { invoke } from "@tauri-apps/api/core";
import type { CancelPendingInput, CreateTeamInput, TeamPresetInput } from "../types";
import { parseCancelled, parseTeamCatalog, parseTeamCreateResult, parseTeamPreset, parseTeamPresets, parseTeams } from "./team-contract";
import { snapshotTeamDraft } from "./team-draft";

type Invoke = (command: string, args?: Record<string, unknown>) => Promise<unknown>;
/** Explicit command allowlist, no activation/actor/grants/path inputs. Injectable only for tests. */
export function createTeamCommands(call: Invoke) {
  return {
    catalog: async () => parseTeamCatalog(await call("team_get_catalog")),
    presets: async () => parseTeamPresets(await call("team_list_presets")),
    list: async () => parseTeams(await call("team_list")),
    create: async (input: CreateTeamInput) => parseTeamCreateResult(await call("team_create", { input: snapshotTeamDraft(input) })),
    savePreset: async (preset: TeamPresetInput, expectedSha256: string | null) => {
      const { schema_version, id, display_name, mission_placeholder, acceptance_placeholder, lead_index } = preset;
      const input = { preset_without_source: { schema_version, id, display_name, mission_placeholder, acceptance_placeholder, lead_index,
        seats: preset.seats.map(({ role, harness, model, reasoning }) => ({ role, harness, model, reasoning })),
        fallbacks: preset.fallbacks.map(({ harness, model, reasoning }) => ({ harness, model, reasoning })),
      }, expected_sha256: expectedSha256 };
      return parseTeamPreset(await call("team_save_preset", { input }));
    },
    cancel: async ({ team, expected_generation, creation_request_id }: CancelPendingInput) =>
      parseCancelled(await call("team_cancel_pending", { input: { team, expected_generation, creation_request_id } }), team),
  };
}
export const teamCommands = createTeamCommands(invoke);
export type TeamCommands = ReturnType<typeof createTeamCommands>;
