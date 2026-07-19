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

export type QueryFields = "summary" | "full";

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
    // Always return full detail for a single task — no filtering, no projection
    return runBd(["show", id, "--json"]);
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
