// aperture-mcp-server-sentry — wrap layer in front of @sentry/mcp-server.
//
// Architecture (Option B per aperture-ttzz plan):
//
//   agent  ──MCP──>  this server ("sentry" namespace) ──stdio MCP client──>
//   spawned @sentry/mcp-server  ──HTTPS──>  sentry.io
//
// On every tool call this server applies, in order:
//   1. R3 — load allowlist; null → "Sentry MCP not configured" (all tools 503)
//   2. agent gate — caller in default_on or opt_in tier
//   3. classify tool (read / mutation / attachment)
//   4. R4 — project-allowlist param coverage check (DENY on unknown shape)
//   5. R2 — justification check for attachment tools (20–500 chars)
//   6. R1 — operator approval flow for mutation + attachment tools
//   7. forward call to upstream via MCP client
//   8. R5 — extract target_user_id from response, cap at 10
//   9. emit audit line to Loki (params_safe via R6 redaction, no PII body)
//
// Every log/audit emission passes through redact() (R6).
//
// Token: read from SENTRY_ACCESS_TOKEN env or ~/.config/aperture/sentry-agent-token
// (the canonical xerox path used by aperture-vzuu). Token is forwarded to
// the upstream via SENTRY_ACCESS_TOKEN env on the spawned process; it
// MUST NEVER appear in this server's logs or audit lines.

import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StdioClientTransport } from "@modelcontextprotocol/sdk/client/stdio.js";
import { CallToolResultSchema } from "@modelcontextprotocol/sdk/types.js";
import { z } from "zod";
import * as fs from "fs";
import * as path from "path";

import {
  loadAllowlist,
  checkAgentAllowed,
  checkProjectAllowlist,
  checkJustification,
  classifyTool,
  type AllowlistConfig,
} from "./gates.js";
import { requestApproval, setWrapLayerAgentName } from "./approval.js";
import {
  buildAuditLine,
  emitAuditLine,
  extractTargetUserIds,
  makeParamsSafe,
  type AuditLine,
} from "./audit.js";
import { createRedactor, setActiveRedactor, redact } from "./redact.js";

const AGENT_NAME = process.env.AGENT_NAME;
if (!AGENT_NAME) {
  process.stderr.write("[sentry-mcp] AGENT_NAME env required\n");
  process.exit(1);
}
setWrapLayerAgentName(AGENT_NAME);

// ── Token loading ──────────────────────────────────────────────────────

function loadSentryToken(): string | null {
  if (process.env.SENTRY_ACCESS_TOKEN) {
    return process.env.SENTRY_ACCESS_TOKEN.trim();
  }
  const home = process.env.HOME ?? "";
  const tokenPath =
    process.env.SENTRY_MCP_TOKEN_PATH ??
    `${home}/.config/aperture/sentry-agent-token`;
  try {
    const raw = fs.readFileSync(tokenPath, "utf8").trim();
    return raw.length > 0 ? raw : null;
  } catch {
    return null;
  }
}

const sentryToken = loadSentryToken();
// Install the redactor immediately — every subsequent log line is filtered.
setActiveRedactor(createRedactor(sentryToken));

// ── Allowlist load (R3 fail-closed) ────────────────────────────────────

let allowlist: AllowlistConfig | null = loadAllowlist();
// Refresh on SIGHUP — operator can edit the allowlist and signal us
// without restarting the whole agent.
process.on("SIGHUP", () => {
  allowlist = loadAllowlist();
  process.stderr.write(
    `[sentry-mcp] allowlist reloaded: ${allowlist === null ? "MISSING/EMPTY (fail-closed)" : "ok"}\n`,
  );
});

// ── Upstream MCP client (spawned @sentry/mcp-server) ───────────────────

interface UpstreamTool {
  name: string;
  description?: string;
  inputSchema?: Record<string, unknown>;
}

let upstreamClient: Client | null = null;
let upstreamTools: UpstreamTool[] = [];

async function startUpstreamClient(): Promise<void> {
  if (!sentryToken) {
    process.stderr.write(
      "[sentry-mcp] Sentry token unreachable — upstream NOT spawned. " +
        "All tool calls will return 'Sentry MCP not configured'.\n",
    );
    return;
  }

  // The upstream is @sentry/mcp-server. We resolve its CLI path via
  // require.resolve fallback because pnpm hoists differently per repo.
  const upstreamCmd = process.env.SENTRY_MCP_UPSTREAM_CMD ?? "npx";
  const upstreamArgs = process.env.SENTRY_MCP_UPSTREAM_ARGS
    ? process.env.SENTRY_MCP_UPSTREAM_ARGS.split(" ")
    : ["-y", "@sentry/mcp-server"];

  const transport = new StdioClientTransport({
    command: upstreamCmd,
    args: upstreamArgs,
    env: {
      ...process.env,
      SENTRY_ACCESS_TOKEN: sentryToken,
    },
  });

  const client = new Client(
    { name: "aperture-sentry-wrap", version: "1.1.0" },
    { capabilities: {} },
  );
  await client.connect(transport);
  upstreamClient = client;

  const listed = await client.listTools();
  upstreamTools = listed.tools.map((t) => ({
    name: t.name,
    description: t.description,
    inputSchema: t.inputSchema as Record<string, unknown> | undefined,
  }));
  process.stderr.write(
    `[sentry-mcp] upstream connected — ${upstreamTools.length} tools discovered\n`,
  );
}

