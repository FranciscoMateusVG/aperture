/**
 * Aperture comms-layer v2 — Phase 1 WS hub (aperture-bus delivery transport).
 *
 * Standalone singleton daemon (`node dist/ws-hub.js`). The per-agent MCP
 * servers are many (one process per agent); this hub is ONE process that owns
 * the WebSocket delivery plane:
 *
 *   - role=agent      → Claude Monitors. Presence-registered by agent name
 *                       (one socket per name; a new connection replaces the
 *                       old). On connect, unread BEADS messages are replayed.
 *   - role=subscriber → presence watchers (GLaDOS, launcher UI). Receive
 *                       {type:"presence", agent, event, ts} with event one of
 *                       "join"|"leave"|"busy"|"idle" (busy/idle come from
 *                       Codex bridge turn state — see codex-bridge.ts — or
 *                       from Claude Code hook presence hints, below). A
 *                       busy/idle that does not change the stored state is
 *                       NOT re-broadcast; join/leave always are.
 *   - role=producer   → MCP send-queue drains and hook hint clients. Send
 *                       {type:"notify", to, id, from, preview}; the hub
 *                       forwards {type:"message", id, from, preview} to the
 *                       recipient if connected and always acks
 *                       {type:"ok", id, outcome} with outcome one of
 *                       "forwarded" (Monitor socket got the push), "codex"
 *                       (injected into the Codex bridge), "offline" (nobody
 *                       to push to — unread replay on reconnect covers it).
 *                       Or send {type:"presence_hint", event:"busy"|"idle"}
 *                       (aperture-trgpo; Claude Code UserPromptSubmit /
 *                       PreToolUse → busy, Stop → idle). The target is ALWAYS
 *                       the authenticated principal (conn.agent); an `agent`
 *                       field naming anyone else is rejected. A hint only
 *                       flips the state of an agent that already has a live
 *                       Monitor socket — it never creates presence (no ghost
 *                       agents; only join does) and is ignored for
 *                       codex-bridged agents (their bridge owns turn state).
 *                       Acked {type:"ok", hint: event, applied: boolean}
 *                       where applied=false means no state transition
 *                       happened (repeat hint, not present, codex-bridged).
 *
 * Delivery semantics: at-least-once, idempotent by message id. A missed push
 * (recipient offline, hub down) is covered by unread replay on reconnect —
 * BEADS remains the store of record; this is transport only.
 *
 * Presence is also mirrored to ~/.aperture/run/presence.json on every change
 * (see presence-snapshot.ts) so MCP servers can answer "who is online" without
 * a hub round-trip. Cleared to zero agents at startup.
 *
 * Env:
 *   APERTURE_WS_PORT        — listen port (default 4517, loopback only)
 *   APERTURE_HUB_SKIP_REPLAY=1 — skip the bd unread-replay on agent connect
 *                                (testing hook; smoke tests have no BEADS)
 *   APERTURE_RUN_DIR        — where presence.json lands (default ~/.aperture/run)
 */
import { WebSocketServer, WebSocket } from "ws";
import { constants, closeSync, fstatSync, fsyncSync, lstatSync, openSync, readFileSync, readdirSync, unlinkSync } from "node:fs";
import { join } from "node:path";
import { createHash, timingSafeEqual } from "node:crypto";
import { getUnreadMessages, persistDeniedNotification } from "./beads.js";
import { startCodexBridges, type PresenceEvent } from "./codex-bridge.js";
import { writePresenceSnapshot, PRESENCE_FILE, type PresenceEntry, type PresenceState } from "./presence-snapshot.js";
import { authorizeMessage, isValidSeatName, loadSeatRegistry } from "./seat-registry.js";
import { identityIsRevoked, revokeGeneration } from "./revocation-store.js";
import { managedOwnerMatches, readManagedOwner } from "./managed-owner.js";
import { fixedRuntimeChild } from "./private-runtime-path.js";

