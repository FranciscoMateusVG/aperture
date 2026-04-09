import { commands } from "../services/tauri-commands";
import type { Objective } from "../types";

const COLUMNS = ["draft", "speccing", "ready", "approved", "in_progress", "done"];
const COLUMN_LABELS: Record<string, string> = {
  draft: "Draft",
  speccing: "Speccing",
  ready: "Ready",
  approved: "Approved",
  in_progress: "In Progress",
  done: "Done",
};

const PRIORITY_COLORS: Record<number, string> = {
  0: "var(--accent-red)",
  1: "var(--accent-orange)",
  2: "var(--accent-blue)",
  3: "var(--text-secondary)",
  4: "#555",
};

interface BeadsTask {
  id: string;
  title: string;
  status: string;
  priority?: number;
  notes?: string;
}

function escHtml(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
}

function taskStatusToColumn(status: string): string {
  // Map BEADS task statuses to our column keys
  if (status === "closed") return "done";
  if (COLUMNS.includes(status)) return status;
  return "draft"; // unknown statuses default to draft column
}

function renderTaskCard(task: BeadsTask): string {
  const pColor = PRIORITY_COLORS[task.priority ?? 2] ?? "var(--text-secondary)";
  return `
    <div class="swimlane__card" data-task-id="${task.id}">
      <div class="swimlane__card-header">
        <span class="swimlane__card-id">${task.id}</span>
        <span class="swimlane__card-priority" style="color:${pColor}">P${task.priority ?? "?"}</span>
      </div>
      <div class="swimlane__card-title">${escHtml(task.title ?? "")}</div>
    </div>
  `;
}

function renderLane(obj: Objective, tasks: BeadsTask[]): string {
  const pColor = PRIORITY_COLORS[obj.priority] ?? "var(--text-secondary)";
  const total = tasks.length;
  const closed = tasks.filter((t) => t.status === "closed").length;
  const progress = total > 0 ? Math.round((closed / total) * 100) : 0;

  // Group tasks by column
  const tasksByCol: Record<string, BeadsTask[]> = {};
  for (const col of COLUMNS) tasksByCol[col] = [];
  for (const task of tasks) {
    const col = taskStatusToColumn(task.status);
    if (tasksByCol[col]) tasksByCol[col].push(task);
  }

  const columnsHtml = COLUMNS.map((col) => {
    const colTasks = tasksByCol[col] || [];
    return `
      <div class="swimlane__cell" data-obj-id="${obj.id}" data-status="${col}">
        ${colTasks.map(renderTaskCard).join("")}
      </div>
    `;
  }).join("");

  return `
    <div class="swimlane" data-obj-id="${obj.id}">
      <div class="swimlane__header">
        <div class="swimlane__header-top">
          <span class="swimlane__title">${escHtml(obj.title)}</span>
          <span class="swimlane__priority" style="color:${pColor}">P${obj.priority}</span>
          <span class="swimlane__status">${obj.status}</span>
          <button class="btn btn--tiny kanban__spec-btn" data-id="${obj.id}" title="Send to Wheatley for speccing">Spec it</button>
          ${obj.spec && tasks.length === 0 ? `<button class="btn btn--tiny kanban__tasks-btn" data-id="${obj.id}" title="Send to Wheatley to create BEADS tasks from spec">Write Tasks</button>` : ""}
          <button class="btn btn--tiny kanban__archive-btn" data-id="${obj.id}" title="Archive">Archive</button>
        </div>
        ${obj.description ? `<div class="swimlane__desc">${escHtml(obj.description)}</div>` : ""}
        ${obj.spec ? `
          <button class="btn btn--tiny swimlane__spec-toggle" data-id="${obj.id}">View Spec</button>
          <div class="swimlane__spec hidden" data-spec-id="${obj.id}">${escHtml(obj.spec)}</div>
        ` : ""}
        ${total > 0 ? `
          <div class="swimlane__progress">
            <div class="swimlane__progress-bar"><div class="swimlane__progress-fill" style="width:${progress}%"></div></div>
            <span class="swimlane__progress-text">${closed}/${total}</span>
          </div>
        ` : ""}
      </div>
      <div class="swimlane__columns">
        ${columnsHtml}
      </div>
    </div>
  `;
}

