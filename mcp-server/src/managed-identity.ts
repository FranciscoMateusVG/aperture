import { createHash } from "node:crypto";

export interface ManagedHelloFields {
  generation: number;
  token_id: string;
}

/**
 * Team launchers set a generation; standing seats omit it. The token id is a
 * digest derived in-process from the bearer, never another caller-supplied
 * authority field.
 */
export function managedHelloFields(token: string): Record<string, unknown> {
  const raw = process.env.APERTURE_TEAM_GENERATION;
  if (raw === undefined || raw === "") return {};
  if (!/^[1-9][0-9]{0,15}$/.test(raw)) {
    throw new Error("managed seat generation is invalid");
  }
  const generation = Number(raw);
  if (!Number.isSafeInteger(generation)) throw new Error("managed seat generation is invalid");
  return {
    generation,
    token_id: createHash("sha256").update(token).digest("hex"),
  };
}
