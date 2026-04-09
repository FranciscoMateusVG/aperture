import { commands } from "../services/tauri-commands";
import type { WarRoomState, TranscriptEntry } from "../types";

const AGENT_THEME: Record<string, { icon: string; color: string }> = {
  planner: { icon: "📋", color: "#e67e22" },
  glados: { icon: "🤖", color: "#9b59b6" },
  wheatley: { icon: "💡", color: "#3498db" },
  peppy: { icon: "🚀", color: "#1abc9c" },
  izzy: { icon: "🧪", color: "#e91e63" },
  vance: { icon: "🎨", color: "#ff6b9d" },
  rex: { icon: "🗄️", color: "#e74c3c" },
  scout: { icon: "📱", color: "#27ae60" },
  cipher: { icon: "🔐", color: "#7f8c8d" },
  sage: { icon: "🌿", color: "#17a589" },
  atlas: { icon: "📚", color: "#8e44ad" },
  sentinel: { icon: "👁️", color: "#34495e" },
  sterling: { icon: "⭐", color: "#d4af37" },
};

const AGENT_NAMES = Object.keys(AGENT_THEME);

function escapeHtml(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
}

export function createWarRoom(container: HTMLElement) {
  let pollTimer: ReturnType<typeof setInterval> | null = null;
  let lastTranscriptLength = 0;

  renderSetup();
  checkInitialState();

  async function checkInitialState() {
    try {
      const state = await commands.getWarroomState();
      if (state) {

        if (state.status === "concluded") {
          const transcript = await commands.getWarroomTranscript();
          renderConcluded(transcript);
        } else {
          renderActive();
          startPolling();
        }
      }
    } catch {
      // No active war room, stay on setup
    }
  }

  async function renderSetup() {
    stopPolling();
    lastTranscriptLength = 0;

    const selected: string[] = [];

    container.innerHTML = `
      <div class="warroom">
        <div class="warroom__setup">
          <div class="section-title">War Room</div>
          <textarea class="warroom__topic-input" placeholder="Enter discussion topic..." rows="3"></textarea>
          <div class="section-title" style="margin-top:4px">Participants</div>
          <div class="warroom__agent-list">
            ${AGENT_NAMES.map(name => {
              const t = AGENT_THEME[name];
              return `
                <label class="warroom__agent-option" data-agent="${name}">
                  <input type="checkbox" value="${name}" />
                  <span class="warroom__order-badge"></span>
                  <span style="font-size:16px">${t.icon}</span>
                  <span style="color:${t.color};font-weight:600;text-transform:capitalize">${name}</span>
                </label>`;
            }).join("")}
          </div>
          <button class="warroom__start-btn" disabled>Start Discussion</button>
          <button class="warroom__history-btn">View Past Discussions</button>
        </div>
      </div>
    `;

    const topicInput = container.querySelector<HTMLTextAreaElement>(".warroom__topic-input")!;
    const startBtn = container.querySelector<HTMLButtonElement>(".warroom__start-btn")!;
    const agentListEl = container.querySelector<HTMLElement>(".warroom__agent-list")!;

    function updateStartBtn() {
      startBtn.disabled = topicInput.value.trim() === "" || selected.length < 2;
    }

    function updateOrderBadges() {
      container.querySelectorAll<HTMLElement>(".warroom__agent-option").forEach(label => {
        const name = label.dataset.agent!;
        const idx = selected.indexOf(name);
        const badge = label.querySelector<HTMLElement>(".warroom__order-badge");
        if (!badge) return;
        if (idx >= 0) {
          badge.textContent = String(idx + 1);
          badge.classList.add("warroom__order-badge--active");
          label.classList.add("warroom__agent-option--selected");
        } else {
          badge.textContent = "";
          badge.classList.remove("warroom__order-badge--active");
          label.classList.remove("warroom__agent-option--selected");
        }
      });
    }

    function wireCheckbox(label: HTMLElement) {
      const cb = label.querySelector<HTMLInputElement>("input[type='checkbox']")!;
      const agentName = label.dataset.agent!;
      cb.addEventListener("change", () => {
        if (cb.checked) {
          selected.push(agentName);
        } else {
          const idx = selected.indexOf(agentName);
          if (idx >= 0) selected.splice(idx, 1);
        }
        updateOrderBadges();
        updateStartBtn();
      });
    }

    // Wire permanent agents
    container.querySelectorAll<HTMLLabelElement>(".warroom__agent-option").forEach(wireCheckbox);

    // Fetch and append active spiderlings
    try {
      const spiderlings = await commands.listSpiderlings();
      const active = spiderlings.filter(s => s.status === "working");
      if (active.length > 0) {
        const divider = document.createElement("div");
        divider.className = "warroom__agent-divider";
        divider.textContent = "Spiderlings";
        agentListEl.appendChild(divider);

        active.forEach(s => {
          const label = document.createElement("label");
          label.className = "warroom__agent-option";
          label.dataset.agent = s.name;
          label.innerHTML = `
            <input type="checkbox" value="${escapeHtml(s.name)}" />
            <span class="warroom__order-badge"></span>
            <span style="font-size:16px">🕷️</span>
            <span style="color:var(--accent-orange);font-weight:600">${escapeHtml(s.name)}</span>
            <span style="color:var(--text-secondary);font-size:10px;margin-left:4px">(spiderling)</span>
          `;
          agentListEl.appendChild(label);
          wireCheckbox(label);
        });
      }
    } catch {
      // Spiderlings unavailable — not a blocker
    }

    topicInput.addEventListener("input", updateStartBtn);

    container.querySelector<HTMLButtonElement>(".warroom__history-btn")!
      .addEventListener("click", async () => {
        try {
          const rooms = await commands.listWarroomHistory();
          renderHistory(rooms);
        } catch (e) {
          console.error("Failed to load history:", e);
        }
      });

    startBtn.addEventListener("click", async () => {
      const topic = topicInput.value.trim();
      if (!topic || selected.length < 2) return;
      startBtn.disabled = true;
      startBtn.textContent = "Starting...";
      try {
        await commands.createWarroom(topic, [...selected]);
        renderActive();
        startPolling();
      } catch (e) {
        console.error("Failed to create war room:", e);
        startBtn.disabled = false;
        startBtn.textContent = "Start Discussion";
      }
    });
  }

  function renderActive() {
    container.innerHTML = `
      <div class="warroom">
        <div class="warroom__active">
          <div class="warroom__topic"></div>
          <div class="warroom__meta">
            <div class="warroom__participants"></div>
            <div class="warroom__round"></div>
          </div>
          <div class="warroom__transcript"></div>
          <div class="warroom__input-row">
            <input class="warroom__input" type="text" placeholder="Interject..." />
            <button class="warroom__send">\u{2191}</button>
          </div>
          <div class="warroom__controls">
            <span class="warroom__conclude-votes hidden"></span>
            <div class="warroom__invite-row hidden">
              <select class="warroom__invite-select"></select>
              <button class="btn warroom__invite-btn">Invite</button>
            </div>
            <button class="btn warroom__skip-btn">Skip</button>
            <button class="btn btn--danger warroom__conclude-btn">Conclude</button>
            <button class="btn warroom__cancel-btn">Cancel</button>
          </div>
        </div>
      </div>
    `;

    const inputEl = container.querySelector<HTMLInputElement>(".warroom__input")!;
    const sendBtn = container.querySelector<HTMLButtonElement>(".warroom__send")!;
    const skipBtn = container.querySelector<HTMLButtonElement>(".warroom__skip-btn")!;
    const concludeBtn = container.querySelector<HTMLButtonElement>(".warroom__conclude-btn")!;

    async function sendInterject() {
      const text = inputEl.value.trim();
      if (!text) return;
      inputEl.value = "";
      try {
        await commands.warroomInterject(text);
      } catch (e) {
        console.error("Failed to interject:", e);
      }
    }

    inputEl.addEventListener("keydown", (e) => {
      if (e.key === "Enter") {
        e.preventDefault();
        sendInterject();
      }
    });
    sendBtn.addEventListener("click", sendInterject);

    skipBtn.addEventListener("click", async () => {
      try { await commands.warroomSkip(); } catch (e) { console.error("Skip failed:", e); }
    });

    concludeBtn.addEventListener("click", async () => {
      try { await commands.warroomConclude(); } catch (e) { console.error("Conclude failed:", e); }
    });

    const cancelBtn = container.querySelector<HTMLButtonElement>(".warroom__cancel-btn")!;
    cancelBtn.addEventListener("click", async () => {
      if (!confirm("Cancel this War Room? The entire transcript will be permanently deleted.")) return;
      try {
        await commands.warroomCancel();
        stopPolling();
        renderSetup();
      } catch (e) {
        console.error("Cancel failed:", e);
      }
    });

    const inviteSelect = container.querySelector<HTMLSelectElement>(".warroom__invite-select")!;
    const inviteBtn = container.querySelector<HTMLButtonElement>(".warroom__invite-btn")!;

    inviteBtn.addEventListener("click", async () => {
      const name = inviteSelect.value;
      if (!name) return;
      inviteBtn.disabled = true;
      inviteBtn.textContent = "Inviting...";
      try {
        await commands.warroomInviteParticipant(name);
      } catch (e) {
        console.error("Invite failed:", e);
      }
      inviteBtn.disabled = false;
      inviteBtn.textContent = "Invite";
    });
  }

  async function updateInviteDropdown(state: WarRoomState) {
    const inviteRow = container.querySelector<HTMLElement>(".warroom__invite-row");
    const inviteSelect = container.querySelector<HTMLSelectElement>(".warroom__invite-select");
    if (!inviteRow || !inviteSelect) return;

    const eligible: { name: string; isSpider: boolean }[] = [];

    // Permanent agents not already in room
    for (const name of AGENT_NAMES) {
      if (!state.participants.includes(name)) {
        eligible.push({ name, isSpider: false });
      }
    }

    // Active spiderlings not already in room
    try {
      const spiderlings = await commands.listSpiderlings();
      for (const s of spiderlings) {
        if (s.status === "working" && !state.participants.includes(s.name)) {
          eligible.push({ name: s.name, isSpider: true });
        }
      }
    } catch { /* ignore */ }

    if (eligible.length === 0) {
      inviteRow.classList.add("hidden");
      return;
    }

    inviteRow.classList.remove("hidden");
    const currentVal = inviteSelect.value;
    inviteSelect.innerHTML = eligible.map(e =>
      `<option value="${escapeHtml(e.name)}">${escapeHtml(e.name)}${e.isSpider ? " (spiderling)" : ""}</option>`
    ).join("");
    // Preserve selection if still valid
    if (eligible.some(e => e.name === currentVal)) {
      inviteSelect.value = currentVal;
    }
  }

  function updateActiveView(state: WarRoomState, transcript: TranscriptEntry[]) {
    const topicEl = container.querySelector<HTMLElement>(".warroom__topic");
    const participantsEl = container.querySelector<HTMLElement>(".warroom__participants");
    const roundEl = container.querySelector<HTMLElement>(".warroom__round");
    const transcriptEl = container.querySelector<HTMLElement>(".warroom__transcript");

    if (!topicEl || !participantsEl || !roundEl || !transcriptEl) return;

    if (!topicEl.dataset.wired) {
      topicEl.dataset.wired = "1";
      topicEl.title = "Click to expand/collapse";
      topicEl.addEventListener("click", () => topicEl.classList.toggle("expanded"));
    }
    topicEl.textContent = state.topic;
    roundEl.textContent = `Round ${state.round}`;

    const votesEl = container.querySelector<HTMLElement>(".warroom__conclude-votes");
    if (votesEl) {
      const votes = state.conclude_votes ?? [];
      if (votes.length > 0) {
        votesEl.textContent = `⚑ ${votes.length}/${state.participants.length} want to conclude`;
        votesEl.classList.remove("hidden");
      } else {
        votesEl.classList.add("hidden");
      }
    }

    participantsEl.innerHTML = state.participants.map(name => {
      const theme = AGENT_THEME[name] ?? { icon: "🕷️", color: "var(--accent-orange)" };
      const isActive = name === state.current_agent;
      const activeClass = isActive ? " warroom__participant--active" : "";
      const style = isActive
        ? `background:${theme.color};border-color:${theme.color}`
        : "";
      return `<span class="warroom__participant${activeClass}" style="${style}">${theme.icon} ${escapeHtml(name)}</span>`;
    }).join("");

    const shouldScroll = transcriptEl.scrollHeight - transcriptEl.scrollTop - transcriptEl.clientHeight < 60;

    transcriptEl.innerHTML = transcript.map(entry => {
      const role = entry.role.toLowerCase();
      if (role === "system") {
        return `<div class="warroom__entry warroom__entry--system">${escapeHtml(entry.content)}</div>`;
      }
      if (role === "operator") {
        return `<div class="warroom__entry warroom__entry--operator">
          <div class="warroom__entry-role">Operator</div>
          <div class="warroom__entry-body">${escapeHtml(entry.content)}</div>
        </div>`;
      }
      // Agent message
      const theme = AGENT_THEME[role] ?? { icon: "🕷️", color: "var(--accent-orange)" };
      const color = theme.color;
      const icon = theme.icon + " ";
      return `<div class="warroom__entry warroom__entry--agent" style="border-color:${color}">
        <div class="warroom__entry-role" style="color:${color}">${icon}${escapeHtml(entry.role)}</div>
        <div class="warroom__entry-body">${escapeHtml(entry.content)}</div>
      </div>`;
    }).join("");

    if (shouldScroll || transcript.length !== lastTranscriptLength) {
      transcriptEl.scrollTop = transcriptEl.scrollHeight;
    }
    lastTranscriptLength = transcript.length;
  }

  function renderTranscriptHtml(transcript: TranscriptEntry[]): string {
    return transcript.map(entry => {
      const role = entry.role.toLowerCase();
      if (role === "system") {
        return `<div class="warroom__entry warroom__entry--system">${escapeHtml(entry.content)}</div>`;
      }
      if (role === "operator") {
        return `<div class="warroom__entry warroom__entry--operator">
          <div class="warroom__entry-role">Operator</div>
          <div class="warroom__entry-body">${escapeHtml(entry.content)}</div>
        </div>`;
      }
      const theme = AGENT_THEME[role] ?? { icon: "🕷️", color: "var(--accent-orange)" };
      const color = theme.color;
      const icon = theme.icon + " ";
      return `<div class="warroom__entry warroom__entry--agent" style="border-color:${color}">
        <div class="warroom__entry-role" style="color:${color}">${icon}${escapeHtml(entry.role)}</div>
        <div class="warroom__entry-body">${escapeHtml(entry.content)}</div>
      </div>`;
    }).join("");
  }

  function renderConcluded(transcript: TranscriptEntry[]) {
    stopPolling();

    container.innerHTML = `
      <div class="warroom">
        <div class="warroom__concluded">
          <h3>Discussion Concluded</h3>
          <button class="warroom__new-btn">New War Room</button>
        </div>
        <div class="warroom__transcript" style="flex:1;overflow-y:auto;padding:8px 0">
          ${renderTranscriptHtml(transcript)}
        </div>
      </div>
    `;

    container.querySelector<HTMLButtonElement>(".warroom__new-btn")!
      .addEventListener("click", () => renderSetup());
  }

  async function renderHistory(rooms: WarRoomState[]) {
    container.innerHTML = `
      <div class="warroom">
        <div class="section-title">Past War Rooms</div>
        <div class="warroom__history-list">
          ${rooms.length === 0 ? '<div style="color:var(--text-secondary);font-size:12px">No past discussions yet.</div>' :
            rooms.map(r => `
              <button class="warroom__history-item" data-id="${escapeHtml(r.id)}">
                <div class="warroom__history-topic">${escapeHtml(r.topic)}</div>
                <div class="warroom__history-meta">${r.participants.join(", ")} · Round ${r.round}</div>
              </button>
            `).join("")}
        </div>
        <button class="warroom__new-btn" style="margin-top:12px;width:100%">Back to Setup</button>
      </div>
    `;

    container.querySelectorAll(".warroom__history-item").forEach(btn => {
      btn.addEventListener("click", async () => {
        const id = (btn as HTMLElement).dataset.id!;
        try {
          const transcript = await commands.getWarroomHistoryTranscript(id);
          const room = rooms.find(r => r.id === id);
          renderHistoryTranscript(id, room?.topic ?? "Unknown", transcript);
        } catch (e) {
          console.error("Failed to load transcript:", e);
        }
      });
    });

    container.querySelector(".warroom__new-btn")!.addEventListener("click", () => renderSetup());
  }

  function renderHistoryTranscript(id: string, topic: string, transcript: TranscriptEntry[]) {
    container.innerHTML = `
      <div class="warroom">
        <div style="display:flex;align-items:center;gap:8px;margin-bottom:8px">
          <button class="warroom__back-btn">&larr;</button>
          <div class="warroom__topic" style="border:none;padding:0;margin:0">${escapeHtml(topic)}</div>
        </div>
        <div class="warroom__transcript" style="flex:1;overflow-y:auto;padding:8px 0">
          ${renderTranscriptHtml(transcript)}
        </div>
        <button class="warroom__export-btn" data-id="${escapeHtml(id)}" style="margin-top:8px">Export as Markdown</button>
      </div>
    `;

    container.querySelector(".warroom__back-btn")!.addEventListener("click", async () => {
      const rooms = await commands.listWarroomHistory();
      renderHistory(rooms);
    });

    container.querySelector(".warroom__export-btn")!.addEventListener("click", () => {
      const md = transcript.map(e => {
        if (e.role === "system") return `*${e.content}*\n`;
        return `**${e.role}**: ${e.content}\n`;
      }).join("\n");
      const blob = new Blob([`# War Room: ${topic}\n\n${md}`], { type: "text/markdown" });
      const url = URL.createObjectURL(blob);
      const a = document.createElement("a");
      a.href = url;
      a.download = `warroom-${id}.md`;
      a.click();
      URL.revokeObjectURL(url);
    });
  }

  function startPolling() {
    stopPolling();
    poll(); // immediate first poll
    pollTimer = setInterval(poll, 2000);
  }

  function stopPolling() {
    if (pollTimer !== null) {
      clearInterval(pollTimer);
      pollTimer = null;
    }
  }

  async function poll() {
    try {
      const [state, transcript] = await Promise.all([
        commands.getWarroomState(),
        commands.getWarroomTranscript(),
      ]);

      if (!state) {
        // War room was cleared externally
        renderSetup();
        return;
      }

      if (state.status === "concluded") {
        renderConcluded(transcript);
        return;
      }

      updateActiveView(state, transcript);
      updateInviteDropdown(state);
    } catch (e) {
      console.error("War room poll error:", e);
    }
  }
}