const HOST = "127.0.0.1";
const PORT = Number(process.env.APERTURE_WS_PORT ?? 4517);
const SKIP_REPLAY = process.env.APERTURE_HUB_SKIP_REPLAY === "1";
const HEARTBEAT_MS = 30_000;
const MAX_FRAME_BYTES = 16 * 1024;
const TOKEN_DIR = fixedRuntimeChild(process.env.APERTURE_HUB_TOKEN_DIR, "hub-tokens");

function secureTokenDirectory(): string {
  const current = fixedRuntimeChild(process.env.APERTURE_HUB_TOKEN_DIR, "hub-tokens");
  if (current !== TOKEN_DIR) throw new Error("E_RUNTIME_PATH_UNSAFE: token root changed");
  return current;
}

type Role = "agent" | "subscriber" | "producer";

interface Conn {
  role: Role | null; // null until a valid hello arrives
  agent: string | null;
  isAlive: boolean;
  generation: number | null;
  tokenId: string | null;
  revoked: boolean;
}

const conns = new Map<WebSocket, Conn>();
/** Presence map: agent name → its (single) live socket. */
const agents = new Map<string, WebSocket>();
/**
 * Latest presence event per agent (aperture-3x136). The hub previously only
 * BROADCAST presence and never stored it, so a subscriber connecting (or
 * reconnecting) AFTER an agent's join/busy/idle had no way to learn the agent
 * was present. This bit the watchdog subscriber: on any hub restart the codex
 * bridges re-join, but if the watchdog reconnects a beat later it misses those
 * joins — and since a codex bridge lives inside the hub, respawning the pane
 * never re-emits its join, so the watchdog false-re-kicked idle codex agents
 * forever. We now snapshot this map to every subscriber on hello.
 *
 * aperture-oeb6q: the map now holds {state, since} (the presence-snapshot
 * contract) instead of the raw last event, and is mirrored to presence.json
 * after every mutation. Rules: "join" → "online" only if the agent is NOT
 * already present (a re-join, e.g. agent_replaced, keeps state + since);
 * "busy"/"idle" → that state; "leave" → delete. `since` moves ONLY on a state
 * transition — a repeated "busy" frame must not bump it.
 */
const presenceState = new Map<string, PresenceEntry>();

/** Structured single-line JSON logging to stderr. */
function log(event: string, fields: Record<string, unknown> = {}): void {
  process.stderr.write(
    JSON.stringify({ ts: new Date().toISOString(), event, ...fields }) + "\n",
  );
}

function send(ws: WebSocket, obj: unknown): void {
  if (ws.readyState === WebSocket.OPEN) {
    ws.send(JSON.stringify(obj));
  }
}

/** Apply a presence event to the state map. Returns true if anything changed. */
function applyPresence(agent: string, event: PresenceEvent, ts: string): boolean {
  if (event === "leave") return presenceState.delete(agent);
  const current = presenceState.get(agent);
  if (event === "join") {
    if (current) return false; // already present: keep state + since
    presenceState.set(agent, { state: "online", since: ts });
    return true;
  }
  const next: PresenceState = event; // "busy" | "idle"
  if (current && current.state === next) return false; // repeated frame: since untouched
  presenceState.set(agent, { state: next, since: ts });
  return true;
}

/** Mirror the state map to presence.json (atomic; best-effort, logged on failure). */
function persistPresence(): void {
  const agents: Record<string, PresenceEntry> = {};
  for (const [name, entry] of presenceState) agents[name] = { ...entry };
  const ok = writePresenceSnapshot({ hub_pid: process.pid, updated_at: new Date().toISOString(), agents });
  if (!ok) log("presence_snapshot_write_failed", { file: PRESENCE_FILE });
}

/** Subscriber wire format is the EVENT name, not the stored state — the Rust
 *  watchdog consumes {type:"presence", agent, event, ts} and must not notice
 *  the storage change. "online" maps back to "join". */
function stateToEvent(state: PresenceState): PresenceEvent {
  return state === "online" ? "join" : state;
}

