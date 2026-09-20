import type { CreateTeamInput, ExecutionTuple, TeamCatalog, TeamPreset, TeamPresetInput } from "../types";
import { deriveTeamSeatNames, initialRepository, validateRepositoryDraft, snapshotTeamDraft, teamErrorCopy, validateTeamDraft, validHumanText, type DraftIssue } from "../services/team-draft";
import { escapeHtml as e } from "../utils/html";

export interface TeamEditorOptions {
  mode: "create" | "edit" | "duplicate" | "blank";
  preset?: TeamPreset;
  catalog: TeamCatalog;
  knownTeams: string[];
  knownSeats: string[];
  submitTeam: (input: CreateTeamInput) => Promise<unknown>;
  submitPreset: (input: TeamPresetInput, expectedSha256: string | null) => Promise<unknown>;
  saved: (kind: "team" | "preset", result: unknown) => void;
}
const tupleKey = (v: ExecutionTuple) => JSON.stringify([v.harness, v.model, v.reasoning]);
export const tupleLabel = (v: ExecutionTuple) => `${v.harness} · ${v.model}${v.reasoning === null ? "" : ` · ${v.reasoning}`}`;
const execution = (v: ExecutionTuple) => ({ harness: v.harness, model: v.model, reasoning: v.reasoning });

export function createInitialTeamDraft(preset?: TeamPreset, catalog?: TeamCatalog): CreateTeamInput {
  return {
    team: "", project: "project:aperture", repo: initialRepository("project:aperture", catalog?.repositories ?? []), mission: preset?.mission_placeholder ?? "", acceptance: preset?.acceptance_placeholder ?? "",
    preset_id: preset?.id ?? null, lead_index: preset?.lead_index ?? 0,
    seats: preset?.seats.map(s => ({ ...s })) ?? [], fallbacks: preset?.fallbacks.map(execution) ?? [],
  };
}
export function validateCatalogDraft(draft: CreateTeamInput, catalog: TeamCatalog, isTeam = true): DraftIssue[] {
  const issues: DraftIssue[] = isTeam ? validateRepositoryDraft(draft, catalog.repositories) : [];
  const tuples = new Set(catalog.execution_tuples.map(tupleKey));
  draft.seats.forEach((seat, i) => {
    if (!catalog.roles.some(r => r.id === seat.role)) issues.push({ field: `seats.${i}.role`, message: "Choose a role from the current backend catalog." });
    if (!tuples.has(tupleKey(seat))) issues.push({ field: `seats.${i}.execution`, message: "Choose an exact execution tuple from the current backend catalog." });
  });
  draft.fallbacks.forEach((tuple, i) => { if (!tuples.has(tupleKey(tuple))) issues.push({ field: `fallbacks.${i}`, message: "This fallback is no longer in the catalog." }); });
  if (draft.seats.length > catalog.limits.max_seats) issues.push({ field: "seats", message: "The backend seat limit was exceeded." });
  if (draft.fallbacks.length > catalog.limits.max_fallbacks) issues.push({ field: "fallbacks", message: "The backend fallback limit was exceeded." });
  return issues;
}

