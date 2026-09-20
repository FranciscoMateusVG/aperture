import {
  lstatSync,
  readFileSync,
  readdirSync,
  realpathSync,
} from "node:fs";
import { homedir } from "node:os";
import { basename, join, relative, resolve, sep } from "node:path";

/** Aperture V4's one canonical internal seat-id rule (§4.1). */
export const SEAT_NAME_SOURCE = "^[a-z0-9][a-z0-9_-]{0,30}$";
export const SEAT_NAME_RE = new RegExp(SEAT_NAME_SOURCE);
export const COORDINATION_TRIO = new Set(["glados", "wheatley", "peppy"]);

export function isValidSeatName(name: string): boolean {
  return SEAT_NAME_RE.test(name);
}

export interface MessageGrant {
  from: string;
  to: string;
  scope: "message";
  by: "glados";
  at: string;
}

export interface SeatPrincipal {
  name: string;
  role: string;
  group: "legacy" | "team";
  team: string | null;
  project: string | null;
  isLead: boolean;
}

interface TeamSnapshot {
  team: string;
  project: string;
  lead: string;
  seats: Array<{ name: string; role: string }>;
  grants: MessageGrant[];
}

interface ActiveTeam {
  snapshot: TeamSnapshot;
  seatByName: Map<string, { name: string; role: string }>;
}

export interface SeatRegistry {
  seats: ReadonlyMap<string, SeatPrincipal>;
  grants: readonly MessageGrant[];
}

export type MessageDenialCode = "E_UNKNOWN_RECIPIENT" | "E_CROSS_TEAM_DENIED";
export type MessageDenialReason =
  | "unknown_sender"
  | "unknown_recipient"
  | "nonlead"
  | "cross_project"
  | "team_not_active"
  | "no_grant";

export type MessageAuthorization =
  | { allowed: true }
  | { allowed: false; code: MessageDenialCode; reason: MessageDenialReason };

function defaultAgentsRoot(): string {
  return process.env.APERTURE_AGENTS_DIR || resolve(homedir(), ".claude", "aperture");
}

function defaultTeamsRoot(): string {
  return process.env.APERTURE_TEAMS_DIR || resolve(homedir(), ".aperture", "teams");
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return !!value && typeof value === "object" && !Array.isArray(value);
}

function parseJsonObject(text: string): Record<string, unknown> | null {
  try {
    const value: unknown = JSON.parse(text);
    return isPlainObject(value) ? value : null;
  } catch {
    return null;
  }
}

function isRealFile(path: string): boolean {
  try {
    const stat = lstatSync(path);
    return stat.isFile() && !stat.isSymbolicLink();
  } catch {
    return false;
  }
}

function isRealDirectory(path: string): boolean {
  try {
    const stat = lstatSync(path);
    return stat.isDirectory() && !stat.isSymbolicLink();
  } catch {
    return false;
  }
}

function pathEntryExists(path: string): boolean {
  try {
    lstatSync(path);
    return true;
  } catch {
    return false;
  }
}

function pathInside(root: string, path: string): boolean {
  try {
    const canonicalRoot = realpathSync(root);
    const canonicalPath = realpathSync(path);
    const rel = relative(canonicalRoot, canonicalPath);
    return rel === "" || (!rel.startsWith(`..${sep}`) && rel !== "..");
  } catch {
    return false;
  }
}

function parseGrant(value: unknown, team: string, seatNames: Set<string>): MessageGrant | null {
  if (!isPlainObject(value)) return null;
  const { from, to, scope, by, at } = value;
  if (
    typeof from !== "string" ||
    typeof to !== "string" ||
    scope !== "message" ||
    by !== "glados" ||
    typeof at !== "string" ||
    at.trim() === "" ||
    (from !== team && !seatNames.has(from)) ||
    !isValidSeatName(to)
  ) {
    return null;
  }
  return { from, to, scope, by, at };
}

/**
 * Read one active team as one coherent snapshot.
 *
 * P1 will publish these files by atomic rename. P0 deliberately has no cache:
 * read state/team twice and require byte identity plus journal absence at both
 * ends, so a concurrent activate/archive is rejected rather than mixed.
 */
