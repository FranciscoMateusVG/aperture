import { stopSeatSchema, parseStopSeatReady } from "./team-stop.js";
import { claudeStartupSmokeSchema, parseClaudeStartupSmoke } from "./team-claude-smoke.js";
import { bootstrapSeatSchema, bootstrapSelection, parseBootstrapStarted, parseTeamList } from "./team-bootstrap.js";
import { createTeamSchema, parseCreatedTeam } from "./team-create.js";
import { parseRepositoryRegistry, parseSavedRepository, saveRepositorySchema } from "./team-repositories.js";
import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { z } from "zod";
import { resolve } from "node:path";
import { homedir } from "node:os";
import { MailboxStore } from "./store.js";
import { MessageQueue } from "./message-queue.js";
import { formatUpdateAck, formatCloseAck, createTask, updateTask, closeTask, queryTasks, storeArtifact, searchTasks, createMessage, getUnreadMessages, formatUnreadMessages, UNREAD_LIMIT, markMessageRead, extractTaskId } from "./beads.js";
import { notifyHub } from "./hub-notify.js";
import { presenceReport, describePresence, type PresenceReport } from "./presence-snapshot.js";
import { buildIndex, recall, recallFull, recallStats, RECALL_K_MAX, RECALL_FULL_MAX_BYTES } from "./memory-index.js";
import { authorizeMessage, hasAuthenticatedMcpIdentity, loadSeatRegistry } from "./seat-registry.js";
import {
  assertActivationMatchesPending,
  assertAuthorizedEpic,
  invokeTeamControl,
  parsePendingList,
} from "./team-control.js";

const AGENT_NAME = process.env.AGENT_NAME;
if (!AGENT_NAME) {
  console.error("AGENT_NAME environment variable is required");
  process.exit(1);
}

// Authenticate the canonical enabled principal before deriving ANY path from
// AGENT_NAME. In particular, MailboxStore.ensureMailbox and MessageQueue.start
// create directories/files; a malformed ../-style identity must exit without
// touching APERTURE_MAILBOX, HOME, or the send-queue tree.
if (!hasAuthenticatedMcpIdentity(AGENT_NAME)) {
  console.error("AGENT_NAME is not an authenticated enabled registry principal");
  process.exit(1);
}

const agentRole = process.env.AGENT_ROLE ?? "agent";
const agentModel = process.env.AGENT_MODEL ?? "unknown";
const mailboxDir = process.env.APERTURE_MAILBOX; // optional override

const store = new MailboxStore(mailboxDir);
store.ensureMailbox(AGENT_NAME);

// aperture-ktwoy — durable fire-and-forget queue for agent-to-agent messages.
// send_message enqueues + returns instantly; this background worker flushes to
// BEADS (createMessage), retrying on failure and replaying persisted messages
// on restart. Only send_message is queued (it is read-after-write-safe — the
// recipient receives it via hub push / unread replay); all task writes stay
// synchronous.
const sendQueue = new MessageQueue({
  queueFilePath: resolve(homedir(), ".aperture", "send-queue", `${AGENT_NAME}.jsonl`),
  flush: async (m) => {
    // createMessage throws on bd failure → the queue keeps the message and
    // retries. It NEVER falls back to a divergent local store (split-brain).
    const result = await createMessage(m.from, m.to, m.content);
    // Comms-layer v2: best-effort push to the WS hub so a connected recipient
    // gets the message immediately. notifyHub never throws and resolves within
    // 1500ms with the hub's delivery outcome (forwarded / codex / offline /
    // unacked). Returning it lets the queue log the truth (aperture-oeb6q);
    // an offline/unacked outcome is NOT a failure — the BEADS row exists and
    // the hub's unread replay on reconnect covers it.
    const id = extractTaskId(result) ?? "";
    const preview = m.content.slice(0, 60).replace(/\n/g, " ");
    const outcome = await notifyHub({ to: m.to, id, from: m.from, preview });
    return { id, outcome };
  },
});
sendQueue.start();

const server = new McpServer({
  name: "aperture-bus",
  version: "1.0.0",
});

// Decommissioned 2026-07-19. Kept only so a message addressed to one of them
// gets a routing hint instead of a bare "unknown recipient".
const RETIRED_RECIPIENTS = ["sage", "atlas", "sterling"];
const RETIRED_HINT = "sage/atlas/sterling were retired 2026-07-19 — route SEO/content to vance, docs to the implementing agent, QA sign-off to izzy.";

// ── Messaging ──

server.tool(
  "send_message",
  "Send a message to an enabled registry seat or the human operator. Routing is authorized from the current trusted seat/team registry; enabled offline seats receive unread replay on their next session. Use 'operator' only as a one-way human doorbell.",
  { to: z.string().describe("Exact canonical recipient seat id, or operator"), message: z.string().describe("Message content. NOTE: avoid literal XML/HTML close-tag patterns like `</message>`, `</reason>` inside the body — they can be misread as parameter terminators by the tool-argument wire format. Use `&lt;/...&gt;` or paraphrase.") },
  async ({ to, message }) => {
    const target = to;
    if (!hasAuthenticatedMcpIdentity(AGENT_NAME)) {
      return {
        content: [{ type: "text", text: "ERROR: E_CROSS_TEAM_DENIED unknown_sender" }],
        isError: true,
      };
    }
    const registry = loadSeatRegistry();
    const authorization = authorizeMessage(registry, AGENT_NAME, target);

    if (!authorization.allowed) {
      const hint = RETIRED_RECIPIENTS.includes(target) ? `\n${RETIRED_HINT}` : "";
      return {
        content: [{
          type: "text",
          text: `ERROR: ${authorization.code} ${authorization.reason}.${hint}`,
        }],
        isError: true,
      };
    }

    if (target === AGENT_NAME) {
      return {
        content: [{
          type: "text",
          text: "ERROR: You cannot send a message to yourself.",
        }],
        isError: true,
      };
    }

    // Operator uses file-based delivery (notification badge mechanic — the
    // poller scans mailbox/operator/ and lights up the sender's attention
    // badge in the launcher).
    if (target === "operator") {
      const filepath = store.sendMessage(AGENT_NAME, target, message);
      return {
        content: [{ type: "text", text: `Message sent to ${target}. Delivered to: ${filepath}` }],
      };
    }

    // All agent-to-agent messages go through BEADS, via the durable
    // fire-and-forget queue (aperture-ktwoy). Enqueue + return INSTANTLY; the
    // background worker flushes to BEADS (createMessage) with retry + restart
    // replay, then pushes over the hub. This is read-after-write-safe: the
    // sender never re-reads a sent message and the recipient receives it via
    // hub push (or unread replay on reconnect), so the small async flush delay
    // is invisible. No file-fallback here — the queue's retry handles backend
    // hiccups; falling back to a divergent store would be split-brain
    // (Cipher/Peppy guardrail).
    sendQueue.enqueue(AGENT_NAME, target, message);
    // Recipient presence rides on the ack (aperture-oeb6q) so the sender knows
    // whether to expect a prompt reply. Best-effort: a presence read failure
    // must never turn a successfully queued send into an error.
    let ack = `Queued for ${target}.`;
    try {
      ack = formatSendAck(target, presenceReport());
    } catch {
      // presence unavailable — the bare ack above stands
    }
    return {
      content: [{ type: "text", text: ack }],
    };
  }
);

