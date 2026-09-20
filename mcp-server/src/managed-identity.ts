import { createHash } from "node:crypto";
import { readManagedOwner } from "./managed-owner.js";

export interface ManagedHelloFields {
  generation: number;
  token_id: string;
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
