import "./style.css";
import "@xterm/xterm/css/xterm.css";
import { createNavbar } from "./components/Navbar";
import { createAgentList } from "./components/AgentList";
import { createMessageLog } from "./components/MessageLog";
import { createChatPanel } from "./components/ChatPanel";
import { createWarRoom } from "./components/WarRoom";
import { createBeadsPanel } from "./components/BeadsPanel";
import { createSpiderlingsPanel } from "./components/SpiderlingsPanel";
import { createObjectivesKanban } from "./components/ObjectivesKanban";
import { createTerminal } from "./components/Terminal";
import { commands } from "./services/tauri-commands";

const SESSION_NAME = "aperture";

async function init() {
  const navbarTitle = document.getElementById("navbar-title")!;
  const navbarActions = document.getElementById("navbar-actions")!;
  const navbarViews = document.getElementById("navbar-views")!;
  const rightPanel = document.getElementById("right-panel")!;
  const panelChat = document.getElementById("panel-chat")!;
  const panelWarroom = document.getElementById("panel-warroom")!;
  const panelMessages = document.getElementById("panel-messages")!;
  const panelBeads = document.getElementById("panel-beads")!;
  const panelSpiders = document.getElementById("panel-spiders")!;
  const sidebarAgents = document.getElementById("sidebar-agents")!;
  const terminalEl = document.getElementById("terminal-container")!;
  const objectivesEl = document.getElementById("objectives-container")!;

  const resizeHandle = document.getElementById("resize-handle")!;
  let activePanel: string | null = null;
  // ── Main Area View Toggle (Terminal / Objectives) ──
  function switchView(view: string) {
    terminalEl.classList.toggle("hidden", view !== "terminal");
    objectivesEl.classList.toggle("hidden", view !== "objectives");

    navbarViews.querySelectorAll(".navbar__view-btn").forEach((btn) => {
      const v = (btn as HTMLElement).dataset.view;
      btn.classList.toggle("navbar__view-btn--active", v === view);
    });

    window.dispatchEvent(new Event("resize"));
  }

  navbarViews.querySelectorAll(".navbar__view-btn").forEach((btn) => {
    btn.addEventListener("click", () => {
      const view = (btn as HTMLElement).dataset.view!;
      switchView(view);
    });
  });

  // ── Right Panel Toggle ──
  function togglePanel(panel: string) {
    if (activePanel === panel) {
      rightPanel.classList.add("hidden");
      resizeHandle.classList.add("hidden");
      activePanel = null;
    } else {
      rightPanel.classList.remove("hidden");
      resizeHandle.classList.remove("hidden");
      panelChat.classList.toggle("hidden", panel !== "chat");
      panelWarroom.classList.toggle("hidden", panel !== "warroom");
      panelMessages.classList.toggle("hidden", panel !== "messages");
      panelBeads.classList.toggle("hidden", panel !== "beads");
      panelSpiders.classList.toggle("hidden", panel !== "spiders");
      activePanel = panel;
    }

    navbarActions.querySelectorAll(".navbar__btn").forEach((btn) => {
      const p = (btn as HTMLElement).dataset.panel;
      btn.classList.toggle("navbar__btn--active", p === activePanel);
    });

    window.dispatchEvent(new Event("resize"));
  }

  // ── Auto-open BEADS panel when objective is clicked ──
  window.addEventListener("objective-selected", () => {
    if (activePanel !== "beads") {
      togglePanel("beads");
    }
  });

  // Drag-to-resize right panel
  let isResizing = false;
  resizeHandle.addEventListener("mousedown", (e) => {
    e.preventDefault();
    isResizing = true;
    document.body.style.cursor = "col-resize";
    document.body.style.userSelect = "none";
  });

  document.addEventListener("mousemove", (e) => {
    if (!isResizing) return;
    const contentRect = document.getElementById("content")!.getBoundingClientRect();
    const newWidth = contentRect.right - e.clientX;
    const clamped = Math.max(200, Math.min(600, newWidth));
    rightPanel.style.width = `${clamped}px`;
    rightPanel.style.minWidth = `${clamped}px`;
    window.dispatchEvent(new Event("resize"));
  });

  document.addEventListener("mouseup", () => {
    if (isResizing) {
      isResizing = false;
      document.body.style.cursor = "";
      document.body.style.userSelect = "";
      window.dispatchEvent(new Event("resize"));
    }
  });

  // Navbar
  const navbar = createNavbar(navbarTitle, navbarActions, togglePanel);

  // Create/attach tmux session
  try {
    await commands.tmuxCreateSession(SESSION_NAME);
    navbar.setConnected(true);
  } catch (e) {
    console.error("Failed to create tmux session:", e);
    navbar.setConnected(false);
    terminalEl.innerHTML = `
      <div style="padding:2rem;color:#e74c3c;font-family:monospace;">
        <h3 style="margin-bottom:0.5rem;">Terminal connection failed</h3>
        <p style="color:#999;">${e instanceof Error ? e.message : String(e)}</p>
        <p style="color:#666;margin-top:0.5rem;font-size:12px;">
          Check that tmux is installed at /opt/homebrew/bin/tmux
        </p>
      </div>`;
    return;
  }

  // Sidebar
  const agentList = createAgentList(sidebarAgents);

  // Active agent indicator (lower-left sidebar)
  const activeAgentEl = document.getElementById("sidebar-active-agent")!;
  window.addEventListener("agent-focused", (e) => {
    const { name, color } = (e as CustomEvent).detail;
    activeAgentEl.innerHTML = `
      <span class="sidebar-active-agent__label">viewing</span>
      <span class="sidebar-active-agent__badge" style="background:${color}">${name}</span>
    `;
  });

  // Right panels
  createChatPanel(panelChat);
  createWarRoom(panelWarroom);
  createMessageLog(panelMessages);
  createBeadsPanel(panelBeads);
  createSpiderlingsPanel(panelSpiders);

  // Main area views
  await createTerminal(terminalEl, SESSION_NAME);
  createObjectivesKanban(objectivesEl);

  setInterval(() => agentList.refresh(), 3000);
}

init();