server.tool(
  "mark_as_read",
  "Mark a BEADS message as read. Call it once per message after you have read it via get_messages (or a hub push event), otherwise it is replayed to you on every reconnect.",
  { message_id: z.string().describe("The BEADS message ID to mark as read (e.g. aperture-abc)") },
  async ({ message_id }) => {
    try {
      if (!hasAuthenticatedMcpIdentity(AGENT_NAME!)) {
        return {
          content: [{ type: "text", text: "ERROR: E_CROSS_TEAM_DENIED unknown_sender" }],
          isError: true,
        };
      }
      await markMessageRead(message_id, AGENT_NAME!);
      return { content: [{ type: "text", text: `Message ${message_id} marked as read.` }] };
    } catch (e: any) {
      return { content: [{ type: "text", text: `ERROR: ${e.message}` }], isError: true };
    }
  }
);

server.tool(
  "get_messages",
  `Get your unread messages from the BEADS message bus, oldest first. At most ${UNREAD_LIMIT} per call — when the reply ends with a "Showing the ${UNREAD_LIMIT} most recent…" notice, mark those read and call again to drain the rest.`,
  {},
  async () => {
    try {
      if (!hasAuthenticatedMcpIdentity(AGENT_NAME!)) {
        return {
          content: [{ type: "text", text: "ERROR: E_CROSS_TEAM_DENIED unknown_sender" }],
          isError: true,
        };
      }
      const result = await getUnreadMessages(AGENT_NAME!);
      const messages = JSON.parse(result);
      // A non-array body is NOT "no messages" — it means the query did not
      // return what we expect, and reporting it as empty is indistinguishable
      // from a genuinely empty inbox. That conflation is dangerous: an agent
      // sits idle believing nothing is queued while real directives wait.
      // Only a real empty array counts as an empty inbox.
      if (!Array.isArray(messages)) {
        return {
          content: [
            {
              type: "text",
              text:
                `ERROR: unexpected bd response shape for get_messages — expected a JSON array, got ${
                  messages === null ? "null" : typeof messages
                }. This is NOT an empty inbox; messages may be queued. Re-run, or fall back to: bd list --type message --status open`,
            },
          ],
          isError: true,
        };
      }
      if (messages.length === 0) {
        return { content: [{ type: "text", text: "No unread messages." }] };
      }
      // Sorted oldest-first + cap notice when the batch hit UNREAD_LIMIT.
      return { content: [{ type: "text", text: formatUnreadMessages(messages) }] };
    } catch (e: any) {
      return { content: [{ type: "text", text: `ERROR: ${e.message}` }], isError: true };
    }
  }
);

// ── Presence (aperture-oeb6q) ──

/** Local-time hh:mm:ss for an ISO timestamp, or null if unparseable. Mirrors
 *  describePresence's formatting so the table and the send ack agree. */
function hhmmss(iso: string | null): string | null {
  if (!iso) return null;
  const t = new Date(iso);
  return Number.isNaN(t.getTime()) ? null : t.toTimeString().slice(0, 8);
}

export const HUB_DOWN_TEXT =
  "Hub: down — presence unknown for all agents. The launcher may be closed or the hub restarting; retry in a few seconds.";

/** One compact text block: a header line plus one padded row per roster
 *  agent (already name-sorted by presenceReport). Exported so it can be unit
 *  tested without booting the MCP server. */
export function formatPresenceTable(report: PresenceReport): string {
  if (report.hub === "down") return HUB_DOWN_TEXT;
  const snap = hhmmss(report.updated_at);
  const nameW = Math.max(...report.agents.map((a) => a.name.length), 4) + 2;
  const stateW = "offline".length + 2;
  const rows = report.agents.map((a) => {
    const since = hhmmss(a.since);
    const line = a.name.padEnd(nameW) + a.state.padEnd(stateW) + (since ? `since ${since}` : "");
    return line.trimEnd();
  });
  return [`Hub: up${snap ? ` (snapshot ${snap})` : ""}`, ...rows].join("\n");
}

/** send_message ack: queued + recipient presence + what that means for
 *  delivery. One line. Exported for the same reason as formatPresenceTable. */
export function formatSendAck(target: string, report: PresenceReport): string {
  const desc = describePresence(report, target);
  const entry = report.agents.find((a) => a.name === target);
  const state = report.hub === "down" ? "unknown" : (entry?.state ?? "offline");
  let ack = `Queued for ${target}. ${desc}.`;
  if (state === "offline" || state === "unknown") {
    ack += " It will be pushed when they reconnect (unread replay); nothing is lost.";
  } else if (state === "busy") {
    ack += " It will interrupt their current turn as a Monitor event.";
  }
  return ack;
}

server.tool(
  "get_presence",
  "Who is online right now. Reads the hub's presence snapshot (no round-trip, no side effects) — cheap, call it freely, especially before dispatching work to another agent or when a reply is overdue. States: online = socket connected, no turn frame yet; busy = mid-turn (a message will interrupt them as a Monitor event); idle = between turns (a message is picked up promptly); offline = no socket (a message waits in BEADS and is replayed when they reconnect); unknown = the hub itself is down, so nobody's state can be read. Each row shows when the current state began.",
  {},
  async () => {
    try {
      return { content: [{ type: "text", text: formatPresenceTable(presenceReport()) }] };
    } catch (e: any) {
      return { content: [{ type: "text", text: `ERROR: ${e.message}` }], isError: true };
    }
  }
);

