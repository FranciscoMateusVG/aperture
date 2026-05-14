// Gates — R3 (fail-closed allowlist), R4 (param-name coverage),
// R2 (justification shape), plus the per-tool gate classification used
// by the proxy in index.ts to decide whether a call needs operator
// approval (R1, delegated to approval.ts).
//
// Cipher's wiring contract for aperture-ttzz pins these constraints to
// PR-blocking unit tests in tests/allowlist.test.ts and
// tests/justification.test.ts. If the param-name list, the mutation tool
// list, or the attachment tool list ever drifts from what Sentry MCP
// actually accepts, the wrap layer defaults to DENY (R4 final clause).

import * as fs from "fs";
import * as yaml from "js-yaml";

// ── Tool classification ──────────────────────────────────────────────

/**
 * Tools that perform mutations on the Sentry side. Calling any of these
 * routes through approval.ts (R1) — operator must approve via BEADS,
 * 10-min timeout, denied calls become an audit line with denied.reason.
 *
 * If a future @sentry/mcp-server version adds a new mutation tool not in
 * this list, the wrap layer auto-classifies any tool whose name starts
 * with `update_`, `create_`, `delete_`, `autofix_`, `resolve_`, or `ignore_`
 * as mutation (defense in depth — see classifyTool below).
 */
export const KNOWN_MUTATION_TOOLS = new Set<string>([
  "autofix_issue",
  "update_issue",
  "create_issue",
  "delete_issue",
  "resolve_issue",
  "ignore_issue",
  "update_project",
  "create_project",
]);

/**
 * Tools that return raw user-uploaded content (screenshots, breadcrumb
 * attachments, source maps from upload, etc). These bypass field-level
 * redaction (binary or arbitrary text) so per-call operator approval is
 * required (R2 — justification mandatory + R1 — approval).
 */
export const KNOWN_ATTACHMENT_TOOLS = new Set<string>([
  "get_event_attachment",
]);

export type ToolClass = "read" | "mutation" | "attachment";

/**
 * Classify a tool name. Unknown tools default to "read" UNLESS their name
 * matches a mutation-suggestive prefix — in which case they default to
 * "mutation" so a new Sentry tool can't silently sneak in past the gate.
 */
export function classifyTool(toolName: string): ToolClass {
  if (KNOWN_ATTACHMENT_TOOLS.has(toolName)) return "attachment";
  if (KNOWN_MUTATION_TOOLS.has(toolName)) return "mutation";
  const lower = toolName.toLowerCase();
  // R4 defense-in-depth: any new mutation-shaped tool name defaults to
  // mutation classification until explicitly classified.
  if (
    /^(?:update|create|delete|autofix|resolve|ignore|set|enable|disable)_/.test(lower)
  ) {
    return "mutation";
  }
  if (lower.startsWith("get_") && lower.includes("attachment")) {
    return "attachment";
  }
  return "read";
}

// ── Project allowlist (R3 fail-closed + R4 param coverage) ───────────

/**
 * R4 — every project-identifying param name we know Sentry MCP tools use.
 * Singular, plural, snake_case, camelCase, slug variants, organisation
 * variants. If a new param name appears in a future tool surface, the
 * defensive regex below catches it and defaults to DENY until this list
 * is updated.
 */
export const PROJECT_PARAM_NAMES: readonly string[] = [
  "project",
  "project_id",
  "projectId",
  "projects",
  "project_slug",
  "projectSlug",
  "org_slug",
  "orgSlug",
  "organization",
  "organization_slug",
  "organizationSlug",
];

/**
 * Defensive shape-matcher — any param name that LOOKS like a project /
 * organisation identifier but is not in PROJECT_PARAM_NAMES triggers
 * DENY. Aggressive on purpose per R4 ("defaults to DENY until the list
 * is updated"): false-positive denial of an unknown param shape is the
 * preferred failure mode over silently allowing a new project-id surface
 * past the gate.
 *
 * Matches any name that starts with `project`, `org`, or `organization`.
 * Yes, this catches words like `projection` or `original` — those are
 * not real Sentry tool param names, and a false-positive denial is
 * cheaper than a silent allowlist bypass.
 */
const PROJECT_SHAPE_REGEX = /^(?:project|org(?:anization)?)/i;

export interface AgentTier {
  /** Agents that can call read tools without per-call gating. */
  default_on: string[];
  /** Agents that need explicit operator approval per session. */
  opt_in: string[];
}

export interface AllowlistConfig {
  project_allowlist: string[];
  agent_default_on: string[];
  agent_opt_in: string[];
}

/**
 * R3 — load the allowlist from disk. Missing file, unreadable file, empty
 * project list, or malformed YAML → return null. Caller must treat null
 * as "Sentry MCP not configured" and refuse ALL tool calls.
 *
 * Default location: ~/.config/aperture/sentry-mcp-allowlist.yaml
 * Override: SENTRY_MCP_ALLOWLIST_PATH env var.
 */
