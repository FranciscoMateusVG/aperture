// Operator approval flow — R1 of Cipher's wiring contract (aperture-ttzz).
//
// When an agent invokes a mutation tool (autofix_issue, update_issue, etc.)
// or an attachment tool (get_event_attachment), the wrap layer:
//
//   1. Files a BEADS message issue (type=message) addressed to "operator"
//      with a structured request body. The aperture poller lights an
//      attention badge in the launcher.
//
//   2. Polls BEADS every 5s for an unread reply from "operator" to the
//      calling agent whose body contains a sentinel:
//        - "approved: <request-id>"                    → proceed
//        - "rejected: <request-id>: <short reason>"    → deny with reason
//
//   3. Times out after 10 minutes. Returns "approval not granted, request
//      expired" — call refused, audit line carries denied.reason.
//
// This module does NOT use the aperture-bus MCP server's send_message
// (avoids circular dep on its own MCP boundary). It shells to the `bd`
// CLI exactly the same way the aperture-bus's createMessage / getUnread
// helpers do, using BEADS's type=message convention (title format
// `[from->to] preview`, status=open = unread, status=closed = read).
//
// Cipher review note: this flow is fail-closed by design. Any failure to
// reach BEADS (bd CLI missing, sqlite locked, etc.) returns "timeout" so
// the audit denied.reason is consistent and the operator can investigate.

import { execFile } from "child_process";
import { randomBytes } from "crypto";
import { promisify } from "util";
import { redact } from "./redact.js";

const execFileP = promisify(execFile);

const APPROVAL_TIMEOUT_MS = 10 * 60 * 1000; // 10 min — Cipher R1 cap
const POLL_INTERVAL_MS = 5_000;
const HEARTBEAT_INTERVAL_MS = 60_000;

// The agent identity the wrap layer uses to FILE approval-request messages.
// Set at server boot from the AGENT_NAME env var. Defaults to "peppy"
// because the wrap layer is owned by Peppy, but the override exists for
// test contexts and future per-agent split.
let WRAP_LAYER_AGENT_NAME = process.env.AGENT_NAME ?? "peppy";
export function setWrapLayerAgentName(name: string): void {
  WRAP_LAYER_AGENT_NAME = name;
}

export interface ApprovalRequest {
  request_id: string;
  agent: string;
  tool: string;
  params_safe: Record<string, unknown>;
  justification?: string;
  requested_at: string;
  expires_at: string;
}

export type ApprovalResult =
  | { status: "approved"; approval_message_id: string }
  | { status: "rejected"; reason: string; approval_message_id: string }
  | { status: "timeout" };

export function newRequestId(): string {
  return "appr-" + randomBytes(4).toString("hex");
}

/**
 * File the approval-request BEADS message. Uses the same shape as
 * aperture-bus/createMessage: type=message, title `[from->to] preview`,
 * description = body. Returns the issue ID on success.
 *
 * Throws on any bd failure — caller turns this into a fail-closed
 * "timeout" result so the agent sees a consistent error shape.
 */
async function fileApprovalRequest(
  callerAgent: string,
  request: ApprovalRequest,
): Promise<string> {
  const body = formatApprovalBody(callerAgent, request);
  const preview = body.slice(0, 60).replace(/\n/g, " ");
  const title = `[${WRAP_LAYER_AGENT_NAME}->operator] ${preview}`;
  const args = [
    "create",
    title,
    "-p",
    "3",
    "--type",
    "message",
    "-d",
    body,
    "--label",
    "project:aperture",
    "--json",
  ];
  const { stdout } = await execFileP("bd", args, { timeout: 30_000 });
  try {
    const parsed = JSON.parse(stdout);
    return parsed.id ?? "unknown";
  } catch {
    return "unknown";
  }
}

function formatApprovalBody(callerAgent: string, request: ApprovalRequest): string {
  const lines = [
    "# Sentry MCP — operator approval requested",
    "",
    "Request: `" + request.request_id + "`",
    "Agent: `" + callerAgent + "`",
    "Tool: `" + request.tool + "`",
    "Requested at: " + request.requested_at,
    "Expires at: " + request.expires_at,
    "",
    "Params (auth/token redacted):",
    "```json",
    JSON.stringify(request.params_safe, null, 2),
    "```",
    "",
  ];
  if (request.justification) {
    lines.push("Justification (transient — NOT persisted to Loki audit):");
    lines.push("> " + request.justification.replace(/\n/g, "\n> "));
    lines.push("");
  }
  lines.push(
    "**To approve, reply** (as a BEADS message from `operator` to `" +
      callerAgent +
      "`):",
    "    approved: " + request.request_id,
    "",
    "**To reject, reply:**",
    "    rejected: " + request.request_id + ": <short reason>",
    "",
    "If no reply within 10 minutes, the request expires automatically and the tool call is refused.",
  );
  return lines.join("\n");
}