function readActiveTeam(root: string, dirName: string): ActiveTeam | null {
  if (!isValidSeatName(dirName)) return null;
  const dir = join(root, dirName);
  if (!isRealDirectory(dir) || !pathInside(root, dir)) return null;
  const statePath = join(dir, "state.json");
  const teamPath = join(dir, "team.json");
  const journalPath = join(dir, "journal.json");
  if (!isRealFile(statePath) || !isRealFile(teamPath) || pathEntryExists(journalPath)) return null;
  if (!pathInside(dir, statePath) || !pathInside(dir, teamPath)) return null;

  try {
    const stateBefore = readFileSync(statePath, "utf8");
    const teamBefore = readFileSync(teamPath, "utf8");
    if (pathEntryExists(journalPath)) return null;
    const teamAfter = readFileSync(teamPath, "utf8");
    const stateAfter = readFileSync(statePath, "utf8");
    if (pathEntryExists(journalPath) || stateBefore !== stateAfter || teamBefore !== teamAfter) return null;

    const state = parseJsonObject(stateBefore);
    const raw = parseJsonObject(teamBefore);
    if (!state || state.state !== "active" || !Number.isSafeInteger(state.generation) || (state.generation as number) < 0 || !raw) return null;
    if (raw.team !== dirName || typeof raw.project !== "string" || !/^project:[a-z0-9][a-z0-9_-]*$/.test(raw.project)) {
      return null;
    }
    if (typeof raw.lead !== "string" || !isValidSeatName(raw.lead) || !Array.isArray(raw.seats)) return null;

    const seats: Array<{ name: string; role: string }> = [];
    const seatNames = new Set<string>();
    for (const value of raw.seats) {
      if (!isPlainObject(value) || typeof value.name !== "string" || typeof value.role !== "string") return null;
      if (!isValidSeatName(value.name) || value.role.trim() === "" || seatNames.has(value.name)) return null;
      seatNames.add(value.name);
      seats.push({ name: value.name, role: value.role });
    }
    if (!seatNames.has(raw.lead)) return null;

    const grants: MessageGrant[] = [];
    const rawGrants = raw.grants === undefined ? [] : raw.grants;
    if (!Array.isArray(rawGrants)) return null;
    for (const value of rawGrants) {
      const grant = parseGrant(value, dirName, seatNames);
      if (!grant) return null;
      grants.push(grant);
    }
    const snapshot: TeamSnapshot = {
      team: dirName,
      project: raw.project,
      lead: raw.lead,
      seats,
      grants,
    };
    return { snapshot, seatByName: new Map(seats.map((seat) => [seat.name, seat])) };
  } catch {
    return null;
  }
}

function loadActiveTeams(root: string): Map<string, ActiveTeam> {
  const teams = new Map<string, ActiveTeam>();
  if (!isRealDirectory(root)) return teams;
  let entries: string[];
  try {
    entries = readdirSync(root);
  } catch {
    return teams;
  }
  for (const name of entries) {
    if (name.startsWith(".") || name.startsWith("_") || name === "archive" || name === "presets") continue;
    const team = readActiveTeam(root, name);
    if (team) teams.set(name, team);
  }
  return teams;
}

function readEnabledManifest(path: string, allowSymlink: boolean): Record<string, unknown> | null {
  try {
    const stat = lstatSync(path);
    if ((!stat.isFile() && !stat.isSymbolicLink()) || (!allowSymlink && stat.isSymbolicLink())) return null;
    const manifest = parseJsonObject(readFileSync(path, "utf8"));
    if (!manifest || manifest.enabled === false) return null;
    return manifest;
  } catch {
    return null;
  }
}

