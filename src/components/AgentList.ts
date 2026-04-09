import { commands } from "../services/tauri-commands";
import { createAgentCard } from "./AgentCard";

export function createAgentList(container: HTMLElement) {
  const wrapper = document.createElement("div");
  wrapper.className = "agent-list";
  container.appendChild(wrapper);

  // Track previous state to avoid unnecessary DOM rebuilds
  let lastAgentHash = "";

  async function refresh() {
    try {
      const agents = await commands.listAgents();
      const order = ["planner", "glados", "wheatley", "peppy", "izzy"];
      agents.sort((a, b) => {
        const ai = order.indexOf(a.name);
        const bi = order.indexOf(b.name);
        return (ai === -1 ? 999 : ai) - (bi === -1 ? 999 : bi);
      });

      // Build a hash of current state to detect changes
      const hash = agents.map(a => `${a.name}:${a.status}`).join("|");

      // Only rebuild DOM if something actually changed
      if (hash !== lastAgentHash) {
        lastAgentHash = hash;
        wrapper.innerHTML = '<h3 class="section-title">Agents</h3>';
        agents.forEach((agent) => {
          wrapper.appendChild(createAgentCard(agent, refresh));
        });
      }
    } catch (e) {
      console.error("Failed to list agents:", e);
    }
  }

  refresh();
  return { refresh };
}
