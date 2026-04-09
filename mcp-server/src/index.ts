import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { z } from "zod";
import { MailboxStore } from "./store.js";
import { createTask, updateTask, closeTask, queryTasks, storeArtifact, searchTasks, createMessage, getUnreadMessages, markMessageRead } from "./beads.js";
import { requestSpawn, requestKill, readActiveSpiderlings, isValidRecipient } from "./spawner.js";

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

const server = new McpServer({
  name: "aperture-bus",
  version: "0.2.0",
});

const PERMANENT_RECIPIENTS = ["glados", "wheatley", "peppy", "izzy", "vance", "rex", "scout", "cipher", "sage", "atlas", "sentinel", "sterling", "operator", "warroom"];

function requireRole(required: string): void {
  if (agentRole !== required) {
    throw new Error(`This tool requires the '${required}' role. You are '${agentRole}'.`);
  }
}

// ── Messaging ──

server.tool(
  "send_message",
  "Send a message to another agent or the human operator. Valid recipients: glados, wheatley, peppy, izzy, operator, plus any active spiderlings. Use 'operator' to reach the human.",
  { to: z.string().describe("Recipient: glados, wheatley, peppy, izzy, operator, or a spiderling name"), message: z.string().describe("Message content") },
  async ({ to, message }) => {
    const target = to.toLowerCase().trim();

    if (!isValidRecipient(target)) {
      const spiderlingNames = readActiveSpiderlings().map(s => s.name);
      const allRecipients = [...PERMANENT_RECIPIENTS, ...spiderlingNames];
      return {
        content: [{
          type: "text",
          text: `ERROR: Unknown recipient "${to}". Valid recipients are: ${allRecipients.join(", ")}. Use "operator" to message the human.`,
        }],
        isError: true,
      };
    }

    if (target === AGENT_NAME) {
      const spiderlingNames = readActiveSpiderlings().map(s => s.name);
      const allRecipients = [...PERMANENT_RECIPIENTS, ...spiderlingNames].filter(r => r !== AGENT_NAME);
      return {
        content: [{
          type: "text",
          text: `ERROR: You cannot send a message to yourself. Valid recipients: ${allRecipients.join(", ")}`,
        }],
        isError: true,
      };
    }

    // Operator and warroom still use file-based delivery (Chat panel + War Room turn mechanics)
    if (target === "operator" || target === "warroom") {
      const filepath = store.sendMessage(AGENT_NAME, target, message);
      return {
        content: [{ type: "text", text: `Message sent to ${target}. Delivered to: ${filepath}` }],
      };
    }

    // All agent-to-agent messages go through BEADS
    try {
      const result = await createMessage(AGENT_NAME, target, message);
      const parsed = JSON.parse(result);
      const msgId = parsed.id ?? "unknown";
      return {
        content: [{ type: "text", text: `Message sent to ${target} via BEADS (${msgId}). The poller will deliver it.` }],
      };
    } catch (e: any) {
      // Fallback to file-based delivery if BEADS fails
      const filepath = store.sendMessage(AGENT_NAME, target, message);
      return {
        content: [{ type: "text", text: `Message sent to ${target} (file fallback). Delivered to: ${filepath}` }],
      };
    }
  }
);

server.tool(
  "mark_as_read",
  "Mark a BEADS message as read. Use this after receiving a message delivered by the poller.",
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
      if (!Array.isArray(messages) || messages.length === 0) {
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

// ── Identity ──

server.tool(
  "get_identity",
  "Get your identity and role within the Aperture orchestration system.",
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
          description: "You are an AI agent inside the Aperture orchestration system. Messages from other agents are delivered directly into your conversation as file contents.",
        }, null, 2),
      }],
    };
  }
);

// ── BEADS Task Tracking ──

server.tool(
  "create_task",
  "Create a new BEADS task. Returns the task ID.",
  {
    title: z.string().describe("Task title"),
    priority: z.number().min(0).max(4).describe("Priority 0-4 (0 = highest)"),
    description: z.string().optional().describe("Task description"),
  },
  async ({ title, priority, description }) => {
    try {
      const result = await createTask(title, priority, description);
      return { content: [{ type: "text", text: result }] };
    } catch (e: any) {
      return { content: [{ type: "text", text: `ERROR: ${e.message}` }], isError: true };
    }
  }
);

