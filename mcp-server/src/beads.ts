import { execFile } from "node:child_process";
import { homedir } from "node:os";
import { resolve } from "node:path";

const BEADS_DIR = resolve(homedir(), ".aperture", ".beads");
const BD_PATH = process.env.BD_PATH ?? "bd";

function getActor(): string {
  return process.env.BD_ACTOR ?? process.env.AGENT_NAME ?? "unknown";
}

function bdEnv(): NodeJS.ProcessEnv {
  return {
    ...process.env,
    BEADS_DIR,
    BD_ACTOR: getActor(),
    PATH: `/opt/homebrew/bin:/usr/local/bin:${process.env.PATH ?? ""}`,
  };
}

export function runBd(args: string[]): Promise<string> {
  return new Promise((resolve, reject) => {
    execFile(BD_PATH, args, { env: bdEnv(), timeout: 30000 }, (err, stdout, stderr) => {
      if (err) {
        reject(new Error(stderr || err.message));
      } else {
        resolve(stdout.trim());
      }
    });
  });
}

export type TaskType = "task" | "bug" | "feature" | "chore" | "epic";

export interface CreateTaskOptions {
  type?: TaskType;
  labels?: string[];
  assignee?: string;
  acceptance?: string;
  blockedBy?: string[];
}

/**
 * Parse `bd create --json` output and return the new task's id.
 * `bd create` emits a single JSON object; in some configurations it can emit
 * a multi-line wrapper. Be tolerant.
 */
export function extractTaskId(raw: string): string | undefined {
  try {
    const parsed = JSON.parse(raw);
    if (parsed && typeof parsed === "object") {
      if (typeof (parsed as Record<string, unknown>).id === "string") {
        return (parsed as Record<string, string>).id;
      }
      // Some bd versions wrap the new task under .issue or .task
      const wrapped = (parsed as Record<string, unknown>).issue ?? (parsed as Record<string, unknown>).task;
      if (wrapped && typeof wrapped === "object" && typeof (wrapped as Record<string, unknown>).id === "string") {
        return (wrapped as Record<string, string>).id;
      }
    }
  } catch {
    // fall through
  }
  return undefined;
}

export async function createTask(
  title: string,
  priority: number,
  description?: string,
  options?: CreateTaskOptions,
): Promise<string> {
  const args = ["create", title, "-p", String(priority), "--json"];
  if (description) {
    args.push("-d", description);
  }
  if (options?.type) {
    args.push("--type", options.type);
  }
  if (options?.labels && options.labels.length > 0) {
    // bd accepts -l with a comma-separated list
    args.push("-l", options.labels.join(","));
  }
  if (options?.assignee) {
    args.push("--assignee", options.assignee);
  }
  if (options?.acceptance) {
    args.push("--acceptance", options.acceptance);
  }

  const result = await runBd(args);

  // Add blocked_by dependencies after creation. We need the new task id.
  const blockedBy = options?.blockedBy ?? [];
  if (blockedBy.length > 0) {
    const newId = extractTaskId(result);
    if (newId) {
      for (const blockerId of blockedBy) {
        try {
          await runBd(["dep", "add", newId, blockerId]);
        } catch (e: any) {
          // Surface the error but keep the task: the agent can retry the dep
          // separately rather than have the whole call fail.
          throw new Error(
            `Task ${newId} created but failed to add dependency on ${blockerId}: ${e.message}`,
          );
        }
      }
    } else {
      throw new Error(
        "Task created but could not parse new task ID from bd output to attach blocked_by dependencies.",
      );
    }
  }

  return result;
}

export interface UpdateTaskOptions {
  assignee?: string;
  addLabels?: string[];
  removeLabels?: string[];
}

