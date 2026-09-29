import { invoke as tauriInvoke } from "@tauri-apps/api/core";

export type CommandCall = <T>(command: string, args?: Record<string, unknown>) => Promise<T>;
let active: CommandCall = tauriInvoke;
/** Defaults to the existing desktop adapter. Browser composition replaces it before init. */
export function setCommandTransport(call: CommandCall): void { active = call; }
export const invoke: CommandCall = (command, args) => active(command, args);
