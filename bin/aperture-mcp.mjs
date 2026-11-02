#!/usr/bin/env node
// aperture-mcp — stdio MCP server giving an agent its Aperture tools.
//
// Tools: send_message(to, body), list_agents(), BEADS tasks (create_task,
// list_tasks, show_task, claim_task, update_task, close_task), and v3's recall
// memory (recall, recall_full). A team lead (--lead true) also gets
// create_team, dissolve_team and list_teams.
// Everything goes to the hub over HTTP (POST /messages, POST /beads/:op), so
// this never competes with the agent's inbox socket, and only the hub runs bd.
//
// Usage: aperture-mcp --agent <name> [--hub http://127.0.0.1:4518] [--lead true]
// Env:   APERTURE_AGENT, APERTURE_HUB_HTTP
// Needs Node 22+. No dependencies.

import { createInterface } from "node:readline";

const args = parseArgs(process.argv.slice(2));
const agent = args.agent ?? process.env.APERTURE_AGENT;
const hub = (args.hub ?? process.env.APERTURE_HUB_HTTP ?? "http://127.0.0.1:4518").replace(/\/$/, "");

if (!agent) {
  process.stderr.write("aperture-mcp: --agent <name> (or APERTURE_AGENT) is required\n");
  process.exit(2);
}

const TOOLS = [
  {
    name: "send_message",
    description:
      "Send a message to another Aperture agent. It is stored and delivered even if they are offline.",
    inputSchema: {
      type: "object",
      properties: {
        to: { type: "string", description: "Recipient agent name, e.g. wheatley" },
        body: { type: "string", description: "Message text" },
      },
      required: ["to", "body"],
    },
  },
  {
    name: "list_agents",
    description: "List the Aperture agents that are connected right now.",
    inputSchema: { type: "object", properties: {} },
  },
  {
    name: "create_task",
    description: "File a new BEADS task.",
    inputSchema: {
      type: "object",
      properties: {
        title: { type: "string" },
        description: { type: "string" },
        priority: { type: "integer", minimum: 0, maximum: 4, description: "0 = highest, default 2" },
        labels: { type: "array", items: { type: "string" }, description: "e.g. project:aperture" },
        assignee: { type: "string", description: "agent name" },
        acceptance: { type: "string", description: "what done means" },
      },
      required: ["title"],
    },
  },
  {
    name: "list_tasks",
    description: "List BEADS tasks (summary fields). ready=true lists open, unblocked tasks.",
    inputSchema: {
      type: "object",
      properties: {
        ready: { type: "boolean" },
        status: { type: "string", enum: ["open", "in_progress", "blocked", "closed"] },
        assignee: { type: "string" },
        label: { type: "string" },
        limit: { type: "integer", minimum: 1, maximum: 200 },
      },
    },
  },
  {
    name: "show_task",
    description: "Show one BEADS task in full.",
    inputSchema: { type: "object", properties: { id: { type: "string" } }, required: ["id"] },
  },
  {
    name: "claim_task",
    description: "Take a task: sets it in_progress and assigns it to you.",
    inputSchema: { type: "object", properties: { id: { type: "string" } }, required: ["id"] },
  },
  {
    name: "update_task",
    description: "Update a task: status, assignee, append a progress note, add labels.",
    inputSchema: {
      type: "object",
      properties: {
        id: { type: "string" },
        status: { type: "string", enum: ["open", "in_progress", "blocked"] },
        assignee: { type: "string" },
        notes: { type: "string", description: "appended to the task's notes" },
        add_labels: { type: "array", items: { type: "string" } },
      },
      required: ["id"],
    },
  },
  {
    name: "close_task",
    description: "Close a task with a short reason.",
    inputSchema: {
      type: "object",
      properties: { id: { type: "string" }, reason: { type: "string" } },
      required: ["id", "reason"],
    },
  },
  {
    name: "recall",
    description:
      "Search the team's banked lessons and decisions from earlier work (v3 memory). Returns keys and one-line gists; use recall_full for the text.",
    inputSchema: {
      type: "object",
      properties: {
        query: { type: "string", description: "words or a memory key" },
        k: { type: "integer", minimum: 1, maximum: 10 },
      },
      required: ["query"],
    },
  },
  {
    name: "recall_full",
    description: "Read one banked memory in full, by key (from recall).",
    inputSchema: { type: "object", properties: { key: { type: "string" } }, required: ["key"] },
  },
];