/**
 * Apply + persist + fan out a presence event. Returns whether the stored
 * state actually changed.
 *
 * join/leave are ALWAYS broadcast and logged (the agent_replaced re-join and
 * the leave-after-delete are protocol pins the watchdog relies on). busy/idle
 * are broadcast ONLY on a transition: Claude Code hook hints fire on every
 * PreToolUse, so a repeated "busy" would otherwise spam every subscriber (and
 * the log) with frames that carry no information (aperture-trgpo).
 */
function broadcastPresence(agent: string, event: PresenceEvent): boolean {
  const ts = new Date().toISOString();
  // Remember current state so a later subscriber hello can be snapshotted,
  // and mirror it to disk for the MCP servers' get_presence.
  const changed = applyPresence(agent, event, ts);
  if (changed) persistPresence();
  if (!changed && (event === "busy" || event === "idle")) return false;
  const msg = { type: "presence", agent, event, ts };
  for (const [ws, conn] of conns) {
    if (conn.role === "subscriber") send(ws, msg);
  }
  log("presence", { agent, presence: event });
  return changed;
}

/** Read a launcher-provisioned token without following symlinks or accepting
 * group/world-readable credentials. The value is never included in logs. */
function readAgentToken(agent: string): Buffer | null {
  if (!isValidSeatName(agent)) return null;
  let fd: number | null = null;
  try {
    fd = openSync(join(secureTokenDirectory(), `${agent}.token`), constants.O_RDONLY | constants.O_NOFOLLOW);
    const stat = fstatSync(fd);
    if (!stat.isFile() || stat.nlink !== 1 || (stat.mode & 0o077) !== 0) return null;
    if (typeof process.getuid === "function" && stat.uid !== process.getuid()) return null;
    const token = readFileSync(fd);
    return token.length >= 32 && token.length <= 256 ? token : null;
  } catch {
    return null;
  } finally {
    if (fd !== null) closeSync(fd);
  }
}

function validAgentToken(agent: string, presented: unknown): boolean {
  if (typeof presented !== "string" || presented.length > 256) return false;
  const expected = readAgentToken(agent);
  if (!expected) return false;
  // Compare fixed-length digests so token length is not observable through an
  // early-return timing difference.
  const actualDigest = createHash("sha256").update(presented).digest();
  const expectedDigest = createHash("sha256").update(expected).digest();
  return timingSafeEqual(actualDigest, expectedDigest);
}

function tokenId(value: string): string {
  return createHash("sha256").update(value).digest("hex");
}

/** True only when the presented token belongs to a different canonical seat. */
function tokenBelongsToAnotherAgent(agent: string, presented: unknown): boolean {
  if (typeof presented !== "string" || presented.length > 256) return false;
  let names: string[];
  try {
    names = readdirSync(secureTokenDirectory());
  } catch {
    return false;
  }
  const actualDigest = createHash("sha256").update(presented).digest();
  for (const file of names) {
    if (!file.endsWith(".token")) continue;
    const candidate = file.slice(0, -".token".length);
    if (candidate === agent || !isValidSeatName(candidate)) continue;
    const token = readAgentToken(candidate);
    if (!token) continue;
    const expectedDigest = createHash("sha256").update(token).digest();
    if (timingSafeEqual(actualDigest, expectedDigest)) return true;
  }
  return false;
}

/**
 * Replay unread BEADS messages to a freshly connected agent.
 * Reuses the MCP server's unread-query (shells out to `bd` via beads.ts).
 * Failure is non-fatal: the agent can always pull via get_messages.
 */
async function replayUnread(agent: string, ws: WebSocket): Promise<void> {
  if (SKIP_REPLAY) {
    log("replay_skipped", { agent });
    return;
  }
  try {
    const raw = await getUnreadMessages(agent);
    const rows = JSON.parse(raw);
    if (!Array.isArray(rows)) {
      log("replay", { agent, count: 0 });
      return;
    }
    let count = 0;
    for (const r of rows as Record<string, unknown>[]) {
      if (typeof r.id !== "string") continue;
      const title = typeof r.title === "string" ? r.title : "";
      const from = title.match(/\[(.+?)->(.+?)\]/)?.[1] ?? "unknown";
      const body = typeof r.description === "string" ? r.description : "";
      const preview = body.slice(0, 60).replace(/\n/g, " ");
      send(ws, { type: "message", id: r.id, from, preview });
      count++;
    }
    log("replay", { agent, count });
  } catch (e: unknown) {
    log("replay_error", {
      agent,
      error: e instanceof Error ? e.message : String(e),
    });
  }
}

