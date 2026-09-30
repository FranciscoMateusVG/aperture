// aperture-fr859: one place to turn a thrown value into operator-readable text.
// The web transport throws plain `{ code, message }` objects (never Error
// instances); rendering those with `String(err)` produced "[object Object]"
// on the agent controls. Codes are the closed allowlist the backend passes
// through (web_server.rs LEGACY_ERROR_CODES); everything else falls back to
// the backend's fixed message plus the code, so the discriminant is always
// visible and no detail beyond what the backend chose is ever shown.

const CODE_COPY: Readonly<Record<string, string>> = {
  E_CODEX_HOME_UNVERIFIED: "codex home could not be verified",
  E_CODEX_LAUNCH_INPUTS_UNVERIFIED: "codex launch inputs could not be verified",
  E_CODEX_WAIT_UNKNOWN: "previous codex process has not been reaped; inspect before retry",
  E_COORDINATOR_SELF_STOP: "stopping this agent would also stop the Aperture server",
  E_LIFECYCLE_DESCENDANTS_UNVERIFIED: "stop/restart blocked: child processes could not be verified",
  E_LIFECYCLE_OUTCOME_UNKNOWN: "lifecycle outcome unknown; inspect before retry",
  E_LIFECYCLE_PROCESS_UNKNOWN: "agent process could not be identified",
  E_LOCAL_PROMPT_UNAVAILABLE: "agent prompt is unavailable",
  E_LOCAL_THREAD_UNVERIFIED: "retained conversation thread could not be verified",
  E_LOCAL_TOOL_MISSING: "a required local tool is missing",
  E_RUNTIME_SELECTOR: "invalid agent or session selector",
  E_TMUX_OUTCOME_UNKNOWN: "tmux outcome unknown; inspect before retry",
  E_TMUX_UNVERIFIED: "tmux window could not be created or verified",
};

const UNKNOWN = "operation failed (unknown error)";

function nonEmpty(v: unknown): v is string {
  return typeof v === "string" && v.length > 0;
}

export function describeError(err: unknown): string {
  if (err instanceof Error) return nonEmpty(err.message) ? err.message : UNKNOWN;
  if (nonEmpty(err)) return err;
  if (typeof err !== "object" || err === null || Array.isArray(err)) return UNKNOWN;
  const { code, message } = err as { code?: unknown; message?: unknown };
  if (!nonEmpty(code)) return UNKNOWN;
  // Own-property lookup only: a code like "__proto__" must not resolve to an
  // inherited object (which would render as "[object Object]" again).
  const copy = Object.prototype.hasOwnProperty.call(CODE_COPY, code)
    ? CODE_COPY[code]
    : nonEmpty(message) ? message : "operation failed";
  return `${copy} (${code})`;
}
