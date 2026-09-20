import type { ArchiveView, ExecutionTuple, PreparedReplacementView, ReplacementView, RuntimeCheckState, TeamView } from "../types";
import { authorizedSelections, canStartReplacement, ownerGeneration, runtimeCommands, runtimeErrorCopy, type RuntimeCommands } from "../services/team-runtime";
import { sameExecutionTuple } from "../services/team-contract";
import { tupleLabel } from "./TeamEditor";
import { escapeHtml as e } from "../utils/html";

const checks = (items: Record<string, RuntimeCheckState>) => `<ul class="v4-checklist">${Object.entries(items).map(([name, state]) => `<li><span>${e(name.replaceAll("_", " "))}</span> — <strong>${e(state)}</strong></li>`).join("")}</ul>`;
const runtimeBlockers = (view: ReplacementView | ArchiveView) => view.blockers.length ? `<ul class="v4-checklist">${view.blockers.map(b => `<li>${e(runtimeErrorCopy(b))}<br><span class="v4-meta">${e(b.code)} · ${e(b.reference)}</span></li>`).join("")}</ul>` : "";
export function renderReplacementEvidence(view: ReplacementView | null, pending = false): string {
  const states = view?.checks ?? { process_stop: "unknown", revocation: "unknown", remote_effects: "unknown" };
  const shown = pending ? { process_stop: "pending", revocation: "pending", remote_effects: "pending" } : states;
  const checkpoint = view?.checkpoint_recovery;
  return `<p class="v4-status">${pending ? "Operation pending — awaiting backend evidence" : view ? `Backend phase: ${e(view.phase)}` : "Replacement outcome: unknown — no backend evidence"}</p>
    <p>${checkpoint ? `Checkpoint recovery: ${e(checkpoint)}${checkpoint === "valid" ? "" : " — warning: native inventory decides safe recovery"}` : "Checkpoint recovery: unknown"}</p>
    ${checks(shown as Record<string, RuntimeCheckState>)}
    ${view?.owner ? `<dl class="v4-seat-facts"><div><dt>Requested incarnation</dt><dd>${e(tupleLabel(view.owner.configured))}</dd></div><div><dt>Observed</dt><dd>${view.owner.actual ? e(tupleLabel(view.owner.actual)) : "Unknown — actual model not observed"}</dd></div><div><dt>Returned owner generation / state</dt><dd>g${view.owner.generation} · ${e(view.owner.state)}</dd></div></dl>` : '<p class="v4-meta">Owner observation unavailable. Missing data does not prove a stopped worker.</p>'}
    ${view ? runtimeBlockers(view) : ""}
    ${view && ["model_unverified", "blocked"].includes(view.phase) ? '<p class="v4-error">No successful replacement is confirmed. A generation may have been consumed; refresh before another operation.</p>' : ""}`;
}
export function renderArchiveEvidence(view: ArchiveView | null, pending = false): string {
  const states = view?.checks ?? { reconciliation: "unknown", reviews: "unknown", metrics: "unknown", process_stop: "unknown", revocation: "unknown", remote_effects: "unknown", worktrees: "unknown" };
  const shown = Object.fromEntries(Object.entries(states).map(([k, v]) => [k, pending ? "pending" : v])) as Record<string, RuntimeCheckState>;
  return `<p class="v4-status">${pending ? "Archive operation pending — awaiting backend evidence" : view ? `Backend archive state: ${e(view.state)}` : "Archive readiness is unknown"}</p>${checks(shown)}${view ? runtimeBlockers(view) : ""}<p class="v4-meta">No client-side rollback, deletion or identity reuse. Empty blocker lists are not proof of readiness.</p>`;
}

export interface LifecycleOptions {
  kind: "replace" | "archive";
  team: TeamView;
  seat?: string;
  current: () => TeamView | undefined;
  refresh: () => Promise<TeamView | undefined>;
  api?: RuntimeCommands;
}