const TEAM_TOOLS = [
  {
    name: "create_team",
    description:
      "Create a team for one piece of work and launch its members (in parallel; returns when they are up). " +
      "Members are named <team>-<role>, report to you, and cannot message the operator. " +
      "Members with worktree=true get their own git worktree on a new branch team/<team>/<role>. " +
      "Send each member its first assignment with send_message afterwards (messages queue until they connect).",
    inputSchema: {
      type: "object",
      properties: {
        name: { type: "string", description: "short slug, e.g. auth or bh-escape-api" },
        project: { type: "string", description: "absolute path of the directory/repo the team works on" },
        base: { type: "string", description: "git ref new worktree branches start from (default HEAD)" },
        members: {
          type: "array",
          minItems: 1,
          items: {
            type: "object",
            properties: {
              role: { type: "string", description: "slug: coder, reviewer, tester, researcher..." },
              brief: { type: "string", description: "self-contained: goal, stack, constraints, what done means" },
              harness: { type: "string", enum: ["claude", "codex"] },
              model: { type: "string", description: "claude: opus, sonnet, haiku...; codex: gpt-6-astra, gpt-5.6-sol..." },
              worktree: { type: "boolean", description: "true for members who edit code" },
            },
            required: ["role", "brief", "harness", "model"],
          },
        },
      },
      required: ["name", "project", "members"],
    },
  },
  {
    name: "dissolve_team",
    description:
      "Dissolve a team when its work is reviewed and done: stops its members, removes clean worktrees, " +
      "keeps any worktree with uncommitted changes, never deletes branches.",
    inputSchema: { type: "object", properties: { name: { type: "string" } }, required: ["name"] },
  },
  {
    name: "list_teams",
    description: "List the live teams with their members, worktrees and branches.",
    inputSchema: { type: "object", properties: {} },
  },
];

const isLead = args.lead === "true";
const ALL_TOOLS = isLead ? [...TOOLS, ...TEAM_TOOLS] : TOOLS;

const RECALL_PATHS = { recall: "/recall", recall_full: "/recall/full" };

const BEADS_OPS = {
  create_task: "create",
  list_tasks: "list",
  show_task: "show",
  claim_task: "claim",
  update_task: "update",
  close_task: "close",
};

const rl = createInterface({ input: process.stdin });
let pending = 0;
let stdinClosed = false;

rl.on("line", async (line) => {
  if (!line.trim()) return;
  let req;
  try {
    req = JSON.parse(line);
  } catch {
    return reply({ jsonrpc: "2.0", id: null, error: { code: -32700, message: "Parse error" } });
  }
  if (req.id === undefined) return; // notification (e.g. notifications/initialized)

  pending += 1;
  try {
    reply({ jsonrpc: "2.0", id: req.id, result: await handle(req.method, req.params ?? {}) });
  } catch (err) {
    reply({ jsonrpc: "2.0", id: req.id, error: { code: err.code ?? -32603, message: err.message } });
  } finally {
    pending -= 1;
    if (stdinClosed && pending === 0) process.exit(0);
  }
});
// Client closed stdin: finish in-flight calls, then exit.
rl.on("close", () => {
  stdinClosed = true;
  if (pending === 0) process.exit(0);
});

async function handle(method, params) {
  switch (method) {
    case "initialize":
      return {
        protocolVersion: params.protocolVersion ?? "2025-06-18",
        capabilities: { tools: {} },
        serverInfo: { name: "aperture", version: "4.0.0-dev" },
      };
    case "ping":
      return {};
    case "tools/list":
      return { tools: ALL_TOOLS };
    case "tools/call":
      return callTool(params.name, params.arguments ?? {});
    default:
      throw Object.assign(new Error(`Method not found: ${method}`), { code: -32601 });
  }
}