export async function updateTask(
  id: string,
  flags: Record<string, string>,
  options?: UpdateTaskOptions,
): Promise<string> {
  const args = ["update", id];
  for (const [key, value] of Object.entries(flags)) {
    if (value === "") {
      args.push(`--${key}`);
    } else {
      args.push(`--${key}`, value);
    }
  }
  if (options?.assignee) {
    args.push("--assignee", options.assignee);
  }
  if (options?.addLabels && options.addLabels.length > 0) {
    for (const lbl of options.addLabels) {
      args.push("--add-label", lbl);
    }
  }
  if (options?.removeLabels && options.removeLabels.length > 0) {
    for (const lbl of options.removeLabels) {
      args.push("--remove-label", lbl);
    }
  }
  args.push("--json");
  return runBd(args);
}

export async function closeTask(id: string, reason: string): Promise<string> {
  return runBd(["close", id, "--reason", reason, "--json"]);
}

const SUMMARY_FIELDS = ["id", "title", "status", "priority", "assignee", "owner", "labels"] as const;
const TRUNCATED_FIELDS = ["description", "notes"] as const;
const TRUNCATE_AT = 200;

// Detail tier (show mode default) — see docs/context-efficiency-spec-jingp.md.
// Show is a deliberate single-task lookup: the caller needs the full work brief
// (description, acceptance) but NOT the unbounded append-only notes history.
const DETAIL_DESCRIPTION_CAP = 4000; // head-truncate: descriptions are authored-once briefs
const DETAIL_NOTES_TAIL = 3000; // TAIL-truncate: notes are chronological, recent end matters

function summarizeTask(t: Record<string, unknown>): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const f of SUMMARY_FIELDS) {
    if (t[f] !== undefined) out[f] = t[f];
  }
  let truncated = false;
  for (const f of TRUNCATED_FIELDS) {
    const v = t[f];
    if (typeof v === "string" && v.length > 0) {
      if (v.length > TRUNCATE_AT) {
        out[f] = v.slice(0, TRUNCATE_AT) + "…";
        truncated = true;
      } else {
        out[f] = v;
      }
    }
  }
  if (truncated) out._truncated = true;
  return out;
}

/**
 * Detail projection for show mode: full meta + acceptance, capped description
 * (head), capped notes (TAIL — chronological log, the recent end is what a
 * resuming agent needs), and dependencies summarized to the 200-char tier
 * (show embeds FULL records of every dependency otherwise — a child of a fat
 * epic inherits the epic's entire weight on every show).
 */
function detailTask(t: Record<string, unknown>): Record<string, unknown> {
  const out: Record<string, unknown> = { ...t };
  let truncated = false;

  const desc = t.description;
  if (typeof desc === "string" && desc.length > DETAIL_DESCRIPTION_CAP) {
    out.description =
      desc.slice(0, DETAIL_DESCRIPTION_CAP) +
      `…[description truncated: ${DETAIL_DESCRIPTION_CAP} of ${desc.length} chars — fields:"full" for complete]`;
    truncated = true;
  }

  const notes = t.notes;
  if (typeof notes === "string" && notes.length > DETAIL_NOTES_TAIL) {
    const entryCount = notes.split("\n").filter((l) => l.trim().length > 0).length;
    out.notes =
      `[notes truncated: showing last ${DETAIL_NOTES_TAIL} of ${notes.length} chars (${entryCount} total entries) — fields:"full" for complete history]\n` +
      notes.slice(-DETAIL_NOTES_TAIL);
    truncated = true;
  }

  // bd show embeds FULL records in BOTH directions: dependencies (what this
  // bead blocks on) and dependents (what depends on it). Summarize both —
  // measured 16.5KB of dependents riding along on a single Hermes bead.
  for (const key of ["dependencies", "dependents"] as const) {
    const arr = t[key];
    if (Array.isArray(arr) && arr.length > 0) {
      out[key] = arr.map((d) => {
        if (!d || typeof d !== "object") return d;
        const dep = d as Record<string, unknown>;
        const summarized = summarizeTask(dep);
        if (dep.dependency_type !== undefined) summarized.dependency_type = dep.dependency_type;
        return summarized;
      });
      truncated = true; // related-record bodies were projected away
    }
  }

  if (truncated) out._truncated = true;
  return out;
}

