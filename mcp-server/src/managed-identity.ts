import { createHash } from "node:crypto";
import { setTimeout as delay } from "node:timers/promises";
import { readManagedOwner, type ManagedOwnerIdentity } from "./managed-owner.js";

export interface ManagedHelloFields {
  generation: number;
  token_id: string;
}

// Native launch has a 170 s total budget, including observation and activation.
const OWNER_STARTUP_WAIT_MS = 170_000;
export class ManagedActivationTimeout extends Error {}

/** Wait locally, without connecting or announcing Starting as an authority.
 * A positional Claude boot prompt can start its monitor before native model
 * observation commits Active. Pin that generation/token; do not follow a
 * replacement, a revoked owner, or a disappearing managed record.
 */
export async function waitForManagedActive(
  agent: string,
  token: string,
  options: {
    managedExpected?: boolean;
    readOwner?: typeof readManagedOwner;
    now?: () => number;
    sleep?: (ms: number) => Promise<void>;
    onWaiting?: () => void;
  } = {},
): Promise<void> {
  const read = options.readOwner ?? readManagedOwner;
  const now = options.now ?? (() => performance.now());
  const sleep = options.sleep ?? delay;
  let first: ManagedOwnerIdentity;
  try {
    first = read(agent);
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT" && !options.managedExpected) return;
    throw new Error("managed seat owner identity is invalid");
  }
  const tokenId = createHash("sha256").update(token).digest("hex");
  const valid = (owner: ManagedOwnerIdentity) => owner.seat === agent &&
    owner.generation >= 1 && owner.generation === first.generation && owner.tokenId === tokenId;
  if (!valid(first)) throw new Error("managed seat owner identity is invalid");
  if (first.state === "active") return;
  if (first.state !== "starting") throw new Error("managed seat owner identity is invalid");
  const until = now() + OWNER_STARTUP_WAIT_MS;
  options.onWaiting?.();
  for (;;) {
    if (now() >= until) throw new ManagedActivationTimeout("managed seat activation deadline exceeded");
    await sleep(Math.min(100, until - now()));
    if (now() >= until) throw new ManagedActivationTimeout("managed seat activation deadline exceeded");
    const current = read(agent);
    if (!valid(current)) throw new Error("managed seat owner identity changed");
    if (current.state === "active") return;
    if (current.state !== "starting") throw new Error("managed seat owner identity is invalid");
  }
}

/**
 * Managed seats derive their generation from the current private OwnerRecord;
 * standing seats without an owner record omit managed fields. The token id is
 * a digest derived in-process from the bearer and must match that owner tuple.
 */
export function managedHelloFields(agent: string, token: string): Record<string, unknown> {
  let owner;
  try {
    owner = readManagedOwner(agent);
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return {};
    throw error;
  }
  const tokenId = createHash("sha256").update(token).digest("hex");
  if (owner.state !== "active" || owner.generation < 1 || owner.tokenId !== tokenId) {
    throw new Error("managed seat owner identity is invalid");
  }
  return {
    generation: owner.generation,
    token_id: tokenId,
  };
}