type HelloResult = { ok: true } | { ok: false; closeCode: 4001 | 4002 | 4003 };

function handleHello(ws: WebSocket, conn: Conn, msg: Record<string, unknown>): HelloResult {
  const role = msg.role;
  if (role !== "agent" && role !== "subscriber" && role !== "producer") {
    log("bad_hello", { reason: "invalid_role", role: String(role) });
    return { ok: false, closeCode: 4001 };
  }
  const agent = typeof msg.agent === "string" && msg.agent.length > 0 ? msg.agent : null;
  if (!agent) {
    log("bad_hello", { reason: "principal_required", role });
    return { ok: false, closeCode: 4001 };
  }
  if (agent === "operator") {
    log("bad_hello", { reason: "operator_is_doorbell_only", role, agent });
    return { ok: false, closeCode: 4002 };
  }
  const registry = loadSeatRegistry();
  const principal = registry.seats.get(agent);
  if (principal?.group === "team") {
    const generation = msg.generation;
    const presentedTokenId = msg.token_id;
    const derivedTokenId = typeof msg.token === "string" ? tokenId(msg.token) : "";
    if (
      !Number.isSafeInteger(generation) ||
      (generation as number) < 1 ||
      typeof presentedTokenId !== "string" ||
      !/^[a-f0-9]{64}$/.test(presentedTokenId) ||
      presentedTokenId !== derivedTokenId
    ) {
      log("bad_hello", { reason: "managed_identity_required", role, agent });
      return { ok: false, closeCode: 4003 };
    }
    try {
      const owner = readManagedOwner(agent);
      if (!managedOwnerMatches(owner, generation as number, presentedTokenId, ["active"])) {
        log("bad_hello", { reason: "managed_owner_mismatch", role, agent, generation });
        return { ok: false, closeCode: 4003 };
      }
      // Durable revocation precedes token-file validation.  This preserves the
      // protocol's 4003 result after the exact bearer file has been deleted,
      // without granting authority to an arbitrary bearer: it must first bind
      // to the current private OwnerRecord tuple.
      if (identityIsRevoked(agent, generation as number, presentedTokenId)) {
        log("bad_hello", { reason: "managed_identity_revoked", role, agent, generation });
        return { ok: false, closeCode: 4003 };
      }
    } catch {
      log("bad_hello", { reason: "revocation_state_unreadable", role, agent });
      return { ok: false, closeCode: 4003 };
    }
    if (!validAgentToken(agent, msg.token)) {
      log("bad_hello", { reason: "managed_token_invalid", role, agent });
      return { ok: false, closeCode: 4003 };
    }
    conn.generation = generation as number;
    conn.tokenId = presentedTokenId;
  } else {
    if (!validAgentToken(agent, msg.token)) {
      const principalMismatch = tokenBelongsToAnotherAgent(agent, msg.token);
      log("bad_hello", { reason: principalMismatch ? "principal_mismatch" : "invalid_token", role, agent });
      return { ok: false, closeCode: principalMismatch ? 4002 : 4001 };
    }
    if (role !== "subscriber" && !registry.seats.has(agent)) {
      log("bad_hello", { reason: "principal_not_enabled", role, agent });
      return { ok: false, closeCode: 4002 };
    }
    conn.generation = null;
    conn.tokenId = null;
  }
  conn.role = role;
  conn.agent = agent;
  log("hello", { role, agent, generation: conn.generation });

  if (role === "subscriber") {
    // aperture-3x136: hand the newcomer the current presence of everyone so it
    // isn't blind to agents that joined before it (re)connected — the fix for
    // the watchdog false-re-kick loop on hub restart.
    for (const [name, entry] of presenceState) {
      send(ws, { type: "presence", agent: name, event: stateToEvent(entry.state), ts: new Date().toISOString() });
    }
  }

  if (role === "agent" && agent) {
    // One socket per agent name — a new connection replaces the old one.
    // The replaced socket's close handler sees it is no longer the mapped
    // socket and does NOT broadcast a spurious leave.
    const old = agents.get(agent);
    if (old && old !== ws) {
      log("agent_replaced", { agent });
      old.close(4000, "replaced by newer connection");
    }
    agents.set(agent, ws);
    broadcastPresence(agent, "join");
    void replayUnread(agent, ws);
  }
  return { ok: true };
}

