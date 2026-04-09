import { commands } from "../services/tauri-commands";
import type { Objective } from "../types";

interface BeadsTask {
  id: string;
  title: string;
  description?: string;
  status: string;
  priority?: number;
  notes?: string;
  owner?: string;
  created_at?: string;
  closed_at?: string;
  close_reason?: string;
}

interface Artifact {
  type: string;
  value: string;
}

function parseArtifacts(notes: string): Artifact[] {
  if (!notes) return [];
  return notes
    .split("\n")
    .filter((l) => l.startsWith("artifact:"))
    .map((l) => {
      const [, type, ...rest] = l.split(":");
      return { type, value: rest.join(":") };
    });
}

function escHtml(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

const PRIORITY_COLORS: Record<number, string> = {
  0: "var(--accent-red)",
  1: "var(--accent-orange)",
  2: "var(--accent-blue)",
  3: "var(--text-secondary)",
  4: "#555",
};

export function createBeadsPanel(container: HTMLElement) {
  container.innerHTML = `
    <div class="beads-panel">
      <div class="beads-panel__header">
        <div class="section-title">BEADS Tasks</div>
        <div class="beads-panel__filter-context hidden">
          <span class="beads-panel__filter-label"></span>
          <button class="btn btn--tiny beads-panel__filter-clear">&times;</button>
        </div>
      </div>
      <div class="beads-panel__controls">
        <input class="beads-panel__search" placeholder="Search tasks..." />
        <select class="beads-panel__filter">
          <option value="all">All</option>
          <option value="open">Open</option>
          <option value="closed">Closed</option>
        </select>
      </div>
      <div class="beads-panel__list"></div>
    </div>
  `;

  const listEl = container.querySelector(".beads-panel__list") as HTMLElement;
  const searchEl = container.querySelector(".beads-panel__search") as HTMLInputElement;
  const filterEl = container.querySelector(".beads-panel__filter") as HTMLSelectElement;
  const filterContext = container.querySelector(".beads-panel__filter-context") as HTMLElement;
  const filterLabel = container.querySelector(".beads-panel__filter-label") as HTMLElement;
  const filterClear = container.querySelector(".beads-panel__filter-clear") as HTMLElement;

  let allTasks: BeadsTask[] = [];
  let lastHash = "";
  let objectiveFilter: Objective | null = null;
  let expandedTasks = new Set<string>();

  // ── Objective filter (from Kanban click) ──
  window.addEventListener("objective-selected", ((e: CustomEvent<Objective>) => {
    objectiveFilter = e.detail;
    filterContext.classList.remove("hidden");
    filterLabel.textContent = `Filtered: ${objectiveFilter.title}`;
    renderFiltered();
  }) as EventListener);

  filterClear.addEventListener("click", () => {
    objectiveFilter = null;
    filterContext.classList.add("hidden");
    renderFiltered();
  });

  searchEl.addEventListener("input", () => renderFiltered());
  filterEl.addEventListener("change", () => renderFiltered());

  // ── Event delegation ──
  listEl.addEventListener("click", async (e) => {
    const target = e.target as HTMLElement;

    // Toggle expand
    const row = target.closest(".beads-task") as HTMLElement;
    if (row && !target.closest("button")) {
      const id = row.dataset.id!;
      if (expandedTasks.has(id)) {
        expandedTasks.delete(id);
      } else {
        expandedTasks.add(id);
      }
      renderFiltered();
      return;
    }

    // Open file
    if (target.classList.contains("beads-artifact__open")) {
      const path = target.dataset.path!;
      await commands.openFile(path);
    }
  });

  function renderFiltered() {
    let filtered = [...allTasks];

    // Objective filter
    if (objectiveFilter) {
      const ids = new Set(objectiveFilter.task_ids);
      filtered = filtered.filter((t) => ids.has(t.id));
    }

    // Status filter
    const statusFilter = filterEl.value;
    if (statusFilter === "open") {
      filtered = filtered.filter((t) => t.status !== "closed");
    } else if (statusFilter === "closed") {
      filtered = filtered.filter((t) => t.status === "closed");
    }

    // Search filter
    const query = searchEl.value.toLowerCase().trim();
    if (query) {
      filtered = filtered.filter(
        (t) =>
          (t.id && t.id.toLowerCase().includes(query)) ||
          (t.title && t.title.toLowerCase().includes(query))
      );
    }

    if (filtered.length === 0) {
      listEl.innerHTML = `<div class="beads-panel__empty">No tasks found.</div>`;
      return;
    }

    listEl.innerHTML = filtered.map((t) => renderTask(t)).join("");
  }

  function renderTask(t: BeadsTask): string {
    const statusColor =
      t.status === "closed"
        ? "var(--accent-green)"
        : t.status === "in_progress"
          ? "var(--accent-orange)"
          : "var(--text-secondary)";
    const pColor = PRIORITY_COLORS[t.priority ?? 2] ?? "var(--text-secondary)";
    const artifacts = parseArtifacts(t.notes ?? "");
    const isExpanded = expandedTasks.has(t.id);

    const expandedHtml = isExpanded
      ? `<div class="beads-task__details">
          ${t.description ? `<div class="beads-task__desc">${escHtml(t.description)}</div>` : ""}
          ${t.close_reason ? `<div class="beads-task__reason"><strong>Closed:</strong> ${escHtml(t.close_reason)}</div>` : ""}
          ${artifacts.length > 0
            ? `<div class="beads-task__artifacts">
                <div class="beads-task__artifacts-title">Artifacts (${artifacts.length})</div>
                ${artifacts.map((a) => renderArtifact(a)).join("")}
              </div>`
            : ""
          }
        </div>`
      : "";

    return `
      <div class="beads-task ${isExpanded ? "beads-task--expanded" : ""}" data-id="${t.id}">
        <div class="beads-task__header">
          <span class="beads-task__chevron">${isExpanded ? "&#9660;" : "&#9654;"}</span>
          <span class="beads-task__id">${t.id}</span>
          <span class="beads-task__priority" style="color:${pColor}">P${t.priority ?? "?"}</span>
          <span class="beads-task__status" style="color:${statusColor}">${t.status ?? "open"}</span>
        </div>
        <div class="beads-task__title">${escHtml(t.title ?? "")}</div>
        ${artifacts.length > 0 && !isExpanded ? `<div class="beads-task__artifact-hint">${artifacts.length} artifact${artifacts.length > 1 ? "s" : ""}</div>` : ""}
        ${expandedHtml}
      </div>
    `;
  }

  function renderArtifact(a: Artifact): string {
    if (a.type === "file") {
      return `<div class="beads-artifact">
        <span class="beads-artifact__type">file</span>
        <span class="beads-artifact__value">${escHtml(a.value)}</span>
        <button class="btn btn--tiny beads-artifact__open" data-path="${escHtml(a.value)}">Open</button>
      </div>`;
    }
    if (a.type === "pr" || a.type === "url") {
      return `<div class="beads-artifact">
        <span class="beads-artifact__type">${a.type}</span>
        <a class="beads-artifact__link" href="${escHtml(a.value)}" target="_blank">${escHtml(a.value)}</a>
      </div>`;
    }
    if (a.type === "note") {
      return `<div class="beads-artifact">
        <span class="beads-artifact__type">note</span>
        <span class="beads-artifact__note">${escHtml(a.value)}</span>
      </div>`;
    }
    return `<div class="beads-artifact">
      <span class="beads-artifact__type">${escHtml(a.type)}</span>
      <span class="beads-artifact__value">${escHtml(a.value)}</span>
    </div>`;
  }

  // ── Polling ──
  async function poll() {
    try {
      const tasks = await commands.listBeadsTasks();
      if (!Array.isArray(tasks)) return;
      const hash = JSON.stringify(tasks);
      if (hash !== lastHash) {
        lastHash = hash;
        allTasks = tasks;
        renderFiltered();
      }
    } catch {
      listEl.innerHTML = `<div class="beads-panel__empty">BEADS not available.</div>`;
    }
  }

  poll();
  const interval = setInterval(poll, 3000);

  return {
    destroy() { clearInterval(interval); },
    refresh: poll,
  };
}
