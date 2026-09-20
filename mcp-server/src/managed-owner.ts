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
    const state = record.state;
    const generation = record.generation;
    if (
      record.schema_version !== 1 ||
      record.seat !== seat ||
      !Number.isSafeInteger(generation) ||
      (generation as number) < 0 ||
      (state !== "starting" && state !== "active" && state !== "stale" && state !== "quarantined") ||
      (incarnation !== null && (!incarnation || typeof incarnation !== "object" || Array.isArray(incarnation)))
    ) {
      throw new Error("E_OWNER_CORRUPT: invalid owner identity");
    }
    const token = incarnation === null ? null : (incarnation as Record<string, unknown>).token_id;
    if (token !== null && (typeof token !== "string" || !TOKEN_ID.test(token))) {
      throw new Error("E_OWNER_CORRUPT: invalid owner token identity");
    }
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
