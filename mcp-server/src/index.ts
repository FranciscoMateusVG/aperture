import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { z } from "zod";
import { resolve } from "node:path";
import { homedir } from "node:os";
import { MailboxStore } from "./store.js";
import { MessageQueue } from "./message-queue.js";
import { formatUpdateAck, formatCloseAck, createTask, updateTask, closeTask, queryTasks, storeArtifact, searchTasks, createMessage, getUnreadMessages, markMessageRead, extractTaskId } from "./beads.js";
import { notifyHub } from "./hub-notify.js";
import { presenceReport, describePresence, type PresenceReport } from "./presence-snapshot.js";

const AGENT_NAME = process.env.AGENT_NAME;
if (!AGENT_NAME) {
  console.error("AGENT_NAME environment variable is required");
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

const PERMANENT_RECIPIENTS = ["glados", "wheatley", "peppy", "izzy", "vance", "rex", "scout", "cipher", "operator"];

// Decommissioned 2026-07-19. Kept only so a message addressed to one of them
// gets a routing hint instead of a bare "unknown recipient".
const RETIRED_RECIPIENTS = ["sage", "atlas", "sterling"];
const RETIRED_HINT = "sage/atlas/sterling were retired 2026-07-19 — route SEO/content to vance, docs to the implementing agent, QA sign-off to izzy.";

function isValidRecipient(name: string): boolean {
  return PERMANENT_RECIPIENTS.includes(name);
}

// ── Messaging ──

server.tool(
  "send_message",
  "Send a message to another agent or the human operator. Valid recipients: glados, wheatley, peppy, izzy, vance, rex, scout, cipher, operator. Use 'operator' to reach the human (lights up an attention badge — does not deliver text to a UI). Agent-to-agent messages are persisted to BEADS and pushed over the hub; the reply tells you the recipient's current presence (online/busy/idle/offline/unknown) so you know whether to expect a prompt response.",
  { to: z.string().describe("Recipient: glados, wheatley, peppy, izzy, vance, rex, scout, cipher, or operator"), message: z.string().describe("Message content. NOTE: avoid literal XML/HTML close-tag patterns like `</message>`, `</reason>` inside the body — they can be misread as parameter terminators by the tool-argument wire format. Use `&lt;/...&gt;` or paraphrase.") },
  async ({ to, message }) => {
    const target = to.toLowerCase().trim();

    if (!isValidRecipient(target)) {
      const hint = RETIRED_RECIPIENTS.includes(target) ? `\n${RETIRED_HINT}` : "";
      return {
        content: [{
          type: "text",
          text: `ERROR: Unknown recipient "${to}". Valid recipients are: ${PERMANENT_RECIPIENTS.join(", ")}. Use "operator" to message the human.${hint}`,
        }],
        isError: true,
      };
    }

    if (target === AGENT_NAME) {
      const allRecipients = PERMANENT_RECIPIENTS.filter(r => r !== AGENT_NAME);
      return {
        content: [{
          type: "text",
          text: `ERROR: You cannot send a message to yourself. Valid recipients: ${allRecipients.join(", ")}`,
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
      await markMessageRead(message_id);
      return { content: [{ type: "text", text: `Message ${message_id} marked as read.` }] };
    } catch (e: any) {
      return { content: [{ type: "text", text: `ERROR: ${e.message}` }], isError: true };
    }
  }
);

server.tool(
  "get_messages",
  "Get all unread messages for you from the BEADS message bus.",
  {},
  async () => {
    try {
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
      const formatted = messages.map((m: any) => {
        const titleMatch = m.title?.match(/\[(.+?)->(.+?)\]/);
        const from = titleMatch?.[1] ?? "unknown";
        return `[${m.id}] From ${from}: ${m.description ?? "(no content)"}`;
      }).join("\n\n");
      return { content: [{ type: "text", text: formatted }] };
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
    labels: z.array(z.string()).optional().describe("Labels to apply at creation. If provided, MUST contain exactly one `project:<name>` label (canonical: project:aperture, project:incluir, project:beads-galaxy, project:mempalace). If omitted, no labels are set — add the project label separately via update_task add_labels."),
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
              text: `ERROR: project label required: must include exactly one project:<name> label (got ${projectLabels.length}: ${JSON.stringify(projectLabels)}). Canonical taxonomy: project:aperture, project:incluir, project:beads-galaxy, project:mempalace.`,
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
  `Query BEADS tasks. Modes: 'list' (active tasks), 'ready' (unblocked), 'show' (single task by ID). In 'list' mode this defaults to YOUR own assigned tasks — pass assignee:"*" for any. List/ready default to summary fields (description/notes truncated to 200 chars). 'show' defaults to the 'detail' tier: full meta + acceptance criteria, description capped at 4k chars, notes capped to the LAST 3k chars (recent history), dependencies summarized — pass fields:"full" for the complete untruncated record when genuinely resuming exact bead state. Use project:"aperture" to filter by the project:aperture label. Done/closed tasks excluded by default; pass include_done:true for historical data.`,
  {
    mode: z.enum(["list", "ready", "show"]).describe("Query mode"),
    id: z.string().optional().describe("Task ID (required for 'show' mode)"),
    include_done: z.boolean().optional().describe("Include done/closed tasks (default: false). Significantly increases response size."),
    project: z.string().optional().describe("Filter by project label (e.g. 'aperture' matches tasks tagged project:aperture)."),
    assignee: z.string().optional().describe("Filter by assignee. Defaults to YOU in 'list' mode. Pass '*' for any assignee. Ignored in 'ready' mode."),
    priority_max: z.number().min(0).max(4).optional().describe("Keep tasks with priority ≤ this value (0=highest, 4=backlog)."),
    label: z.string().optional().describe("Filter by an arbitrary label."),
    fields: z.enum(["summary", "detail", "full"]).optional().describe("Projection tier. 'summary' (list/ready default): id,title,status,priority,assignee,owner,labels + 200-char description/notes. 'detail' (show default): full meta, description head-capped 4k, notes TAIL-capped 3k, dependencies summarized. 'full': complete untruncated record — use only when genuinely resuming exact bead state."),
  },
  async ({ mode, id, include_done, project, assignee, priority_max, label, fields }) => {
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
      });
      return { content: [{ type: "text", text: result }] };
    } catch (e: any) {
      return { content: [{ type: "text", text: `ERROR: ${e.message}` }], isError: true };
    }
  }
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
  `Search BEADS tasks. Defaults to summary fields with description/notes truncated — pass fields:"full" for everything. Use project:"aperture" to filter by the project:aperture label. Done/closed tasks excluded by default. Unlike query_tasks, this does NOT auto-filter by assignee — pass assignee explicitly if you need it.`,
  {
    label: z.string().optional().describe("Filter by label."),
    project: z.string().optional().describe("Filter by project label (e.g. 'aperture' matches tasks tagged project:aperture)."),
    assignee: z.string().optional().describe("Filter by assignee. Pass '*' or omit for any assignee."),
    priority_max: z.number().min(0).max(4).optional().describe("Keep tasks with priority ≤ this value (0=highest, 4=backlog)."),
    include_done: z.boolean().optional().describe("Include done/closed tasks (default: false)."),
    fields: z.enum(["summary", "full"]).optional().describe("Projection mode. 'summary' (default) returns id,title,status,priority,assignee,owner,labels + truncated description/notes. 'full' returns everything."),
  },
  async ({ label, project, assignee, priority_max, include_done, fields }) => {
    try {
      const result = await searchTasks({
        label,
        project,
        assignee,
        priorityMax: priority_max,
        includeDone: include_done,
        fields,
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

// ── Start ──

async function main() {
  const transport = new StdioServerTransport();
  await server.connect(transport);
}

main().catch((err) => {
  console.error("Failed to start MCP server:", err);
  process.exit(1);
});
