import { closeSync, constants, fstatSync, openSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { isValidSeatName } from "./seat-registry.js";
import { fixedExistingRuntimeChild } from "./private-runtime-path.js";

const MAX_OWNER_BYTES = 256 * 1024;
const TOKEN_ID = /^[a-f0-9]{64}$/;

export type ManagedOwnerState = "starting" | "active" | "stale" | "quarantined";

export interface ManagedOwnerIdentity {
  seat: string;
  generation: number;
  state: ManagedOwnerState;
  tokenId: string | null;
}

export interface ManagedExecutionTuple {
  harness: "codex" | "claude";
  model: string;
  reasoning: string | null;
}

export interface ManagedStartingRuntime extends ManagedOwnerIdentity {
  state: "starting";
  tokenId: string;
  requested: ManagedExecutionTuple;
  pid: number;
  startTimeUs: number;
}

export interface ManagedActiveRuntime extends ManagedOwnerIdentity {
  state: "active";
  tokenId: string;
  requested: ManagedExecutionTuple;
  pid: number;
  startTimeUs: number;
  threadId: string;
}

export interface ManagedObservedStartingRuntime extends ManagedStartingRuntime {
  threadId: string;
}

function ownerRoot(): string {
  return fixedExistingRuntimeChild(process.env.APERTURE_OWNER_DIR, "owner");
}

export function readManagedOwner(seat: string): ManagedOwnerIdentity {
  if (!isValidSeatName(seat)) throw new Error("E_OWNER_CORRUPT: invalid owner seat");
  const path = join(ownerRoot(), `${seat}.json`);
  let fd: number | null = null;
  try {
    fd = openSync(path, constants.O_RDONLY | constants.O_NOFOLLOW);
    const stat = fstatSync(fd);
    if (
      !stat.isFile() ||
      stat.nlink !== 1 ||
      (stat.mode & 0o077) !== 0 ||
      stat.size > MAX_OWNER_BYTES ||
      (typeof process.getuid === "function" && stat.uid !== process.getuid())
    ) {
      throw new Error("E_OWNER_CORRUPT: unsafe owner record");
    }
    const value: unknown = JSON.parse(readFileSync(fd, "utf8"));
    if (!value || typeof value !== "object" || Array.isArray(value)) {
      throw new Error("E_OWNER_CORRUPT: malformed owner record");
    }
    const record = value as Record<string, unknown>;
    const incarnation = record.incarnation;
    const provisional = record.provisional_token_id ?? null;
    const state = record.state;
    const generation = record.generation;
    if (
      record.schema_version !== 1 ||
      record.seat !== seat ||
      !Number.isSafeInteger(generation) ||
      (generation as number) < 0 ||
      (state !== "starting" && state !== "active" && state !== "stale" && state !== "quarantined") ||
      (incarnation !== null && (!incarnation || typeof incarnation !== "object" || Array.isArray(incarnation))) ||
      (provisional !== null && (typeof provisional !== "string" || !TOKEN_ID.test(provisional)))
    ) {
      throw new Error("E_OWNER_CORRUPT: invalid owner identity");
    }
    const incarnationToken = incarnation === null ? null : (incarnation as Record<string, unknown>).token_id;
    if (incarnationToken !== null && (typeof incarnationToken !== "string" || !TOKEN_ID.test(incarnationToken))) {
      throw new Error("E_OWNER_CORRUPT: invalid owner token identity");
    }
    if (state === "starting") {
      if (provisional === null || (incarnationToken !== null && incarnationToken !== provisional)) {
        throw new Error("E_OWNER_CORRUPT: invalid provisional owner identity");
      }
    }
    if (state === "active" && (incarnationToken === null || provisional !== null)) {
      throw new Error("E_OWNER_CORRUPT: invalid active owner identity");
    }
    const token = state === "starting" ? provisional : incarnationToken;
    return { seat, generation: generation as number, state, tokenId: token };
  } catch (error) {
    if (error instanceof SyntaxError) throw new Error("E_OWNER_CORRUPT: malformed owner record");
    throw error;
  } finally {
    if (fd !== null) closeSync(fd);
  }
}

export function managedOwnerMatches(
  owner: ManagedOwnerIdentity,
  generation: number,
  tokenId: string,
  allowedStates: readonly ManagedOwnerState[],
): boolean {
  return owner.generation === generation && owner.tokenId === tokenId && allowedStates.includes(owner.state);
}

/**
 * Read the exact gated runtime identity used by the Codex bridge. A Starting
 * record without a durable process candidate is not launchable yet; no
 * caller/environment value can fill these fields.
 */
export function readManagedStartingRuntime(seat: string): ManagedStartingRuntime | null {
  const identity = readManagedOwner(seat);
  if (identity.state !== "starting") return null;

  const path = join(ownerRoot(), `${seat}.json`);
  let fd: number | null = null;
  try {
    fd = openSync(path, constants.O_RDONLY | constants.O_NOFOLLOW);
    const stat = fstatSync(fd);
    if (
      !stat.isFile() ||
      stat.nlink !== 1 ||
      (stat.mode & 0o077) !== 0 ||
      stat.size > MAX_OWNER_BYTES ||
      (typeof process.getuid === "function" && stat.uid !== process.getuid())
    ) {
      throw new Error("E_OWNER_CORRUPT: unsafe owner record");
    }
    const value = JSON.parse(readFileSync(fd, "utf8")) as Record<string, unknown>;
    const requested = value.requested;
    const incarnation = value.incarnation;
    if (
      !requested ||
      typeof requested !== "object" ||
      Array.isArray(requested) ||
      !incarnation ||
      typeof incarnation !== "object" ||
      Array.isArray(incarnation)
    ) {
      throw new Error("E_OWNER_CORRUPT: gated runtime identity is incomplete");
    }
    const tuple = requested as Record<string, unknown>;
    const candidate = incarnation as Record<string, unknown>;
    if (
      value.schema_version !== 1 ||
      value.seat !== seat ||
      value.state !== "starting" ||
      value.generation !== identity.generation ||
      value.provisional_token_id !== identity.tokenId ||
      (tuple.harness !== "codex" && tuple.harness !== "claude") ||
      typeof tuple.model !== "string" ||
      !/^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$/.test(tuple.model) ||
      (tuple.reasoning !== null &&
        tuple.reasoning !== "low" &&
        tuple.reasoning !== "medium" &&
        tuple.reasoning !== "high" &&
        tuple.reasoning !== "xhigh" &&
        tuple.reasoning !== "max" &&
        tuple.reasoning !== "ultra") ||
      !Number.isSafeInteger(candidate.pid) ||
      (candidate.pid as number) < 1 ||
      !Number.isSafeInteger(candidate.start_time) ||
      (candidate.start_time as number) < 1 ||
      candidate.token_id !== identity.tokenId ||
      (candidate.observed !== false && candidate.observed !== true) ||
      typeof candidate.thread_id !== "string"
    ) {
      throw new Error("E_OWNER_CORRUPT: invalid gated runtime identity");
    }
    if (candidate.observed === true) return null;
    if (candidate.thread_id !== "") {
      throw new Error("E_OWNER_CORRUPT: invalid gated runtime identity");
    }
    return {
      ...identity,
      state: "starting",
      tokenId: identity.tokenId as string,
      requested: {
        harness: tuple.harness,
        model: tuple.model,
        reasoning: tuple.reasoning as string | null,
      },
      pid: candidate.pid as number,
      startTimeUs: candidate.start_time as number,
    };
  } catch (error) {
    if (error instanceof SyntaxError) throw new Error("E_OWNER_CORRUPT: malformed owner record");
    throw error;
  } finally {
    if (fd !== null) closeSync(fd);
  }
}

/** Exact owner identity during the narrow native observation -> Active CAS
 * transition. This is not an Active owner and grants no delivery authority. */
export function readManagedObservedStartingRuntime(
  seat: string,
): ManagedObservedStartingRuntime | null {
  const identity = readManagedOwner(seat);
  if (identity.state !== "starting") return null;

  const path = join(ownerRoot(), `${seat}.json`);
  let fd: number | null = null;
  try {
    fd = openSync(path, constants.O_RDONLY | constants.O_NOFOLLOW);
    const stat = fstatSync(fd);
    if (
      !stat.isFile() ||
      stat.nlink !== 1 ||
      (stat.mode & 0o077) !== 0 ||
      stat.size > MAX_OWNER_BYTES ||
      (typeof process.getuid === "function" && stat.uid !== process.getuid())
    ) {
      throw new Error("E_OWNER_CORRUPT: unsafe owner record");
    }
    const value = JSON.parse(readFileSync(fd, "utf8")) as Record<string, unknown>;
    const requested = value.requested;
    const incarnation = value.incarnation;
    if (
      !requested ||
      typeof requested !== "object" ||
      Array.isArray(requested) ||
      !incarnation ||
      typeof incarnation !== "object" ||
      Array.isArray(incarnation)
    ) {
      throw new Error("E_OWNER_CORRUPT: observed runtime identity is incomplete");
    }
    const tuple = requested as Record<string, unknown>;
    const candidate = incarnation as Record<string, unknown>;
    if (
      value.schema_version !== 1 ||
      value.seat !== seat ||
      value.state !== "starting" ||
      value.generation !== identity.generation ||
      value.provisional_token_id !== identity.tokenId ||
      tuple.harness !== "codex" ||
      typeof tuple.model !== "string" ||
      !/^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$/.test(tuple.model) ||
      (tuple.reasoning !== "low" &&
        tuple.reasoning !== "medium" &&
        tuple.reasoning !== "high" &&
        tuple.reasoning !== "xhigh" &&
        tuple.reasoning !== "max" &&
        tuple.reasoning !== "ultra") ||
      !Number.isSafeInteger(candidate.pid) ||
      (candidate.pid as number) < 1 ||
      !Number.isSafeInteger(candidate.start_time) ||
      (candidate.start_time as number) < 1 ||
      candidate.token_id !== identity.tokenId ||
      (candidate.observed !== false && candidate.observed !== true) ||
      typeof candidate.thread_id !== "string"
    ) {
      throw new Error("E_OWNER_CORRUPT: invalid observed runtime identity");
    }
    if (candidate.observed === false) return null;
    if (
      typeof candidate.thread_id !== "string" ||
      !/^[A-Za-z0-9-]{1,128}$/.test(candidate.thread_id) ||
      candidate.harness !== tuple.harness ||
      candidate.model !== tuple.model ||
      candidate.reasoning !== tuple.reasoning
    ) {
      throw new Error("E_OWNER_CORRUPT: invalid observed runtime identity");
    }
    return {
      ...identity,
      state: "starting",
      tokenId: identity.tokenId as string,
      requested: {
        harness: "codex",
        model: tuple.model,
        reasoning: tuple.reasoning,
      },
      pid: candidate.pid as number,
      startTimeUs: candidate.start_time as number,
      threadId: candidate.thread_id,
    };
  } catch (error) {
    if (error instanceof SyntaxError) throw new Error("E_OWNER_CORRUPT: malformed owner record");
    throw error;
  } finally {
    if (fd !== null) closeSync(fd);
  }
}

/** Exact owner-bound thread for reconnect; managed seats never fall back to a
 * newest-thread heuristic once a generation is Active. */
export function readManagedActiveRuntime(seat: string): ManagedActiveRuntime | null {
  const identity = readManagedOwner(seat);
  if (identity.state !== "active") return null;

  const path = join(ownerRoot(), `${seat}.json`);
  let fd: number | null = null;
  try {
    fd = openSync(path, constants.O_RDONLY | constants.O_NOFOLLOW);
    const stat = fstatSync(fd);
    if (
      !stat.isFile() ||
      stat.nlink !== 1 ||
      (stat.mode & 0o077) !== 0 ||
      stat.size > MAX_OWNER_BYTES ||
      (typeof process.getuid === "function" && stat.uid !== process.getuid())
    ) {
      throw new Error("E_OWNER_CORRUPT: unsafe owner record");
    }
    const value = JSON.parse(readFileSync(fd, "utf8")) as Record<string, unknown>;
    const requested = value.requested;
    const incarnation = value.incarnation;
    if (
      !requested ||
      typeof requested !== "object" ||
      Array.isArray(requested) ||
      !incarnation ||
      typeof incarnation !== "object" ||
      Array.isArray(incarnation)
    ) {
      throw new Error("E_OWNER_CORRUPT: active runtime identity is incomplete");
    }
    const tuple = requested as Record<string, unknown>;
    const active = incarnation as Record<string, unknown>;
    if (
      value.schema_version !== 1 ||
      value.seat !== seat ||
      value.state !== "active" ||
      value.generation !== identity.generation ||
      value.provisional_token_id !== null ||
      (tuple.harness !== "codex" && tuple.harness !== "claude") ||
      typeof tuple.model !== "string" ||
      !/^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$/.test(tuple.model) ||
      (tuple.reasoning !== null &&
        tuple.reasoning !== "low" &&
        tuple.reasoning !== "medium" &&
        tuple.reasoning !== "high" &&
        tuple.reasoning !== "xhigh" &&
        tuple.reasoning !== "max" &&
        tuple.reasoning !== "ultra") ||
      !Number.isSafeInteger(active.pid) ||
      (active.pid as number) < 1 ||
      !Number.isSafeInteger(active.start_time) ||
      (active.start_time as number) < 1 ||
      active.token_id !== identity.tokenId ||
      active.observed !== true ||
      typeof active.thread_id !== "string" ||
      !/^[A-Za-z0-9-]{1,128}$/.test(active.thread_id) ||
      active.harness !== tuple.harness ||
      active.model !== tuple.model ||
      active.reasoning !== tuple.reasoning
    ) {
      throw new Error("E_OWNER_CORRUPT: invalid active runtime identity");
    }
    return {
      ...identity,
      state: "active",
      tokenId: identity.tokenId as string,
      requested: {
        harness: tuple.harness,
        model: tuple.model,
        reasoning: tuple.reasoning as string | null,
      },
      pid: active.pid as number,
      startTimeUs: active.start_time as number,
      threadId: active.thread_id,
    };
  } catch (error) {
    if (error instanceof SyntaxError) throw new Error("E_OWNER_CORRUPT: malformed owner record");
    throw error;
  } finally {
    if (fd !== null) closeSync(fd);
  }
}