// ── MCP server (our "sentry" namespace) ────────────────────────────────

const server = new McpServer({
  name: "aperture-sentry",
  version: "1.1.0",
});

interface GateOutcome {
  proceed: boolean;
  errorMessage?: string;
  auditPartial: Partial<AuditLine>;
}

/**
 * Resolve the gate stack for one tool call. Returns either proceed=true
 * with audit metadata (operator_approved flag, justification flag) or
 * proceed=false with the MCP-error message + the denied audit reason.
 */
async function runGateStack(
  toolName: string,
  params: Record<string, unknown>,
): Promise<GateOutcome> {
  // R3 — allowlist must be loaded.
  if (allowlist === null) {
    const reason =
      "Sentry MCP not configured — allowlist missing or empty " +
      "(~/.config/aperture/sentry-mcp-allowlist.yaml). Wrap layer fails closed.";
    return { proceed: false, errorMessage: reason, auditPartial: { denied: { reason } } };
  }

  // Agent gate.
  const agentResult = checkAgentAllowed(AGENT_NAME!, allowlist);
  if (!agentResult.ok) {
    return {
      proceed: false,
      errorMessage: agentResult.reason,
      auditPartial: { denied: { reason: agentResult.reason } },
    };
  }

  // Tool class.
  const toolClass = classifyTool(toolName);

  // R4 — project allowlist param coverage.
  const projectResult = checkProjectAllowlist(params, allowlist);
  if (!projectResult.ok) {
    return {
      proceed: false,
      errorMessage: projectResult.reason,
      auditPartial: { denied: { reason: projectResult.reason } },
    };
  }

  // R2 — justification required for attachment tools.
  let justificationText: string | undefined;
  if (toolClass === "attachment") {
    const jResult = checkJustification(params);
    if (!jResult.ok) {
      return {
        proceed: false,
        errorMessage: jResult.reason,
        auditPartial: { denied: { reason: jResult.reason } },
      };
    }
    justificationText = jResult.text;
  }

  // R1 — operator approval for mutation + attachment tools.
  if (toolClass === "mutation" || toolClass === "attachment") {
    const safeParams = makeParamsSafe(params);
    const approval = await requestApproval({
      callerAgent: AGENT_NAME!,
      tool: toolName,
      params_safe: safeParams,
      justification: justificationText,
    });
    if (approval.status === "approved") {
      return {
        proceed: true,
        auditPartial: {
          operator_approved: true,
          approval_message_id: approval.approval_message_id,
          has_justification: justificationText !== undefined ? true : undefined,
        },
      };
    }
    if (approval.status === "rejected") {
      return {
        proceed: false,
        errorMessage: `Operator rejected request: ${approval.reason}`,
        auditPartial: {
          operator_approved: false,
          approval_message_id: approval.approval_message_id,
          has_justification: justificationText !== undefined ? true : undefined,
          denied: { reason: `operator-rejected: ${approval.reason}` },
        },
      };
    }
    // timeout
    return {
      proceed: false,
      errorMessage: "approval not granted, request expired (10 minute timeout)",
      auditPartial: {
        operator_approved: false,
        has_justification: justificationText !== undefined ? true : undefined,
        denied: { reason: "approval-timeout" },
      },
    };
  }

  return {
    proceed: true,
    auditPartial: {},
  };
}

/**
 * Build a trace ID for one tool call. Spec §6 says capture trace_id; the
 * upstream Sentry MCP exposes its own trace via its tooling, but for the
 * audit line we mint a local ID at gate entry so cross-correlation
 * between gate denial and call execution stays consistent.
 */
function newTraceId(): string {
  return "tr-" + Date.now().toString(36) + "-" + Math.random().toString(36).slice(2, 10);
}

function countResultItems(response: unknown): number {
  if (response === null || response === undefined) return 0;
  if (Array.isArray(response)) return response.length;
  if (typeof response !== "object") return 1;
  const obj = response as Record<string, unknown>;
  for (const key of ["issues", "events", "items", "results", "traces", "docs"]) {
    const v = obj[key];
    if (Array.isArray(v)) return v.length;
  }
  return 1;
}

/**
 * Register each upstream tool on our own server with the wrap layer in
 * between. We use a permissive Zod schema (any record) because the actual
 * argument-shape is validated by the upstream — our gates only need to
 * inspect known shapes (project-id params, justification).
 */
