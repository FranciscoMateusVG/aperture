import { invoke } from "./command-transport";
import type { AgentDef } from "../types";

export interface VersionInfo {
  semver: string;
  sha: string;
  built_at: string;
}

// Frontend-exposed Tauri commands. The launcher only needs a handful of
// things: bootstrap the tmux session, list/start/stop/configure agents,
// clear an agent's attention badge, and read build metadata for the footer.
export function createCommands(call: typeof invoke) { return {
  tmuxCreateSession: (sessionName: string) =>
    call<string>("tmux_create_session", { sessionName }),
  tmuxSelectWindow: (windowId: string) =>
    call<void>("tmux_select_window", { windowId }),
  startAgent: (name: string) => call<void>("start_agent", { name }),
  stopAgent: (name: string) => call<void>("stop_agent", { name }),
  /** Stop-if-running then boot (aperture-ull4y). Tolerates an agent that is
   *  already stopped/crashed — the one case the stop→start two-click dance
   *  could never handle. */
  restartAgent: (name: string) => call<void>("restart_agent", { name }),
  listAgents: () => call<AgentDef[]>("list_agents"),
  updateAgentModel: (name: string, model: string) =>
    call<void>("update_agent_model", { name, model }),
  clearAttention: (name: string) =>
    call<void>("clear_attention", { name }),
  getVersion: () => call<VersionInfo>("get_version"),
}; }
export const commands = createCommands(invoke);