/** Load enabled principals from disk. Invalid or ambiguous entries disappear. */
export function loadSeatRegistry(options: { agentsRoot?: string; teamsRoot?: string } = {}): SeatRegistry {
  const aRoot = options.agentsRoot ?? defaultAgentsRoot();
  const tRoot = options.teamsRoot ?? defaultTeamsRoot();
  const activeTeams = loadActiveTeams(tRoot);
  const teamMembership = new Map<string, ActiveTeam[]>();
  for (const team of activeTeams.values()) {
    for (const seat of team.snapshot.seats) {
      const current = teamMembership.get(seat.name) ?? [];
      current.push(team);
      teamMembership.set(seat.name, current);
    }
  }

  const seats = new Map<string, SeatPrincipal>();
  if (isRealDirectory(aRoot)) {
    let entries: string[] = [];
    try {
      entries = readdirSync(aRoot);
    } catch {
      entries = [];
    }
    for (const name of entries) {
      if (name === "shared" || name.startsWith("_") || !isValidSeatName(name)) continue;
      const dir = join(aRoot, name);
      if (!isRealDirectory(dir) || !pathInside(aRoot, dir)) continue;
      const marker = join(dir, "TEAM");
      const memberships = teamMembership.get(name) ?? [];
      const markerExists = pathEntryExists(marker);
      const isTeamSeat = isRealFile(marker);
      // A principal named by an active team can never silently downgrade to
      // the broad legacy compatibility group because its marker is missing or
      // malformed. Likewise a symlink TEAM marker is an invalid team seat,
      // not a legacy seat.
      if ((!isTeamSeat && memberships.length > 0) || (markerExists && !isTeamSeat)) continue;
      // Legacy just-setup registries intentionally use repo-owned symlinks for
      // manifest.json. P1 team seats are runtime-owned and reject all such
      // indirection at this trust boundary.
      const manifest = readEnabledManifest(join(dir, "manifest.json"), !isTeamSeat);
      if (!manifest) continue;

      if (!isTeamSeat) {
        const role = typeof manifest.role === "string" ? manifest.role : "legacy";
        seats.set(name, { name, role, group: "legacy", team: null, project: null, isLead: false });
        continue;
      }

      if (!isRealFile(join(dir, ".complete"))) continue;
      if (memberships.length !== 1) continue;
      const team = memberships[0]!;
      const seat = team.seatByName.get(name);
      if (!seat) continue;
      seats.set(name, {
        name,
        role: seat.role,
        group: "team",
        team: team.snapshot.team,
        project: team.snapshot.project,
        isLead: team.snapshot.lead === name,
      });
    }
  }

  const grants = [...activeTeams.values()].flatMap((team) => team.snapshot.grants);
  return { seats, grants };
}

function hasGrant(registry: SeatRegistry, sender: SeatPrincipal, recipient: SeatPrincipal): boolean {
  return registry.grants.some(
    (grant) =>
      grant.scope === "message" &&
      grant.to === recipient.name &&
      (grant.from === sender.name || (sender.team !== null && grant.from === sender.team)),
  );
}

/** Server-side routing authorization from trusted registry facts only. */
export function authorizeMessage(
  registry: SeatRegistry,
  senderName: string,
  recipientName: string,
): MessageAuthorization {
  const sender = registry.seats.get(senderName);
  if (recipientName === "operator") {
    return sender
      ? { allowed: true }
      : { allowed: false, code: "E_CROSS_TEAM_DENIED", reason: "unknown_sender" };
  }
  const recipient = registry.seats.get(recipientName);
  if (!recipient) return { allowed: false, code: "E_UNKNOWN_RECIPIENT", reason: "unknown_recipient" };
  if (!sender) return { allowed: false, code: "E_CROSS_TEAM_DENIED", reason: "unknown_sender" };

  if (sender.group === "legacy" && recipient.group === "legacy") return { allowed: true };
  if (sender.group === "team" && recipient.group === "team" && sender.team === recipient.team) {
    return { allowed: true };
  }
  if (COORDINATION_TRIO.has(sender.name) || COORDINATION_TRIO.has(recipient.name)) return { allowed: true };
  if (hasGrant(registry, sender, recipient)) return { allowed: true };

  if (sender.group === "team" && recipient.group === "team") {
    if (!sender.isLead || !recipient.isLead) {
      return { allowed: false, code: "E_CROSS_TEAM_DENIED", reason: "nonlead" };
    }
    if (sender.project !== recipient.project) {
      return { allowed: false, code: "E_CROSS_TEAM_DENIED", reason: "cross_project" };
    }
    return { allowed: true };
  }
  return { allowed: false, code: "E_CROSS_TEAM_DENIED", reason: "no_grant" };
}

/** Exact principal binding used by the stdio MCP before a BEADS write/read. */
export function hasAuthenticatedMcpIdentity(agentName: string): boolean {
  if (!isValidSeatName(agentName) || !loadSeatRegistry().seats.has(agentName)) return false;
  const tokenFile = process.env.APERTURE_HUB_TOKEN_FILE;
  const tokenRoot = process.env.APERTURE_HUB_TOKEN_DIR || resolve(homedir(), ".aperture", "run", "hub-tokens");
  if (!tokenFile || basename(tokenFile) !== `${agentName}.token`) return false;
  if (resolve(tokenFile) !== resolve(tokenRoot, `${agentName}.token`)) return false;
  try {
    const stat = lstatSync(tokenFile);
    if (!stat.isFile() || stat.isSymbolicLink() || (stat.mode & 0o077) !== 0) return false;
    const token = readFileSync(tokenFile);
    return token.length >= 32 && token.length <= 256;
  } catch {
    return false;
  }
}
