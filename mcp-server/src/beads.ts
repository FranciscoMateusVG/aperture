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

export async function createTask(
  title: string,
  priority: number,
  description?: string,
): Promise<string> {
  const args = ["create", title, "-p", String(priority), "--json"];
  if (description) {
    args.push("-d", description);
  }
  return runBd(args);
}

export async function updateTask(id: string, flags: Record<string, string>): Promise<string> {
  const args = ["update", id];
  for (const [key, value] of Object.entries(flags)) {
    if (value === "") {
      args.push(`--${key}`);
    } else {
      args.push(`--${key}`, value);
    }
  }
  args.push("--json");
  return runBd(args);
}

export async function closeTask(id: string, reason: string): Promise<string> {
  return runBd(["close", id, "--reason", reason, "--json"]);
}

const SLIM_FIELDS = ["id", "title", "status", "priority", "assignee", "owner"] as const;

function slimTask(t: Record<string, unknown>): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const f of SLIM_FIELDS) {
    if (t[f] !== undefined) out[f] = t[f];
  }
  return out;
}

export async function queryTasks(
  mode: string,
  id?: string,
  options?: { includeDone?: boolean; slim?: boolean },
): Promise<string> {
  if (mode === "show" && id) {
    // Always return full detail for a single task
    return runBd(["show", id, "--json"]);
  }
  if (mode === "ready") {
    // ready already excludes done tasks; slim by default
    const raw = await runBd(["ready", "--json"]);
    if (options?.slim === false) return raw;
    try {
      const tasks = JSON.parse(raw);
      return JSON.stringify(Array.isArray(tasks) ? tasks.map(slimTask) : tasks);
    } catch {
      return raw;
    }
  }
  // mode === "list"
  const raw = await runBd(["list", "--json"]);
  try {
    let tasks: Record<string, unknown>[] = JSON.parse(raw);
    // Exclude done/closed tasks by default — this alone saves ~80% of payload
    if (!options?.includeDone) {
      tasks = tasks.filter(
        (t) => t.status !== "done" && t.status !== "closed",
      );
    }
    // Slim fields by default — only return what agents actually need for routing
    if (options?.slim !== false) {
      tasks = tasks.map(slimTask);
    }
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
  const artifactLine = `artifact:${type}:${value}`;
  return runBd(["update", taskId, "--notes", artifactLine, "--json"]);
}

export async function searchTasks(label?: string, options?: { includeDone?: boolean; slim?: boolean }): Promise<string> {
  const args = ["list", "--json"];
  if (label) {
    args.push("--label", label);
  }
  const raw = await runBd(args);
  try {
    let tasks: Record<string, unknown>[] = JSON.parse(raw);
    if (!options?.includeDone) {
      tasks = tasks.filter(
        (t) => t.status !== "done" && t.status !== "closed",
      );
    }
    if (options?.slim !== false) {
      tasks = tasks.map(slimTask);
    }
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