function validateExactToken(seat: string, expectedTokenId: string): void {
  const path = join(secureTokenDirectory(), `${seat}.token`);
  let fd: number | null = null;
  try {
    fd = openSync(path, constants.O_RDONLY | constants.O_NOFOLLOW);
    const stat = fstatSync(fd);
    if (!stat.isFile() || stat.nlink !== 1 || (stat.mode & 0o077) !== 0) {
      throw new Error("unsafe token file");
    }
    if (typeof process.getuid === "function" && stat.uid !== process.getuid()) {
      throw new Error("token owner mismatch");
    }
    const actualTokenId = createHash("sha256").update(readFileSync(fd)).digest("hex");
    if (actualTokenId !== expectedTokenId) throw new Error("token identity mismatch");
  } finally {
    if (fd !== null) closeSync(fd);
  }
}

interface TokenDeletionProof {
  tokenDeleted: boolean;
  tokenAbsentVerified: true;
  tokenDirectorySynced: true;
}

function deleteExactToken(seat: string, expectedTokenId: string): TokenDeletionProof {
  const tokenDirectory = secureTokenDirectory();
  const path = join(tokenDirectory, `${seat}.token`);
  let tokenDeleted = false;
  try {
    validateExactToken(seat, expectedTokenId);
  } catch (error: unknown) {
    if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
  }
  try {
    unlinkSync(path);
    tokenDeleted = true;
  } catch (error: unknown) {
    if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
  }
  const dirFd = openSync(tokenDirectory, constants.O_RDONLY | constants.O_DIRECTORY | constants.O_NOFOLLOW);
  try {
    const stat = fstatSync(dirFd);
    if (!stat.isDirectory() || (stat.mode & 0o077) !== 0) throw new Error("unsafe token directory");
    if (typeof process.getuid === "function" && stat.uid !== process.getuid()) {
      throw new Error("token directory owner mismatch");
    }
    fsyncSync(dirFd);
  } finally {
    closeSync(dirFd);
  }
  try {
    lstatSync(path);
    throw new Error("token path still exists after revocation");
  } catch (error: unknown) {
    if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
  }
  return { tokenDeleted, tokenAbsentVerified: true, tokenDirectorySynced: true };
}

function closeManagedSocket(candidate: WebSocket): Promise<boolean> {
  if (candidate.readyState === WebSocket.CLOSED) return Promise.resolve(true);
  return new Promise((resolve) => {
    let settled = false;
    const finish = (closed: boolean): void => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      resolve(closed);
    };
    const timer = setTimeout(() => finish(false), 1_000);
    candidate.once("close", () => finish(candidate.readyState === WebSocket.CLOSED));
    candidate.close(4001, "generation revoked");
  });
}

