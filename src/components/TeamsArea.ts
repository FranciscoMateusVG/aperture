import type { AgentDef, TeamView } from "../types";
import { teamCommands, type TeamCommands } from "../services/team-commands";
import { teamErrorCopy } from "../services/team-draft";
import { escapeHtml as e } from "../utils/html";
import { tupleLabel } from "./TeamEditor";
import { openTeamLifecycle } from "./TeamLifecycle";

export function renderTeamGroup(team: TeamView, agents: readonly AgentDef[] = []): string {
  const turn = (name: string) => {
    const observed = agents.find(a => a.name === name)?.turn_state;
    return observed === "busy" || observed === "idle" ? observed : "Unknown — no turn observation";
  };
  const { snapshot: s, state } = team;
  const active = state.state === "active";
  const running = team.seats.filter(seat => seat.observed_owner?.state === "active").length;
  const count = active ? `${team.seats.length} seats · ${running} active` : `${s.seats.length} seats`;
  return `<section class="v4-card v4-team"><div class="v4-toolbar v4-team__head"><div><h2>${e(s.team)}</h2><p class="v4-meta">repository: ${e(s.repo)} (immutable) · ${e(s.project)} · lead: ${e(s.lead)} · ${state.epic_id ? `epic: ${e(state.epic_id)}` : "epic: pending"}</p></div><div class="v4-team__state"><span class="v4-badge v4-status">${e(state.state)} · g${state.generation}</span><span class="v4-meta">${count}</span></div></div>
    ${state.state === "pending" ? '<p class="v4-notice">Awaiting registration and approval by GLaDOS. No workers have been started; start is coordinated by GLaDOS after approval.</p>' : ""}
    ${state.state === "failed" ? `<p class="v4-notice" role="status">Recovery needs attention. ${e(teamErrorCopy(state.failure))}</p>` : ""}
    ${active ? `<ul class="v4-seats">${team.seats.map(({ configured, observed_owner: owner }) => `<li class="v4-seat"><div class="v4-seat__row"><span class="v4-seat__name">${e(configured.name)}${configured.name === s.lead ? " · LEAD" : ""}</span><span class="v4-meta">${e(configured.role)} · ${e(tupleLabel(configured))} · ${owner ? `${e(owner.state)} · g${owner.generation}` : "no owner observation"} · ${e(turn(configured.name))}</span><button class="v4-button v4-button--small" data-action="open-seat" data-seat="${e(configured.name)}">Open</button></div>
      <details class="v4-details"><summary>Diagnostics and advanced actions</summary><dl class="v4-seat-facts"><div><dt>Snapshot · immutable configuration</dt><dd>${e(tupleLabel(configured))}</dd></div><div><dt>Requested · current incarnation</dt><dd>${owner ? e(tupleLabel(owner.configured)) : "Unknown — no owner observation"}</dd></div><div><dt>Observed model</dt><dd>${owner?.actual ? e(tupleLabel(owner.actual)) : "Unknown — not observed"}</dd></div><div><dt>Owner / generation</dt><dd>${owner ? `${e(owner.state)} · g${owner.generation}` : "Unknown — no owner observation"}</dd></div><div><dt>Checkpoint / context</dt><dd>Not available from this backend</dd></div><div><dt>Current turn</dt><dd>${e(turn(configured.name))}</dd></div><div><dt>Thread binding</dt><dd>${owner ? owner.thread_bound ? "Bound (reported)" : "Not bound" : "Unknown"}</dd></div></dl>
      <div class="v4-actions"><button class="v4-button" disabled title="Checkpoint command is not integrated">Checkpoint unavailable</button><button class="v4-button" data-action="replace" data-team="${e(s.team)}" data-seat="${e(configured.name)}">Replace worker…</button><button class="v4-button" disabled title="Team stop requires the verified lifecycle protocol">Stop unavailable</button></div></details></li>`).join("")}</ul><p class="v4-meta">Worker start is coordinated by GLaDOS; no manual start from this launcher.</p>` : ""}
    <details class="v4-details"><summary>Mission, acceptance and team actions</summary><p>${e(s.mission)}</p><p class="v4-meta">Acceptance: ${e(s.acceptance)}</p><div class="v4-actions"><button class="v4-button" data-action="archive" data-team="${e(s.team)}">Archive checklist…</button></div></details>
    ${state.state === "pending" ? `<div class="v4-actions"><button class="v4-button" data-action="cancel-pending" data-team="${e(s.team)}" ${team.capabilities.cancel ? "" : "disabled"}>Cancel pending team</button></div>` : ""}</section>`;
}

export interface TeamsAreaOptions {
  api?: TeamCommands;
  listAgents: () => Promise<AgentDef[]>;
  openAgent: (name: string) => Promise<void>;
  onTeamSeats: (names: string[]) => void;
}
/**
 * Two launcher areas: Coordination (the fixed roster, mounted from `legacyRoster`)
 * Worker start is coordinated by GLaDOS after approval; the launcher offers no manual bootstrap.
 * and Teams (project teams read from the backend). Team creation and presets are
 * not launcher actions — GLaDOS creates teams through the authenticated control
 * path, so refresh reads team state only and never touches presets or the catalog.
 */
