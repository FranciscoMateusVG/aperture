import type { CreateTeamSelectors } from "./team-create.js";
import type { SaveRepositorySelectors } from "./team-repositories.js";
import { spawn } from "node:child_process";
import { constants, fstatSync, lstatSync, openSync, closeSync } from "node:fs";
import { homedir } from "node:os";
import { isAbsolute, join } from "node:path";

const MAX_REQUEST_BYTES = 16 * 1024;
const MAX_STDOUT_BYTES = 1024 * 1024;
const MAX_STDERR_BYTES = 4 * 1024;
const ORDINARY_TIMEOUT_MS = 15_000;
// The native replace child owns a 170s absolute deadline and reserves its
// final 30s for cleanup. This parent watchdog is deliberately larger and is
// only a last-resort crash boundary; expiry never means rollback succeeded.
const REPLACE_TIMEOUT_MS = 180_000;
// Archive performs two bounded native projections (30s + 20s) before the
// journaled fsync/move/readback phase. Expiry is crash/unknown semantics; the
// durable journal remains the only recovery authority.
const ARCHIVE_TIMEOUT_MS = 90_000;

export interface ActivationSelectors {
  team: string;
  expected_generation: number;
  creation_request_id: string;
  epic_id: string;
}

export interface CancelSelectors {
  team: string;
  expected_generation: number;
  creation_request_id: string;
}

export interface ReplacementSelectors {
  target_seat: string;
  expected_generation: number;
  selection: {
    harness: "claude" | "codex";
    model: string;
    reasoning: "low" | "medium" | "high" | "xhigh" | "max" | "ultra" | null;
  };
}

export interface ArchiveSelectors {
  team: string;
  expected_generation: number;
}

export interface CheckpointSelectors {
  schema_version: number;
  payload: {
    task_id: string;
    worktree: string;
    branch: string;
    head_sha: string;
    dirty_files: string[];
    open_pr: null | { repository: string; number: number; head_sha: string };
    running_procs: Array<{ pid: number; start_time: string }>;
    decisions: Array<{ code: string; text: string; evidence_ref: string | null }>;
    next_step: string;
    remote_effects: Array<{ kind: string; reference: string; state: string }>;
  };
}

export interface RemoteTargetSelectors {
  target_seat: string;
  expected_generation: number;
}

export interface RemoteResolutionSelectors extends RemoteTargetSelectors {
  resolution: {
    expected_inventory_hash: string;
    scope: "effect_resolution" | "inventory_risk_acceptance";
    reference: string | null;
    decision: "finished" | "cancelled" | "proceed_with_unobserved_effects";
    evidence_ref: string;
  };
}

export type TeamControlRequest =
  | { action: "catalog" }
  | { action: "list_repositories" }
  | { action: "save_repository"; input: SaveRepositorySelectors }
  | { action: "create"; input: CreateTeamSelectors }
  | { action: "list_pending" }
  | { action: "list_teams" }
  | { action: "bootstrap_seat"; input: { team: string; seat: string; expected_generation: number } }
  | { action: "approve"; input: ActivationSelectors }
  | { action: "cancel"; input: CancelSelectors }
  | { action: "checkpoint"; input: CheckpointSelectors }
  | { action: "inspect_remote"; input: RemoteTargetSelectors }
  | { action: "resolve_remote"; input: RemoteResolutionSelectors }
  | { action: "replace"; input: ReplacementSelectors }
  | { action: "archive"; input: ArchiveSelectors }
  | { action: "rollback_archive"; input: ArchiveSelectors };

export interface PendingTeamView {
  snapshot: {
    team: string;
    project: string;
    repo: string;
    creation_request_id: string;
  };
  state: { state: "pending"; generation: number };
}

export interface PendingListResponse {
  action: "list_pending";
  result: PendingTeamView[];
}

function binaryPath(): string {
  const configured = process.env.APERTURE_TEAM_CONTROL_BIN;
  const path = configured ?? join(homedir(), ".aperture", "bin", "aperture-team-control");
  if (!isAbsolute(path)) throw new Error("E_CONTROL_UNAVAILABLE: team control binary path is not absolute");
  return path;
}

function validateBinary(path: string): void {
  const entry = lstatSync(path);
  if (!entry.isFile() || entry.isSymbolicLink() || entry.nlink !== 1) {
    throw new Error("E_CONTROL_UNAVAILABLE: unsafe team control binary");
  }
  if (typeof process.getuid === "function" && entry.uid !== process.getuid()) {
    throw new Error("E_CONTROL_UNAVAILABLE: team control binary owner mismatch");
  }
  if ((entry.mode & 0o022) !== 0 || (entry.mode & 0o111) === 0) {
    throw new Error("E_CONTROL_UNAVAILABLE: unsafe team control binary permissions");
  }
  const fd = openSync(path, constants.O_RDONLY | constants.O_NOFOLLOW);
  try {
    const opened = fstatSync(fd);
    if (opened.dev !== entry.dev || opened.ino !== entry.ino) {
      throw new Error("E_CONTROL_UNAVAILABLE: team control binary changed during validation");
    }
  } finally {
    closeSync(fd);
  }
}

function parseObject(text: string): Record<string, unknown> {
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch {
    throw new Error("E_CONTROL_FAILED: malformed team control response");
  }
  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
    throw new Error("E_CONTROL_FAILED: malformed team control response");
  }
  return parsed as Record<string, unknown>;
}

