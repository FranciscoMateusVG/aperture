import { commands } from "../services/tauri-commands";
import type { AgentMessage } from "../types";

interface Conversation {
  key: string;
  label: string;
  messages: AgentMessage[];
}

export function createMessageLog(container: HTMLElement) {
  const wrapper = document.createElement("div");
  wrapper.className = "message-log";
  wrapper.innerHTML = `
    <div class="message-log__header">
      <h3 class="section-title">Messages</h3>
      <button class="message-log__clear" title="Clear history">🗑</button>
    </div>
    <div class="message-log__conversations"></div>
  `;
  container.appendChild(wrapper);
  const conversationsEl = wrapper.querySelector(".message-log__conversations")!;

  wrapper.querySelector(".message-log__clear")!.addEventListener("click", async () => {
    await commands.clearMessageHistory();
    conversationsEl.innerHTML = "";
  });

  // Track which conversations are collapsed
  const collapsed = new Set<string>();

  function getConversationKey(a: string, b: string): string {
    return [a, b].sort().join(":");
  }

  function groupByConversation(messages: AgentMessage[]): Conversation[] {
    const map = new Map<string, Conversation>();

    for (const m of messages) {
      const key = getConversationKey(m.from_agent, m.to_agent);
      if (!map.has(key)) {
        const [a, b] = key.split(":");
        map.set(key, {
          key,
          label: `${a} ↔ ${b}`,
          messages: [],
        });
      }
      map.get(key)!.messages.push(m);
    }

    return Array.from(map.values());
  }

  function renderConversations(messages: AgentMessage[]) {
    const conversations = groupByConversation(messages);

    conversationsEl.innerHTML = conversations
      .map((conv) => {
        const isCollapsed = collapsed.has(conv.key);
        const msgCount = conv.messages.length;
        const messagesHtml = isCollapsed
          ? ""
          : conv.messages
              .map(
                (m) => `
            <div class="message-log__item">
              <div class="message-log__sender">${m.from_agent}</div>
              <div class="message-log__body">${escapeHtml(m.content)}</div>
            </div>
          `
              )
              .join("");

        return `
        <div class="message-log__conv">
          <div class="message-log__conv-row">
            <button class="message-log__conv-header" data-key="${conv.key}">
              <span class="message-log__conv-arrow">${isCollapsed ? "▶" : "▼"}</span>
              <span class="message-log__conv-label">${conv.label}</span>
              <span class="message-log__conv-count">${msgCount}</span>
            </button>
            <button class="message-log__conv-clear" data-conv-key="${conv.key}" title="Clear this conversation">🗑</button>
          </div>
          <div class="message-log__conv-messages">${messagesHtml}</div>
        </div>
      `;
      })
      .join("");

    // Bind toggle handlers
    conversationsEl.querySelectorAll(".message-log__conv-header").forEach((btn) => {
      btn.addEventListener("click", () => {
        const key = (btn as HTMLElement).dataset.key!;
        if (collapsed.has(key)) {
          collapsed.delete(key);
        } else {
          collapsed.add(key);
        }
        renderConversations(messages);
      });
    });

    // Bind per-conversation clear handlers
    conversationsEl.querySelectorAll(".message-log__conv-clear").forEach((btn) => {
      btn.addEventListener("click", async (e) => {
        e.stopPropagation();
        const convKey = (btn as HTMLElement).dataset.convKey!;
        const [a, b] = convKey.split(":");
        await commands.clearConversationHistory(a, b);
        poll();
      });
    });
  }

  function escapeHtml(s: string): string {
    return s
      .replace(/&/g, "&amp;")
      .replace(/</g, "&lt;")
      .replace(/>/g, "&gt;");
  }

  async function poll() {
    try {
      const messages = await commands.getRecentMessages();
      renderConversations(messages);
    } catch {
      // Log might not exist yet
    }
  }

  poll();
  const interval = setInterval(poll, 3000);

  return {
    destroy() {
      clearInterval(interval);
    },
  };
}