server.tool(
  "update_task",
  "Update a BEADS task. Use claim to assign to yourself.",
  {
    id: z.string().describe("Task ID (e.g. bd-a1b2)"),
    claim: z.boolean().optional().describe("Claim this task for yourself"),
    status: z.string().optional().describe("New status"),
    description: z.string().optional().describe("New description"),
    notes: z.string().optional().describe("Append notes"),
  },
  async ({ id, claim, status, description, notes }) => {
    try {
      const flags: Record<string, string> = {};
      if (claim) flags["claim"] = "";
      if (status) flags["status"] = status;
      if (description) flags["description"] = description;
      if (notes) flags["notes"] = notes;
      const result = await updateTask(id, flags);
      return { content: [{ type: "text", text: result }] };
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
    reason: z.string().describe("Reason for closing"),
  },
  async ({ id, reason }) => {
    try {
      const result = await closeTask(id, reason);
      return { content: [{ type: "text", text: result }] };
    } catch (e: any) {
      return { content: [{ type: "text", text: `ERROR: ${e.message}` }], isError: true };
    }
  }
);

server.tool(
  "query_tasks",
  "Query BEADS tasks. Modes: 'list' (active tasks, slim fields), 'ready' (unblocked, slim), 'show' (full detail for single task by ID). Done/closed tasks are excluded by default from list/ready — set include_done=true only when you need historical data.",
  {
    mode: z.enum(["list", "ready", "show"]).describe("Query mode"),
    id: z.string().optional().describe("Task ID (required for 'show' mode)"),
    include_done: z.boolean().optional().describe("Include done/closed tasks (default: false). Only set true if you need historical data — it significantly increases response size."),
  },
  async ({ mode, id, include_done }) => {
    try {
      const result = await queryTasks(mode, id, { includeDone: include_done });
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
    value: z.string().describe("Artifact value (path, URL, or text)"),
  },
  async ({ task_id, type, value }) => {
    try {
      const result = await storeArtifact(task_id, type, value);
      return { content: [{ type: "text", text: `Artifact stored: ${type}:${value}\n${result}` }] };
    } catch (e: any) {
      return { content: [{ type: "text", text: `ERROR: ${e.message}` }], isError: true };
    }
  }
);

server.tool(
  "search_tasks",
  "Search BEADS tasks, optionally filtered by label. Returns slim fields, excludes done tasks by default.",
  {
    label: z.string().optional().describe("Filter by label"),
    include_done: z.boolean().optional().describe("Include done/closed tasks (default: false)"),
  },
  async ({ label, include_done }) => {
    try {
      const result = await searchTasks(label, { includeDone: include_done });
      return { content: [{ type: "text", text: result }] };
    } catch (e: any) {
      return { content: [{ type: "text", text: `ERROR: ${e.message}` }], isError: true };
    }
  }
);

// ── Spiderling Spawning ──

server.tool(
  "spawn_spiderling",
  "Spawn an ephemeral Claude Code worker in a git worktree. Orchestrator only.",
  {
    name: z.string().describe("Spiderling name (lowercase alphanumeric + hyphens, e.g. 'spider-auth')"),
    task_id: z.string().describe("BEADS task ID this spiderling will work on"),
    prompt: z.string().describe("Task description and instructions for the spiderling"),
    project_path: z.string().optional().describe("Path to the target project repo for the worktree (e.g. '~/projects/fitt'). If omitted, uses the Aperture repo."),
  },
  async ({ name, task_id, prompt, project_path }) => {
    try {
      requireRole("orchestrator");
      const result = requestSpawn(name, task_id, prompt, AGENT_NAME!, project_path);
      return { content: [{ type: "text", text: `Spawn request submitted for '${result}'. It will appear shortly.` }] };
    } catch (e: any) {
      return { content: [{ type: "text", text: `ERROR: ${e.message}` }], isError: true };
    }
  }
);

server.tool(
  "list_spiderlings",
  "List all active spiderlings and their status.",
  {},
  async () => {
    const spiderlings = readActiveSpiderlings();
    if (spiderlings.length === 0) {
      return { content: [{ type: "text", text: "No active spiderlings." }] };
    }
    const summary = spiderlings
      .map((s) => `${s.name} | task: ${s.task_id} | status: ${s.status} | by: ${s.requested_by}`)
      .join("\n");
    return { content: [{ type: "text", text: summary }] };
  }
);

server.tool(
  "kill_spiderling",
  "Kill a spiderling and clean up its worktree. Orchestrator only.",
  {
    name: z.string().describe("Spiderling name to kill"),
  },
  async ({ name }) => {
    try {
      requireRole("orchestrator");
      requestKill(name);
      return { content: [{ type: "text", text: `Kill request submitted for '${name}'.` }] };
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