async function callTool(name, input) {
  try {
    if (name === "send_message") {
      if (typeof input.to !== "string" || typeof input.body !== "string") {
        return toolError("send_message needs string `to` and `body`");
      }
      const res = await fetch(`${hub}/messages`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ from: agent, to: input.to, body: input.body }),
      });
      if (!res.ok) {
        const data = await res.json().catch(() => ({}));
        return toolError(data.error ?? `hub returned ${res.status}`);
      }
      const { id, status } = await res.json();
      const note = status === "queued" ? " (recipient offline; delivered when they connect)" : "";
      return toolText(`Message #${id} to ${input.to}: ${status}${note}`);
    }
    if (name === "list_agents") {
      const res = await fetch(`${hub}/agents`);
      if (!res.ok) return toolError(`hub returned ${res.status}`);
      const { agents } = await res.json();
      return toolText(agents.length ? `Connected: ${agents.join(", ")}` : "No agents connected");
    }
    if (isLead && name === "create_team") {
      return hubResult(await post("/teams", { ...input, from: agent }));
    }
    if (isLead && name === "dissolve_team") {
      if (typeof input.name !== "string") return toolError("dissolve_team needs `name`");
      return hubResult(await post(`/teams/${encodeURIComponent(input.name)}/dissolve`, { from: agent }));
    }
    if (isLead && name === "list_teams") {
      const res = await fetch(`${hub}/teams`);
      const data = await res.json().catch(() => ({}));
      if (!res.ok) return toolError(data.error ?? `hub returned ${res.status}`);
      return toolText(JSON.stringify(data.teams));
    }
    if (RECALL_PATHS[name]) {
      const res = await fetch(`${hub}${RECALL_PATHS[name]}`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ ...input, from: agent }),
      });
      const data = await res.json().catch(() => ({}));
      if (!res.ok) return toolError(data.error ?? `hub returned ${res.status}`);
      return toolText(JSON.stringify(data.result));
    }
    if (BEADS_OPS[name]) {
      // Always below Codex's 60s default. Operators/tests may only shorten it.
      const configured = Number(process.env.APERTURE_BEADS_TIMEOUT_MS);
      const timeoutMs = configured > 0 ? Math.min(configured, 48_000) : 48_000;
      try {
        const res = await fetch(`${hub}/beads/${BEADS_OPS[name]}`, {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ ...input, from: agent }),
          signal: AbortSignal.timeout(timeoutMs),
        });
        // The same deadline covers JSON body consumption, not just headers.
        const data = await res.json();
        if (res.status === 409 && data.result?.outcome === "indeterminate") return toolError(JSON.stringify(data.result));
        if (!res.ok) return toolError(data.error ?? `hub returned ${res.status}`);
        if (!Object.hasOwn(data, "result")) throw new Error("missing BEADS result");
        return toolText(JSON.stringify(data.result));
      } catch {
        if (["list", "show"].includes(BEADS_OPS[name])) {
          return toolError("BEADS read deadline or transport failure; no result available.");
        }
        return toolError(JSON.stringify({
          outcome: "indeterminate", operation: BEADS_OPS[name], retry_safe: false,
          message: "BEADS request deadline or transport failure; a write may have committed. Do not retry automatically. Wait and use show_task/list_tasks to read back before any manual follow-up.",
        }));
      }
    }
    return toolError(`Unknown tool: ${name}`);
  } catch (err) {
    return toolError(`Aperture hub unreachable at ${hub}: ${err.message}`);
  }
}

function post(path, body) {
  return fetch(`${hub}${path}`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  });
}

async function hubResult(res) {
  const data = await res.json().catch(() => ({}));
  if (!res.ok) return toolError(data.error ?? `hub returned ${res.status}`);
  return toolText(JSON.stringify(data.result));
}

function toolText(text) {
  return { content: [{ type: "text", text }] };
}

function toolError(text) {
  return { content: [{ type: "text", text }], isError: true };
}

function reply(msg) {
  process.stdout.write(JSON.stringify(msg) + "\n");
}

function parseArgs(argv) {
  const out = {};
  for (let i = 0; i < argv.length; i++) {
    if (argv[i].startsWith("--")) out[argv[i].slice(2)] = argv[i + 1], i++;
  }
  return out;
}
