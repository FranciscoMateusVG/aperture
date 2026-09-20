import { lstatSync, mkdirSync } from "node:fs";
import { homedir } from "node:os";
import { isAbsolute, relative, resolve, sep } from "node:path";
import { RUN_DIR } from "./presence-snapshot.js";

function isWithin(parent: string, child: string): boolean {
  const rel = relative(parent, child);
  return rel === "" || (!rel.startsWith(`..${sep}`) && rel !== ".." && !isAbsolute(rel));
}

function assertOwnedDirectory(path: string, privateDirectory: boolean): void {
  const stat = lstatSync(path);
  if (!stat.isDirectory() || stat.isSymbolicLink()) {
    throw new Error("E_RUNTIME_PATH_UNSAFE: runtime path component is not a directory");
  }
  if (typeof process.getuid === "function" && stat.uid !== process.getuid()) {
    throw new Error("E_RUNTIME_PATH_UNSAFE: runtime path owner mismatch");
  }
  const forbidden = privateDirectory ? 0o077 : 0o022;
  if ((stat.mode & forbidden) !== 0) {
    throw new Error("E_RUNTIME_PATH_UNSAFE: runtime path permissions are unsafe");
  }
}

/**
 * Validate the fixed runtime tree component-by-component.  `realpath` and
 * `canonicalize` are deliberately not used: following a symlink first and
 * validating its destination would make the check meaningless.
 *
 * The home and ~/.aperture ancestors may be readable, but must be owned and
 * non-writable by group/other.  The runtime root and every managed child are
 * private 0700 directories.
 */
export function assertPrivateRuntimeDirectory(path: string): string {
  const home = resolve(homedir());
  const run = resolve(RUN_DIR);
  const target = resolve(path);
  if (!isWithin(home, run) || !isWithin(run, target)) {
    throw new Error("E_RUNTIME_PATH_UNSAFE: runtime path escaped fixed roots");
  }

  assertOwnedDirectory(home, false);
  let cursor = home;
  const components = relative(home, target).split(sep).filter(Boolean);
  for (const component of components) {
    cursor = resolve(cursor, component);
    assertOwnedDirectory(cursor, isWithin(run, cursor));
  }
  return target;
}

/** Create one exact child beneath the already-private runtime root. */
export function ensurePrivateRuntimeChild(name: string): string {
  if (!/^[a-z][a-z0-9-]{0,63}$/.test(name)) {
    throw new Error("E_RUNTIME_PATH_UNSAFE: invalid runtime child name");
  }
  assertPrivateRuntimeDirectory(RUN_DIR);
  const path = resolve(RUN_DIR, name);
  try {
    lstatSync(path);
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== "ENOENT") throw error;
    mkdirSync(path, { mode: 0o700 });
  }
  return assertPrivateRuntimeDirectory(path);
}

/** Environment overrides may name the fixed child, never another tree. */
export function fixedRuntimeChild(envValue: string | undefined, name: string): string {
  const expected = resolve(RUN_DIR, name);
  if (envValue !== undefined && resolve(envValue) !== expected) {
    throw new Error("E_RUNTIME_PATH_UNSAFE: runtime child override is not canonical");
  }
  return ensurePrivateRuntimeChild(name);
}

/** Read-only variant: validate the fixed child but never create it. */
export function fixedExistingRuntimeChild(envValue: string | undefined, name: string): string {
  if (!/^[a-z][a-z0-9-]{0,63}$/.test(name)) {
    throw new Error("E_RUNTIME_PATH_UNSAFE: invalid runtime child name");
  }
  const expected = resolve(RUN_DIR, name);
  if (envValue !== undefined && resolve(envValue) !== expected) {
    throw new Error("E_RUNTIME_PATH_UNSAFE: runtime child override is not canonical");
  }
  return assertPrivateRuntimeDirectory(expected);
}
