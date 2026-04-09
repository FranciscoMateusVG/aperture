import { commands } from "../services/tauri-commands";
import type { SpiderlingDef } from "../types";

export function createSpiderlingsPanel(container: HTMLElement) {
  container.innerHTML = `
    <div class="spiderlings-panel">
      <div class="spiderlings-panel__header">
        <h3 class="section-title">🕷️ Spiderlings</h3>
      </div>
      <div class="spiderlings-panel__list"></div>
    </div>
  `;

  const listEl = container.querySelector(".spiderlings-panel__list")!;
  let lastHash = "";

  function renderSpiderling(s: SpiderlingDef): string {
    return `
      <div class="spiderling-row" data-window-id="${s.tmux_window_id ?? ""}">
        <div class="spiderling-row__info">
          <span class="spiderling-row__name">🕷️ ${s.name}</span>
          <span class="spiderling-row__task">${s.task_id}</span>
          <span class="spiderling-row__status spiderling-row__status--${s.status}">${s.status}</span>
        </div>
        <button class="btn btn--tiny btn--danger spiderling-row__kill" data-kill="${s.name}" title="Kill spiderling">✖</button>
      </div>
    `;
  }

  async function refresh() {
    try {
      const spiderlings = await commands.listSpiderlings();
      const hash = spiderlings.map(s => `${s.name}:${s.status}`).join("|");

      if (hash === lastHash) return;
      lastHash = hash;

      if (spiderlings.length === 0) {
        listEl.innerHTML = `<div class="spiderlings-panel__empty">No active spiderlings.<br><span style="color: var(--text-secondary); font-size: 0.85em;">GLaDOS will spawn them as needed.</span></div>`;
        return;
      }

      listEl.innerHTML = spiderlings.map(renderSpiderling).join("");

      // Click row to switch to spiderling's tmux window
      listEl.querySelectorAll(".spiderling-row").forEach(row => {
        row.addEventListener("click", async (e) => {
          if ((e.target as HTMLElement).closest("[data-kill]")) return;
          const windowId = (row as HTMLElement).dataset.windowId;
          if (windowId) {
            await commands.tmuxSelectWindow(windowId);
          }
        });
      });

      // Kill buttons
      listEl.querySelectorAll("[data-kill]").forEach(btn => {
        btn.addEventListener("click", async (e) => {
          e.stopPropagation();
          const name = (btn as HTMLElement).dataset.kill!;
          try {
            await commands.killSpiderling(name);
            lastHash = ""; // Force refresh
            await refresh();
          } catch (err) {
            console.error("Failed to kill spiderling:", err);
          }
        });
      });
    } catch {
      // Not ready yet
    }
  }

  refresh();
  const interval = setInterval(refresh, 3000);

  return {
    refresh,
    destroy() { clearInterval(interval); }
  };
}