/**
 * Tolerantly parse a bd --json mutation echo (object or single-element array)
 * into a task record. Used to build compact acks without echoing the record.
 */
export function parseTaskRecord(raw: string): Record<string, unknown> | undefined {
  try {
    const parsed = JSON.parse(raw);
    const rec = Array.isArray(parsed) ? parsed[0] : parsed;
    if (rec && typeof rec === "object") return rec as Record<string, unknown>;
  } catch {
    // fall through
  }
  return undefined;
}

export interface UpdateAckParts {
  claim?: boolean;
  status?: string;
  description?: string;
  notes?: string;
  replaceNotes?: boolean;
  assignee?: string;
  addLabels?: string[];
  removeLabels?: string[];
  actor?: string;
}

/**
 * Compact ack for update_task — the mark_as_read pattern. The writer already
 * knows what they wrote; echoing the full accumulated record back (measured
 * 4,259 bytes for a one-line note append on a YOUNG bead, scales with record
 * size) was pure waste. See docs/context-efficiency-spec-jingp.md §(b).
 */
export function formatUpdateAck(id: string, raw: string, parts: UpdateAckParts): string {
  const rec = parseTaskRecord(raw);
  const changes: string[] = [];
  if (parts.claim) changes.push(`claimed by ${parts.actor ?? getActor()}`);
  if (parts.status) changes.push(`status → ${parts.status}`);
  if (parts.description) changes.push(`description replaced (${parts.description.length} chars)`);
  if (parts.notes) {
    changes.push(
      parts.replaceNotes
        ? `notes REPLACED (${parts.notes.length} chars)`
        : `note appended (${parts.notes.length} chars)`,
    );
  }
  if (parts.assignee) changes.push(`assignee → ${parts.assignee}`);
  if (parts.addLabels?.length) changes.push(`labels +[${parts.addLabels.join(",")}]`);
  if (parts.removeLabels?.length) changes.push(`labels -[${parts.removeLabels.join(",")}]`);
  const status = typeof rec?.status === "string" ? rec.status : "updated";
  return `Updated ${id} (${status}): ${changes.join("; ") || "no changes specified"}`;
}

/** Compact ack for close_task. */
export function formatCloseAck(id: string, raw: string, reason: string): string {
  const rec = parseTaskRecord(raw);
  const status = typeof rec?.status === "string" ? rec.status : "closed";
  const preview = reason.length > 120 ? reason.slice(0, 120) + "…" : reason;
  return `Closed ${id} (${status}): ${preview}`;
}

export type QueryFields = "summary" | "detail" | "full";

export interface QueryOptions {
  includeDone?: boolean;
  fields?: QueryFields;
  project?: string;
  assignee?: string; // "*" means no filter
  priorityMax?: number;
  label?: string;
}

function taskHasLabel(t: Record<string, unknown>, label: string): boolean {
  const labels = t.labels;
  if (!Array.isArray(labels)) return false;
  return labels.some((l) => typeof l === "string" && l === label);
}

function applyPostFilters(
  tasks: Record<string, unknown>[],
  options: QueryOptions | undefined,
): Record<string, unknown>[] {
  let out = tasks;
  if (!options?.includeDone) {
    out = out.filter((t) => t.status !== "done" && t.status !== "closed");
  }
  if (options?.project) {
    const projectLabel = `project:${options.project}`;
    out = out.filter((t) => taskHasLabel(t, projectLabel));
  }
  if (typeof options?.priorityMax === "number") {
    const max = options.priorityMax;
    out = out.filter((t) => typeof t.priority === "number" && (t.priority as number) <= max);
  }
  return out;
}

function projectFields(
  tasks: Record<string, unknown>[],
  fields: QueryFields | undefined,
): Record<string, unknown>[] {
  if (fields === "full") return tasks;
  // default: summary
  return tasks.map(summarizeTask);
}