async function handleRevokeGeneration(ws: WebSocket, conn: Conn, msg: Record<string, unknown>): Promise<void> {
  if (conn.role !== "subscriber" || conn.agent !== "watchdog") {
    log("revocation_rejected", { reason: "launcher_control_required" });
    send(ws, { type: "error", code: "E_CONTROL_UNAUTHORIZED" });
    return;
  }
  const seat = typeof msg.seat === "string" ? msg.seat : "";
  const generation = msg.generation;
  const requestedTokenId = typeof msg.token_id === "string" ? msg.token_id : "";
  const principal = loadSeatRegistry().seats.get(seat);
  if (
    principal?.group !== "team" ||
    !Number.isSafeInteger(generation) ||
    (generation as number) < 1 ||
    !/^[a-f0-9]{64}$/.test(requestedTokenId)
  ) {
    log("revocation_rejected", { reason: "invalid_managed_identity", seat });
    send(ws, { type: "error", code: "E_REVOCATION_INVALID" });
    return;
  }
  try {
    const owner = readManagedOwner(seat);
    if (!managedOwnerMatches(owner, generation as number, requestedTokenId, ["starting", "active"])) {
      throw new Error("owner identity mismatch");
    }
    // OwnerRecord is the authority.  The token file is intentionally not a
    // precondition here: idempotent replay happens after durable deletion.
    revokeGeneration(seat, generation as number, requestedTokenId);
    const matchingSockets: Array<{ socket: WebSocket; identity: Conn }> = [];
    for (const [candidate, identity] of conns) {
      if (
        identity.agent === seat &&
        identity.generation === generation &&
        identity.tokenId === requestedTokenId
      ) {
        // Fence authority synchronously at the durable revocation point,
        // before token cleanup or the asynchronous close handshake.
        identity.revoked = true;
        matchingSockets.push({ socket: candidate, identity });
      }
    }
    log("generation_revocation_fenced", { seat, generation, sockets_fenced: matchingSockets.length });
    let tokenProof: TokenDeletionProof | null = null;
    let cleanupFailed = false;
    try {
      tokenProof = deleteExactToken(seat, requestedTokenId);
    } catch {
      cleanupFailed = true;
    }
    const closeResults = await Promise.all(matchingSockets.map(({ socket }) => closeManagedSocket(socket)));
    const socketsClosed = closeResults.filter(Boolean).length;
    if (socketsClosed !== matchingSockets.length) {
      log("revocation_failed", { seat, generation, reason: "socket_close_unverified" });
      send(ws, { type: "error", code: "E_REVOCATION_INCOMPLETE" });
      return;
    }
    if (cleanupFailed || tokenProof === null) {
      log("revocation_failed", { seat, generation, reason: "token_cleanup_unverified", sockets_closed_verified: socketsClosed });
      send(ws, { type: "error", code: "E_REVOCATION_FAILED" });
      return;
    }
    log("generation_revoked", {
      seat,
      generation,
      token_deleted: tokenProof.tokenDeleted,
      token_absent_verified: tokenProof.tokenAbsentVerified,
      token_directory_synced: tokenProof.tokenDirectorySynced,
      sockets_close_requested: matchingSockets.length,
      sockets_closed_verified: socketsClosed,
    });
    send(ws, {
      type: "ok",
      control: "revoke_generation",
      seat,
      generation,
      token_deleted: tokenProof.tokenDeleted,
      token_absent_verified: tokenProof.tokenAbsentVerified,
      token_directory_synced: tokenProof.tokenDirectorySynced,
      sockets_close_requested: matchingSockets.length,
      sockets_closed_verified: socketsClosed,
    });
  } catch {
    log("revocation_failed", { seat, generation });
    send(ws, { type: "error", code: "E_REVOCATION_FAILED" });
  }
}

async function handleNotify(ws: WebSocket, conn: Conn, msg: Record<string, unknown>): Promise<void> {
  const to = typeof msg.to === "string" ? msg.to : "";
  const id = typeof msg.id === "string" ? msg.id : "";
  const from = conn.agent ?? "unknown";
  if (typeof msg.from === "string" && msg.from !== from) {
    log("notify_rejected", { reason: "from_mismatch", agent: from });
    send(ws, { type: "ok", id, outcome: "withheld" });
    return;
  }
  const authorization = authorizeMessage(loadSeatRegistry(), from, to);
  if (!authorization.allowed) {
    try {
      const reason = await persistDeniedNotification(id, from, to);
      log("notify_withheld", { to, id, from, reason });
    } catch {
      // Metadata only: never log the stored message body, bd stderr, or a
      // parsed record. The fixed reason distinguishes persistence failure
      // without turning observability into a content side channel.
      // A forged/mismatched id remains withheld but cannot label another row.
      log("notify_withhold_failed", {
        to,
        id,
        from,
        reason: "authoritative_binding_failed",
      });
    }
    send(ws, { type: "ok", id, outcome: "withheld" });
    return;
  }
  const preview = typeof msg.preview === "string" ? msg.preview : "";
  const target = agents.get(to);
  let outcome: NotifyOutcome;
  if (codexBridges.has(to)) {
    // Codex agent: no Monitor socket — deliver by injecting a turn into its
    // app-server thread. The bridge fetches the full body from BEADS itself.
    codexBridges.deliver(to);
    outcome = "codex";
    log("notify_codex", { to, id, from });
  } else if (target) {
    send(target, { type: "message", id, from, preview });
    outcome = "forwarded";
    log("notify_forwarded", { to, id, from });
  } else {
    // Recipient offline: no-op — unread replay on reconnect covers it.
    outcome = "offline";
    log("notify_offline", { to, id, from });
  }
  // Always ack so the producer's await resolves — and say what actually
  // happened, so the producer's log line is honest (aperture-oeb6q).
  send(ws, { type: "ok", id, outcome });
}