function registerProxiedTools(): void {
  // Zod v4: z.record requires (keySchema, valueSchema). Keys for an MCP
  // params record are always strings.
  const passthroughSchema = { params: z.record(z.string(), z.unknown()).optional() };

  for (const tool of upstreamTools) {
    const localName = tool.name; // exposed as mcp__sentry__<name>
    server.tool(
      localName,
      tool.description ?? `Sentry MCP tool: ${tool.name}`,
      passthroughSchema,
      async (args: { params?: Record<string, unknown> }) => {
        const params: Record<string, unknown> = args.params ?? {};
        const traceId = newTraceId();
        const start = Date.now();

        const gate = await runGateStack(localName, params);
        if (!gate.proceed) {
          // Emit denied audit line and return MCP error.
          const safeParams = makeParamsSafe(params);
          const denied: AuditLine = buildAuditLine({
            agent: AGENT_NAME!,
            tool: localName,
            params_safe: safeParams,
            trace_id: traceId,
            duration_ms: Date.now() - start,
            ...gate.auditPartial,
          });
          await emitAuditLine(denied);
          return {
            content: [{ type: "text", text: redact(gate.errorMessage ?? "denied") }],
            isError: true,
          };
        }

        if (!upstreamClient) {
          const reason =
            "Upstream Sentry MCP not running. Token may be missing or upstream " +
            "spawn failed. Check stderr.";
          const denied = buildAuditLine({
            agent: AGENT_NAME!,
            tool: localName,
            params_safe: makeParamsSafe(params),
            trace_id: traceId,
            duration_ms: Date.now() - start,
            denied: { reason: "upstream-unreachable" },
          });
          await emitAuditLine(denied);
          return {
            content: [{ type: "text", text: redact(reason) }],
            isError: true,
          };
        }

        // Forward to upstream. Pass CallToolResultSchema explicitly so the
        // result type is the strict modern shape (not the union with the
        // legacy CompatibilityCallToolResult `toolResult`-only variant).
        try {
          const upstreamResult = await upstreamClient.callTool(
            {
              name: localName,
              arguments: params,
            },
            CallToolResultSchema,
          );
          const targetUsers = extractTargetUserIds(upstreamResult);
          const safeParams = makeParamsSafe(params);
          const audit = buildAuditLine({
            agent: AGENT_NAME!,
            tool: localName,
            params_safe: safeParams,
            trace_id: traceId,
            duration_ms: Date.now() - start,
            result_count: countResultItems(upstreamResult),
            target_user_id: targetUsers.user_ids,
            target_user_id_truncated: targetUsers.truncated ? true : undefined,
            target_user_id_count: targetUsers.truncated ? targetUsers.total : undefined,
            ...gate.auditPartial,
          });
          await emitAuditLine(audit);

          // Narrow to the modern { content: [...] } shape. The MCP SDK's
          // callTool return is a union with the legacy
          // CompatibilityCallToolResult { toolResult } shape; if we ever
          // talk to an upstream that responds in the legacy shape, wrap
          // it into the modern shape so MCP clients downstream see a
          // consistent surface.
          if ("content" in upstreamResult && Array.isArray(upstreamResult.content)) {
            return upstreamResult as { content: Array<{ type: "text"; text: string }> };
          }
          return {
            content: [
              {
                type: "text" as const,
                text: JSON.stringify((upstreamResult as { toolResult?: unknown }).toolResult ?? upstreamResult),
              },
            ],
          };
        } catch (err) {
          const errMsg = redact((err as Error).message ?? String(err));
          const audit = buildAuditLine({
            agent: AGENT_NAME!,
            tool: localName,
            params_safe: makeParamsSafe(params),
            trace_id: traceId,
            duration_ms: Date.now() - start,
            denied: { reason: `upstream-error: ${errMsg.slice(0, 200)}` },
            ...gate.auditPartial,
          });
          await emitAuditLine(audit);
          return {
            content: [{ type: "text", text: `Upstream error: ${errMsg}` }],
            isError: true,
          };
        }
      },
    );
  }
}

// ── Boot ───────────────────────────────────────────────────────────────

async function main(): Promise<void> {
  try {
    await startUpstreamClient();
  } catch (err) {
    process.stderr.write(
      redact(
        `[sentry-mcp] upstream client start failed: ${(err as Error).message}\n`,
      ),
    );
  }
  registerProxiedTools();

  // Always expose at least a no-op probe tool so the MCP server surfaces
  // its name even when the upstream failed to start — that lets agents
  // see "sentry" in their tool list and get a useful 503-style error
  // instead of "namespace doesn't exist".
  if (upstreamTools.length === 0) {
    server.tool(
      "_unavailable",
      "Sentry MCP is currently unavailable (token, allowlist, or upstream issue). See stderr.",
      { params: z.record(z.string(), z.unknown()).optional() },
      async () => ({
        content: [
          {
            type: "text",
            text:
              "Sentry MCP not configured or upstream unavailable. " +
              "Inspect aperture-bus stderr for cause (missing token, missing allowlist, " +
              "upstream spawn failure).",
          },
        ],
        isError: true,
      }),
    );
  }

  const transport = new StdioServerTransport();
  await server.connect(transport);
  process.stderr.write("[sentry-mcp] server ready on stdio\n");
}

main().catch((err) => {
  process.stderr.write(
    redact(`[sentry-mcp] fatal: ${(err as Error).message ?? err}\n`),
  );
  process.exit(1);
});
