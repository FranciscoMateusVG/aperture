import { invoke } from "@tauri-apps/api/core";
import type { CancelPendingInput, CreateTeamInput, TeamPresetInput } from "../types";
import { parseCancelled, parseTeamCatalog, parseTeamCreateResult, parseTeamPreset, parseTeamPresets, parseTeams, sameSeats, sameFallbacks, samePresetContent } from "./team-contract";
import { deriveTeamSeatNames, snapshotTeamDraft } from "./team-draft";

type Invoke = (command: string, args?: Record<string, unknown>) => Promise<unknown>;
/** Explicit command allowlist, no activation/actor/grants/path inputs. Injectable only for tests. */
export function createTeamCommands(call: Invoke) {
  return {
    catalog: async () => parseTeamCatalog(await call("team_get_catalog")),
    presets: async () => parseTeamPresets(await call("team_list_presets")),
    list: async () => parseTeams(await call("team_list")),
    create: async (input: CreateTeamInput) => {
      const request = snapshotTeamDraft(input);
      const result = parseTeamCreateResult(await call("team_create", { input: request }));
      const echoed = result.team.snapshot;
      const names = deriveTeamSeatNames(request.team, request.seats);
      if (echoed.team !== request.team || echoed.project !== request.project ||
        echoed.mission !== request.mission || echoed.acceptance !== request.acceptance || echoed.preset.id !== request.preset_id ||
        !sameSeats(echoed.seats, request.seats) || echoed.seats.some((seat, i) => seat.name !== names[i]) ||
        echoed.lead !== names[request.lead_index] || !sameFallbacks(echoed.fallbacks, request.fallbacks)) {
        throw { code: "E_RESPONSE_INVALID", message: "Team response did not match request" };
      }
      return result;
    },
    savePreset: async (preset: TeamPresetInput, expectedSha256: string | null) => {
      const { schema_version, id, display_name, mission_placeholder, acceptance_placeholder, lead_index } = preset;
      const input = { preset_without_source: { schema_version, id, display_name, mission_placeholder, acceptance_placeholder, lead_index,
        seats: preset.seats.map(({ role, harness, model, reasoning }) => ({ role, harness, model, reasoning })),
        fallbacks: preset.fallbacks.map(({ harness, model, reasoning }) => ({ harness, model, reasoning })),
      }, expected_sha256: expectedSha256 };
      const result = parseTeamPreset(await call("team_save_preset", { input }));
      if (result.source !== "local" || !samePresetContent(result, input.preset_without_source)) throw { code: "E_RESPONSE_INVALID", message: "Preset response did not match request" };
      return result;
    },
    cancel: async ({ team, expected_generation, creation_request_id }: CancelPendingInput) =>
      parseCancelled(await call("team_cancel_pending", { input: { team, expected_generation, creation_request_id } }), team),
  };
}
export const teamCommands = createTeamCommands(invoke);
export type TeamCommands = ReturnType<typeof createTeamCommands>;