// ── Identity ──

server.tool(
  "get_identity",
  "Get your identity and role within the Aperture orchestration system, plus how inbound messages reach you.",
  {},
  async () => {
    return {
      content: [{
        type: "text",
        text: JSON.stringify({
          name: AGENT_NAME,
          role: agentRole,
          model: agentModel,
          system: "Aperture AI Orchestration Platform",
          description: "You are an AI agent inside the Aperture orchestration system. Messages from other agents are persisted to BEADS and delivered by the hub: Claude agents receive hub push events on their inbox Monitor; Codex agents receive them as injected turns. On a push (or whenever you suspect unread mail), call get_messages, then mark_as_read for each message you have handled.",
        }, null, 2),
      }],
    };
  }
);

// ── BEADS Task Tracking ──

server.tool(
  "create_task",
  "Create a new BEADS task. Returns the task ID. Optional fields cover the full filing flow in one call: type, labels (must include exactly one project:<name>), assignee, acceptance, blocked_by. If labels is omitted, no project label is added — caller is responsible for adding one separately.",
  {
    title: z.string().describe("Task title"),
    priority: z.number().min(0).max(4).describe("Priority 0-4 (0 = highest)"),
    description: z.string().optional().describe("Task description. NOTE: avoid literal XML/HTML close-tag patterns like `</reason>`, `</notes>`, `</description>` inside the text — the tool-argument wire format can misinterpret them as parameter terminators, causing argument truncation. If you must reference such tags, use `&lt;/reason&gt;` or paraphrase (e.g. \"the reason field\")."),
    type: z.enum(["task", "bug", "feature", "chore", "epic"]).optional().describe("Task type. Defaults to 'task'."),
    labels: z.array(z.string()).optional().describe("Labels to apply at creation. If provided, MUST contain exactly one `project:<name>` label (normally project:<repository-key>; explicit registered project bindings may differ). If omitted, no labels are set — add the project label separately via update_task add_labels."),
    assignee: z.string().optional().describe("Assignee (agent name: glados, wheatley, peppy, izzy, vance, rex, scout, cipher — or any string). Set without a separate update call."),
    acceptance: z.string().optional().describe("Testable acceptance criteria. NOTE: avoid literal XML/HTML close-tag patterns like `</acceptance>` inside the text; they can be misread as parameter terminators. Use `&lt;/...&gt;` or paraphrase."),
    blocked_by: z.array(z.string()).optional().describe("Task IDs that block this one. Each is wired up via `bd dep add <new> <blocker>` after creation."),
  },
  async ({ title, priority, description, type, labels, assignee, acceptance, blocked_by }) => {
    try {
      // Project-label validation: when labels are provided at all, exactly one
      // project:<name> entry is required. Empty/omitted labels are allowed
      // for backwards compatibility.
      if (labels !== undefined) {
        const projectLabels = labels.filter((l) => l.startsWith("project:"));
        if (projectLabels.length !== 1) {
          return {
            content: [{
              type: "text",
              text: `ERROR: project label required: must include exactly one project:<name> label (got ${projectLabels.length}: ${JSON.stringify(projectLabels)}). Use the exact project label registered for the selected repository.`,
            }],
            isError: true,
          };
        }
      }
      const result = await createTask(title, priority, description, {
        type,
        labels,
        assignee,
        acceptance,
        blockedBy: blocked_by,
      });
      return { content: [{ type: "text", text: result }] };
    } catch (e: any) {
      return { content: [{ type: "text", text: `ERROR: ${e.message}` }], isError: true };
    }
  }
);

server.tool(
  "update_task",
  "Update a BEADS task. Use claim to assign to yourself. Supports reassigning (assignee) and label edits (add_labels / remove_labels) without shelling to bd.",
  {
    id: z.string().describe("Task ID (e.g. bd-a1b2)"),
    claim: z.boolean().optional().describe("Claim this task for yourself"),
    status: z.string().optional().describe("New status"),
    description: z.string().optional().describe("New description (REPLACES existing description). NOTE: avoid literal XML/HTML close-tag patterns like `</reason>`, `</notes>` inside the text — they can be misread as parameter terminators by the tool-argument wire format. Use `&lt;/...&gt;` or paraphrase."),
    notes: z.string().optional().describe("Note to add to the task. APPENDS to existing notes by default (with newline separator) — your write does NOT replace anyone else's content. Pass replace_notes:true if you really want to overwrite (rare; cleanup/canonicalization only). NOTE: avoid literal XML/HTML close-tag patterns like `</reason>`, `</notes>` inside the text — they can be misread as parameter terminators by the tool-argument wire format. Use `&lt;/...&gt;` or paraphrase."),
    replace_notes: z.boolean().optional().describe("If true, the notes field is REPLACED with the new value (destructive). Default false (append). Use only for cleanup/canonicalization, never for routine progress updates."),
    assignee: z.string().optional().describe("Reassign the task to a different agent or user."),
    add_labels: z.array(z.string()).optional().describe("Labels to add. Useful when retroactively attaching a project:<name> label after a 3-arg create."),
    remove_labels: z.array(z.string()).optional().describe("Labels to remove."),
  },
  async ({ id, claim, status, description, notes, replace_notes, assignee, add_labels, remove_labels }) => {
    try {
      const flags: Record<string, string> = {};
      if (claim) flags["claim"] = "";
      if (status) flags["status"] = status;
      if (description) flags["description"] = description;
      // Default to append-notes so a write never silently destroys prior content
      // (aperture-e8qp). Caller can opt into destructive overwrite via replace_notes.
      if (notes) flags[replace_notes ? "notes" : "append-notes"] = notes;
      const result = await updateTask(id, flags, {
        assignee,
        addLabels: add_labels,
        removeLabels: remove_labels,
      });
      // Compact ack — do NOT echo the full record back (context-efficiency:
      // the echo rode the entire accumulated notes history on every mutation).
      const ack = formatUpdateAck(id, result, {
        claim,
        status,
        description,
        notes,
        replaceNotes: replace_notes,
        assignee,
        addLabels: add_labels,
        removeLabels: remove_labels,
        actor: AGENT_NAME ?? undefined,
      });
      return { content: [{ type: "text", text: ack }] };
    } catch (e: any) {
      return { content: [{ type: "text", text: `ERROR: ${e.message}` }], isError: true };
    }
  }
);

