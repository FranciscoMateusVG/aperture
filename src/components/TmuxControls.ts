import { commands } from "../services/tauri-commands";

export function createTmuxControls(container: HTMLElement, sessionName: string) {
  const wrapper = document.createElement("div");
  wrapper.className = "tmux-controls";
  wrapper.innerHTML = `
    <h3 class="section-title">Sessions</h3>
    <div class="tmux-controls__windows" id="window-list"></div>
    <div class="tmux-controls__actions">
      <button class="btn btn--small" id="btn-add-window">+ Window</button>
    </div>
  `;

  // Agent names to filter out from the sessions list
  const agentNames = new Set(["planner", "glados", "wheatley", "peppy", "izzy", "vance", "rex", "scout", "cipher", "sage", "atlas", "sentinel", "sterling"]);
  container.appendChild(wrapper);

  const windowList = wrapper.querySelector("#window-list")!;

  wrapper.querySelector("#btn-add-window")!.addEventListener("click", async () => {
    try {
      const name = `win-${Date.now().toString(36)}`;
      await commands.tmuxCreateWindow(sessionName, name);
      await refreshWindows();
    } catch (e) {
      console.error("Failed to create window:", e);
    }
  });

  async function refreshWindows() {
    try {
      const allWindows = await commands.tmuxListWindows(sessionName);
      // Only show non-agent windows
      const windows = allWindows.filter(w => !agentNames.has(w.name) && !w.name.startsWith("spider-"));
      windowList.innerHTML = windows.map(w => `
        <div class="tmux-controls__window" data-window-id="${w.window_id}">
          <span class="tmux-controls__window-name">${w.name}</span>
          <span class="tmux-controls__window-cmd">${w.command}</span>
          <button class="btn btn--tiny btn--danger" data-kill="${w.window_id}">x</button>
        </div>
      `).join("");

      // Click window row to switch to it
      windowList.querySelectorAll(".tmux-controls__window").forEach(row => {
        row.addEventListener("click", async (e) => {
          // Don't switch if clicking the kill button
          if ((e.target as HTMLElement).closest("[data-kill]")) return;
          const id = (row as HTMLElement).dataset.windowId!;
          await commands.tmuxSelectWindow(id);
        });
      });

      windowList.querySelectorAll("[data-kill]").forEach(btn => {
        btn.addEventListener("click", async (e) => {
          e.stopPropagation();
          const id = (btn as HTMLElement).dataset.kill!;
          await commands.tmuxKillWindow(id);
          await refreshWindows();
        });
      });
    } catch {
      // Session might not exist yet
    }
  }

  refreshWindows();
  const interval = setInterval(refreshWindows, 5000);

  return {
    refresh: refreshWindows,
    destroy() { clearInterval(interval); }
  };
}
