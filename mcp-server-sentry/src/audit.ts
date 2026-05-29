// Audit emission — R5 (target_user_id cap) + R9 (audit-fail fallback) of
// Cipher's wiring contract (aperture-ttzz).
//
// Every mcp__sentry__* call emits one `agent.sentry_query` line to Loki at
// the end of execution (success OR refusal). The line shape mirrors Rex's
// `coordenador.pii_access` from PR #229 (incluir) for cross-channel
// consistency:
//
//   {
//     event: "agent.sentry_query",
//     ts: ISO8601,
//     agent: string,
//     tool: string,
//     params_safe: object        // params with auth/token stripped
//     trace_id: string,
//     target_user_id: string[]   // capped at 10 — see R5
//     target_user_id_truncated?: true
//     target_user_id_count?: N   // full count when truncated
//     result_count: number,
//     duration_ms: number,
//     operator_approved?: boolean
//     approval_message_id?: string
//     has_justification?: boolean
//     denied?: { reason: string }
//   }
//
// NEVER LOGGED in this body: justification text (R2), user emails / names /
// usernames (LGPD), full token, attachment contents.
//
// R9 fallback: Loki push failure → stderr emission only (NOT a retry queue —
// don't block the tool call). The audit-fail alert rule
// (monitoring/sentry-mcp-audit-fail.alert.yml) watches for these stderr
// lines and notifies the operator within 5 min.

import { redact, redactObject } from "./redact.js";

const LOKI_URL = process.env.LOKI_URL ?? "http://localhost:3100";
const LOKI_PUSH_PATH = "/loki/api/v1/push";
const SERVICE_LABEL = "aperture-bus-sentry";

const TARGET_USER_ID_CAP = 10;

export interface AuditLine {
  event: "agent.sentry_query";
  ts: string;
  agent: string;
  tool: string;
  params_safe: Record<string, unknown>;
  trace_id: string;
  target_user_id?: string[];
  target_user_id_truncated?: true;
  target_user_id_count?: number;
  result_count?: number;
  duration_ms: number;
  operator_approved?: boolean;
  approval_message_id?: string;
  has_justification?: boolean;
  denied?: { reason: string };
}

/**
 * Extract user IDs from a Sentry tool response. Sentry tool responses come
 * in several shapes:
 *   - { issues: [{ user: { id } | userId, ... }] }
 *   - { events: [{ user: { id }, ... }] }
 *   - { user: { id } } (single)
 *   - bare arrays with embedded users
 *
 * R5: dedupe, cap at 10, set truncation flag when the full set was larger.
 * NEVER include email / username / name fields — those are PII fingerprints.
 */
export function extractTargetUserIds(response: unknown): {
  user_ids: string[];
  truncated: boolean;
  total: number;
} {
  const all = new Set<string>();
  walk(response, all);
  const total = all.size;
  if (total <= TARGET_USER_ID_CAP) {
    return { user_ids: [...all], truncated: false, total };
  }
  // Deterministic truncation — take iteration order (insertion order from
  // walk traversal) and slice. Cap at 10.
  const capped = [...all].slice(0, TARGET_USER_ID_CAP);
  return { user_ids: capped, truncated: true, total };
}

function walk(node: unknown, out: Set<string>): void {
  if (node === null || node === undefined) return;
  if (typeof node !== "object") return;
  if (Array.isArray(node)) {
    for (const item of node) walk(item, out);
    return;
  }
  const obj = node as Record<string, unknown>;
  // Direct user.id field
  if (obj.user && typeof obj.user === "object") {
    const userId = (obj.user as Record<string, unknown>).id;
    if (typeof userId === "string" && userId.length > 0) out.add(userId);
    else if (typeof userId === "number") out.add(String(userId));
  }
  // Top-level userId / user_id on the record itself
  for (const field of ["userId", "user_id"]) {
    const v = obj[field];
    if (typeof v === "string" && v.length > 0) out.add(v);
    else if (typeof v === "number") out.add(String(v));
  }
  // Recurse — Sentry responses nest arbitrarily
  for (const key of Object.keys(obj)) {
    if (key === "user" || key === "userId" || key === "user_id") continue;
    walk(obj[key], out);
  }
}

/**
 * Strip auth/token fields from params before audit emission. Belt and
 * braces — gates.ts checks the param shape too, but this layer enforces
 * a known-blocklist at the audit boundary as defense in depth.
 */
export function makeParamsSafe(params: Record<string, unknown>): Record<string, unknown> {
  // All entries kept lowercase — comparison is case-insensitive via
  // k.toLowerCase() below so both camelCase and snake_case auth fields
  // get stripped.
  const STRIPPED_KEYS = new Set([
    "authorization",
    "auth",
    "token",
    "access_token",
    "accesstoken",
    "bearer",
    "api_key",
    "apikey",
    // R2: justification text never goes to Loki. Has-flag set elsewhere.
    "justification",
  ]);
  const safe: Record<string, unknown> = {};
  for (const [k, v] of Object.entries(params ?? {})) {
    if (STRIPPED_KEYS.has(k.toLowerCase())) continue;
    safe[k] = v;
  }
  return redactObject(safe);
}

export function buildAuditLine(input: Partial<AuditLine> & {
  agent: string;
  tool: string;
  trace_id: string;
  duration_ms: number;
}): AuditLine {
  return {
    event: "agent.sentry_query",
    ts: new Date().toISOString(),
    params_safe: {},
    ...input,
  };
}

/**
 * Best-effort Loki push. Failure → stderr emission only (R9). NEVER throws.
 */
export async function emitAuditLine(line: AuditLine): Promise<void> {
  const redactedLine = redactObject(line);
  const bodyLine = JSON.stringify(redactedLine);
  const tsNanos = String(Date.now()) + "000000";

  const payload = {
    streams: [
      {
        stream: {
          service: SERVICE_LABEL,
          event: "agent.sentry_query",
          agent: redactedLine.agent,
        },
        values: [[tsNanos, bodyLine]],
      },
    ],
  };

  try {
    const res = await fetch(`${LOKI_URL}${LOKI_PUSH_PATH}`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(payload),
    });
    if (!res.ok) {
      // Stay non-throwing — R9 says audit blackout surfaces via stderr.
      // Read body but redact in case Loki echoes any auth headers.
      const errText = await res.text().catch(() => "");
      process.stderr.write(
        redact(
          `[sentry-mcp] audit emission failed: status=${res.status} ` +
            `body=${errText.slice(0, 200)}\n`,
        ),
      );
    }
  } catch (err) {
    process.stderr.write(
      redact(`[sentry-mcp] audit emission failed: ${(err as Error).message ?? err}\n`),
    );
  }
}