export function createTeamsArea(container: HTMLElement, legacyRoster: HTMLElement, options: TeamsAreaOptions) {
  const api = options.api ?? teamCommands;
  const root = document.createElement("section"); root.className = "v4";
  root.innerHTML = `<div class="v4-tabs" role="tablist" aria-label="Launcher area"><button class="v4-tab" id="v4-coordination-tab" role="tab" aria-selected="true" aria-controls="v4-coordination" tabindex="0" data-tab="coordination">Coordination</button><button class="v4-tab" id="v4-teams-tab" role="tab" aria-selected="false" aria-controls="v4-teams" tabindex="-1" data-tab="teams">Teams</button></div><section id="v4-coordination" role="tabpanel" aria-labelledby="v4-coordination-tab"><div class="v4-toolbar"><div><h1>Coordination</h1><p class="v4-meta">Fixed seats only: GLaDOS, Wheatley and Peppy. Project team seats are listed under Teams.</p></div></div><div class="v4-stack" data-coordination></div></section><section id="v4-teams" role="tabpanel" aria-labelledby="v4-teams-tab" hidden><div class="v4-toolbar"><div><h1>Teams</h1><p class="v4-meta">Project teams are created and approved by GLaDOS. Team seats never join the coordination roster.</p></div></div><p class="v4-notice" role="status" aria-live="polite" data-global-status>Loading team state…</p><div class="v4-actions"><button class="v4-button" data-refresh>Refresh teams</button></div><div class="v4-stack" data-teams></div></section>`;
  container.appendChild(root);
  const status = root.querySelector<HTMLElement>("[data-global-status]")!;
  const teamsEl = root.querySelector<HTMLElement>("[data-teams]")!;
  const coordinationEl = root.querySelector<HTMLElement>("[data-coordination]")!;
  // The actual standing roster (and its existing listeners) lives inside Coordination and is independent of team state.
  coordinationEl.appendChild(legacyRoster);
  const refreshButton = root.querySelector<HTMLButtonElement>("[data-refresh]")!;
  let teams: TeamView[] = [];
  let busy = false, loaded = false, mutation = false;
  function switchTab(tab: string, focus = false) {
    root.querySelectorAll<HTMLButtonElement>("[data-tab]").forEach(el => { const selected = el.dataset.tab === tab; el.setAttribute("aria-selected", String(selected)); el.tabIndex = selected ? 0 : -1; if (selected && focus) el.focus(); });
    root.querySelector<HTMLElement>("#v4-coordination")!.hidden = tab !== "coordination";
    root.querySelector<HTMLElement>("#v4-teams")!.hidden = tab !== "teams";
  }
  root.querySelector('[role="tablist"]')!.addEventListener("keydown", event => {
    const key = (event as KeyboardEvent).key;
    if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(key)) return;
    event.preventDefault();
    const current = root.querySelector<HTMLElement>('[aria-selected="true"]')!.dataset.tab;
    switchTab(key === "Home" ? "coordination" : key === "End" ? "teams" : current === "coordination" ? "teams" : "coordination", true);
  });
  function disableTeamActions() {
    teamsEl.querySelectorAll<HTMLButtonElement>("button").forEach(el => { el.disabled = true; });
  }
  async function refresh() {
    if (busy || mutation) return;
    busy = true; refreshButton.disabled = true;
    status.textContent = loaded ? "Refreshing team state…" : "Loading team state…";
    try {
      const [nextTeams, agents] = await Promise.all([api.list(), options.listAgents()]);
      teams = nextTeams;
      options.onTeamSeats(teams.flatMap(t => t.snapshot.seats.map(s => s.name)));
      teamsEl.innerHTML = teams.length ? teams.map(t => renderTeamGroup(t, agents)).join("") : '<p class="v4-notice">No teams registered. Coordination seats are unchanged.</p>';
      loaded = true; status.textContent = "Team state loaded. Configured models are not inferred session observations.";
    } catch (error) {
      // Last-known state remains readable but cannot initiate any action.
      loaded = false; status.textContent = `Team controls unavailable; any displayed data is last known. ${teamErrorCopy(error)} Coordination seats remain independent.`;
      disableTeamActions();
    } finally { busy = false; refreshButton.disabled = false; }
  }
  root.addEventListener("click", async event => {
    const button = (event.target as HTMLElement).closest<HTMLButtonElement>("button");
    if (!button || button.disabled) return;
    if (button.dataset.tab) { switchTab(button.dataset.tab); return; }
    if (button.hasAttribute("data-refresh")) { void refresh(); return; }
    if (!loaded || busy || mutation) return;
    const action = button.dataset.action;
    const team = teams.find(t => t.snapshot.team === button.dataset.team);
    if ((action === "replace" || action === "archive") && team) {
      const teamName = team.snapshot.team;
      openTeamLifecycle({ kind: action, team, seat: button.dataset.seat,
        current: () => loaded ? teams.find(t => t.snapshot.team === teamName) : undefined,
        refresh: async () => { await refresh(); return loaded ? teams.find(t => t.snapshot.team === teamName) : undefined; },
      }); return;
    }
    if (action === "open-seat" && button.dataset.seat) {
      try { await options.openAgent(button.dataset.seat); } catch { status.textContent = "No current terminal window could be opened. Refresh the observed state."; }
      return;
    }
    if (action === "cancel-pending" && team && team.state.state === "pending" && team.capabilities.cancel) {
      mutation = true; button.disabled = true; status.textContent = "Cancelling pending registration…";
      try {
        await api.cancel({ team: team.snapshot.team, expected_generation: team.state.generation, creation_request_id: team.snapshot.creation_request_id });
        mutation = false; await refresh();
      } catch (error) { mutation = false; loaded = false; status.textContent = teamErrorCopy(error); }
    }
  });
  void refresh();
  return { refresh };
}
