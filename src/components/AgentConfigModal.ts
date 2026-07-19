import type { AgentDef } from "../types";
import { commands } from "../services/tauri-commands";

// value = what the CLI accepts; label = what the operator sees.
// Claude aliases resolve to the current generation (fable → Fable 5, sonnet → Sonnet 5, opus → Opus 4.8).
const CLAUDE_MODELS = [
  { value: "fable", label: "fable 5" },
  { value: "sonnet", label: "sonnet 5" },
  { value: "opus", label: "opus 4.8" },
] as const;
// GPT-5.6 celestial family (live catalog 2026-07-19)
const CODEX_MODELS = [
  { value: "codex/gpt-5.6-sol", label: "gpt-5.6-sol" },
  { value: "codex/gpt-5.6-terra", label: "gpt-5.6-terra" },
  { value: "codex/gpt-5.6-luna", label: "gpt-5.6-luna" },
] as const;
const ALL_MODELS = [...CLAUDE_MODELS, ...CODEX_MODELS].map(m => m.value);

export interface AgentConfigModal {
  open: (agent: AgentDef) => void;
  close: () => void;
}

export function createAgentConfigModal(onSave: () => void): AgentConfigModal {
  const overlay = document.createElement("div");
  overlay.className = "agent-config-modal";
  overlay.setAttribute("aria-modal", "true");
  overlay.setAttribute("role", "dialog");
  overlay.innerHTML = `
    <div class="agent-config-modal__card">
      <div class="agent-config-modal__header">
        <span class="agent-config-modal__title">Agent Config</span>
        <button class="agent-config-modal__close" title="Close">✕</button>
      </div>
      <div class="agent-config-modal__body">
        <div class="agent-config-modal__row">
          <span class="agent-config-modal__label">Agent</span>
          <span class="agent-config-modal__agent-name"></span>
        </div>
        <div class="agent-config-modal__row">
          <span class="agent-config-modal__label">Role</span>
          <span class="agent-config-modal__agent-role"></span>
        </div>
        <div class="agent-config-modal__row">
          <span class="agent-config-modal__label">Model</span>
          <select class="agent-config-modal__select">
            <optgroup label="Claude">
              ${CLAUDE_MODELS.map(m => `<option value="${m.value}">${m.label}</option>`).join("")}
            </optgroup>
            <optgroup label="Codex">
              ${CODEX_MODELS.map(m => `<option value="${m.value}">${m.label}</option>`).join("")}
            </optgroup>
          </select>
        </div>
      </div>
      <div class="agent-config-modal__footer">
        <span class="agent-config-modal__status"></span>
        <button class="agent-config-modal__save">Save</button>
      </div>
    </div>
  `;
  document.body.appendChild(overlay);

  let currentAgent: AgentDef | null = null;

  const nameEl = overlay.querySelector<HTMLElement>(".agent-config-modal__agent-name")!;
  const roleEl = overlay.querySelector<HTMLElement>(".agent-config-modal__agent-role")!;
  const select = overlay.querySelector<HTMLSelectElement>(".agent-config-modal__select")!;
  const saveBtn = overlay.querySelector<HTMLButtonElement>(".agent-config-modal__save")!;
  const closeBtn = overlay.querySelector<HTMLButtonElement>(".agent-config-modal__close")!;
  const statusEl = overlay.querySelector<HTMLElement>(".agent-config-modal__status")!;

  function close() {
    overlay.classList.remove("agent-config-modal--visible");
    currentAgent = null;
  }

  function open(agent: AgentDef) {
    currentAgent = agent;
    nameEl.textContent = agent.name;
    roleEl.textContent = agent.role;
    select.value = (ALL_MODELS as readonly string[]).includes(agent.model) ? agent.model : "sonnet";
    statusEl.textContent = "";
    saveBtn.disabled = false;
    overlay.classList.add("agent-config-modal--visible");
  }

  closeBtn.addEventListener("click", close);
  overlay.addEventListener("click", (e) => {
    if (e.target === overlay) close();
  });

  saveBtn.addEventListener("click", async () => {
    if (!currentAgent) return;
    const model = select.value;
    saveBtn.disabled = true;
    statusEl.textContent = "Saving…";
    try {
      await commands.updateAgentModel(currentAgent.name, model);
      statusEl.textContent = "Saved!";
      setTimeout(() => {
        close();
        onSave();
      }, 500);
    } catch (err) {
      statusEl.textContent = `Error: ${err}`;
      saveBtn.disabled = false;
    }
  });

  return { open, close };
}