server.tool(
  "close_task",
  "Close a BEADS task with a reason.",
  {
    id: z.string().describe("Task ID"),
    reason: z.string().describe("Reason for closing. CRITICAL: do NOT include literal XML/HTML close-tag patterns like `</reason>`, `</notes>`, `</close>` inside this text — the tool-argument wire format treats them as parameter terminators, which causes the rest of your tool call to be silently swallowed and bleed into the next call. If you need to reference such a tag, escape it (`&lt;/reason&gt;`) or paraphrase (e.g. \"the reason field\"). Plain prose is always safe."),
  },
  async ({ id, reason }) => {
    try {
      const result = await closeTask(id, reason);
      // Compact ack — close fires exactly when notes history is at its maximum.
      return { content: [{ type: "text", text: formatCloseAck(id, result, reason) }] };
    } catch (e: any) {
      return { content: [{ type: "text", text: `ERROR: ${e.message}` }], isError: true };
    }
  }
);

server.tool(
  "query_tasks",
  `Query BEADS tasks. Modes: 'list' (active tasks), 'ready' (unblocked), 'show' (single task by ID). In 'list' mode this defaults to YOUR own assigned tasks — pass assignee:"*" for any. List/ready default to summary fields (description/notes truncated to 200 chars). 'show' defaults to the 'detail' tier: full meta + acceptance criteria, description capped at 4k chars, notes capped to the LAST 3k chars (recent history), dependencies summarized — pass fields:"full" for the complete untruncated record when genuinely resuming exact bead state. Use project:"aperture" to filter by the project:aperture label. Done/closed tasks excluded by default; pass include_done:true for historical data. Results are CAPPED: 'list' returns at most 50 tasks and 'ready' at most 10 by default (bd's own limits) — pass limit (1-500) to raise the cap when a filtered query might exceed it.`,
  {
    mode: z.enum(["list", "ready", "show"]).describe("Query mode"),
    id: z.string().optional().describe("Task ID (required for 'show' mode)"),
    include_done: z.boolean().optional().describe("Include done/closed tasks (default: false). Significantly increases response size."),
    project: z.string().optional().describe("Filter by project label (e.g. 'aperture' matches tasks tagged project:aperture)."),
    assignee: z.string().optional().describe("Filter by assignee. Defaults to YOU in 'list' mode. Pass '*' for any assignee. Ignored in 'ready' mode."),
    priority_max: z.number().min(0).max(4).optional().describe("Keep tasks with priority ≤ this value (0=highest, 4=backlog)."),
    label: z.string().optional().describe("Filter by an arbitrary label."),
    fields: z.enum(["summary", "detail", "full"]).optional().describe("Projection tier. 'summary' (list/ready default): id,title,status,priority,assignee,owner,labels + 200-char description/notes. 'detail' (show default): full meta, description head-capped 4k, notes TAIL-capped 3k, dependencies summarized. 'full': complete untruncated record — use only when genuinely resuming exact bead state."),
    limit: z.number().int().min(1).max(500).optional().describe("Max tasks to return (1-500). Default: bd's own cap — 50 for 'list', 10 for 'ready'. Ignored in 'show' mode."),
  },
  async ({ mode, id, include_done, project, assignee, priority_max, label, fields, limit }) => {
    try {
      // Default to caller's own tasks in list mode unless they ask for "*".
      const effectiveAssignee =
        mode === "list" && assignee === undefined ? AGENT_NAME : assignee;
      const result = await queryTasks(mode, id, {
        includeDone: include_done,
        project,
        assignee: effectiveAssignee,
        priorityMax: priority_max,
        label,
        fields,
        limit,
      });
      return { content: [{ type: "text", text: result }] };
    } catch (e: any) {
      return { content: [{ type: "text", text: `ERROR: ${e.message}` }], isError: true };
    }
  }
);

// ── V4 team activation control ──

function gladosControlDenied(): { content: Array<{ type: "text"; text: string }>; isError: true } | null {
  if (AGENT_NAME === "glados" && hasAuthenticatedMcpIdentity("glados")) return null;
  return {
    content: [{ type: "text", text: "ERROR: E_CONTROL_UNAUTHORIZED exact authenticated GLaDOS context required" }],
    isError: true,
  };
}

const teamIdSchema = z.string().regex(/^[a-z0-9][a-z0-9_-]{0,15}$/);
const seatIdSchema = z.string().regex(/^[a-z0-9][a-z0-9_-]{0,30}$/);
const requestIdSchema = z.string().uuid();
const epicIdSchema = z.string().regex(/^aperture-[a-z0-9][a-z0-9-]{0,63}$/);

server.tool(
  "team_list_activation_requests",
  "GLaDOS-only: list durable pending team activation requests. This reads the Rust transaction engine; it does not claim to wake or notify GLaDOS.",
  {},
  async () => {
    const denied = gladosControlDenied();
    if (denied) return denied;
    try {
      const pending = parsePendingList(await invokeTeamControl({ action: "list_pending" }));
      return { content: [{ type: "text", text: JSON.stringify(pending.result) }] };
    } catch (e: any) {
      return { content: [{ type: "text", text: `ERROR: ${e.message}` }], isError: true };
    }
  },
);

server.tool(
  "team_get_creation_catalog",
  "GLaDOS-only read-only native catalog of roles, repository availability and execution tuples. Use this before proposing a team; catalog configuration is not proof of live harness support. Claude managed launches remain unavailable.",
  {},
  async () => {
    const denied = gladosControlDenied();
    if (denied) return denied;
    try {
      const result = await invokeTeamControl({ action: "catalog" });
      if (result.action !== "catalog" || !result.result || typeof result.result !== "object") throw new Error("E_CONTROL_FAILED: unexpected catalog response");
      return { content: [{ type: "text", text: JSON.stringify(result) }] };
    } catch (e: any) {
      return { content: [{ type: "text", text: `ERROR: ${e.message}` }], isError: true };
    }
  },
);

server.tool(
  "team_list",
  "GLaDOS-only read-only team state, including native owner observations. Use this to orchestrate approved teams and inspect unknown outcomes; never infer that an active team means its workers are running.",
  {},
  async () => {
    const denied = gladosControlDenied();
    if (denied) return denied;
    try {
      const raw = await invokeTeamControl({ action: "list_teams" });
      parseTeamList(raw);
      return { content: [{ type: "text", text: JSON.stringify(raw) }] };
    } catch (e: any) {
      return { content: [{ type: "text", text: `ERROR: ${e.message}` }], isError: true };
    }
  },
);

