import { writeFileSync, readFileSync, mkdirSync } from "node:fs";
import { join, resolve } from "node:path";
import { homedir } from "node:os";

const MAILBOX_BASE = resolve(
  process.env.APERTURE_MAILBOX ?? join(homedir(), ".aperture", "mailbox"),
);

const NAME_RE = /^[a-z0-9][a-z0-9-]{0,30}$/;
const PERMANENT_NAMES = ["glados", "wheatley", "peppy", "izzy", "vance", "rex", "scout", "cipher", "sage", "atlas", "sentinel", "sterling", "operator", "warroom"];

export interface SpiderlingInfo {
  name: string;
  task_id: string;
  tmux_window_id: string | null;
  worktree_path: string;
  worktree_branch: string;
  source_repo?: string;
  requested_by: string;
  status: string;
  spawned_at: string;
}

function activeSpiderlingsPath(): string {
  return resolve(homedir(), ".aperture", "active-spiderlings.json");
}

export function readActiveSpiderlings(): SpiderlingInfo[] {
  try {
    const data = readFileSync(activeSpiderlingsPath(), "utf-8");
    return JSON.parse(data);
  } catch {
    return [];
  }
}

export function isValidRecipient(name: string): boolean {
  if (PERMANENT_NAMES.includes(name)) return true;
  const spiderlings = readActiveSpiderlings();
  return spiderlings.some((s) => s.name === name);
}

export function requestSpawn(
  name: string,
  taskId: string,
  prompt: string,
  requestedBy: string,
  projectPath?: string,
): string {
  if (!NAME_RE.test(name)) {
    throw new Error(
      `Invalid spiderling name '${name}'. Must match [a-z0-9][a-z0-9-]{0,30}`,
    );
  }
  if (PERMANENT_NAMES.includes(name)) {
    throw new Error(`Name '${name}' conflicts with a permanent agent`);
  }
  const existing = readActiveSpiderlings();
  if (existing.some((s) => s.name === name)) {
    throw new Error(`Spiderling '${name}' already exists`);
  }

  const spawnDir = join(MAILBOX_BASE, "_spawn");
  mkdirSync(spawnDir, { recursive: true });

  const timestamp = Date.now();
  const filename = `${timestamp}-${name}.json`;
  const request: Record<string, string> = { name, task_id: taskId, prompt, requested_by: requestedBy, timestamp: String(timestamp) };
  if (projectPath) request.project_path = projectPath;
  writeFileSync(join(spawnDir, filename), JSON.stringify(request, null, 2));
  return name;
}

export function requestKill(name: string): void {
  const killDir = join(MAILBOX_BASE, "_kill");
  mkdirSync(killDir, { recursive: true });
  const timestamp = Date.now();
  writeFileSync(join(killDir, `${timestamp}-${name}.txt`), name);
}
