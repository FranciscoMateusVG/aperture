import { commands } from "../services/tauri-commands";
import type { SpiderlingDef } from "../types";

export function createSpiderlingCard(
  spiderling: SpiderlingDef,
  onRefresh: () => void,
): HTMLElement {
  const card = document.createElement("div");
  card.className = "agent-mini agent-mini--running";
  card.dataset.role = "spiderling";
  card.style.setProperty("--agent-color", "#95a5a6");

  const icon = document.createElement("span");
  icon.className = "agent-mini__icon";
  icon.textContent = "\u{1F577}\u{FE0F}"; // spider emoji

  const name = document.createElement("span");
  name.className = "agent-mini__name";
  name.textContent = spiderling.name;
  name.style.color = "#95a5a6";

  const meta = document.createElement("span");
  meta.className = "agent-mini__model";
  meta.textContent = spiderling.task_id;

  const killBtn = document.createElement("button");
  killBtn.className = "agent-mini__toggle";
  killBtn.textContent = "\u{2716}"; // ✖
  killBtn.title = "Kill spiderling";
  killBtn.style.background = "rgba(231, 76, 60, 0.2)";
  killBtn.style.color = "var(--accent-red)";
  killBtn.addEventListener("click", async (e) => {
    e.stopPropagation();
    try {
      await commands.killSpiderling(spiderling.name);
      onRefresh();
    } catch (err) {
      console.error("Failed to kill spiderling:", err);
    }
  });

  // Click to switch to spiderling's tmux window
  card.addEventListener("click", async () => {
    if (spiderling.tmux_window_id) {
      await commands.tmuxSelectWindow(spiderling.tmux_window_id);
    }
  });

  card.appendChild(icon);
  card.appendChild(name);
  card.appendChild(meta);
  card.appendChild(killBtn);

  return card;
}
