import "./style.css";
import "./teams.css";
import { createTeamsArea } from "./components/TeamsArea";
import { createNavbar } from "./components/Navbar";
import { createAgentList } from "./components/AgentList";
import { createFooter } from "./components/Footer";
import { commands } from "./services/tauri-commands";

const SESSION_NAME = "aperture";

async function init() {
  const navbarTitle = document.getElementById("navbar-title")!;
  const sidebarAgents = document.getElementById("sidebar-agents")!;
  const sidebarFooter = document.getElementById("sidebar-footer")!;

  // Top-of-sidebar logo + connection dot.
  const navbar = createNavbar(navbarTitle);

  // Bootstrap the shared tmux session that every agent attaches into.
  // The operator opens agent windows themselves (e.g. `tmux attach -t
  // aperture` then switch windows, or click an agent in the sidebar to
  // focus its window in an already-attached terminal).
  try {
    await commands.tmuxCreateSession(SESSION_NAME);
    navbar.setConnected(true);
  } catch (e) {
    console.error("Failed to create tmux session:", e);
    navbar.setConnected(false);
  }

  // The agent launcher — the only feature the panel offers.
  const agentList = createAgentList(sidebarAgents);
  const teamHost = document.createElement("div");
  teamHost.id = "teams-area";
  sidebarAgents.before(teamHost);
  createTeamsArea(teamHost, sidebarAgents, {
    listAgents: commands.listAgents,
    onTeamSeats: names => agentList.setTeamSeats(names),
    openAgent: async name => {
      const agent = (await commands.listAgents()).find(a => a.name === name);
      if (!agent?.tmux_window_id) throw new Error("No current window");
      await commands.tmuxSelectWindow(agent.tmux_window_id);
    },
  });

  // Bottom-of-sidebar version line (semver · git SHA · build date).
  // Fire-and-forget: if it fails the launcher still works.
  void createFooter(sidebarFooter);

  // Refresh status + attention badges every 3s.
  setInterval(() => agentList.refresh(), 3000);
}

init();