export function openTeamEditor(options: TeamEditorOptions): HTMLDialogElement {
  const { catalog, mode, preset } = options;
  const isTeam = mode === "create", editing = mode === "edit";
  const origin = document.activeElement instanceof HTMLElement ? document.activeElement : null;
  let draft = createInitialTeamDraft(preset, catalog);
  let busy = false;
  const dialog = document.createElement("dialog");
  dialog.className = "v4-dialog";
  dialog.setAttribute("aria-labelledby", "v4-editor-title");
  const field = (name: string, label: string, value: string, extra = "") => `<div class="v4-field"><label for="v4-${name}">${label}</label><input id="v4-${name}" name="${name}" value="${e(value)}" ${extra} aria-describedby="v4-error-${name}"><span class="v4-error" id="v4-error-${name}"></span></div>`;
  dialog.innerHTML = `<form novalidate><div class="v4-dialog__body">
    <h2 id="v4-editor-title">${isTeam ? "New team" : editing ? "Edit preset" : mode === "duplicate" ? "Duplicate preset" : "New blank preset"}</h2>
    <p class="v4-meta">${isTeam ? "Creation requests registration and approval by GLaDOS. It does not start workers." : "Saved presets affect future teams only. Existing team snapshots never change."}</p>
    <div class="v4-pair">${isTeam
      ? field("team", "Team name", "", 'maxlength="16" autocomplete="off"') + `<div class="v4-field"><label for="v4-project">Project</label><select id="v4-project" name="project" aria-describedby="v4-error-project">${["aperture", "incluir", "beads-galaxy", "mempalace", "frame"].map(p => `<option value="project:${p}">project:${p}</option>`).join("")}</select><span class="v4-error" id="v4-error-project"></span></div>`
      : field("presetId", "Preset identifier", editing ? preset!.id : "", editing ? "readonly" : 'autocomplete="off"') + field("displayName", "Display name", mode === "duplicate" ? `${preset?.display_name ?? ""} copy` : preset?.display_name ?? "")}</div>
    ${isTeam ? '<div class="v4-field" data-repository-field></div>' : ""}
    ${field("mission", isTeam ? "Mission" : "Mission placeholder", draft.mission)}
    ${field("acceptance", isTeam ? "Acceptance" : "Acceptance placeholder", draft.acceptance)}
    <fieldset><legend>Seats · exactly one lead</legend><div class="v4-seats"></div><div class="v4-actions"><button type="button" class="v4-button" data-action="add-seat">+ Add seat</button></div><p class="v4-error" id="v4-error-seats"></p><p class="v4-error" id="v4-error-lead_index"></p></fieldset>
    <fieldset><legend>Configured fallbacks</legend><p class="v4-meta">Catalog choices only. Replacement authorization is enforced against the immutable team policy by the backend.</p><div class="v4-fallbacks"></div><button type="button" class="v4-button" data-action="add-fallback">+ Add fallback</button><p class="v4-error" id="v4-error-fallbacks"></p></fieldset>
    <p id="v4-editor-errors" role="alert" class="v4-error" data-errors tabindex="-1"></p>
    <p role="status" aria-live="polite" data-status></p>
    </div><div class="v4-dialog__actions"><button class="v4-button" type="button" data-action="cancel">Cancel</button><button class="v4-button v4-button--primary" type="submit">${isTeam ? "Create team" : "Save preset"}</button></div></form>`;
  const form = dialog.querySelector<HTMLFormElement>("form")!;
  const seatsEl = dialog.querySelector<HTMLElement>(".v4-seats")!;
  const fallbackEl = dialog.querySelector<HTMLElement>(".v4-fallbacks")!;
  const status = dialog.querySelector<HTMLElement>("[data-status]")!;
  const errors = dialog.querySelector<HTMLElement>("[data-errors]")!;
  const get = (name: string) => (form.elements.namedItem(name) as HTMLInputElement | HTMLSelectElement | null)?.value ?? "";
  function renderRepository() {
    if (!isTeam) return;
    const choices = catalog.repositories.filter(r => r.project === draft.project);
    const issue = validateRepositoryDraft(draft, catalog.repositories)[0];
    dialog.querySelector<HTMLElement>("[data-repository-field]")!.innerHTML = `<label for="v4-repo">Repository · immutable team binding</label>
      <select id="v4-repo" name="repo" required aria-describedby="v4-repo-help v4-error-repo" ${issue ? 'aria-invalid="true"' : ""}>
      <option value="" ${draft.repo === "" ? "selected" : ""}>Choose repository…</option>
      ${choices.map(r => `<option value="${e(r.repo)}" ${draft.repo === r.repo ? "selected" : ""} ${r.available ? "" : "disabled"}>${e(r.display_name)} · ${e(r.repo)}${r.available ? "" : " — unavailable locally"}</option>`).join("")}</select>
      <p id="v4-repo-help" class="v4-meta">Explicitly submitted for GLaDOS approval. This binding cannot be changed later; it grants no permission to edit the repository.</p>
      <span class="v4-error" id="v4-error-repo">${e(issue?.message ?? "")}</span>`;
    updateSubmit();
  }
  function updateSubmit() {
    const submit = form.querySelector<HTMLButtonElement>('button[type="submit"]')!;
    submit.disabled = busy || (isTeam && validateRepositoryDraft(draft, catalog.repositories).length > 0);
  }
  function tupleOptions(current: ExecutionTuple) {
    const selected = catalog.execution_tuples.findIndex(t => tupleKey(t) === tupleKey(current));
    return `<option value="" ${selected < 0 ? "selected" : ""}>Choose execution…</option>` + catalog.execution_tuples.map((t, i) => `<option value="${i}" ${i === selected ? "selected" : ""}>${e(tupleLabel(t))}</option>`).join("");
  }
  function selectedTuple(name: string): ExecutionTuple {
    const raw = get(name), index = raw === "" ? -1 : Number(raw);
    return Number.isInteger(index) && catalog.execution_tuples[index] ? execution(catalog.execution_tuples[index]) : { harness: "codex", model: "", reasoning: null };
  }
  function capture() {
    draft = { ...draft, team: isTeam ? get("team") : "preview", project: isTeam ? get("project") : "project:aperture", repo: isTeam ? get("repo") : "", mission: get("mission"), acceptance: get("acceptance"),
      seats: draft.seats.map((_, i) => ({ role: get(`role-${i}`), ...selectedTuple(`execution-${i}`) })),
      fallbacks: draft.fallbacks.map((_, i) => selectedTuple(`fallback-${i}`)),
      lead_index: get("lead") === "" ? -1 : Number(get("lead")),
    };
  }
  function previewNames() {
    const names = deriveTeamSeatNames(isTeam ? get("team") || "team" : "team", draft.seats);
    seatsEl.querySelectorAll<HTMLElement>("[data-seat-name]").forEach((el, i) => { el.textContent = names[i]; });
  }
  function renderSeats() {
    seatsEl.innerHTML = draft.seats.map((seat, i) => `<div class="v4-seat-edit">
      <label class="v4-radio-label"><input type="radio" name="lead" value="${i}" ${draft.lead_index === i ? "checked" : ""} aria-label="Lead seat ${i + 1}" aria-describedby="v4-editor-errors">Lead</label>
      <div class="v4-field"><label for="v4-role-${i}">Role ${i + 1}</label><select id="v4-role-${i}" name="role-${i}" aria-describedby="v4-editor-errors"><option value="">Choose role…</option>${catalog.roles.map(r => `<option value="${e(r.id)}" ${seat.role === r.id ? "selected" : ""}>${e(r.display_name)}</option>`).join("")}</select></div>
      <div class="v4-field v4-execution"><label for="v4-execution-${i}">Harness · model · reasoning</label><select id="v4-execution-${i}" name="execution-${i}" aria-describedby="v4-editor-errors">${tupleOptions(seat)}</select></div>
      <button type="button" class="v4-button" data-action="remove-seat" data-index="${i}" aria-label="Remove seat ${i + 1}">Remove</button><span class="v4-seat-name" data-seat-name></span></div>`).join("");
    previewNames();
  }
  function renderFallbacks() {
    fallbackEl.innerHTML = draft.fallbacks.map((tuple, i) => `<div class="v4-fallback-row"><div class="v4-field"><label for="v4-fallback-${i}">Fallback ${i + 1}</label><select id="v4-fallback-${i}" name="fallback-${i}" aria-describedby="v4-editor-errors">${tupleOptions(tuple)}</select></div><button type="button" class="v4-button" data-action="remove-fallback" data-index="${i}" aria-label="Remove fallback ${i + 1}">Remove</button></div>`).join("");
  }
  function setBusy(value: boolean) {
    busy = value;
    form.setAttribute("aria-busy", String(value));
    form.querySelectorAll<HTMLInputElement | HTMLButtonElement | HTMLSelectElement>("input,button,select").forEach(el => { el.disabled = value; });
    updateSubmit();
    status.textContent = value ? "Waiting for the backend. No success is confirmed yet." : "";
  }
  function showIssues(issues: DraftIssue[]) {
    form.querySelectorAll("[aria-invalid]").forEach(el => el.removeAttribute("aria-invalid"));
    form.querySelectorAll<HTMLElement>('[id^="v4-error-"]').forEach(el => { el.textContent = ""; });
    for (const issue of issues) {
      const target = document.getElementById(`v4-error-${issue.field}`);
      if (target && dialog.contains(target)) target.textContent = issue.message;
      const seatField = /^seats\.(\d+)\.(role|execution|name)$/.exec(issue.field);
      const fallbackField = /^fallbacks\.(\d+)$/.exec(issue.field);
      const name = seatField ? `${seatField[2] === "execution" ? "execution" : "role"}-${seatField[1]}` : fallbackField ? `fallback-${fallbackField[1]}` : issue.field;
      const control = form.elements.namedItem(name);
      if (control instanceof HTMLElement) control.setAttribute("aria-invalid", "true");
    }
    errors.textContent = issues.map(i => i.message).join(" ");
    if (issues.length) (form.querySelector<HTMLElement>('[aria-invalid="true"]') ?? errors).focus();
  }
  form.addEventListener("input", event => {
    if (busy) return;
    if ((event.target as HTMLInputElement).name === "team") previewNames();
  });
  form.addEventListener("change", event => {
    if (busy) return;
    capture();
    if ((event.target as HTMLInputElement | null)?.name === "project") {
      draft.repo = initialRepository(draft.project, catalog.repositories);
      renderRepository();
    } else if ((event.target as HTMLInputElement | null)?.name === "repo") {
      // Keep the focused select node; replacing it on change loses native focus.
      const issue = validateRepositoryDraft(draft, catalog.repositories)[0];
      const control = form.elements.namedItem("repo") as HTMLSelectElement;
      if (issue) control.setAttribute("aria-invalid", "true"); else control.removeAttribute("aria-invalid");
      dialog.querySelector<HTMLElement>("#v4-error-repo")!.textContent = issue?.message ?? "";
    }
    previewNames(); updateSubmit();
  });
  form.addEventListener("click", event => {
    const button = (event.target as HTMLElement).closest<HTMLButtonElement>("button[data-action]");
    if (!button || busy) return;
    const action = button.dataset.action;
    if (action === "cancel") { dialog.close(); return; }
    capture();
    if (action === "add-seat") {
      if (draft.seats.length >= Math.min(99, catalog.limits.max_seats)) { showIssues([{ field: "seats", message: "Seat limit reached." }]); return; }
      draft.seats.push({ role: "", harness: "codex", model: "", reasoning: null });
      if (draft.seats.length === 1) draft.lead_index = 0;
      renderSeats(); seatsEl.querySelector<HTMLSelectElement>(`[name="role-${draft.seats.length - 1}"]`)?.focus();
    } else if (action === "remove-seat") {
      const index = Number(button.dataset.index); draft.seats.splice(index, 1);
      draft.lead_index = draft.lead_index === index ? -1 : draft.lead_index > index ? draft.lead_index - 1 : draft.lead_index;
      renderSeats(); form.querySelector<HTMLElement>('[data-action="add-seat"]')?.focus();
    } else if (action === "add-fallback") {
      if (draft.fallbacks.length >= Math.min(16, catalog.limits.max_fallbacks)) { showIssues([{ field: "fallbacks", message: "Fallback limit reached." }]); return; }
      draft.fallbacks.push({ harness: "codex", model: "", reasoning: null }); renderFallbacks();
      fallbackEl.querySelector<HTMLSelectElement>(`[name="fallback-${draft.fallbacks.length - 1}"]`)?.focus();
    } else if (action === "remove-fallback") {
      draft.fallbacks.splice(Number(button.dataset.index), 1); renderFallbacks(); form.querySelector<HTMLElement>('[data-action="add-fallback"]')?.focus();
    }
  });
  form.addEventListener("submit", async event => {
    event.preventDefault(); if (busy) return; capture();
    const issues = [...validateTeamDraft(draft, isTeam ? options.knownSeats : [], isTeam ? options.knownTeams : []), ...validateCatalogDraft(draft, catalog, isTeam)];
    if (!isTeam) {
      if (!/^[a-z0-9][a-z0-9_-]{0,30}$/.test(get("presetId"))) issues.push({ field: "presetId", message: "Use a canonical lowercase preset identifier of 1–31 characters." });
      for (const field of ["displayName", "mission", "acceptance"]) if (!validHumanText(get(field), 80, 320)) issues.push({ field, message: "Use 1–80 characters without controls for preset text." });
    }
    showIssues(issues); if (issues.length) return;
    const submittedDraft = snapshotTeamDraft(draft);
    const presetInput: TeamPresetInput = { schema_version: 1, id: get("presetId"), display_name: get("displayName"), mission_placeholder: draft.mission, acceptance_placeholder: draft.acceptance,
      seats: submittedDraft.seats, lead_index: draft.lead_index, fallbacks: submittedDraft.fallbacks };
    setBusy(true);
    try {
      const result = isTeam ? await options.submitTeam(submittedDraft) : await options.submitPreset(presetInput, editing ? preset!.sha256 : null);
      setBusy(false); dialog.close(); options.saved(isTeam ? "team" : "preset", result);
    } catch (error) {
      setBusy(false); errors.textContent = teamErrorCopy(error); errors.focus();
    }
  });
  dialog.addEventListener("cancel", event => { if (busy) event.preventDefault(); });
  dialog.addEventListener("close", () => { dialog.remove(); if (origin?.isConnected) origin.focus(); });
  renderRepository(); renderSeats(); renderFallbacks(); document.body.appendChild(dialog); dialog.showModal();
  return dialog;
}
