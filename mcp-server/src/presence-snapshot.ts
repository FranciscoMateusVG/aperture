/**
 * presence-snapshot — the shared contract for "who is online" (aperture-oeb6q).
 *
 * The ws-hub already tracks every agent's presence (join/busy/idle/leave) but
 * only broadcast it to `subscriber` sockets — i.e. the launcher's Rust
 * watchdog. No agent, GLaDOS included, could see it. Rather than add a new
 * request/response frame (round-trip per call, useless when the hub is
 * down), the hub WRITES this file on every presence change and anyone —
 * the MCP server's `get_presence` tool, `send_message`'s recipient hint,
 * scripts — READS it.
 *
 *   ~/.aperture/run/presence.json   (override: APERTURE_RUN_DIR)
 *   {
 *     "hub_pid": 12345,
 *     "updated_at": "2026-09-06T14:03:11.204Z",
 *     "agents": {
 *       "rex":   { "state": "busy", "since": "2026-09-06T14:02:58.000Z" },
 *       "vance": { "state": "idle", "since": "..." }
 *     }
 *   }
 *
 * Semantics:
 *   - An agent absent from `agents` is OFFLINE (the hub deletes on `leave`).
 *   - `state` is the hub's last positive presence event: "online" (joined,
 *     no busy/idle frame yet), "busy" (mid-turn), "idle" (between turns).
 *   - `since` is when the CURRENT state began — it changes only on a state
 *     transition, never on a repeated frame of the same state.
 *   - `hub_pid` must be alive for the file to mean anything. A reader that
 *     finds the pid dead reports `hub: "down"` and every agent as "unknown"
 *     — NEVER as "offline". The snapshot is also cleared to zero agents on
 *     hub startup so a stale file from a crashed hub can't lie for long.
 *
 * Writes are atomic (tmp + rename) so a reader never sees a torn file.
 */

import { readFileSync, writeFileSync, renameSync, mkdirSync, readdirSync, statSync } from "node:fs";
import { join, resolve } from "node:path";
import { homedir } from "node:os";

export type PresenceState = "online" | "busy" | "idle";

export interface PresenceEntry {
  state: PresenceState;
  /** ISO 8601 — when the current `state` began. */
  since: string;
}

export interface PresenceSnapshot {
  hub_pid: number;
  updated_at: string;
  agents: Record<string, PresenceEntry>;
}

export const RUN_DIR = process.env.APERTURE_RUN_DIR ?? resolve(homedir(), ".aperture", "run");
export const PRESENCE_FILE = join(RUN_DIR, "presence.json");

/** The launcher runtime tree — one dir per agent, plus `shared/`. */
export const AGENTS_DIR = process.env.APERTURE_AGENTS_DIR ?? resolve(homedir(), ".claude", "aperture");

// ── writer (hub side) ─────────────────────────────────────────────────────

/** Atomically write the snapshot. Never throws — a failed write is logged by
 *  the caller if it cares; presence is best-effort observability. */
export function writePresenceSnapshot(snapshot: PresenceSnapshot, file: string = PRESENCE_FILE): boolean {
  try {
    mkdirSync(RUN_DIR, { recursive: true });
    const tmp = `${file}.${process.pid}.tmp`;
    writeFileSync(tmp, JSON.stringify(snapshot), { mode: 0o600 });
    renameSync(tmp, file);
    return true;
  } catch {
    return false;
  }
}

// ── reader (MCP server / anyone) ──────────────────────────────────────────

export type RosterState = PresenceState | "offline" | "unknown";

export interface PresenceReport {
  /** "up" when the file exists AND hub_pid is alive; "down" otherwise. */
  hub: "up" | "down";
  updated_at: string | null;
  /** Every roster agent, sorted by name. `offline` only when hub is up. */
  agents: Array<{ name: string; state: RosterState; since: string | null }>;
}

function pidAlive(pid: number): boolean {
  if (!Number.isInteger(pid) || pid <= 0) return false;
  try {
    process.kill(pid, 0);
    return true;
  } catch (e: unknown) {
    // EPERM = exists but not ours; still alive.
    return (e as NodeJS.ErrnoException)?.code === "EPERM";
  }
}

/** Roster = agent dirs in the runtime tree (excluding `shared/`), so a new
 *  agent added via `just setup` shows up without a code change. Falls back
 *  to the known roster if the tree is unreadable. */
export function readRoster(agentsDir: string = AGENTS_DIR): string[] {
  try {
    const names = readdirSync(agentsDir).filter((n) => {
      if (n === "shared" || n.startsWith(".")) return false;
      try {
        return statSync(join(agentsDir, n)).isDirectory();
      } catch {
        return false;
      }
    });
    if (names.length > 0) return names.sort();
  } catch {
    // fall through
  }
  return ["glados", "wheatley", "peppy", "izzy", "vance", "rex", "scout", "cipher"];
}

export function readPresenceSnapshot(file: string = PRESENCE_FILE): PresenceSnapshot | null {
  try {
    const parsed = JSON.parse(readFileSync(file, "utf8")) as Partial<PresenceSnapshot>;
    if (typeof parsed?.hub_pid !== "number" || typeof parsed.agents !== "object" || parsed.agents === null) return null;
    return {
      hub_pid: parsed.hub_pid,
      updated_at: typeof parsed.updated_at === "string" ? parsed.updated_at : "",
      agents: parsed.agents as Record<string, PresenceEntry>,
    };
  } catch {
    return null;
  }
}

/** The one call the MCP tool and `send_message` use. */
export function presenceReport(opts: { file?: string; agentsDir?: string; isAlive?: (pid: number) => boolean } = {}): PresenceReport {
  const roster = readRoster(opts.agentsDir);
  const snap = readPresenceSnapshot(opts.file);
  const alive = opts.isAlive ?? pidAlive;
  const hubUp = snap !== null && alive(snap.hub_pid);
  return {
    hub: hubUp ? "up" : "down",
    updated_at: hubUp ? snap!.updated_at : null,
    agents: roster.map((name) => {
      if (!hubUp) return { name, state: "unknown" as const, since: null };
      const e = snap!.agents[name];
      return e ? { name, state: e.state, since: e.since } : { name, state: "offline" as const, since: null };
    }),
  };
}

/** One-line human summary for tool replies: "rex is busy (since 14:02:58)". */
export function describePresence(report: PresenceReport, name: string): string {
  if (report.hub === "down") return `${name}'s presence is unknown (hub down)`;
  const e = report.agents.find((a) => a.name === name);
  if (!e || e.state === "offline") return `${name} is offline`;
  if (e.state === "unknown") return `${name}'s presence is unknown`;
  const t = e.since ? new Date(e.since) : null;
  const hhmmss = t && !Number.isNaN(t.getTime()) ? t.toTimeString().slice(0, 8) : null;
  return hhmmss ? `${name} is ${e.state} (since ${hhmmss})` : `${name} is ${e.state}`;
}