server.tool(
  "team_bootstrap_seat",
  "GLaDOS-only: start one eligible Codex seat of an already approved team through native ownership, fresh session and exact model observation. GLaDOS orchestrates all seats sequentially; the operator does not click per worker. Each call has its own bounded deadline. No retries after unknown outcomes; inspect team_list and stop the batch on a blocker. No actor, model override or caller authority.",
  { input: bootstrapSeatSchema },
  async ({ input }) => {
    const denied = gladosControlDenied();
    if (denied) return denied;
    try {
      const request = bootstrapSeatSchema.parse(input);
      const expected = bootstrapSelection(await invokeTeamControl({ action: "list_teams" }), request);
      const result = parseBootstrapStarted(await invokeTeamControl({ action: "bootstrap_seat", input: request }), request, expected);
      return { content: [{ type: "text", text: JSON.stringify(result) }] };
    } catch (e: any) {
      return { content: [{ type: "text", text: `ERROR: ${e.message}` }], isError: true };
    }
  },
);

server.tool(
  "team_claude_startup_smoke",
  "GLaDOS-only, operator-authorized diagnostic: one fresh approved Claude Sonnet 5 seat at generation zero. Performs real startup observation WITHOUT an initial prompt, then always stops/revokes the same candidate and leaves it quarantined. This consumes the attempt and cannot be retried automatically. It does NOT prove MCP readiness, enable Claude publicly, start a business mission, or leave a worker available. Exact selectors only; no caller model, authority, force, prompt or timeout. UNKNOWN requires inspection, never a retry.",
  { input: claudeStartupSmokeSchema },
  async ({ input }) => {
    const denied = gladosControlDenied();
    if (denied) return denied;
    try {
      const request = claudeStartupSmokeSchema.parse(input);
      const result = parseClaudeStartupSmoke(await invokeTeamControl({ action: "claude_startup_smoke", input: request }), request);
      return { content: [{ type: "text", text: JSON.stringify(result) }] };
    } catch (e: any) {
      return { content: [{ type: "text", text: `ERROR: ${e.message}` }], isError: true };
    }
  },
);

server.tool(
  "team_stop_seat",
  "GLaDOS-only: stop and revoke one exact seat with a freshly validated checkpoint, preserving context. This does not replace the worker or archive the team; archive is a separate action. Owner remains active at the same generation until archive, so do not infer process_count zero from owner metadata. Native bounded lifecycle proof is required. On unknown outcome inspect and reconcile; never automatically retry. No caller proofs, force/discard, model or authority fields.",
  { input: stopSeatSchema },
  async ({ input }) => {
    const denied = gladosControlDenied();
    if (denied) return denied;
    try {
      const request = stopSeatSchema.parse(input);
      const result = parseStopSeatReady(await invokeTeamControl({ action: "stop_seat", input: request }), request);
      return { content: [{ type: "text", text: JSON.stringify(result) }] };
    } catch (e: any) {
      return { content: [{ type: "text", text: `ERROR: ${e.message}` }], isError: true };
    }
  },
);

server.tool(
  "team_create",
  "GLaDOS-only: after the operator confirms mission, repository and exact team composition, create a durable PENDING team. This never activates or starts workers. Use the native catalog first; team_approve_activation with the authorized epic remains a separate step. On unknown outcome inspect pending requests before retrying. No actor, grants or generated provenance input.",
  { input: createTeamSchema },
  async ({ input }) => {
    const denied = gladosControlDenied();
    if (denied) return denied;
    try {
      const request = createTeamSchema.parse(input);
      const result = parseCreatedTeam(await invokeTeamControl({ action: "create", input: request }), request);
      return { content: [{ type: "text", text: JSON.stringify(result) }] };
    } catch (e: any) {
      return { content: [{ type: "text", text: `ERROR: ${e.message}` }], isError: true };
    }
  },
);

server.tool(
  "team_list_repositories",
  "GLaDOS-only: read the runtime repository registry (project, repo key, display name, enabled, local availability) with its current sha256 for CAS. Keys resolve only under HOME/projects; no paths are accepted or returned. Reloaded on every call.",
  {},
  async () => {
    const denied = gladosControlDenied();
    if (denied) return denied;
    try {
      const result = parseRepositoryRegistry(await invokeTeamControl({ action: "list_repositories" }), "list_repositories");
      return { content: [{ type: "text", text: JSON.stringify(result) }] };
    } catch (e: any) {
      return { content: [{ type: "text", text: `ERROR: ${e.message}` }], isError: true };
    }
  },
);

server.tool(
  "team_save_repository",
  "GLaDOS-only: register or edit one repository offer (display_name / enabled) with CAS against expected_sha256 from team_list_repositories. project+repo identify the entry and are never remapped or deleted; disabling only blocks new team create/approve admissions and never touches existing teams. No paths, no actor, no activation.",
  { input: saveRepositorySchema },
  async ({ input }) => {
    const denied = gladosControlDenied();
    if (denied) return denied;
    try {
      const request = saveRepositorySchema.parse(input);
      const result = parseSavedRepository(await invokeTeamControl({ action: "save_repository", input: request }), request);
      return { content: [{ type: "text", text: JSON.stringify(result) }] };
    } catch (e: any) {
      return { content: [{ type: "text", text: `ERROR: ${e.message}` }], isError: true };
    }
  },
);

server.tool(
  "team_approve_activation",
  "GLaDOS-only: approve one exact pending team request after verifying its active authorized epic and project label. Uses the authenticated Rust transaction engine; no caller actor field is accepted.",
  {
    team: teamIdSchema,
    expected_generation: z.number().int().nonnegative(),
    creation_request_id: requestIdSchema,
    epic_id: epicIdSchema,
  },
  async (input) => {
    const denied = gladosControlDenied();
    if (denied) return denied;
    try {
      const pending = parsePendingList(await invokeTeamControl({ action: "list_pending" }));
      const matched = assertActivationMatchesPending(pending, input);
      const epic = await queryTasks("show", input.epic_id, { fields: "full" });
      assertAuthorizedEpic(epic, input.epic_id, matched.snapshot.project);
      const result = await invokeTeamControl({ action: "approve", input });
      return { content: [{ type: "text", text: JSON.stringify(result) }] };
    } catch (e: any) {
      return { content: [{ type: "text", text: `ERROR: ${e.message}` }], isError: true };
    }
  },
);