export function createObjectivesKanban(container: HTMLElement) {
  container.innerHTML = `
    <div class="kanban">
      <div class="kanban__toolbar">
        <h2 class="kanban__title">Objectives</h2>
        <button class="btn kanban__add-btn">+ New Objective</button>
      </div>
      <div class="kanban__col-headers">
        <div class="kanban__col-headers-spacer"></div>
        ${COLUMNS.map((col) => `<div class="kanban__col-header">${COLUMN_LABELS[col]}</div>`).join("")}
      </div>
      <div class="kanban__lanes"></div>
      <div class="kanban__create-form hidden">
        <input class="kanban__input" id="obj-title" placeholder="Objective title" />
        <textarea class="kanban__textarea" id="obj-desc" placeholder="Description (optional)" rows="2"></textarea>
        <div class="kanban__form-row">
          <select class="kanban__select" id="obj-priority">
            <option value="0">P0 - Critical</option>
            <option value="1">P1 - High</option>
            <option value="2" selected>P2 - Medium</option>
            <option value="3">P3 - Low</option>
            <option value="4">P4 - Minimal</option>
          </select>
          <button class="btn kanban__create-btn">Create</button>
          <button class="btn kanban__cancel-btn">Cancel</button>
        </div>
      </div>
      <div class="kanban__empty hidden">
        <p>No objectives yet. Create one to get started.</p>
      </div>
    </div>
  `;

  const lanesEl = container.querySelector(".kanban__lanes") as HTMLElement;
  const createForm = container.querySelector(".kanban__create-form") as HTMLElement;
  const emptyState = container.querySelector(".kanban__empty") as HTMLElement;

  let lastHash = "";
  let objectives: Objective[] = [];
  let allTasks: BeadsTask[] = [];

  // ── Create Form ──
  container.querySelector(".kanban__add-btn")?.addEventListener("click", () => {
    createForm.classList.toggle("hidden");
    const titleInput = document.getElementById("obj-title") as HTMLInputElement;
    if (titleInput) titleInput.focus();
  });

  container.querySelector(".kanban__cancel-btn")?.addEventListener("click", () => {
    createForm.classList.add("hidden");
  });

  container.querySelector(".kanban__create-btn")?.addEventListener("click", async () => {
    const title = (document.getElementById("obj-title") as HTMLInputElement).value.trim();
    const desc = (document.getElementById("obj-desc") as HTMLTextAreaElement).value.trim();
    const priority = parseInt((document.getElementById("obj-priority") as HTMLSelectElement).value);
    if (!title) return;
    await commands.createObjective(title, desc, priority);
    (document.getElementById("obj-title") as HTMLInputElement).value = "";
    (document.getElementById("obj-desc") as HTMLTextAreaElement).value = "";
    createForm.classList.add("hidden");
    poll();
  });

  // ── Event Delegation ──
  lanesEl.addEventListener("click", async (e) => {
    const target = e.target as HTMLElement;

    // Spec toggle button
    if (target.classList.contains("swimlane__spec-toggle")) {
      const id = target.dataset.id!;
      const specEl = lanesEl.querySelector(`.swimlane__spec[data-spec-id="${id}"]`) as HTMLElement;
      if (specEl) {
        const isHidden = specEl.classList.toggle("hidden");
        target.textContent = isHidden ? "View Spec" : "Hide Spec";
      }
      return;
    }

    // Spec it button
    if (target.classList.contains("kanban__spec-btn")) {
      const id = target.dataset.id!;
      const obj = objectives.find((o) => o.id === id);
      if (!obj) return;
      await commands.sendChat(
        "wheatley",
        `New objective to spec: "${obj.title}". Description: ${obj.description || "(none)"}. ` +
        `Please research the codebase, write a detailed spec, then update the objective with your spec. ` +
        `Use update_objective(id: "${obj.id}", spec: "your spec here") when done.`
      );
      return;
    }

    // Write Tasks button
    if (target.classList.contains("kanban__tasks-btn")) {
      const id = target.dataset.id!;
      const obj = objectives.find((o) => o.id === id);
      if (!obj) return;
      await commands.sendChat(
        "wheatley",
        `Objective "${obj.title}" (${obj.id}) has an approved spec. Please break it into BEADS tasks using create_task() for each one, ` +
        `then link them to the objective using update_objective(id: "${obj.id}", task_ids: ["task-id-1", "task-id-2", ...]). ` +
        `Here's the spec:\n\n${obj.spec}`
      );
      return;
    }

    // Archive button
    if (target.classList.contains("kanban__archive-btn")) {
      const id = target.dataset.id!;
      await commands.deleteObjective(id);
      poll();
      return;
    }

    // Task card click → open BEADS panel filtered to parent objective
    const card = target.closest(".swimlane__card") as HTMLElement;
    if (card) {
      const lane = card.closest(".swimlane") as HTMLElement;
      const objId = lane?.dataset.objId;
      const obj = objectives.find((o) => o.id === objId);
      if (obj) {
        window.dispatchEvent(new CustomEvent("objective-selected", { detail: obj }));
      }
      return;
    }

    // Lane header click → open BEADS panel filtered
    const header = target.closest(".swimlane__header") as HTMLElement;
    if (header && !target.closest("button")) {
      const lane = header.closest(".swimlane") as HTMLElement;
      const objId = lane?.dataset.objId;
      const obj = objectives.find((o) => o.id === objId);
      if (obj) {
        window.dispatchEvent(new CustomEvent("objective-selected", { detail: obj }));
      }
    }
  });

  // ── Drag and Drop (mouse-event based, WKWebView compatible) ──
  let dragState: { card: HTMLElement; taskId: string; ghost: HTMLElement; offsetX: number; offsetY: number } | null = null;

  lanesEl.addEventListener("mousedown", (e) => {
    const card = (e.target as HTMLElement).closest(".swimlane__card") as HTMLElement;
    if (!card) return;
    const taskId = card.dataset.taskId;
    if (!taskId) return;

    e.preventDefault();
    const rect = card.getBoundingClientRect();

    // Offset of click within the card
    const offsetX = e.clientX - rect.left;
    const offsetY = e.clientY - rect.top;

    // Create ghost element
    const ghost = card.cloneNode(true) as HTMLElement;
    ghost.classList.add("swimlane__card--ghost");
    ghost.style.width = `${rect.width}px`;
    ghost.style.left = `${rect.left}px`;
    ghost.style.top = `${rect.top}px`;
    document.body.appendChild(ghost);

    card.classList.add("swimlane__card--dragging");
    dragState = { card, taskId, ghost, offsetX, offsetY };
  });

  document.addEventListener("mousemove", (e) => {
    if (!dragState) return;
    dragState.ghost.style.left = `${e.clientX - dragState.offsetX}px`;
    dragState.ghost.style.top = `${e.clientY - dragState.offsetY}px`;

    // Highlight cell under cursor
    lanesEl.querySelectorAll(".swimlane__cell--dragover").forEach((el) =>
      el.classList.remove("swimlane__cell--dragover")
    );
    const elUnder = document.elementFromPoint(e.clientX, e.clientY);
    if (elUnder) {
      const cell = elUnder.closest(".swimlane__cell") as HTMLElement;
      if (cell && lanesEl.contains(cell)) {
        cell.classList.add("swimlane__cell--dragover");
      }
    }
  });

  document.addEventListener("mouseup", async (e) => {
    if (!dragState) return;
    const { card, taskId, ghost } = dragState;
    dragState = null;

    // Clean up
    card.classList.remove("swimlane__card--dragging");
    ghost.remove();
    lanesEl.querySelectorAll(".swimlane__cell--dragover").forEach((el) =>
      el.classList.remove("swimlane__cell--dragover")
    );

    // Find drop target
    const elUnder = document.elementFromPoint(e.clientX, e.clientY);
    if (!elUnder) return;
    const cell = elUnder.closest(".swimlane__cell") as HTMLElement;
    if (!cell || !lanesEl.contains(cell)) return;

    const newStatus = cell.dataset.status;
    if (!newStatus) return;

    // Use column name directly as BEADS status (bd accepts any string)
    // "done" maps to "closed" for BEADS compatibility
    const beadsStatus = newStatus === "done" ? "closed" : newStatus;

    // Optimistic update
    const task = allTasks.find((t) => t.id === taskId);
    if (task) {
      const prevStatus = task.status;
      task.status = beadsStatus;
      render();

      // Persist to BEADS
      try {
        await commands.updateBeadsTaskStatus(taskId, beadsStatus);
      } catch (err) {
        console.error("Failed to update task status:", err);
        task.status = prevStatus;
        render();
      }
    }

    poll();
  });

  // ── Render ──
  function render() {
    const activeObjs = objectives.filter((o) => (o.status as string) !== "archived");

    if (activeObjs.length === 0) {
      lanesEl.innerHTML = "";
      emptyState.classList.remove("hidden");
      return;
    }
    emptyState.classList.add("hidden");

    // Build task map
    const taskMap = new Map<string, BeadsTask>();
    allTasks.forEach((t) => taskMap.set(t.id, t));

    lanesEl.innerHTML = activeObjs
      .map((obj) => {
        const objTasks = obj.task_ids
          .map((id) => taskMap.get(id))
          .filter((t): t is BeadsTask => t !== undefined);
        return renderLane(obj, objTasks);
      })
      .join("");
  }

  // ── Polling ──
  async function poll() {
    try {
      const [objs, tasks] = await Promise.all([
        commands.listObjectives(),
        commands.listBeadsTasks().catch(() => []),
      ]);
      const hash = JSON.stringify(objs) + JSON.stringify(tasks);
      if (hash !== lastHash) {
        lastHash = hash;
        objectives = objs;
        allTasks = Array.isArray(tasks) ? tasks : [];
        render();
      }
    } catch {
      // silent
    }
  }

  poll();
  const interval = setInterval(poll, 3000);

  return {
    destroy() { clearInterval(interval); },
    refresh: poll,
  };
}
