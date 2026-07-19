import type { AgentDef } from "../types";
import { commands } from "../services/tauri-commands";
import { deriveDotState, dotTooltip, deriveWorkSummary } from "../services/hub-presence";
import type { AgentConfigModal } from "./AgentConfigModal";

const AGENT_THEME: Record<string, { icon: string; color: string }> = {
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
  sterling:  { icon: "⭐", color: "#d4af37" },  // gold     — quality
};

const DEFAULT_THEME = { icon: "⚙️", color: "#f39c12" };

// Bead titles (current-work summary, aperture-nr65b) originate from BEADS —
// including, transitively, user-submitted report titles via the AI-intake
// pipeline (aperture:incluir-intake-to-bead). Unlike agent.name/agent.model
// (constrained to a fixed manifest list), a bead title is free text, so it
// gets escaped before landing in innerHTML. Everything else already
// interpolated in this file's template comes from a closed, trusted set.
function escapeHtml(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

export function createAgentCard(agent: AgentDef, modal: AgentConfigModal, onUpdate: () => void): HTMLElement {
  const card = document.createElement("div");
  const theme = AGENT_THEME[agent.name] ?? DEFAULT_THEME;
  render();

  function render() {
    const isRunning = agent.status === "running";
    const wantsAttention = agent.attention === true;
    card.className = [
      "agent-mini",
      isRunning ? "agent-mini--running" : "",
      wantsAttention ? "agent-mini--attention" : "",
    ].filter(Boolean).join(" ");
    card.dataset.agentName = agent.name;
    card.style.setProperty("--agent-color", theme.color);

    // Presence dot — additive layer, only meaningful while a process
    // actually exists. Never rendered for a stopped agent (that would be
    // a fifth phantom state; see docs/presence-dots-spec.md non-goals).
    let presenceDot = "";
    let workLine = "";
    if (isRunning) {
      const dotState = deriveDotState(agent);
      const tooltip = dotTooltip(dotState, agent);
      presenceDot = `<span class="agent-mini__presence agent-mini__presence--${dotState}" title="${tooltip}"></span>`;

      // Current-work summary line (aperture-nr65b). null = no data available
      // (agent stopped — can't happen here since isRunning, or the backend's
      // bd query failed this cycle) — render nothing, same as before this
      // feature existed.
      const summary = deriveWorkSummary(agent, dotState);
      if (summary) {
        workLine = `<div class="agent-mini__work" title="${escapeHtml(summary.tooltip)}">${escapeHtml(summary.text)}</div>`;
      }
    }

    card.innerHTML = `
      <div class="agent-mini__row">
        <span class="agent-mini__icon">${theme.icon}${presenceDot}</span>
        <span class="agent-mini__name">${agent.name}</span>
        ${wantsAttention ? `<span class="agent-mini__badge" title="Agent has a message — open their tmux window to read it">●</span>` : ""}
        <span class="agent-mini__model">${agent.model}</span>
        <button class="agent-mini__config" title="Configure">⚙</button>
        <button class="agent-mini__toggle" title="${isRunning ? "Stop" : "Start"}">
          ${isRunning ? "■" : "▶"}
        </button>
      </div>
      ${workLine}
    `;

    card.addEventListener("click", async () => {
      // Clicking the row clears the attention badge — the operator is
      // acknowledging they'll go look at the agent's tmux window.
      if (wantsAttention) {
        try { await commands.clearAttention(agent.name); } catch (_) { /* ignore */ }
      }
      // If the agent is running, also focus its tmux window so any
      // already-attached terminal jumps to it.
      if (isRunning && agent.tmux_window_id) {
        try { await commands.tmuxSelectWindow(agent.tmux_window_id); } catch (_) { /* ignore */ }
      }
      onUpdate();
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