/** What the hub actually did with a notify — carried on the ok ack. */
type NotifyOutcome = "forwarded" | "codex" | "offline" | "withheld";

/**
 * aperture-trgpo: a Claude Code hook (UserPromptSubmit / PreToolUse → busy,
 * Stop → idle) tells the hub what its agent is doing. Only the codex bridge
 * had turn-state before this; Claude agents' Monitors emit join/leave only,
 * so the launcher chip and get_presence showed them as merely "online".
 *
 * The target is the authenticated principal — never a field in the frame. A
 * hint never creates presence: if the agent has no live Monitor socket it is
 * offline as far as the hub is concerned and the hint is dropped, so a hook
 * firing during a Monitor reconnect can't leave a ghost "busy" entry behind.
 */
function handlePresenceHint(ws: WebSocket, conn: Conn, msg: Record<string, unknown>): void {
  const agent = conn.agent ?? "unknown";
  if (typeof msg.agent === "string" && msg.agent !== agent) {
    log("presence_hint_rejected", { reason: "agent_mismatch", agent });
    return;
  }
  const event = msg.event;
  if (event !== "busy" && event !== "idle") {
    log("presence_hint_rejected", { reason: "bad_event", agent, hint: String(event) });
    return;
  }
  if (codexBridges.has(agent)) {
    // The bridge owns codex turn state; a hint would fight it.
    log("presence_hint_ignored", { reason: "codex_bridged", agent, hint: event });
    send(ws, { type: "ok", hint: event, applied: false });
    return;
  }
  if (!agents.has(agent)) {
    log("presence_hint_ignored", { reason: "not_present", agent, hint: event });
    send(ws, { type: "ok", hint: event, applied: false });
    return;
  }
  const applied = broadcastPresence(agent, event);
  log("presence_hint", { agent, hint: event, applied });
  send(ws, { type: "ok", hint: event, applied });
}

// aperture-oeb6q: clear the presence snapshot at startup — BEFORE the codex
// bridges start, so this write is guaranteed to carry zero agents and a stale
// file left by a crashed hub (dead hub_pid, phantom agents) can't lie past
// this boot. Every later join/busy/idle/leave rewrites the file.
persistPresence();

// Phase 2: Codex bridge clients — one WS-over-unix-socket JSON-RPC client per
// discovered Codex agent (manifest model "codex/…"). A connected+bound bridge
// counts as presence for that agent; busy/idle tracks turn state.
const codexBridges = startCodexBridges({
  broadcastPresence,
  log,
  skipReplay: SKIP_REPLAY,
});

const wss = new WebSocketServer({ host: HOST, port: PORT, maxPayload: MAX_FRAME_BYTES });

wss.on("listening", () => {
  log("listening", { host: HOST, port: PORT, skipReplay: SKIP_REPLAY });
});

