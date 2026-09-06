import { invoke } from "@tauri-apps/api/core";
import type { AgentDef } from "../types";

export interface VersionInfo {
  semver: string;
  sha: string;
  built_at: string;
}

// Frontend-exposed Tauri commands. The launcher only needs a handful of
// things: bootstrap the tmux session, list/start/stop/configure agents,
// clear an agent's attention badge, and read build metadata for the footer.
export const commands = {
  tmuxCreateSession: (sessionName: string) =>
    invoke<string>("tmux_create_session", { sessionName }),
  tmuxSelectWindow: (windowId: string) =>
    invoke<void>("tmux_select_window", { windowId }),
  startAgent: (name: string) => invoke<void>("start_agent", { name }),
  stopAgent: (name: string) => invoke<void>("stop_agent", { name }),
  /** Stop-if-running then boot (aperture-ull4y). Tolerates an agent that is
   *  already stopped/crashed — the one case the stop→start two-click dance
   *  could never handle. */
  restartAgent: (name: string) => invoke<void>("restart_agent", { name }),
  listAgents: () => invoke<AgentDef[]>("list_agents"),
  updateAgentModel: (name: string, model: string) =>
    invoke<void>("update_agent_model", { name, model }),
  clearAttention: (name: string) =>
    invoke<void>("clear_attention", { name }),
  getVersion: () => invoke<VersionInfo>("get_version"),
};
