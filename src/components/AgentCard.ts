import type { AgentDef } from "../types";
import { commands } from "../services/tauri-commands";
import type { AgentConfigModal } from "./AgentConfigModal";

const AGENT_THEME: Record<string, { icon: string; color: string }> = {
  planner:   { icon: "📋", color: "#e67e22" },  // orange   — director
  glados:    { icon: "🤖", color: "#9b59b6" },  // purple   — orchestrator
  wheatley:  { icon: "💡", color: "#3498db" },  // blue     — planning & research
  peppy:     { icon: "🚀", color: "#1abc9c" },  // teal     — infra
  izzy:      { icon: "🧪", color: "#e91e63" },  // pink     — tester
  vance:     { icon: "🎨", color: "#ff6b9d" },  // magenta  — designer
  rex:       { icon: "🗄️", color: "#e74c3c" },  // red      — backend
  scout:     { icon: "📱", color: "#27ae60" },  // green    — mobile
  cipher:    { icon: "🔐", color: "#7f8c8d" },  // steel    — security
  sage:      { icon: "🌿", color: "#17a589" },  // sage     — SEO/growth
  atlas:     { icon: "📚", color: "#8e44ad" },  // violet   — docs
  sentinel:  { icon: "👁️", color: "#34495e" },  // slate    — overseer
  sterling:  { icon: "⭐", color: "#d4af37" },  // gold     — quality
};

const DEFAULT_THEME = { icon: "⚙️", color: "#f39c12" };

export function createAgentCard(agent: AgentDef, modal: AgentConfigModal, onUpdate: () => void): HTMLElement {
  const card = document.createElement("div");
  const theme = AGENT_THEME[agent.name] ?? DEFAULT_THEME;
  render();

  function render() {
    const isRunning = agent.status === "running";
    card.className = `agent-mini ${isRunning ? "agent-mini--running" : ""}`;
    card.style.setProperty("--agent-color", theme.color);
    card.innerHTML = `
      <span class="agent-mini__icon">${theme.icon}</span>
      <span class="agent-mini__name">${agent.name}</span>
      <span class="agent-mini__model">${agent.model}</span>
      <button class="agent-mini__config" title="Configure">⚙</button>
      <button class="agent-mini__toggle" title="${isRunning ? "Stop" : "Start"}">
        ${isRunning ? "■" : "▶"}
      </button>
    `;

    card.addEventListener("click", async () => {
      if (isRunning && agent.tmux_window_id) {
        await commands.tmuxSelectWindow(agent.tmux_window_id);
        window.dispatchEvent(new CustomEvent("agent-focused", {
          detail: { name: agent.name, color: theme.color }
        }));
      }
    });

    card.querySelector(".agent-mini__config")!.addEventListener("click", (e) => {
      e.stopPropagation();
      modal.open(agent);
    });

    card.querySelector(".agent-mini__toggle")!.addEventListener("click", async (e) => {
      e.stopPropagation();
      try {
        if (isRunning) {
          await commands.stopAgent(agent.name);
        } else {
          await commands.startAgent(agent.name);
        }
        onUpdate();
      } catch (err) {
        const msg = err instanceof Error ? err.message : String(err);
        console.error(`Failed to toggle agent ${agent.name}:`, err);
        alert(`Failed to ${isRunning ? "stop" : "start"} ${agent.name}:\n${msg}`);
      }
    });
  }

  return card;
}