/**
 * Poll for an operator reply matching the request_id. Looks at unread
 * type=message issues whose title starts with `[operator->${callerAgent}]`.
 *
 * Match precedence: approval first, then rejection. Either match closes
 * the message (marks-as-read).
 */
async function pollForApproval(
  callerAgent: string,
  requestId: string,
  deadlineMs: number,
): Promise<ApprovalResult> {
  let lastHeartbeat = Date.now();
  while (Date.now() < deadlineMs) {
    const messages = await listUnreadMessages(callerAgent);
    for (const msg of messages) {
      const body = msg.description ?? "";
      const id = msg.id ?? "";
      if (!body.includes(requestId)) continue;

      if (new RegExp(`approved:\\s*${escapeRegex(requestId)}`).test(body)) {
        await markRead(id);
        return { status: "approved", approval_message_id: id };
      }
      // INTENTIONAL: `(.+?)$` with the `m` flag captures up to end-of-LINE,
      // not end-of-body. If the operator writes a multi-line reject reason,
      // only the first line is captured. This is by design (aperture-hyyj,
      // Cipher's R1-R9 sign-off observation 2026-05-14):
      //   - The .slice(0, 500) below caps reason length anyway
      //   - The contract is that reject reasons are SHORT LABELS, not
      //     discussions ("not allowlisted", "wrong project", etc.)
      //   - Multi-line capture would let an operator paste a 10KB stack
      //     trace into a reject and have it land verbatim in the audit
      //     line, defeating the implicit length-control
      // If you're tempted to "fix" this into a multi-line regex, file
      // a follow-up bead first and route through Cipher.
      const rejectMatch = body.match(
        new RegExp(`rejected:\\s*${escapeRegex(requestId)}\\s*:\\s*(.+?)$`, "m"),
      );
      if (rejectMatch) {
        await markRead(id);
        return {
          status: "rejected",
          reason: rejectMatch[1].trim().slice(0, 500),
          approval_message_id: id,
        };
      }
    }

    if (Date.now() - lastHeartbeat >= HEARTBEAT_INTERVAL_MS) {
      process.stderr.write(
        redact(
          `[sentry-mcp] still waiting for operator approval (request=${requestId})\n`,
        ),
      );
      lastHeartbeat = Date.now();
    }

    await sleep(POLL_INTERVAL_MS);
  }
  return { status: "timeout" };
}

interface BeadsMessage {
  id?: string;
  title?: string;
  description?: string;
}

async function listUnreadMessages(callerAgent: string): Promise<BeadsMessage[]> {
  // Mirror aperture-bus/getUnreadMessages exactly: bd query for unread
  // type=message issues whose title contains `->${callerAgent}]`.
  const queryStr = `type=message AND status=open AND title="->${callerAgent}]"`;
  try {
    const { stdout } = await execFileP(
      "bd",
      ["query", queryStr, "--json", "-n", "0"],
      { timeout: 30_000 },
    );
    const parsed = JSON.parse(stdout);
    if (!Array.isArray(parsed)) return [];
    // Defensive: only return messages whose title prefix shows operator->.
    return parsed.filter(
      (m: BeadsMessage) =>
        typeof m.title === "string" && m.title.startsWith("[operator->"),
    );
  } catch (err) {
    process.stderr.write(
      redact(
        `[sentry-mcp] bd query failed during approval poll: ${(err as Error).message}\n`,
      ),
    );
    return [];
  }
}

async function markRead(messageId: string): Promise<void> {
  if (!messageId) return;
  try {
    await execFileP("bd", ["close", messageId, "--reason", "delivered", "--json"], {
      timeout: 30_000,
    });
  } catch (err) {
    process.stderr.write(
      redact(
        `[sentry-mcp] bd close failed for approval reply ${messageId}: ` +
          `${(err as Error).message}\n`,
      ),
    );
  }
}

function escapeRegex(s: string): string {
  return s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

export async function requestApproval(args: {
  callerAgent: string;
  tool: string;
  params_safe: Record<string, unknown>;
  justification?: string;
}): Promise<ApprovalResult> {
  const requestedAtMs = Date.now();
  const requestId = newRequestId();
  const request: ApprovalRequest = {
    request_id: requestId,
    agent: args.callerAgent,
    tool: args.tool,
    params_safe: args.params_safe,
    justification: args.justification,
    requested_at: new Date(requestedAtMs).toISOString(),
    expires_at: new Date(requestedAtMs + APPROVAL_TIMEOUT_MS).toISOString(),
  };

  try {
    await fileApprovalRequest(args.callerAgent, request);
  } catch (err) {
    process.stderr.write(
      redact(
        `[sentry-mcp] approval channel unreachable: ${(err as Error).message}\n`,
      ),
    );
    return { status: "timeout" };
  }

  return pollForApproval(args.callerAgent, requestId, requestedAtMs + APPROVAL_TIMEOUT_MS);
}