server.tool(
  "team_cancel_activation",
  "GLaDOS-only: cancel one exact pending team request through the same locked Rust transaction engine. Operator UI cancel remains a separate human action.",
  {
    team: teamIdSchema,
    expected_generation: z.number().int().nonnegative(),
    creation_request_id: requestIdSchema,
  },
  async (input) => {
    const denied = gladosControlDenied();
    if (denied) return denied;
    try {
      const pending = parsePendingList(await invokeTeamControl({ action: "list_pending" }));
      assertActivationMatchesPending(pending, { ...input, epic_id: "aperture-selector-only" });
      const result = await invokeTeamControl({ action: "cancel", input });
      return { content: [{ type: "text", text: JSON.stringify(result) }] };
    } catch (e: any) {
      return { content: [{ type: "text", text: `ERROR: ${e.message}` }], isError: true };
    }
  },
);

server.tool(
  "team_archive",
  "GLaDOS-only: recollect the complete archive inventory, durably approve its exact hashes, and archive one active team through the native journal. A blocked checklist performs no archive mutation.",
  { team: teamIdSchema, expected_generation: z.number().int().positive() },
  async (input) => {
    const denied = gladosControlDenied();
    if (denied) return denied;
    try {
      const result = await invokeTeamControl({ action: "archive", input });
      return { content: [{ type: "text", text: JSON.stringify(result) }] };
    } catch (e: any) {
      return { content: [{ type: "text", text: `ERROR: ${e.message}` }], isError: true };
    }
  },
);

server.tool(
  "team_rollback_archive",
  "GLaDOS-only manual inverse for an archived or partially archived team. Uses the durable byte manifest and no-replace journal; missing or conflicting evidence fails closed.",
  { team: teamIdSchema, expected_generation: z.number().int().positive() },
  async (input) => {
    const denied = gladosControlDenied();
    if (denied) return denied;
    try {
      const result = await invokeTeamControl({ action: "rollback_archive", input });
      return { content: [{ type: "text", text: JSON.stringify(result) }] };
    } catch (e: any) {
      return { content: [{ type: "text", text: `ERROR: ${e.message}` }], isError: true };
    }
  },
);

// ── V4 managed-seat control ──

// These tools carry selectors and bounded data only. The Rust child is the
// authority: it opens the canonical current bearer, derives the managed seat,
// generation, team and lead policy, then revalidates them before mutation.
// This MCP-side check is defense in depth for a seat archived mid-session.
function managedSeatControlDenied(): { content: Array<{ type: "text"; text: string }>; isError: true } | null {
  if (AGENT_NAME && hasAuthenticatedMcpIdentity(AGENT_NAME)) return null;
  return {
    content: [{ type: "text", text: "ERROR: E_CONTROL_UNAUTHORIZED current managed-seat capability required" }],
    isError: true,
  };
}

const sha40Schema = z.string().regex(/^[a-f0-9]{40}$/);
const sha256Schema = z.string().regex(/^[a-f0-9]{64}$/);
const checkpointPayloadSchema = z.object({
  task_id: z.string().min(1).max(100),
  worktree: z.string().min(1).max(512),
  branch: z.string().min(1).max(512),
  head_sha: sha40Schema,
  dirty_files: z.array(z.string().min(1).max(512)).max(200),
  open_pr: z.object({
    repository: z.string().min(1).max(200),
    number: z.number().int().positive(),
    head_sha: sha40Schema,
  }).strict().nullable(),
  running_procs: z.array(z.object({
    pid: z.number().int().min(2),
    start_time: z.string().min(1).max(80),
  }).strict()).max(256),
  decisions: z.array(z.object({
    code: z.string().min(1).max(64),
    text: z.string().min(1).max(1024),
    evidence_ref: z.string().min(1).max(200).nullable(),
  }).strict()).max(32),
  next_step: z.string().min(1).max(1024),
  remote_effects: z.array(z.object({
    kind: z.enum(["ssh", "deploy", "ci", "provider", "shell"]),
    reference: z.string().min(1).max(200),
    // A worker may report its view, but the native inventory deliberately
    // projects every checkpoint declaration as untrusted/unknown until an
    // authorized resolution fact exists.
    state: z.enum(["finished", "cancelled", "unknown"]),
  }).strict()).max(64),
}).strict();

server.tool(
  "team_checkpoint",
  "Managed seat only: append one bounded checkpoint for the caller's current owner generation. Team, seat, generation, writer, sequence and validation are derived by the native control child.",
  // Unknown u32 schemas reach the native writer and are retained as rejected
  // evidence; the transport must not silently erase that recovery history.
  { schema_version: z.number().int().nonnegative().max(0xffff_ffff), payload: checkpointPayloadSchema },
  async (input) => {
    const denied = managedSeatControlDenied();
    if (denied) return denied;
    try {
      const result = await invokeTeamControl({ action: "checkpoint", input });
      return { content: [{ type: "text", text: JSON.stringify(result) }] };
    } catch (e: any) {
      return { content: [{ type: "text", text: `ERROR: ${e.message}` }], isError: true };
    }
  },
);

server.tool(
  "team_inspect_remote_effects",
  "Managed team lead only: inspect the native remote-effect uncertainty inventory for one current seat generation. This is read-only and does not treat declarations as observations.",
  { target_seat: seatIdSchema, expected_generation: z.number().int().positive() },
  async (input) => {
    const denied = managedSeatControlDenied();
    if (denied) return denied;
    try {
      const result = await invokeTeamControl({ action: "inspect_remote", input });
      return { content: [{ type: "text", text: JSON.stringify(result) }] };
    } catch (e: any) {
      return { content: [{ type: "text", text: `ERROR: ${e.message}` }], isError: true };
    }
  },
);

server.tool(
  "team_resolve_remote_effect",
  "Managed team lead only: append one authenticated decision for an exact native inventory hash. This records authorized_decision, never provider observation or zero effects.",
  {
    target_seat: seatIdSchema,
    expected_generation: z.number().int().positive(),
    resolution: z.object({
      expected_inventory_hash: sha256Schema,
      scope: z.enum(["effect_resolution", "inventory_risk_acceptance"]),
      reference: z.string().min(1).max(200).nullable(),
      decision: z.enum(["finished", "cancelled", "proceed_with_unobserved_effects"]),
      evidence_ref: z.string().min(1).max(200),
    }).strict(),
  },
  async (input) => {
    const denied = managedSeatControlDenied();
    if (denied) return denied;
    try {
      const result = await invokeTeamControl({ action: "resolve_remote", input });
      return { content: [{ type: "text", text: JSON.stringify(result) }] };
    } catch (e: any) {
      return { content: [{ type: "text", text: `ERROR: ${e.message}` }], isError: true };
    }
  },
);