export function loadAllowlist(): AllowlistConfig | null {
  const path =
    process.env.SENTRY_MCP_ALLOWLIST_PATH ??
    `${process.env.HOME ?? ""}/.config/aperture/sentry-mcp-allowlist.yaml`;

  let raw: string;
  try {
    raw = fs.readFileSync(path, "utf8");
  } catch {
    return null;
  }
  if (raw.trim().length === 0) return null;

  let parsed: unknown;
  try {
    parsed = yaml.load(raw);
  } catch {
    return null;
  }
  if (!parsed || typeof parsed !== "object") return null;
  const obj = parsed as Record<string, unknown>;

  const proj = obj.project_allowlist;
  const def = obj.agent_default_on;
  const opt = obj.agent_opt_in;

  if (!Array.isArray(proj) || proj.length === 0) return null;
  // R3 — an explicit empty project list also fails closed.

  return {
    project_allowlist: proj.filter((s): s is string => typeof s === "string"),
    agent_default_on: Array.isArray(def)
      ? def.filter((s): s is string => typeof s === "string")
      : [],
    agent_opt_in: Array.isArray(opt)
      ? opt.filter((s): s is string => typeof s === "string")
      : [],
  };
}

export type ProjectCheckResult =
  | { ok: true }
  | { ok: false; reason: string };

/**
 * R4 — verify every project-identifying param in `params` resolves to an
 * allowlisted value. Any unrecognised shape (matches the defensive regex
 * but not the known list) → DENY.
 */
export function checkProjectAllowlist(
  params: Record<string, unknown>,
  allowlist: AllowlistConfig,
): ProjectCheckResult {
  const allowed = new Set(allowlist.project_allowlist);

  for (const key of Object.keys(params)) {
    const value = params[key];
    const known = PROJECT_PARAM_NAMES.includes(key);
    const looksLikeProject = PROJECT_SHAPE_REGEX.test(key);

    if (!known && !looksLikeProject) continue;

    if (!known && looksLikeProject) {
      return {
        ok: false,
        reason:
          `Unknown project-identifying param "${key}" — wrap layer defaults to DENY ` +
          `until the allowlist param-name coverage is updated to include this shape.`,
      };
    }

    if (value === null || value === undefined) continue;

    if (Array.isArray(value)) {
      for (const v of value) {
        if (typeof v !== "string" && typeof v !== "number") {
          return { ok: false, reason: `Param "${key}" carries non-scalar value.` };
        }
        if (!allowed.has(String(v))) {
          return {
            ok: false,
            reason:
              `Project "${v}" (via param "${key}") not in allowlist. ` +
              `Allowed: ${[...allowed].join(", ")}.`,
          };
        }
      }
      continue;
    }

    if (typeof value !== "string" && typeof value !== "number") {
      return { ok: false, reason: `Param "${key}" carries non-scalar value.` };
    }
    if (!allowed.has(String(value))) {
      return {
        ok: false,
        reason:
          `Project "${value}" (via param "${key}") not in allowlist. ` +
          `Allowed: ${[...allowed].join(", ")}.`,
      };
    }
  }

  return { ok: true };
}

// ── Agent gate ───────────────────────────────────────────────────────

export type AgentCheckResult =
  | { ok: true; tier: "default_on" | "opt_in" }
  | { ok: false; reason: string };

export function checkAgentAllowed(
  agentName: string,
  allowlist: AllowlistConfig,
): AgentCheckResult {
  if (allowlist.agent_default_on.includes(agentName)) {
    return { ok: true, tier: "default_on" };
  }
  if (allowlist.agent_opt_in.includes(agentName)) {
    return { ok: true, tier: "opt_in" };
  }
  return {
    ok: false,
    reason: `Agent "${agentName}" not in Sentry MCP allowlist (default_on or opt_in).`,
  };
}

// ── Justification gate (R2) ──────────────────────────────────────────

export type JustificationCheckResult =
  | { ok: true; text: string }
  | { ok: false; reason: string };

const JUSTIFICATION_MIN = 20;
const JUSTIFICATION_MAX = 500;

/**
 * R2 — get_event_attachment requires a `justification: string` param with
 * 20 ≤ length ≤ 500. The text is delivered to the operator inside the
 * BEADS approval message but is NEVER logged to Loki — only the
 * `has_justification: true` flag.
 */
export function checkJustification(
  params: Record<string, unknown>,
): JustificationCheckResult {
  const j = params.justification;
  if (typeof j !== "string") {
    return {
      ok: false,
      reason:
        `Attachment tool requires a "justification" string param ` +
        `(${JUSTIFICATION_MIN}–${JUSTIFICATION_MAX} chars) explaining why the attachment is needed.`,
    };
  }
  const trimmed = j.trim();
  if (trimmed.length < JUSTIFICATION_MIN) {
    return {
      ok: false,
      reason: `Justification too short — minimum ${JUSTIFICATION_MIN} characters (got ${trimmed.length}).`,
    };
  }
  if (trimmed.length > JUSTIFICATION_MAX) {
    return {
      ok: false,
      reason: `Justification too long — maximum ${JUSTIFICATION_MAX} characters (got ${trimmed.length}).`,
    };
  }
  return { ok: true, text: trimmed };
}
