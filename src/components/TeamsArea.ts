import type { AgentDef, TeamCatalog, TeamPreset, TeamView, TeamCreateResult } from "../types";
import { teamCommands, type TeamCommands } from "../services/team-commands";
import { teamErrorCopy } from "../services/team-draft";
import { escapeHtml as e } from "../utils/html";
import { openTeamEditor, tupleLabel } from "./TeamEditor";
import { openTeamLifecycle } from "./TeamLifecycle";

export function renderPresetCard(preset: TeamPreset): string {
  return `<article class="v4-card"><h2>${e(preset.display_name)}</h2><p class="v4-meta">${e(preset.mission_placeholder)}</p><ul>${preset.seats.map((s, i) => `<li>${e(s.role)}${i === preset.lead_index ? " · lead" : ""}<br><span class="v4-meta">${e(tupleLabel(s))}</span></li>`).join("")}</ul><p class="v4-meta">${e(preset.source)} preset · snapshot on creation</p><div class="v4-actions"><button class="v4-button v4-button--primary" data-action="create" data-preset="${e(preset.id)}">New team from preset</button><button class="v4-button" data-action="edit" data-preset="${e(preset.id)}">Edit preset</button><button class="v4-button" data-action="duplicate" data-preset="${e(preset.id)}">Duplicate</button></div></article>`;
}
export function renderTeamGroup(team: TeamView, agents: readonly AgentDef[] = []): string {
  const turn = (name: string) => {
    const observed = agents.find(a => a.name === name)?.turn_state;
    return observed === "busy" || observed === "idle" ? observed : "Unknown — no turn observation";
  };
  const { snapshot: s, state } = team;
  const active = state.state === "active";
  return `<section class="v4-card"><div class="v4-toolbar"><div><h2>${e(s.team)}</h2><p class="v4-meta">${e(s.project)} · repository: ${e(s.repo)} (immutable) · lead: ${e(s.lead)} · ${state.epic_id ? `epic: ${e(state.epic_id)}` : "epic: pending"}</p></div><span class="v4-badge v4-status">${e(state.state)} · g${state.generation}</span></div><p>${e(s.mission)}</p><p class="v4-meta">Acceptance: ${e(s.acceptance)}</p>
    ${state.state === "pending" ? '<p class="v4-notice">Awaiting registration and approval by GLaDOS. No workers have been started.</p>' : ""}
    ${state.state === "failed" ? `<p class="v4-notice" role="status">Recovery needs attention. ${e(teamErrorCopy(state.failure))}</p>` : ""}
    ${active ? `<div class="v4-stack">${team.seats.map(({ configured, observed_owner: owner }) => `<article class="v4-card"><h3>${e(configured.name)}${configured.name === s.lead ? " · LEAD" : ""}</h3>
      <dl class="v4-seat-facts"><div><dt>Snapshot · immutable configuration</dt><dd>${e(tupleLabel(configured))}</dd></div><div><dt>Requested · current incarnation</dt><dd>${owner ? e(tupleLabel(owner.configured)) : "Unknown — no owner observation"}</dd></div><div><dt>Observed model</dt><dd>${owner?.actual ? e(tupleLabel(owner.actual)) : "Unknown — not observed"}</dd></div><div><dt>Owner / generation</dt><dd>${owner ? `${e(owner.state)} · g${owner.generation}` : "Unknown — no owner observation"}</dd></div><div><dt>Checkpoint / context</dt><dd>Not available from this backend</dd></div><div><dt>Current turn</dt><dd>${e(turn(configured.name))}</dd></div><div><dt>Thread binding</dt><dd>${owner ? owner.thread_bound ? "Bound (reported)" : "Not bound" : "Unknown"}</dd></div></dl>
      <div class="v4-actions"><button class="v4-button" data-action="open-seat" data-seat="${e(configured.name)}">Open</button><button class="v4-button" disabled title="Checkpoint command is not integrated">Checkpoint unavailable</button><button class="v4-button" data-action="replace" data-team="${e(s.team)}" data-seat="${e(configured.name)}">Replace worker…</button><button class="v4-button" disabled title="Team stop requires the verified lifecycle protocol">Stop unavailable</button></div></article>`).join("")}</div>` : ""}
    <div class="v4-actions">${state.state === "pending" ? `<button class="v4-button" data-action="cancel-pending" data-team="${e(s.team)}" ${team.capabilities.cancel ? "" : "disabled"}>Cancel pending team</button>` : ""}<button class="v4-button" data-action="archive" data-team="${e(s.team)}">Archive checklist…</button></div></section>`;
}