export function teamControlWatchdogMs(action: TeamControlRequest["action"]): number {
  return (action === "replace" || action === "bootstrap_seat") ? REPLACE_TIMEOUT_MS : action === "archive" || action === "rollback_archive" ? ARCHIVE_TIMEOUT_MS : ORDINARY_TIMEOUT_MS;
}

function killControlProcessGroup(child: ReturnType<typeof spawn>, isolated: boolean): void {
  if (isolated && child.pid !== undefined) {
    try {
      process.kill(-child.pid, "SIGKILL");
      return;
    } catch {
      // The group may have exited between the timer and signal. Fall through
      // to the exact child without treating either path as cleanup proof.
    }
  }
  child.kill("SIGKILL");
}

export async function invokeTeamControl(request: TeamControlRequest): Promise<Record<string, unknown>> {
  const input = `${JSON.stringify(request)}\n`;
  if (Buffer.byteLength(input) > MAX_REQUEST_BYTES) {
    throw new Error("E_CONTROL_INVALID: team control request exceeds size limit");
  }
  const path = binaryPath();
  validateBinary(path);
  return await new Promise((resolve, reject) => {
    const isolated = request.action === "bootstrap_seat" || request.action === "replace" || request.action === "archive" || request.action === "rollback_archive";
    const child = spawn(path, [], {
      stdio: ["pipe", "pipe", "pipe"],
      env: process.env,
      detached: isolated,
    });
    const stdout: Buffer[] = [];
    let stdoutBytes = 0;
    let stderrBytes = 0;
    let overflow = false;
    let timedOut = false;
    const timer = setTimeout(() => {
      timedOut = true;
      killControlProcessGroup(child, isolated);
    }, teamControlWatchdogMs(request.action));
    child.stdout.on("data", (chunk: Buffer) => {
      stdoutBytes += chunk.length;
      if (stdoutBytes > MAX_STDOUT_BYTES) {
        overflow = true;
        child.kill("SIGKILL");
      } else {
        stdout.push(chunk);
      }
    });
    child.stderr.on("data", (chunk: Buffer) => {
      stderrBytes += chunk.length;
      if (stderrBytes > MAX_STDERR_BYTES) child.kill("SIGKILL");
    });
    child.on("error", () => {
      clearTimeout(timer);
      reject(new Error("E_CONTROL_UNAVAILABLE: team control process failed to start"));
    });
    child.on("close", (code, signal) => {
      clearTimeout(timer);
      if (timedOut) {
        reject(new Error("E_CONTROL_UNKNOWN: team control outcome is incomplete; explicit reconciliation is required"));
        return;
      }
      if (overflow || stderrBytes > MAX_STDERR_BYTES) {
        reject(new Error("E_CONTROL_FAILED: team control output exceeded limit"));
        return;
      }
      const response = parseObject(Buffer.concat(stdout).toString("utf8"));
      if (code !== 0 || signal) {
        const errorCode = typeof response.code === "string" ? response.code : "E_CONTROL_FAILED";
        const message = typeof response.message === "string" ? response.message : "team control rejected the request";
        reject(new Error(`${errorCode}: ${message}`));
        return;
      }
      resolve(response);
    });
    child.stdin.end(input);
  });
}

export function parsePendingList(value: Record<string, unknown>): PendingListResponse {
  if (value.action !== "list_pending" || !Array.isArray(value.result)) {
    throw new Error("E_CONTROL_FAILED: unexpected pending-team response");
  }
  for (const item of value.result) {
    if (!item || typeof item !== "object" || Array.isArray(item)) {
      throw new Error("E_CONTROL_FAILED: malformed pending-team entry");
    }
    const team = item as Record<string, unknown>;
    const snapshot = team.snapshot as Record<string, unknown> | undefined;
    const state = team.state as Record<string, unknown> | undefined;
    if (
      !snapshot || !state ||
      typeof snapshot.team !== "string" ||
      typeof snapshot.project !== "string" ||
      typeof snapshot.repo !== "string" ||
      typeof snapshot.creation_request_id !== "string" ||
      state.state !== "pending" ||
      !Number.isSafeInteger(state.generation)
    ) {
      throw new Error("E_CONTROL_FAILED: malformed pending-team entry");
    }
  }
  return value as unknown as PendingListResponse;
}

export function assertActivationMatchesPending(
  pending: PendingListResponse,
  input: ActivationSelectors,
): PendingTeamView {
  const matches = pending.result.filter((team) => team.snapshot.team === input.team);
  if (matches.length !== 1) throw new Error("E_CONTROL_STALE: pending team is missing or ambiguous");
  const match = matches[0];
  if (
    match.state.generation !== input.expected_generation ||
    match.snapshot.creation_request_id !== input.creation_request_id
  ) {
    throw new Error("E_CONTROL_STALE: activation request changed");
  }
  return match;
}

export function assertAuthorizedEpic(raw: string, epicId: string, project: string): void {
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    throw new Error("E_CONTROL_EPIC_INVALID: epic lookup was malformed");
  }
  const values = Array.isArray(parsed) ? parsed : [parsed];
  if (values.length !== 1 || !values[0] || typeof values[0] !== "object" || Array.isArray(values[0])) {
    throw new Error("E_CONTROL_EPIC_INVALID: epic lookup was missing or ambiguous");
  }
  const task = values[0] as Record<string, unknown>;
  const labels = Array.isArray(task.labels) ? task.labels : [];
  if (
    task.id !== epicId ||
    task.issue_type !== "epic" ||
    !["open", "in_progress"].includes(String(task.status)) ||
    !labels.includes(project)
  ) {
    throw new Error("E_CONTROL_EPIC_INVALID: epic is not active and authorized for this project");
  }
}