wss.on("error", (err) => {
  log("server_error", { error: err.message });
  // aperture-3x136: a fatal listen error (EADDRINUSE from a stale/orphan hub,
  // EACCES, etc.) means this process will never serve. Previously we only
  // logged and stayed alive — the Rust supervisor's try_wait then saw the
  // child as "still running" and never respawned, so freeing the port by
  // hand did NOT self-heal. Exit non-zero so the supervisor's respawn loop
  // fires: once the squatter is gone (its own shutdown sweep, or the
  // supervisor's residual-listener kill), the next spawn binds cleanly.
  const code = (err as NodeJS.ErrnoException).code;
  if (code === "EADDRINUSE" || code === "EACCES" || code === "EADDRNOTAVAIL") {
    log("server_error_fatal_exit", { code });
    process.exit(1);
  }
});

wss.on("connection", (ws) => {
  const conn: Conn = { role: null, agent: null, isAlive: true, generation: null, tokenId: null, revoked: false };
  conns.set(ws, conn);
  const helloDeadline = setTimeout(() => {
    if (conn.role === null) ws.close(4001, "expected hello");
  }, 5_000);
  helloDeadline.unref?.();

  ws.on("pong", () => {
    conn.isAlive = true;
  });

  ws.on("message", (data) => {
    let msg: Record<string, unknown>;
    try {
      const parsed = JSON.parse(data.toString());
      if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
        throw new Error("not an object");
      }
      msg = parsed as Record<string, unknown>;
    } catch {
      if (conn.role === null) {
        // First message must be a valid hello.
        log("bad_first_message", { reason: "unparseable" });
        ws.close(4001, "expected hello");
      } else {
        log("bad_message", { role: conn.role, agent: conn.agent });
      }
      return;
    }

    if (conn.role === null) {
      if (msg.type !== "hello") {
        ws.close(4001, "expected hello");
        return;
      }
      const result = handleHello(ws, conn, msg);
      if (!result.ok) {
        const reason = result.closeCode === 4002
          ? "principal mismatch"
          : result.closeCode === 4003
            ? "managed identity rejected"
            : "expected hello";
        ws.close(result.closeCode, reason);
      }
      else clearTimeout(helloDeadline);
      return;
    }

    if (conn.revoked) {
      log("revoked_frame_rejected", { role: conn.role, agent: conn.agent, frame_type_kind: typeof msg.type });
      return;
    }

    if (conn.role === "producer" && msg.type === "notify") {
      void handleNotify(ws, conn, msg);
      return;
    }

    if (conn.role === "producer" && msg.type === "presence_hint") {
      handlePresenceHint(ws, conn, msg);
      return;
    }

    if (msg.type === "revoke_generation") {
      void handleRevokeGeneration(ws, conn, msg);
      return;
    }

    // Anything else post-hello is ignored (logged for forensics).
    log("ignored_message", { role: conn.role, agent: conn.agent, frame_type_kind: typeof msg.type });
  });

  ws.on("close", () => {
    clearTimeout(helloDeadline);
    conns.delete(ws);
    if (conn.role === "agent" && conn.agent && agents.get(conn.agent) === ws) {
      agents.delete(conn.agent);
      broadcastPresence(conn.agent, "leave");
    }
  });

  ws.on("error", (err) => {
    log("socket_error", { role: conn.role, agent: conn.agent, error: err.message });
  });
});

// Heartbeat: ping every 30s; terminate sockets that missed the previous ping.
// terminate() fires the close handler → leave broadcast for agents.
const heartbeat = setInterval(() => {
  for (const [ws, conn] of conns) {
    if (!conn.isAlive) {
      log("heartbeat_dead", { role: conn.role, agent: conn.agent });
      ws.terminate();
      continue;
    }
    conn.isAlive = false;
    ws.ping();
  }
}, HEARTBEAT_MS);
heartbeat.unref?.();

function shutdown(signal: string): void {
  log("shutdown", { signal });
  clearInterval(heartbeat);
  codexBridges.stop();
  for (const ws of conns.keys()) {
    ws.close(1001, "hub shutting down");
  }
  wss.close(() => {
    process.exit(0);
  });
  // Belt-and-braces: don't hang forever if a socket won't close.
  setTimeout(() => process.exit(0), 2000).unref?.();
}

process.on("SIGTERM", () => shutdown("SIGTERM"));
process.on("SIGINT", () => shutdown("SIGINT"));
