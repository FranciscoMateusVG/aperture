import type { AgentDef } from "../types";
import { commands } from "../services/tauri-commands";
import { escapeHtml } from "../utils/html";

// value = what the CLI accepts; label = what the operator sees.
// Claude aliases resolve to the current generation (fable → Fable 5, sonnet → Sonnet 5, haiku → Haiku 4.5, opus → Opus 4.8).
//
// This list is the single source of the Claude aliases the launcher offers.
// The backend validator (agents.rs::is_valid_model) must accept exactly these;
// its `picker_and_validator_agree` test parses the `value:` entries below at
// test time, so adding/removing an alias here without updating the Rust side
// fails `cargo test` rather than silently 400-ing on Save.
const CLAUDE_MODELS = [
  { value: "fable", label: "fable 5" },
  { value: "sonnet", label: "sonnet 5" },
  { value: "haiku", label: "haiku 4.5" },
  { value: "opus", label: "opus 4.8" },
] as const;
// GPT-5.6 celestial family (live catalog 2026-07-19); gpt-6-astra added
// 2026-09-06 as the new default Codex model (source: operator's
// ~/.codex/config.toml `model = "gpt-6-astra"`). First entry = default.
const CODEX_MODELS = [
  { value: "codex/gpt-6-astra", label: "gpt-6-astra" },
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
      <div class="agent-config-modal__restart hidden">
        <span class="agent-config-modal__restart-text"></span>
        <span class="agent-config-modal__restart-status"></span>
        <div class="agent-config-modal__restart-actions">
          <button class="agent-config-modal__restart-later">Later</button>
          <button class="agent-config-modal__restart-now">Restart now</button>
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
  const footerEl = overlay.querySelector<HTMLElement>(".agent-config-modal__footer")!;
  const restartEl = overlay.querySelector<HTMLElement>(".agent-config-modal__restart")!;
  const restartTextEl = overlay.querySelector<HTMLElement>(".agent-config-modal__restart-text")!;
  const restartStatusEl = overlay.querySelector<HTMLElement>(".agent-config-modal__restart-status")!;
  const restartNowBtn = overlay.querySelector<HTMLButtonElement>(".agent-config-modal__restart-now")!;
  const laterBtn = overlay.querySelector<HTMLButtonElement>(".agent-config-modal__restart-later")!;

  function close() {
    overlay.classList.remove("agent-config-modal--visible");
    currentAgent = null;
  }

  // The save footer and the restart prompt are mutually exclusive: the
  // prompt swaps in after a successful save on a running agent, and open()
  // always puts the footer back.
  function hideRestartPrompt() {
    restartEl.classList.add("hidden");
    footerEl.classList.remove("hidden");
    restartTextEl.textContent = "";
    restartStatusEl.textContent = "";
  }

  function showRestartPrompt(agent: AgentDef, oldModel: string, newModel: string) {
    restartTextEl.innerHTML =
      `Saved. <strong>${escapeHtml(agent.name)}</strong> is still running on ` +
      `<code>${escapeHtml(oldModel)}</code> — restart now to apply <code>${escapeHtml(newModel)}</code>?`;
    restartStatusEl.textContent = "";
    restartNowBtn.disabled = false;
    laterBtn.disabled = false;
    footerEl.classList.add("hidden");
    restartEl.classList.remove("hidden");
  }

  function open(agent: AgentDef) {
    currentAgent = agent;
    nameEl.textContent = agent.name;
    roleEl.textContent = agent.role;
    select.value = (ALL_MODELS as readonly string[]).includes(agent.model) ? agent.model : "sonnet";
    statusEl.textContent = "";
    saveBtn.disabled = false;
    hideRestartPrompt();
    overlay.classList.add("agent-config-modal--visible");
  }

  closeBtn.addEventListener("click", close);
  overlay.addEventListener("click", (e) => {
    if (e.target === overlay) close();
  });

  saveBtn.addEventListener("click", async () => {
    // Snapshot: close() nulls currentAgent, and the await below may outlive
    // the modal if the operator dismisses it mid-save.
    const agent = currentAgent;
    if (!agent) return;
    const model = select.value;
    if (model === agent.model) {
      close();
      return;
    }
    saveBtn.disabled = true;
    statusEl.textContent = "Saving…";
    try {
      await commands.updateAgentModel(agent.name, model);
    } catch (err) {
      statusEl.textContent = `Error: ${err}`;
      saveBtn.disabled = false;
      return;
    }
    // update_agent_model only persists (agents.rs) — a running agent keeps
    // the old model until its next boot, so offer the restart here.
    if (agent.status === "running") {
      statusEl.textContent = "";
      showRestartPrompt(agent, agent.model, model);
      return;
    }
    statusEl.textContent = "Saved — applies on next start.";
    setTimeout(() => {
      close();
      onSave();
    }, 500);
  });

  laterBtn.addEventListener("click", () => {
    close();
    onSave();
  });

  restartNowBtn.addEventListener("click", async () => {
    const agent = currentAgent;
    if (!agent) return;
    restartNowBtn.disabled = true;
    laterBtn.disabled = true;
    restartStatusEl.textContent = "Restarting…";
    try {
      await commands.restartAgent(agent.name);
      if (currentAgent === agent) close();
      onSave();
    } catch (err) {
      // Only touch the modal if it's still showing this agent.
      if (currentAgent !== agent) return;
      restartStatusEl.textContent = `Error: ${err}`;
      restartNowBtn.disabled = false;
      laterBtn.disabled = false;
    }
  });

  return { open, close };
}