export async function queryTasks(
  mode: string,
  id?: string,
  options?: QueryOptions,
): Promise<string> {
  if (mode === "show" && id) {
    // Show defaults to the "detail" tier: full meta + acceptance, capped
    // description (4k head), capped notes (3k TAIL), dependencies summarized.
    // fields:"full" restores the historical unconditional dump; "summary" gives
    // the cheap 200-char tier. See docs/context-efficiency-spec-jingp.md.
    const raw = await runBd(["show", id, "--json"]);
    if (options?.fields === "full") return raw;
    try {
      const parsed = JSON.parse(raw);
      const arr: Record<string, unknown>[] = Array.isArray(parsed) ? parsed : [parsed];
      const projected = arr.map((t) =>
        options?.fields === "summary" ? summarizeTask(t) : detailTask(t),
      );
      return JSON.stringify(Array.isArray(parsed) ? projected : projected[0]);
    } catch {
      return raw;
    }
  }

  const baseArgs: string[] = mode === "ready" ? ["ready", "--json"] : ["list", "--json"];

  // Pass label filters to bd when we can — narrows the JSON before we parse.
  if (options?.label) {
    baseArgs.push("--label", options.label);
  }
  // For project we also use the label flag (same machinery in bd).
  if (options?.project) {
    baseArgs.push("--label", `project:${options.project}`);
  }
  // assignee: "*" means any (skip filter). For ready mode we never auto-filter.
  if (mode !== "ready" && options?.assignee && options.assignee !== "*") {
    baseArgs.push("--assignee", options.assignee);
  }

  const raw = await runBd(baseArgs);
  try {
    let tasks: Record<string, unknown>[] = JSON.parse(raw);
    if (!Array.isArray(tasks)) return raw;
    tasks = applyPostFilters(tasks, options);
    tasks = projectFields(tasks, options?.fields);
    return JSON.stringify(tasks);
  } catch {
    return raw;
  }
}

export async function storeArtifact(
  taskId: string,
  type: string,
  value: string,
): Promise<string> {
  // Artifacts have no first-class home in bd today, so we append a tagged
  // line to the notes log. Use --append-notes (not --notes) so we don't
  // clobber existing notes content — that was the aperture-e8qp footgun.
  const artifactLine = `artifact:${type}:${value}`;
  return runBd(["update", taskId, "--append-notes", artifactLine, "--json"]);
}

export async function searchTasks(
  options?: QueryOptions,
): Promise<string> {
  const args = ["list", "--json"];
  if (options?.label) {
    args.push("--label", options.label);
  }
  if (options?.project) {
    args.push("--label", `project:${options.project}`);
  }
  if (options?.assignee && options.assignee !== "*") {
    args.push("--assignee", options.assignee);
  }
  const raw = await runBd(args);
  try {
    let tasks: Record<string, unknown>[] = JSON.parse(raw);
    if (!Array.isArray(tasks)) return raw;
    tasks = applyPostFilters(tasks, options);
    tasks = projectFields(tasks, options?.fields);
    return JSON.stringify(tasks);
  } catch {
    return raw;
  }
}

// ── BEADS Message Bus ──

/**
 * Create a BEADS message record.
 * Title format: [sender->recipient] preview...
 * Description: full message content
 * Type: message, Status: open (unread)
 */
export async function createMessage(
  from: string,
  to: string,
  content: string,
): Promise<string> {
  const preview = content.slice(0, 60).replace(/\n/g, " ");
  const title = `[${from}->${to}] ${preview}`;
  const args = ["create", title, "-p", "3", "--type", "message", "-d", content, "--json"];
  return runBd(args);
}

/**
 * Query all unread (open) messages for a specific recipient.
 * Returns JSON array of message records.
 */
export async function getUnreadMessages(recipient: string): Promise<string> {
  // Query all open messages, then filter by recipient in title
  // bd query title= does contains search, so title=->recipient matches [sender->recipient]
  return runBd(["query", `type=message AND status=open AND title="->${recipient}]"`, "--json", "-n", "0"]);
}

/**
 * Mark a message as read by closing it.
 */
export async function markMessageRead(messageId: string): Promise<string> {
  return runBd(["close", messageId, "--reason", "delivered", "--json"]);
}
