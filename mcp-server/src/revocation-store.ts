import {
  closeSync,
  constants,
  fstatSync,
  fsyncSync,
  lstatSync,
  openSync,
  readFileSync,
  renameSync,
  unlinkSync,
  writeFileSync,
} from "node:fs";
import { join } from "node:path";
import { randomUUID } from "node:crypto";
import { isValidSeatName } from "./seat-registry.js";
import { fixedRuntimeChild } from "./private-runtime-path.js";

const MAX_STORE_BYTES = 256 * 1024;
const MAX_REVOKED_TOKENS = 4096;

export interface RevocationRecord {
  schema_version: 1;
  seat: string;
  revoked_through_generation: number;
  revoked_token_ids: string[];
}

function defaultRoot(): string {
  return fixedRuntimeChild(process.env.APERTURE_REVOCATION_DIR, "revocations");
}

function privateDirectory(path: string): void {
  const stat = lstatSync(path);
  if (!stat.isDirectory() || stat.isSymbolicLink() || (stat.mode & 0o077) !== 0) {
    throw new Error("E_REVOCATION_CORRUPT: unsafe revocation directory");
  }
  if (typeof process.getuid === "function" && stat.uid !== process.getuid()) {
    throw new Error("E_REVOCATION_CORRUPT: revocation directory owner mismatch");
  }
}

function ensureRoot(): string {
  const root = defaultRoot();
  privateDirectory(root);
  return root;
}

function recordPath(root: string, seat: string): string {
  if (!isValidSeatName(seat)) throw new Error("E_REVOCATION_CORRUPT: invalid seat");
  return join(root, `${seat}.json`);
}

function validTokenId(value: unknown): value is string {
  return typeof value === "string" && /^[a-f0-9]{64}$/.test(value);
}

function parseRecord(raw: Buffer, seat: string): RevocationRecord {
  if (raw.length > MAX_STORE_BYTES) throw new Error("E_REVOCATION_CORRUPT: revocation state exceeds limit");
  let value: unknown;
  try {
    value = JSON.parse(raw.toString("utf8"));
  } catch {
    throw new Error("E_REVOCATION_CORRUPT: malformed revocation state");
  }
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new Error("E_REVOCATION_CORRUPT: malformed revocation state");
  }
  const record = value as Record<string, unknown>;
  const keys = Object.keys(record).sort();
  if (keys.join(",") !== "revoked_through_generation,revoked_token_ids,schema_version,seat") {
    throw new Error("E_REVOCATION_CORRUPT: unknown revocation fields");
  }
  if (
    record.schema_version !== 1 ||
    record.seat !== seat ||
    !Number.isSafeInteger(record.revoked_through_generation) ||
    (record.revoked_through_generation as number) < 0 ||
    !Array.isArray(record.revoked_token_ids) ||
    record.revoked_token_ids.length > MAX_REVOKED_TOKENS ||
    !record.revoked_token_ids.every(validTokenId)
  ) {
    throw new Error("E_REVOCATION_CORRUPT: invalid revocation state");
  }
  const tokenIds = record.revoked_token_ids as string[];
  if (new Set(tokenIds).size !== tokenIds.length || [...tokenIds].sort().join(",") !== tokenIds.join(",")) {
    throw new Error("E_REVOCATION_CORRUPT: noncanonical revocation state");
  }
  return {
    schema_version: 1,
    seat,
    revoked_through_generation: record.revoked_through_generation as number,
    revoked_token_ids: tokenIds,
  };
}

function readRevocationAtRoot(seat: string, root: string): RevocationRecord {
  privateDirectory(root);
  const path = recordPath(root, seat);
  let before;
  try {
    before = lstatSync(path);
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") {
      return { schema_version: 1, seat, revoked_through_generation: 0, revoked_token_ids: [] };
    }
    throw error;
  }
  if (!before.isFile() || before.isSymbolicLink() || before.nlink !== 1 || (before.mode & 0o077) !== 0) {
    throw new Error("E_REVOCATION_CORRUPT: unsafe revocation file");
  }
  let fd: number | undefined;
  try {
    fd = openSync(path, constants.O_RDONLY | constants.O_NOFOLLOW);
    const opened = fstatSync(fd);
    if (opened.dev !== before.dev || opened.ino !== before.ino || opened.size > MAX_STORE_BYTES) {
      throw new Error("E_REVOCATION_CORRUPT: revocation file changed during read");
    }
    return parseRecord(readFileSync(fd), seat);
  } finally {
    if (fd !== undefined) closeSync(fd);
  }
}

export function readRevocation(seat: string): RevocationRecord {
  return readRevocationAtRoot(seat, ensureRoot());
}

function persist(record: RevocationRecord, root: string): void {
  privateDirectory(root);
  const path = recordPath(root, record.seat);
  try {
    lstatSync(path);
    readRevocationAtRoot(record.seat, root);
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
  }
  const temp = join(root, `.${record.seat}.${randomUUID()}.tmp`);
  const bytes = Buffer.from(`${JSON.stringify(record)}\n`);
  if (bytes.length > MAX_STORE_BYTES) throw new Error("E_REVOCATION_CORRUPT: revocation state exceeds limit");
  let fd: number | undefined;
  try {
    fd = openSync(temp, constants.O_WRONLY | constants.O_CREAT | constants.O_EXCL | constants.O_NOFOLLOW, 0o600);
    writeFileSync(fd, bytes);
    fsyncSync(fd);
    closeSync(fd);
    fd = undefined;
    renameSync(temp, path);
    const dirFd = openSync(root, constants.O_RDONLY | constants.O_DIRECTORY | constants.O_NOFOLLOW);
    try { fsyncSync(dirFd); } finally { closeSync(dirFd); }
    readRevocationAtRoot(record.seat, root);
  } catch (error) {
    if (fd !== undefined) closeSync(fd);
    try { unlinkSync(temp); } catch { /* best effort temp cleanup */ }
    throw error;
  }
}

export function revokeGeneration(seat: string, generation: number, tokenId: string): RevocationRecord {
  const root = ensureRoot();
  if (!Number.isSafeInteger(generation) || generation < 1 || !validTokenId(tokenId)) {
    throw new Error("E_REVOCATION_INVALID: invalid generation or token id");
  }
  const current = readRevocationAtRoot(seat, root);
  if (generation <= current.revoked_through_generation) {
    if (!current.revoked_token_ids.includes(tokenId)) {
      throw new Error("E_REVOCATION_CONFLICT: generation was revoked with another token id");
    }
    return current;
  }
  if (generation !== current.revoked_through_generation + 1 || current.revoked_token_ids.includes(tokenId)) {
    throw new Error("E_REVOCATION_CONFLICT: revocation sequence or token reuse is invalid");
  }
  const next: RevocationRecord = {
    schema_version: 1,
    seat,
    revoked_through_generation: generation,
    revoked_token_ids: [...current.revoked_token_ids, tokenId].sort(),
  };
  persist(next, root);
  return next;
}

export function identityIsRevoked(seat: string, generation: number, tokenId: string): boolean {
  const current = readRevocation(seat);
  return generation <= current.revoked_through_generation || current.revoked_token_ids.includes(tokenId);
}
