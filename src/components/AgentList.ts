import type { AgentDef } from "../types";
import { commands } from "../services/tauri-commands";
import { createAgentCard } from "./AgentCard";
import type { CardLifecycle } from "./AgentCard";
import { createAgentConfigModal } from "./AgentConfigModal";
import { deriveDotState, deriveStateChip } from "../services/hub-presence";

type PendingOp = NonNullable<AgentDef["op_pending"]>;

const LIFECYCLE_ERROR_TTL_MS = 15_000;

const OP_VERB: Record<PendingOp, string> = {
  starting: "start",
  stopping: "stop",
  restarting: "restart",
};

function errMsg(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

function clock(): string {
  // HH:MM:SS, locale-independent.
  return new Date().toTimeString().slice(0, 8);
}

export function createAgentList(container: HTMLElement) {
  // ── Error strip (aperture-ull4y) ──
  // Sits above the list. Two message sources, latest set wins:
  //   - backend: list_agents failing. Stamped with the FIRST failure time and
  //     kept stable across repeated failures; never auto-cleared while the
  //     poll is still failing — only a successful poll clears it.
  //   - lifecycle: a start/stop/restart rejection. Auto-clears after 15s,
  //     falling back to the backend message if that is still active.
  const strip = document.createElement("div");
  strip.className = "agent-list__strip";
  strip.hidden = true;
  container.appendChild(strip);

  const wrapper = document.createElement("div");
  wrapper.className = "agent-list";
  container.appendChild(wrapper);

  let backendMsg: string | null = null;
  let lifecycleMsg: string | null = null;
  let lifecycleTimer: ReturnType<typeof setTimeout> | null = null;
  let stripSource: "backend" | "lifecycle" | null = null;

  function paintStrip() {
    // Resolve the source to show: the most recently set one, falling back
    // to the other if it has since been cleared.
    let source = stripSource;
    if (source === "lifecycle" && lifecycleMsg == null) source = backendMsg ? "backend" : null;
    if (source === "backend" && backendMsg == null) source = lifecycleMsg ? "lifecycle" : null;
    const msg = source === "lifecycle" ? lifecycleMsg : source === "backend" ? backendMsg : null;
    strip.hidden = msg == null;
    strip.textContent = msg ?? "";
    strip.className = `agent-list__strip${source ? ` agent-list__strip--${source}` : ""}`;
  }

  function showLifecycleError(msg: string) {
    lifecycleMsg = msg;
    stripSource = "lifecycle";
    if (lifecycleTimer) clearTimeout(lifecycleTimer);
    lifecycleTimer = setTimeout(() => {
      lifecycleTimer = null;
      lifecycleMsg = null;
      stripSource = backendMsg ? "backend" : null;
      paintStrip();
    }, LIFECYCLE_ERROR_TTL_MS);
    paintStrip();
  }

  function setBackendDown(err: unknown) {
    console.error("Failed to list agents:", err);
    if (backendMsg == null) {
      // First failure: stamp the time once; later failures keep it.
      backendMsg = `Backend unreachable since ${clock()} — showing last known state`;
      stripSource = "backend";
    }
    wrapper.classList.add("agent-list--stale");
    paintStrip();
  }

  function setBackendUp() {
    if (backendMsg != null) {
      backendMsg = null;
      if (stripSource === "backend") stripSource = lifecycleMsg ? "lifecycle" : null;
    }
    wrapper.classList.remove("agent-list--stale");
    paintStrip();
  }

  // ── Per-agent in-flight lock (aperture-ull4y) ──
  // Merged into each agent's `op_pending` at render time so the card can
  // lock its buttons and show a spinner across polls. Cleared when the
  // invoke settles either way.
  const pendingOps = new Map<string, PendingOp>();

  // Last successful list_agents result. Re-rendered from cache (no fetch)
  // when a pending op flips, so the spinner shows even if the backend is
  // blocked inside a multi-second boot and list_agents is queued behind it.
  let lastAgents: AgentDef[] = [];
  let lastAgentHash = "";
  let isBulkToggling = false;

  const modal = createAgentConfigModal(() => refresh());

  async function runOp(name: string, op: PendingOp): Promise<void> {
    if (pendingOps.has(name)) return; // already in flight — ignore the re-click
    pendingOps.set(name, op);
    render();
    try {
      if (op === "starting") await commands.startAgent(name);
      else if (op === "stopping") await commands.stopAgent(name);
      else await commands.restartAgent(name);
    } catch (err) {
      console.error(`Failed to ${OP_VERB[op]} agent ${name}:`, err);
      showLifecycleError(`${OP_VERB[op]} failed for ${name}: ${errMsg(err)}`);
      throw err;
    } finally {
      pendingOps.delete(name);
      render();
    }
  }

  const lifecycle: CardLifecycle = {
    start: (name) => { runOp(name, "starting").catch(() => {}).then(refresh); },
    stop: (name) => { runOp(name, "stopping").catch(() => {}).then(refresh); },
    restart: (name) => { runOp(name, "restarting").catch(() => {}).then(refresh); },
  };

  async function bulkToggle(agents: AgentDef[], allRunning: boolean) {
    isBulkToggling = true;
    render();
    // Yield to let the browser paint the spinner before kicking off Tauri calls
    await new Promise(r => requestAnimationFrame(r));
    const targets = allRunning ? agents : agents.filter(a => a.status !== "running");
    // Each op reports its own rejection into the strip (runOp) and carries
    // its own op_pending entry; allSettled just waits for the fleet.
    await Promise.allSettled(targets.map(a => runOp(a.name, allRunning ? "stopping" : "starting")));
    isBulkToggling = false;
    await refresh();
  }

  /** Rebuild the DOM from the cached agent list when anything render-
   *  relevant changed. Frontend-local state (op_pending, bulk spinner) is
   *  part of the hash so it triggers a rebuild immediately, not on the
   *  next poll. */
  function render() {
    const now = Date.now();
    const agents: AgentDef[] = lastAgents.map(a => ({
      ...a,
      op_pending: pendingOps.get(a.name) ?? null,
    }));

    // Build a hash of current state to detect changes. Include attention so
    // the badge appears/disappears without waiting for status/model changes.
    // Also include the derived presence-dot state so a dot_state flip (or
    // the 60s stuck deadline the backend watchdog computes) triggers a
    // rebuild on the very next poll tick, and the current-work fields
    // (aperture-nr65b) so a fresh claim/close on BEADS shows up within one
    // poll cycle too — same self-healing mechanism as status/model,
    // nothing dot- or work-line-specific needed here.
    //
    // current_task_id is a 3-state field (undefined/null = no data, "" =
    // idle, non-empty = a real id) — `?? ""` would collapse "no data" and
    // "idle" into the same hash bucket, silently swallowing that
    // transition. Use JSON.stringify so null/undefined and "" hash
    // distinctly.
    //
    // The state-chip LABEL (not just kind) is hashed on purpose: while
    // booting/stuck it carries the live "{N}s" counter, so hashing it is
    // what makes the counter tick on every 3s poll without a timer.
    const hash = [isBulkToggling ? "bulk" : ""]
      .concat(agents.map(a =>
        `${a.name}:${a.status}:${a.model}:${a.attention ? 1 : 0}:${a.attention_reason ?? ""}:${deriveDotState(a)}:${deriveStateChip(a, now)?.label ?? ""}:${a.op_pending ?? ""}:${JSON.stringify(a.current_task_id)}:${a.current_task_title ?? ""}:${a.current_task_extra_count ?? ""}`))
      .join("|");

    // Only rebuild DOM if something actually changed
    if (hash === lastAgentHash) return;
    lastAgentHash = hash;
    wrapper.innerHTML = "";

    const allRunning = agents.every(a => a.status === "running");

    const header = document.createElement("div");
    header.className = "agent-list__header";
    header.innerHTML = `<h3 class="section-title">Agents</h3>`;

    const toggleAll = document.createElement("button");
    if (isBulkToggling) {
      toggleAll.disabled = true;
      toggleAll.innerHTML = `<span class="agent-list__spinner"></span> All`;
      toggleAll.className = "agent-list__toggle-all agent-list__toggle-all--loading";
    } else {
      toggleAll.className = `agent-list__toggle-all ${allRunning ? "agent-list__toggle-all--stop" : "agent-list__toggle-all--play"}`;
      toggleAll.title = allRunning ? "Stop all" : "Start all";
      toggleAll.textContent = allRunning ? "■ All" : "▶ All";
      toggleAll.addEventListener("click", () => { void bulkToggle(agents, allRunning); });
    }
    header.appendChild(toggleAll);
    wrapper.appendChild(header);

    agents.forEach((agent) => {
      wrapper.appendChild(createAgentCard(agent, modal, refresh, lifecycle));
    });
  }

  async function refresh() {
    let agents: AgentDef[];
    try {
      agents = await commands.listAgents();
    } catch (e) {
      // Keep the last-rendered cards, but say so: a dead backend must not
      // look like a healthy fleet.
      setBackendDown(e);
      return;
    }
    setBackendUp();
    const order = ["glados", "wheatley", "peppy", "izzy"];
    agents.sort((a, b) => {
      const ai = order.indexOf(a.name);
      const bi = order.indexOf(b.name);
      return (ai === -1 ? 999 : ai) - (bi === -1 ? 999 : bi);
    });
    lastAgents = agents;
    render();
  }

  refresh();
  return { refresh };
}