server.tool(
  "team_replace_in_policy",
  "Managed team lead only: replace one same-team seat at an exact target generation with an exact snapshot/fallback execution tuple. The native child derives caller authority and performs stop, revocation, reconciliation, fresh start and model verification; timeout is an unknown outcome and is never retried automatically.",
  {
    target_seat: seatIdSchema,
    expected_generation: z.number().int().positive(),
    selection: z.object({
      harness: z.enum(["claude", "codex"]),
      model: z.string().min(1).max(128),
      reasoning: z.enum(["low", "medium", "high", "xhigh", "max", "ultra"]).nullable(),
    }).strict(),
  },
  async (input) => {
    const denied = managedSeatControlDenied();
    if (denied) return denied;
    try {
      const result = await invokeTeamControl({ action: "replace", input });
      return { content: [{ type: "text", text: JSON.stringify(result) }] };
    } catch (e: any) {
      return { content: [{ type: "text", text: `ERROR: ${e.message}` }], isError: true };
    }
  },
);

server.tool(
  "store_artifact",
  "Store an artifact reference on a BEADS task. Types: file, pr, session, url, note.",
  {
    task_id: z.string().describe("Task ID to attach artifact to"),
    type: z.enum(["file", "pr", "session", "url", "note"]).describe("Artifact type"),
    value: z.string().describe("Artifact value (path, URL, or text). NOTE: avoid literal XML/HTML close-tag patterns like `</value>`, `</note>` inside text artifacts — they can be misread as parameter terminators. Use `&lt;/...&gt;` or paraphrase."),
  },
  async ({ task_id, type, value }) => {
    try {
      await storeArtifact(task_id, type, value);
      // Compact ack — previously appended the full bd update echo after this line.
      return { content: [{ type: "text", text: `Artifact stored on ${task_id}: ${type}:${value}` }] };
    } catch (e: any) {
      return { content: [{ type: "text", text: `ERROR: ${e.message}` }], isError: true };
    }
  }
);

server.tool(
  "search_tasks",
  `Search BEADS tasks. Defaults to summary fields with description/notes truncated — pass fields:"full" for everything. Use project:"aperture" to filter by the project:aperture label. Done/closed tasks excluded by default. Unlike query_tasks, this does NOT auto-filter by assignee — pass assignee explicitly if you need it. Results are CAPPED at 50 tasks by default (bd's own limit) — pass limit (1-500) to raise the cap when a filtered search might exceed it.`,
  {
    label: z.string().optional().describe("Filter by label."),
    project: z.string().optional().describe("Filter by project label (e.g. 'aperture' matches tasks tagged project:aperture)."),
    assignee: z.string().optional().describe("Filter by assignee. Pass '*' or omit for any assignee."),
    priority_max: z.number().min(0).max(4).optional().describe("Keep tasks with priority ≤ this value (0=highest, 4=backlog)."),
    include_done: z.boolean().optional().describe("Include done/closed tasks (default: false)."),
    fields: z.enum(["summary", "full"]).optional().describe("Projection mode. 'summary' (default) returns id,title,status,priority,assignee,owner,labels + truncated description/notes. 'full' returns everything."),
    limit: z.number().int().min(1).max(500).optional().describe("Max tasks to return (1-500). Default: bd's own cap of 50."),
  },
  async ({ label, project, assignee, priority_max, include_done, fields, limit }) => {
    try {
      const result = await searchTasks({
        label,
        project,
        assignee,
        priorityMax: priority_max,
        includeDone: include_done,
        fields,
        limit,
      });
      return { content: [{ type: "text", text: result }] };
    } catch (e: any) {
      return { content: [{ type: "text", text: `ERROR: ${e.message}` }], isError: true };
    }
  }
);

// ── Objectives ──

import { listObjectives, updateObjectiveFile } from "./objectives.js";

server.tool(
  "list_objectives",
  "List all objectives from the Kanban board.",
  {},
  async () => {
    try {
      const objectives = listObjectives();
      if (objectives.length === 0) {
        return { content: [{ type: "text", text: "No objectives found." }] };
      }
      const summary = objectives
        .map((o) => `${o.id} | ${o.status} | P${o.priority} | ${o.title}${o.task_ids.length > 0 ? ` (${o.task_ids.length} tasks)` : ""}`)
        .join("\n");
      return { content: [{ type: "text", text: summary }] };
    } catch (e: any) {
      return { content: [{ type: "text", text: `ERROR: ${e.message}` }], isError: true };
    }
  }
);

server.tool(
  "update_objective",
  "Update an objective's fields. Use this to set spec, status, task_ids, etc.",
  {
    id: z.string().describe("Objective ID"),
    title: z.string().optional().describe("New title"),
    description: z.string().optional().describe("New description"),
    spec: z.string().optional().describe("Spec content (markdown)"),
    status: z.string().optional().describe("New status: draft, speccing, ready, approved, in_progress, done"),
    priority: z.number().optional().describe("Priority 0-4"),
    task_ids: z.array(z.string()).optional().describe("Array of BEADS task IDs linked to this objective"),
  },
  async ({ id, title, description, spec, status, priority, task_ids }) => {
    try {
      const updated = updateObjectiveFile(id, { title, description, spec, status, priority, task_ids });
      return { content: [{ type: "text", text: `Objective ${id} updated. Status: ${updated.status}` }] };
    } catch (e: any) {
      return { content: [{ type: "text", text: `ERROR: ${e.message}` }], isError: true };
    }
  }
);

// ── Memory recall (aperture-trgpo — context diet §4) ──
//
// Three read-only tools over the indexed memory bank. buildIndex() is called
// uncached on every invocation ON PURPOSE: memory-index.ts owns the cache and
// re-hashes bank+sidecar per call, so a new memory or a sidecar edit is picked
// up immediately without a server restart. Every string these tools return
// has already been through redact() inside memory-index.ts; secret-tagged
// entries never reach this layer at all.

const RECALL_INDEX_UNAVAILABLE = "ERROR: memory index unavailable";

