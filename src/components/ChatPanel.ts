import { commands } from "../services/tauri-commands";
import type { ChatMessage } from "../types";

/** Lightweight markdown → HTML for agent messages. Handles the common cases. */
function renderMarkdown(src: string): string {
  // Escape HTML first to prevent injection, but preserve code blocks unescaped
  // Extract code blocks first (protect them from further processing)
  const codeBlocks: string[] = [];
  let processed = src.replace(/```([\w]*)\n?([\s\S]*?)```/g, (_m, _lang, c) => {
    const idx = codeBlocks.length;
    codeBlocks.push(`<pre><code>${c.trimEnd().replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;")}</code></pre>`);
    return `\x00CODE${idx}\x00`;
  });

  // Escape remaining HTML
  processed = processed.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");

  // GFM tables — match pipe-delimited blocks
  processed = processed.replace(
    /((?:^[^\n]*\|[^\n]*\n)+)/gm,
    (block) => {
      const rows = block.trim().split("\n");
      if (rows.length < 2) return block;
      const isSeparator = (r: string) => /^[\s|:\-]+$/.test(r);
      const sep = rows.findIndex(isSeparator);
      if (sep !== 1) return block; // not a valid table
      const head = `<thead><tr>${rows[0].split("|").map(c => c.trim()).filter(c => c).map(c => `<th>${c}</th>`).join("")}</tr></thead>`;
      const body = rows.slice(2).map(r =>
        `<tr>${r.split("|").map(c => c.trim()).filter(c => c).map(c => `<td>${c}</td>`).join("")}</tr>`
      ).join("");
      return `<table>${head}<tbody>${body}</tbody></table>`;
    }
  );

  // Inline code
  processed = processed.replace(/`([^`]+)`/g, "<code>$1</code>");
  // Headings
  processed = processed.replace(/^### (.+)$/gm, "<h3>$1</h3>");
  processed = processed.replace(/^## (.+)$/gm, "<h2>$1</h2>");
  processed = processed.replace(/^# (.+)$/gm, "<h1>$1</h1>");
  // Bold + italic
  processed = processed.replace(/\*\*\*(.+?)\*\*\*/g, "<strong><em>$1</em></strong>");
  processed = processed.replace(/\*\*(.+?)\*\*/g, "<strong>$1</strong>");
  processed = processed.replace(/\*(.+?)\*/g, "<em>$1</em>");
  // HR
  processed = processed.replace(/^---+$/gm, "<hr>");
  // Unordered lists
  processed = processed.replace(/((?:^[-*] .+\n?)+)/gm, (block) => {
    const items = block.trim().split("\n").map(l => `<li>${l.replace(/^[-*] /, "")}</li>`).join("");
    return `<ul>${items}</ul>`;
  });
  // Ordered lists
  processed = processed.replace(/((?:^\d+\. .+\n?)+)/gm, (block) => {
    const items = block.trim().split("\n").map(l => `<li>${l.replace(/^\d+\. /, "")}</li>`).join("");
    return `<ol>${items}</ol>`;
  });
  // Paragraphs
  processed = processed.split(/\n{2,}/)
    .map(block => block.trim())
    .filter(Boolean)
    .map(block => /^[\x00<]/.test(block) ? block : `<p>${block.replace(/\n/g, "<br>")}</p>`)
    .join("\n");

  // Restore code blocks
  processed = processed.replace(/\x00CODE(\d+)\x00/g, (_m, i) => codeBlocks[Number(i)]);

  return processed;
}

const AGENT_COLORS: Record<string, string> = {
  planner:  "#e67e22",
  glados:   "#9b59b6",
  wheatley: "#3498db",
  peppy:    "#1abc9c",
  izzy:     "#e91e63",
  vance:    "#ff6b9d",
  rex:      "#e74c3c",
  scout:    "#27ae60",
  cipher:   "#7f8c8d",
  sage:     "#17a589",
  atlas:    "#8e44ad",
  sentinel: "#34495e",
  sterling: "#d4af37",
};

const AGENTS = ["planner", "glados", "wheatley", "peppy", "izzy", "vance", "rex", "scout", "cipher", "sage", "atlas", "sentinel", "sterling"];

export function createChatPanel(container: HTMLElement) {
  let activeAgent = AGENTS[0];

  container.innerHTML = `
    <div class="chat">
      <div class="chat__header">
        <h3 class="section-title">Chat</h3>
        <button class="message-log__clear chat__clear-btn" title="Clear chat history">🗑</button>
      </div>
      <div class="chat__tabs"></div>
      <div class="chat__messages"></div>
      <div class="chat__input-row">
        <input class="chat__input" type="text" placeholder="Message..." />
        <button class="chat__send">↑</button>
      </div>
    </div>
  `;

  const tabsEl = container.querySelector(".chat__tabs")!;
  const messagesEl = container.querySelector(".chat__messages")!;
  const inputEl = container.querySelector(".chat__input") as HTMLInputElement;
  const sendBtn = container.querySelector(".chat__send")!;
  const clearBtn = container.querySelector(".chat__clear-btn")!;

  function renderTabs() {
    const color = AGENT_COLORS[activeAgent] ?? "#f39c12";
    container.style.setProperty("--chat-agent-color", color);

    tabsEl.innerHTML = AGENTS
      .map(
        (name) =>
          `<button class="chat__tab ${name === activeAgent ? "chat__tab--active" : ""}" data-agent="${name}" style="${name === activeAgent ? `color: ${AGENT_COLORS[name] ?? "#f39c12"}` : ""}">${name}</button>`
      )
      .join("");

    tabsEl.querySelectorAll(".chat__tab").forEach((tab) => {
      tab.addEventListener("click", () => {
        activeAgent = (tab as HTMLElement).dataset.agent!;
        renderTabs();
        poll();
      });
    });
  }

  function renderMessages(messages: ChatMessage[]) {
    // Filter to only show messages between operator and active agent
    const filtered = messages.filter(
      (m) =>
        (m.from === "operator" && m.to === activeAgent) ||
        (m.from === activeAgent && m.to === "operator")
    );

    // Check if user is near the bottom before re-rendering
    const wasAtBottom =
      messagesEl.scrollHeight - messagesEl.scrollTop - messagesEl.clientHeight < 40;

    messagesEl.innerHTML = filtered
      .map((m) => {
        const isMe = m.from === "operator";
        const body = isMe
          ? escapeHtml(m.content)
          : renderMarkdown(m.content);
        return `
          <div class="chat__msg ${isMe ? "chat__msg--me" : "chat__msg--agent"}">
            <div class="chat__msg-sender">${isMe ? "You" : m.from}</div>
            <div class="chat__msg-body${isMe ? "" : " chat__msg-body--md"}">${body}</div>
          </div>
        `;
      })
      .join("");

    // Only auto-scroll if user was already at the bottom
    if (wasAtBottom) {
      messagesEl.scrollTop = messagesEl.scrollHeight;
    }
  }

  function escapeHtml(s: string): string {
    return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
  }

  async function sendMessage() {
    const text = inputEl.value.trim();
    if (!text) return;
    inputEl.value = "";
    await commands.sendChat(activeAgent, text);
    poll();
  }

  sendBtn.addEventListener("click", sendMessage);
  inputEl.addEventListener("keydown", (e) => {
    if (e.key === "Enter") sendMessage();
  });

  clearBtn.addEventListener("click", async () => {
    await commands.clearChatHistory();
    messagesEl.innerHTML = "";
  });

  async function poll() {
    try {
      const messages = await commands.getChatMessages();
      renderMessages(messages);
    } catch {
      // Log might not exist yet
    }
  }

  renderTabs();
  poll();
  const interval = setInterval(poll, 2000);

  return {
    destroy() {
      clearInterval(interval);
    },
  };
}
