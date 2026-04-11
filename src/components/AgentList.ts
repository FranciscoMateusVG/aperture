import { commands } from "../services/tauri-commands";
import { createAgentCard } from "./AgentCard";
import { createAgentConfigModal } from "./AgentConfigModal";

export function createAgentList(container: HTMLElement) {
  const wrapper = document.createElement("div");
  wrapper.className = "agent-list";
  container.appendChild(wrapper);

  // Track previous state to avoid unnecessary DOM rebuilds
  let lastAgentHash = "";

  const modal = createAgentConfigModal(() => refresh());

  async function refresh() {
    try {
      const agents = await commands.listAgents();
      const order = ["planner", "glados", "wheatley", "peppy", "izzy"];
      agents.sort((a, b) => {
        const ai = order.indexOf(a.name);
        const bi = order.indexOf(b.name);
        return (ai === -1 ? 999 : ai) - (bi === -1 ? 999 : bi);
      });

      // Build a hash of current state to detect changes (include model for config updates)
      const hash = agents.map(a => `${a.name}:${a.status}:${a.model}`).join("|");

      // Only rebuild DOM if something actually changed
      if (hash !== lastAgentHash) {
        lastAgentHash = hash;
        wrapper.innerHTML = "";

        const allRunning = agents.every(a => a.status === "running");

        const header = document.createElement("div");
        header.className = "agent-list__header";
        header.innerHTML = `<h3 class="section-title">Agents</h3>`;

        const toggleAll = document.createElement("button");
        toggleAll.className = `agent-list__toggle-all ${allRunning ? "agent-list__toggle-all--stop" : "agent-list__toggle-all--play"}`;
        toggleAll.title = allRunning ? "Stop all" : "Start all";
        toggleAll.textContent = allRunning ? "■ All" : "▶ All";
        toggleAll.addEventListener("click", async () => {
          toggleAll.disabled = true;
          toggleAll.innerHTML = `<span class="agent-list__spinner"></span> All`;
          toggleAll.className = "agent-list__toggle-all agent-list__toggle-all--loading";
          for (const agent of agents) {
            try {
              if (allRunning) {
                await commands.stopAgent(agent.name);
              } else if (agent.status !== "running") {
                await commands.startAgent(agent.name);
              }
            } catch (e) {
              console.error(`Failed to toggle ${agent.name}:`, e);
            }
          }
          refresh();
        });
        header.appendChild(toggleAll);
        wrapper.appendChild(header);

        agents.forEach((agent) => {
          wrapper.appendChild(createAgentCard(agent, modal, refresh));
        });
      }
    } catch (e) {
      console.error("Failed to list agents:", e);
    }
  }

  refresh();
  return { refresh };
}