function formatAge(ageDays: number | null): string {
  return ageDays === null ? "age?" : `${ageDays}d`;
}

function formatTags(tags: string[], standing: boolean): string {
  const all = standing ? ["standing", ...tags] : tags;
  return all.length ? all.join(",") : "-";
}

server.tool(
  "recall",
  "Search the BEADS memory bank (BM25 over key + body) and return a ranked list of matching memories — one line per hit: `key · score · age · tags · gist` — followed by a footer `total=N next_offset=M index_built_at=…`. Returns ≤12-word gists only, never bodies; call recall_full(key) for the text. Every gist is redacted (API keys, tokens, passwords, PEM blocks, drawer paths → [REDACTED]) and memories tagged secret are excluded entirely. Superseded memories are hidden unless include_superseded=true; standing decisions rank higher; entries older than 90 days rank lower. Errors with `ERROR: memory index unavailable: …` when bd or the index cannot be read — it never falls back to dumping the bank.",
  {
    query: z.string().min(2).describe("Free-text query — bead ids, hostnames, env var names and PR numbers match exactly; prose is ranked by BM25"),
    k: z.number().int().min(1).max(RECALL_K_MAX).optional().describe(`Results per page, 1..${RECALL_K_MAX} (default 5)`),
    offset: z.number().int().min(0).optional().describe("Skip this many ranked results (paging; use the footer's next_offset)"),
    project: z.string().optional().describe("Only memories whose sidecar project matches (e.g. aperture, incluir)"),
    tags: z.array(z.string()).optional().describe("Only memories carrying ALL of these sidecar tags"),
    include_superseded: z.boolean().optional().describe("Also return memories that a newer memory supersedes (never reveals secret-tagged ones)"),
  },
  async ({ query, k, offset, project, tags, include_superseded }) => {
    let idx;
    try {
      idx = await buildIndex();
    } catch (e: any) {
      return { content: [{ type: "text", text: `${RECALL_INDEX_UNAVAILABLE}: ${e?.message ?? String(e)}` }], isError: true };
    }
    try {
      const r = recall(idx, { query, k, offset, project, tags, include_superseded });
      const lines = r.items.map((it) => {
        const line = `${it.key} · ${it.score.toFixed(2)} · ${formatAge(it.ageDays)} · ${formatTags(it.tags, it.standing)} · ${it.gist}`;
        return it.supersededBy ? `${line} (superseded by ${it.supersededBy})` : line;
      });
      if (lines.length === 0) lines.push("(no matches)");
      lines.push(`total=${r.total} next_offset=${r.next_offset ?? "none"} index_built_at=${r.index_built_at}`);
      return { content: [{ type: "text", text: lines.join("\n") }] };
    } catch (e: any) {
      return { content: [{ type: "text", text: `ERROR: ${e.message}` }], isError: true };
    }
  }
);

server.tool(
  "recall_full",
  "Return one memory's full body by key (from a recall hit or the boot index). Reply = a one-line header `key · bytes_total=N · truncated=yes|no · supersedes=… · superseded_by=…` then the body. The body is redacted (API keys, tokens, passwords, PEM blocks, drawer paths → [REDACTED]) and truncated to max_bytes with a trailing notice when longer. Memories tagged secret are never returned: unknown and secret-excluded keys both answer `ERROR: no such memory (or it is secret-excluded): <key>`.",
  {
    key: z.string().min(1).describe("Memory key, exactly as shown by recall or the boot index"),
    max_bytes: z.number().int().min(256).max(RECALL_FULL_MAX_BYTES).optional().describe(`Truncate the body to this many bytes, 256..${RECALL_FULL_MAX_BYTES} (default ${RECALL_FULL_MAX_BYTES})`),
  },
  async ({ key, max_bytes }) => {
    let idx;
    try {
      idx = await buildIndex();
    } catch (e: any) {
      return { content: [{ type: "text", text: `${RECALL_INDEX_UNAVAILABLE}: ${e?.message ?? String(e)}` }], isError: true };
    }
    try {
      const r = recallFull(idx, key, max_bytes);
      if (r === null) {
        return { content: [{ type: "text", text: `ERROR: no such memory (or it is secret-excluded): ${key}` }], isError: true };
      }
      const header = [
        r.key,
        `bytes_total=${r.bytesTotal}`,
        `truncated=${r.truncated ? "yes" : "no"}`,
        `supersedes=${r.supersedes.length ? r.supersedes.join(",") : "-"}`,
        `superseded_by=${r.supersededBy ?? "-"}`,
      ].join(" · ");
      return { content: [{ type: "text", text: `${header}\n${r.body}` }] };
    } catch (e: any) {
      return { content: [{ type: "text", text: `ERROR: ${e.message}` }], isError: true };
    }
  }
);

server.tool(
  "recall_stats",
  "Sanitised counts about the memory index, for audits: total vs live entries, standing decisions, superseded, secret-excluded, redacted spans, counts by project and by tag, index build time and cache age. Returns counts only — no bodies, no gists, and never the keys of secret-tagged memories.",
  {},
  async () => {
    let idx;
    try {
      idx = await buildIndex();
    } catch (e: any) {
      return { content: [{ type: "text", text: `${RECALL_INDEX_UNAVAILABLE}: ${e?.message ?? String(e)}` }], isError: true };
    }
    try {
      const s = recallStats(idx);
      const kv = (o: Record<string, number>) =>
        Object.keys(o).length ? Object.entries(o).sort(([a], [b]) => a.localeCompare(b)).map(([k, v]) => `${k}=${v}`).join(" ") : "-";
      const text = [
        `total=${s.total} live=${s.live} standing=${s.standing} superseded=${s.superseded} secret_excluded=${s.secretExcluded} redacted_spans=${s.redactedSpans}`,
        `index_built_at=${s.index_built_at} cache_age_seconds=${s.cache_age_seconds ?? "none"}`,
        `by_project: ${kv(s.byProject)}`,
        `by_tag: ${kv(s.byTag)}`,
      ].join("\n");
      return { content: [{ type: "text", text }] };
    } catch (e: any) {
      return { content: [{ type: "text", text: `ERROR: ${e.message}` }], isError: true };
    }
  }
);

// ── Start ──

async function main() {
  const transport = new StdioServerTransport();
  await server.connect(transport);
}

main().catch((err) => {
  console.error("Failed to start MCP server:", err);
  process.exit(1);
});