/** Native dialog, read-only opening. Reserved runtime calls are capability-gated. */
export function openTeamLifecycle(options: LifecycleOptions): HTMLDialogElement {
  const api = options.api ?? runtimeCommands;
  const origin = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  const seat = options.seat ?? "";
  let team = options.team;
  let choices = authorizedSelections(team, seat);
  const initialOwner = team.seats.find(s => s.configured.name === seat)?.observed_owner;
  let selected: ExecutionTuple | undefined = choices.find(t => initialOwner && sameExecutionTuple(t, initialOwner.configured)) ?? choices[0];
  let prepared: PreparedReplacementView | null = null, latest: ReplacementView | null = null, archived: ArchiveView | null = null;
  let inflight = false, refreshing = false, needsRefresh = false, closed = false;
  const dialog = document.createElement("dialog"); dialog.className = "v4-dialog"; dialog.setAttribute("aria-labelledby", "v4-runtime-title");
  dialog.innerHTML = `<div class="v4-dialog__body"><h2 id="v4-runtime-title">${options.kind === "replace" ? "Replace worker" : "Archive team"}</h2><p>${e(seat || team.snapshot.team)}</p>
    ${options.kind === "replace" ? `<div class="v4-field"><label for="v4-runtime-selection">Replacement · exact immutable policy tuple</label><select id="v4-runtime-selection"></select></div><label class="v4-radio-label"><input type="checkbox" data-confirm> I confirm this configuration change, including harness, reasoning and budget intent. Backend authorization still applies.</label><p class="v4-meta">Snapshot and configured fallbacks determine choices; the catalog alone does not grant replacement authority.</p>` : '<p>The backend will verify reconciliation, reviews, metrics, processes, revocation, remote effects and worktree preservation. This action may archive the team if every native gate passes; it is not a read-only check.</p>'}
    <div data-evidence></div><p role="status" aria-live="polite" data-status></p><p class="v4-error" role="alert" data-error tabindex="-1"></p>
    <p class="v4-meta">Closing does not cancel or undo an initiated stop, revoke or archive. There is no cancellation RPC.</p></div>
    <div class="v4-dialog__actions"><button class="v4-button" data-action="refresh">Refresh state</button>${options.kind === "replace" ? '<button class="v4-button" data-action="prepare">Prepare / stop / verify</button><button class="v4-button v4-button--primary" data-action="start">Start replacement</button>' : '<button class="v4-button v4-button--primary" data-action="archive">Verify and archive</button>'}<button class="v4-button" data-action="close">Close</button></div>`;
  const evidence = dialog.querySelector<HTMLElement>("[data-evidence]")!;
  const status = dialog.querySelector<HTMLElement>("[data-status]")!;
  const error = dialog.querySelector<HTMLElement>("[data-error]")!;
  const selection = dialog.querySelector<HTMLSelectElement>("#v4-runtime-selection");
  const confirm = dialog.querySelector<HTMLInputElement>("[data-confirm]");
  const button = (action: string) => dialog.querySelector<HTMLButtonElement>(`[data-action="${action}"]`);
  function refreshChoices() {
    choices = authorizedSelections(team, seat);
    const priorSelection = selected;
    const currentIndex = priorSelection ? choices.findIndex(t => sameExecutionTuple(t, priorSelection)) : -1;
    selected = currentIndex < 0 ? choices[0] : choices[currentIndex];
    if (selection) selection.innerHTML = choices.length ? choices.map((t, i) => `<option value="${i}" ${selected && sameExecutionTuple(t, selected) ? "selected" : ""}>${e(tupleLabel(t))}</option>`).join("") : '<option value="">No authorized tuple available</option>';
  }
  function live(): TeamView | undefined { return options.current(); }
  function allowed(): boolean {
    const current = live();
    return !!current && current.snapshot.team === team.snapshot.team && current.state.state === "active" &&
      current.capabilities?.[options.kind] === true &&
      (options.kind === "archive" ? current.state.generation === team.state.generation : ownerGeneration(current, seat) !== null && ownerGeneration(current, seat) === ownerGeneration(team, seat));
  }
  function confirmationRequired(): boolean {
    const requested = team.seats.find(s => s.configured.name === seat)?.observed_owner?.configured;
    return !!selected && (!requested || !sameExecutionTuple(selected, requested));
  }
  function paint() {
    if (closed) return;
    evidence.innerHTML = options.kind === "replace" ? renderReplacementEvidence(latest, inflight && !refreshing) : renderArchiveEvidence(archived, inflight && !refreshing);
    const available = allowed();
    button("refresh")!.disabled = inflight;
    if (selection) selection.disabled = inflight || !available;
    if (confirm) { confirm.disabled = inflight || !available; confirm.parentElement!.hidden = !confirmationRequired(); }
    const prepare = button("prepare"), start = button("start"), archive = button("archive");
    if (prepare) prepare.disabled = inflight || needsRefresh || !available;
    const current = live();
    if (start) start.disabled = inflight || needsRefresh || !available || !current || !selected ||
      !canStartReplacement(current, seat, prepared, selected) || (confirmationRequired() && !confirm?.checked);
    if (archive) archive.disabled = inflight || needsRefresh || !available;
    if (!available) status.textContent = "Not available: the backend has not enabled this capability, or its authoritative generation is missing/changed. No operation was requested by opening this dialog.";
    else if (refreshing) status.textContent = "Refreshing authoritative state. Refresh does not initiate a lifecycle operation.";
    else if (inflight) status.textContent = "Waiting for native evidence. No successful outcome is confirmed yet.";
    else if (needsRefresh) status.textContent = "Refresh authoritative state before another operation. Prior effects are not undone.";
    else status.textContent = "Only returned native evidence can enable a start or confirm archival.";
  }
  selection?.addEventListener("change", () => {
    if (inflight) return;
    const index = selection.value === "" ? -1 : Number(selection.value);
    selected = Number.isInteger(index) ? choices[index] : undefined;
    if (prepared || latest) needsRefresh = true;
    prepared = null; latest = null; if (confirm) confirm.checked = false; paint();
  });
  confirm?.addEventListener("change", paint);
  dialog.addEventListener("click", async event => {
    const target = (event.target as HTMLElement).closest<HTMLButtonElement>("button[data-action]");
    if (!target || target.disabled) return;
    const action = target.dataset.action;
    if (action === "close") { dialog.close(); return; }
    if (inflight) return;
    error.textContent = "";
    if (action === "refresh") {
      prepared = null; latest = null; archived = null;
      inflight = true; refreshing = true; paint();
      try {
        const refreshed = await options.refresh();
        if (!refreshed || refreshed.snapshot.team !== team.snapshot.team) throw new Error("State unavailable");
        team = refreshed; prepared = null; latest = null; archived = null; needsRefresh = false; if (confirm) confirm.checked = false; refreshChoices();
      } catch { needsRefresh = true; error.textContent = "Authoritative state unavailable. No further operation is enabled."; }
      finally { inflight = false; refreshing = false; paint(); }
      return;
    }
    if (!allowed() || needsRefresh) { paint(); return; }
    if (action === "start" && (!selected || !prepared || !live() || !canStartReplacement(live()!, seat, prepared, selected) || (confirmationRequired() && !confirm?.checked))) { paint(); return; }
    const activePreparation = prepared;
    if (action === "start") { prepared = null; needsRefresh = true; } // consume locally before awaiting, no double start
    inflight = true; paint();
    try {
      if (action === "prepare") {
        const result = await api.prepare(team, seat);
        latest = result; prepared = result; needsRefresh = result.phase !== "ready";
      } else if (action === "start" && activePreparation && selected) {
        latest = await api.start(team, seat, activePreparation, selected); // selection already immutable while pending
      } else if (action === "archive") {
        needsRefresh = true; archived = await api.archive(team);
      }
    } catch (failure) {
      prepared = null; latest = null; archived = null; needsRefresh = true; error.textContent = runtimeErrorCopy(failure);
    } finally { inflight = false; paint(); if (closed) void options.refresh().catch(() => {}); }
  });
  dialog.addEventListener("close", () => { closed = true; dialog.remove(); if (origin?.isConnected) origin.focus(); });
  refreshChoices(); paint(); document.body.appendChild(dialog); dialog.showModal();
  return dialog;
}