export interface TeamsAreaOptions {
  api?: TeamCommands;
  listAgents: () => Promise<AgentDef[]>;
  openAgent: (name: string) => Promise<void>;
  onTeamSeats: (names: string[]) => void;
}
export function createTeamsArea(container: HTMLElement, legacyRoster: HTMLElement, options: TeamsAreaOptions) {
  const api = options.api ?? teamCommands;
  const root = document.createElement("section"); root.className = "v4";
  root.innerHTML = `<div class="v4-tabs" role="tablist" aria-label="Launcher area"><button class="v4-tab" id="v4-sessions-tab" role="tab" aria-selected="true" aria-controls="v4-sessions" tabindex="0" data-tab="sessions">Sessions</button><button class="v4-tab" id="v4-presets-tab" role="tab" aria-selected="false" aria-controls="v4-presets" tabindex="-1" data-tab="presets">Presets</button></div><p class="v4-notice" role="status" aria-live="polite" data-global-status>Loading team state…</p><div class="v4-actions"><button class="v4-button" data-refresh>Refresh teams</button></div><section id="v4-sessions" role="tabpanel" aria-labelledby="v4-sessions-tab"><div class="v4-toolbar"><div><h1>Sessions</h1><p class="v4-meta">Coordination and standing specialists remain available below.</p></div></div><div class="v4-stack" data-teams></div></section><section id="v4-presets" role="tabpanel" aria-labelledby="v4-presets-tab" hidden><div class="v4-toolbar"><div><h1>A team starts with a clear mission.</h1><p class="v4-meta">Reusable presets. Independent snapshots. Existing teams stay unchanged.</p></div><button class="v4-button" data-action="blank" disabled>New blank preset</button></div><div class="v4-grid" data-presets></div></section>`;
  container.appendChild(root);
  const status = root.querySelector<HTMLElement>("[data-global-status]")!;
  const teamsEl = root.querySelector<HTMLElement>("[data-teams]")!;
  const presetsEl = root.querySelector<HTMLElement>("[data-presets]")!;
  // Keep the actual standing roster (and its existing listeners) ahead of project teams.
  teamsEl.before(legacyRoster);
  const blank = root.querySelector<HTMLButtonElement>('[data-action="blank"]')!;
  const refreshButton = root.querySelector<HTMLButtonElement>("[data-refresh]")!;
  let teams: TeamView[] = [], presets: TeamPreset[] = [], catalog: TeamCatalog | null = null, knownSeats: string[] = [];
  let busy = false, loaded = false, mutation = false;
  function switchTab(tab: string, focus = false) {
    root.querySelectorAll<HTMLButtonElement>("[data-tab]").forEach(el => { const selected = el.dataset.tab === tab; el.setAttribute("aria-selected", String(selected)); el.tabIndex = selected ? 0 : -1; if (selected && focus) el.focus(); });
    root.querySelector<HTMLElement>("#v4-sessions")!.hidden = tab !== "sessions";
    root.querySelector<HTMLElement>("#v4-presets")!.hidden = tab !== "presets";
    legacyRoster.hidden = tab !== "sessions";
  }
  root.querySelector('[role="tablist"]')!.addEventListener("keydown", event => {
    const key = (event as KeyboardEvent).key;
    if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(key)) return;
    event.preventDefault();
    const current = root.querySelector<HTMLElement>('[aria-selected="true"]')!.dataset.tab;
    switchTab(key === "Home" ? "sessions" : key === "End" ? "presets" : current === "sessions" ? "presets" : "sessions", true);
  });
  async function refresh() {
    if (busy || mutation) return;
    busy = true; refreshButton.disabled = true; blank.disabled = true;
    status.textContent = loaded ? "Refreshing team state…" : "Loading team state…";
    try {
      const [nextTeams, nextPresets, nextCatalog, agents] = await Promise.all([api.list(), api.presets(), api.catalog(), options.listAgents()]);
      teams = nextTeams; presets = nextPresets; catalog = nextCatalog; knownSeats = agents.map(a => a.name);
      options.onTeamSeats(teams.flatMap(t => t.snapshot.seats.map(s => s.name)));
      teamsEl.innerHTML = teams.length ? teams.map(t => renderTeamGroup(t, agents)).join("") : '<p class="v4-notice">No teams registered. Your standing roster is unchanged.</p>';
      presetsEl.innerHTML = presets.length ? presets.map(renderPresetCard).join("") : '<p class="v4-notice">No presets are available. Create a blank preset from the backend catalog.</p>';
      loaded = true; status.textContent = "Team state loaded. Configured models are not inferred session observations.";
      blank.disabled = !catalog.roles.length || !catalog.execution_tuples.length;
    } catch (error) {
      // Last-known state remains readable but cannot initiate any action.
      loaded = false; status.textContent = `Team controls unavailable; any displayed data is last known. ${teamErrorCopy(error)} Standing roster remains independent.`;
      teamsEl.querySelectorAll<HTMLButtonElement>("button").forEach(el => { el.disabled = true; });
      presetsEl.querySelectorAll<HTMLButtonElement>("button").forEach(el => { el.disabled = true; });
    } finally { busy = false; refreshButton.disabled = false; }
  }
  root.addEventListener("click", async event => {
    const button = (event.target as HTMLElement).closest<HTMLButtonElement>("button");
    if (!button || button.disabled) return;
    if (button.dataset.tab) { switchTab(button.dataset.tab); return; }
    if (button.hasAttribute("data-refresh")) { void refresh(); return; }
    if (!loaded || !catalog || busy || mutation) return;
    const action = button.dataset.action;
    if (["create", "edit", "duplicate", "blank"].includes(action ?? "")) {
      const preset = presets.find(p => p.id === button.dataset.preset);
      if (action !== "blank" && !preset) return;
      openTeamEditor({ mode: action as "create" | "edit" | "duplicate" | "blank", preset, catalog,
        knownTeams: teams.map(t => t.snapshot.team), knownSeats,
        submitTeam: api.create, submitPreset: api.savePreset,
        saved: (kind, result) => {
          if (kind === "team") { switchTab("sessions"); const t = (result as TeamCreateResult).team; status.textContent = `Backend returned ${t.state.state}. Awaiting authoritative refresh.`; }
          else status.textContent = "Preset saved by the backend. Refreshing library…";
          void refresh();
        },
      }); return;
    }
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
