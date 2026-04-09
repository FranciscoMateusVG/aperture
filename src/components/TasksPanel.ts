import { commands } from "../services/tauri-commands";

export function createTasksPanel(container: HTMLElement) {
  container.innerHTML = `
    <div class="tasks-panel">
      <div class="section-title">BEADS Tasks</div>
      <div class="tasks-panel__list"></div>
    </div>
  `;

  const listEl = container.querySelector(".tasks-panel__list") as HTMLElement;

  async function refresh() {
    try {
      const tasks = await commands.listBeadsTasks();
      if (!Array.isArray(tasks) || tasks.length === 0) {
        listEl.innerHTML =
          '<div style="color: var(--text-secondary); font-size: 12px; padding: 8px;">No tasks yet.</div>';
        return;
      }

      listEl.innerHTML = tasks
        .map((t: any) => {
          const statusColor =
            t.status === "closed"
              ? "var(--accent-green)"
              : t.status === "in_progress"
                ? "var(--accent-orange)"
                : "var(--text-secondary)";
          const priorityLabel =
            t.priority !== undefined ? `P${t.priority}` : "";
          const notes: string = t.notes ?? "";
          const artifacts = notes
            .split("\n")
            .filter((l: string) => l.startsWith("artifact:"))
            .map((l: string) => {
              const [, type, ...rest] = l.split(":");
              const value = rest.join(":");
              return `<div class="tasks-panel__artifact">${type}: ${value}</div>`;
            })
            .join("");

          return `
            <div class="tasks-panel__task">
              <div class="tasks-panel__task-header">
                <span class="tasks-panel__task-id">${t.id ?? ""}</span>
                <span class="tasks-panel__task-priority">${priorityLabel}</span>
                <span class="tasks-panel__task-status" style="color: ${statusColor}">${t.status ?? "open"}</span>
              </div>
              <div class="tasks-panel__task-title">${t.title ?? ""}</div>
              <div class="tasks-panel__task-meta">${t.assignee ?? "unassigned"}</div>
              ${artifacts ? `<div class="tasks-panel__artifacts">${artifacts}</div>` : ""}
            </div>
          `;
        })
        .join("");
    } catch {
      listEl.innerHTML =
        '<div style="color: var(--text-secondary); font-size: 12px; padding: 8px;">BEADS not available.</div>';
    }
  }

  refresh();
  setInterval(refresh, 5000);

  return { refresh };
}
