import type { AgentDef } from "../types";

// Card order in the launcher. The backend hands agents over from a HashMap,
// so without a total order two consecutive polls could disagree and the
// list would reshuffle under the operator's cursor. Roster members sort by
// this list; anyone not on it (a new agent dir, a stub in the boot-verify
// harness) sorts alphabetically after the roster.
export const ROSTER_ORDER: readonly string[] = [
  "glados",
  "wheatley",
  "peppy",
  "izzy",
  "vance",
  "rex",
  "scout",
  "cipher",
];

/** Pure: returns a new array, input untouched. Total order — same input
 *  set always yields the same sequence regardless of arrival order. */
export function sortAgents<T extends Pick<AgentDef, "name">>(agents: readonly T[]): T[] {
  const rank = (name: string) => {
    const i = ROSTER_ORDER.indexOf(name);
    return i === -1 ? ROSTER_ORDER.length : i;
  };
  return [...agents].sort((a, b) => {
    const d = rank(a.name) - rank(b.name);
    return d !== 0 ? d : a.name.localeCompare(b.name);
  });
}
