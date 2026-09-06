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

/**
 * Render argv for a diagnostic message WITHOUT leaking free-text content.
 *
 * bd invocations carry task notes, descriptions and message bodies as
 * arguments. Those can contain credentials or customer data, and this string
 * ends up in an error surfaced to the calling agent, so long values are
 * replaced by a length marker. Short values (subcommands, ids, flags, query
 * expressions) are kept verbatim — they are what makes a failure diagnosable.
 */
const ARGV_INLINE_MAX = 120;

export function redactArgv(args: string[]): string {
  const rendered = args.map((a, i) =>
    // args[0] is the subcommand — always safe and always useful.
    i === 0 || a.length <= ARGV_INLINE_MAX ? a : `<redacted ${a.length}b>`,
  );
  return JSON.stringify(rendered);
}

export function runBd(args: string[]): Promise<string> {
  const startedAt = Date.now();
  return new Promise((resolve, reject) => {
    execFile(BD_PATH, args, { env: bdEnv(), timeout: 30000 }, (err, stdout, stderr) => {
      if (err) {
        // Surface enough to diagnose the NEXT occurrence from evidence rather
        // than re-deriving it: which invocation, how it exited, how long it
        // ran (a ~30000ms duration with killed=true is the execFile timeout),
        // and stderr. Previously this reported `stderr || err.message`, which
        // discarded the exit code and duration whenever stderr was non-empty.
        const e = err as NodeJS.ErrnoException & { killed?: boolean; signal?: string };
        const detail = [
          `bd invocation failed after ${Date.now() - startedAt}ms`,
          `argv: ${redactArgv([BD_PATH, ...args])}`,
          `exit: ${e.code ?? "n/a"}`,
          e.signal ? `signal: ${e.signal}` : undefined,
          e.killed ? "killed: true (execFile timeout is 30000ms)" : undefined,
          stderr?.trim() ? `stderr: ${stderr.trim()}` : "stderr: (empty)",
        ]
          .filter(Boolean)
          .join(" | ");
        reject(new Error(detail));
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
  /** Max rows to request from bd (`-n`). Unset = bd's own default (list 50, ready 10). */
  limit?: number;
}

/** How much of bd's raw output an unparseable-output error quotes. */
const SHAPE_ERROR_PREVIEW = 300;

/**
 * bd printed something that is not the JSON shape we asked for. Previously the
 * callers returned the raw stdout unprojected on this path — which silently
 * bypassed the summary/detail tiers and dumped whatever bd printed (a pager
 * banner, a Dolt warning, a full pretty-printed table…) straight into the
 * agent's context. Now it is an error: a short quote of the output plus the
 * (redacted) argv is enough to diagnose without the dump.
 */
function bdShapeError(problem: string, raw: string, args: string[]): Error {
  const preview = raw.length > SHAPE_ERROR_PREVIEW ? raw.slice(0, SHAPE_ERROR_PREVIEW) + "…" : raw;
  return new Error(
    `bd returned unexpected output (${problem}) | argv: ${redactArgv([BD_PATH, ...args])} | output (first ${SHAPE_ERROR_PREVIEW} chars of ${raw.length}): ${preview}`,
  );
}

function parseJsonOrThrow(raw: string, args: string[]): unknown {
  try {
    return JSON.parse(raw);
  } catch {
    return bdShapeErrorThrow("not valid JSON", raw, args);
  }
}

function bdShapeErrorThrow(problem: string, raw: string, args: string[]): never {
  throw bdShapeError(problem, raw, args);
}

/** Parse bd list/ready/query stdout as a task array or throw a diagnosable error. */
function parseTaskArray(raw: string, args: string[]): Record<string, unknown>[] {
  const parsed = parseJsonOrThrow(raw, args);
  if (!Array.isArray(parsed)) {
    bdShapeErrorThrow(`expected a JSON array, got ${parsed === null ? "null" : typeof parsed}`, raw, args);
  }
  return parsed as Record<string, unknown>[];
}

function taskHasLabel(t: Record<string, unknown>, label: string): boolean {
  const labels = t.labels;
  if (!Array.isArray(labels)) return false;
  return labels.some((l) => typeof l === "string" && l === label);
}

function projectFields(
  tasks: Record<string, unknown>[],
  fields: QueryFields | undefined,
): Record<string, unknown>[] {
  if (fields === "full") return tasks;
  // default: summary
  return tasks.map(summarizeTask);
}

type ListMode = "list" | "ready";

interface ListPlan {
  args: string[];
  /**
   * priorityMax could not be pushed down to bd and must be applied here, after
   * parsing. Only `bd ready` lacks `--priority-max` (verified bd 1.0.2: `bd list`
   * has `--priority-max`, `bd ready` has only an exact-match `-p`).
   */
  postFilterPriorityMax?: number;
  /** Client-side cap, only when the bd-side `-n` had to be disabled (see below). */
  postLimit?: number;
}

/**
 * Build the bd argv for list/ready, pushing every filter bd understands down
 * to bd so the JSON is narrowed BEFORE we parse it.
 *
 * Filters and where they are applied (bd 1.0.2):
 *   label / project  → `--label` (both modes)
 *   assignee         → `--assignee` (list only; ready never auto-filters)
 *   includeDone      → `--all` (list only). bd's default already excludes
 *                      closed — verified: `bd list` returned statuses
 *                      {open, in_progress}; `bd list --all` adds closed — so
 *                      the old post-filter for the default case was redundant.
 *                      `bd ready` is open-and-unblocked by definition: no
 *                      `--all` flag exists and includeDone is meaningless there.
 *   priorityMax      → `--priority-max` (list only). `bd ready` has no such
 *                      flag, so ready keeps a post-filter — and in THAT case we
 *                      must pass `-n 0`: bd applies its default limit (10 for
 *                      ready) BEFORE we filter, so a bounded fetch would drop
 *                      matching rows silently. Any caller `limit` is then
 *                      applied client-side after the filter.
 *   limit            → `-n` (both modes) unless the trap above forces `-n 0`.
 */
function planListQuery(mode: ListMode, options: QueryOptions | undefined): ListPlan {
  const args: string[] = [mode, "--json"];
  const plan: ListPlan = { args };

  if (options?.label) {
    args.push("--label", options.label);
  }
  // For project we also use the label flag (same machinery in bd).
  if (options?.project) {
    args.push("--label", `project:${options.project}`);
  }
  // assignee: "*" means any (skip filter). For ready mode we never auto-filter.
  if (mode !== "ready" && options?.assignee && options.assignee !== "*") {
    args.push("--assignee", options.assignee);
  }
  if (mode === "list" && options?.includeDone) {
    args.push("--all");
  }

  const priorityMax = typeof options?.priorityMax === "number" ? options.priorityMax : undefined;
  if (priorityMax !== undefined && mode === "list") {
    args.push("--priority-max", String(priorityMax));
  }

  if (priorityMax !== undefined && mode === "ready") {
    // Post-filter trap: fetch unbounded so bd's own limit cannot truncate
    // before our filter runs; re-apply the caller's limit afterwards.
    plan.postFilterPriorityMax = priorityMax;
    plan.postLimit = options?.limit;
    args.push("-n", "0");
  } else if (typeof options?.limit === "number") {
    args.push("-n", String(options.limit));
  }

  return plan;
}

async function runListQuery(mode: ListMode, options: QueryOptions | undefined): Promise<string> {
  const plan = planListQuery(mode, options);
  const raw = await runBd(plan.args);
  let tasks = parseTaskArray(raw, plan.args);
  if (options?.project) {
    // Belt-and-braces alongside the --label push-down (kept from before).
    const projectLabel = `project:${options.project}`;
    tasks = tasks.filter((t) => taskHasLabel(t, projectLabel));
  }
  if (plan.postFilterPriorityMax !== undefined) {
    const max = plan.postFilterPriorityMax;
    tasks = tasks.filter((t) => typeof t.priority === "number" && (t.priority as number) <= max);
  }
  if (plan.postLimit !== undefined) {
    tasks = tasks.slice(0, plan.postLimit);
  }
  return JSON.stringify(projectFields(tasks, options?.fields));
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
    const args = ["show", id, "--json"];
    const raw = await runBd(args);
    if (options?.fields === "full") return raw; // opt-in raw passthrough, by design
    const parsed = parseJsonOrThrow(raw, args);
    if (!parsed || typeof parsed !== "object") {
      bdShapeErrorThrow(`expected a JSON object, got ${parsed === null ? "null" : typeof parsed}`, raw, args);
    }
    const arr: Record<string, unknown>[] = Array.isArray(parsed) ? parsed : [parsed as Record<string, unknown>];
    const projected = arr.map((t) =>
      options?.fields === "summary" ? summarizeTask(t) : detailTask(t),
    );
    return JSON.stringify(Array.isArray(parsed) ? projected : projected[0]);
  }

  return runListQuery(mode === "ready" ? "ready" : "list", options);
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

/**
 * search_tasks is `bd list` with every filter pushed down (see planListQuery).
 * It differs from queryTasks("list") only in that the caller never defaults
 * the assignee — that defaulting lives in index.ts.
 */
export async function searchTasks(
  options?: QueryOptions,
): Promise<string> {
  return runListQuery("list", options);
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
 * Cap on unread messages fetched per getUnreadMessages call.
 *
 * Previously `-n 0` (unbounded): after a long outage a backlog of hundreds of
 * unread messages inlined every full body into ONE get_messages reply. Message
 * bodies are the payload (they cannot be truncated), so the only lever is the
 * row count. 200 keeps a worst-case reply bounded; the agent drains the rest by
 * marking these read and calling again — formatUnreadMessages says so.
 *
 * Ordering (verified against bd 1.0.2): `bd query` returns NEWEST-first, and
 * `--sort created -r` does not help because bd applies `-n` BEFORE `-r`
 * (`--sort created -r -n 3` returned the 3 newest, merely reversed). So when a
 * backlog exceeds the cap the slice is the 200 MOST RECENT messages, not the
 * oldest. formatUnreadMessages re-sorts them oldest-first so the agent still
 * processes each batch in chronological order. (`bd list --include-infra
 * --type message --sort created -r -n N` does yield the true oldest N, but
 * list applies different default hiding rules to infra/ephemeral beads — not
 * worth the silent-drop risk for a subcommand swap.)
 */
export const UNREAD_LIMIT = 200;

/**
 * Query unread (open) messages for a specific recipient — at most UNREAD_LIMIT.
 * Returns the raw bd JSON array (string); callers parse it.
 */
export async function getUnreadMessages(recipient: string): Promise<string> {
  // Query all open messages, then filter by recipient in title
  // bd query title= does contains search, so title=->recipient matches [sender->recipient]
  return runBd([
    "query",
    `type=message AND status=open AND title="->${recipient}]"`,
    "--json",
    "-n",
    String(UNREAD_LIMIT),
  ]);
}

/** The line get_messages appends when the reply hit UNREAD_LIMIT. */
export const UNREAD_CAP_NOTICE = `Showing the ${UNREAD_LIMIT} most recent unread messages (oldest first); older messages are still queued. Call get_messages again after marking these read.`;

/**
 * Render the get_messages reply body: one `[id] From sender: body` block per
 * message, oldest first (see UNREAD_LIMIT for why we sort here), plus the cap
 * notice when the batch is exactly UNREAD_LIMIT rows (i.e. bd may have had
 * more). The caller has already rejected non-array and empty inputs.
 */
export function formatUnreadMessages(messages: Record<string, unknown>[]): string {
  const created = (m: Record<string, unknown>): string =>
    typeof m.created_at === "string" ? m.created_at : "";
  const ordered = [...messages].sort((a, b) => created(a).localeCompare(created(b)));
  const blocks = ordered.map((m) => {
    const title = typeof m.title === "string" ? m.title : "";
    const from = title.match(/\[(.+?)->(.+?)\]/)?.[1] ?? "unknown";
    const body = typeof m.description === "string" ? m.description : "(no content)";
    return `[${m.id}] From ${from}: ${body}`;
  });
  if (messages.length >= UNREAD_LIMIT) {
    blocks.push(UNREAD_CAP_NOTICE);
  }
  return blocks.join("\n\n");
}

/**
 * Mark a message as read by closing it.
 */
export async function markMessageRead(messageId: string): Promise<string> {
  return runBd(["close", messageId, "--reason", "delivered", "--json"]);
}
