import { z } from "zod";

const name = z.string().regex(/^[a-z0-9][a-z0-9_-]*$/).max(31);
const generation = z.number().int().positive().max(Number.MAX_SAFE_INTEGER);

/** Selectors only. The Rust child authenticates GLaDOS and derives all proof. */
export const stopSeatSchema = z.object({
  team: name.max(16),
  seat: name,
  expected_generation: generation,
}).strict();
export type StopSeatSelectors = z.infer<typeof stopSeatSchema>;

const ready = z.object({
  action: z.literal("stop_seat"),
  result: z.object({
    team: name.max(16),
    seat: name,
    generation,
    phase: z.literal("ready"),
    checkpoint_recovery: z.literal("valid"),
    owner_state: z.literal("active"),
    blockers: z.array(z.never()).length(0),
  }).strict(),
}).strict();

/** A stop receipt is neither archive nor replacement nor a zero-process owner. */
export function parseStopSeatReady(value: unknown, input: StopSeatSelectors) {
  const parsed = ready.safeParse(value);
  if (!parsed.success || parsed.data.result.team !== input.team ||
      parsed.data.result.seat !== input.seat ||
      parsed.data.result.generation !== input.expected_generation) {
    throw new Error("E_CONTROL_UNKNOWN: stop receipt invalid; inspect and reconcile before any retry");
  }
  return parsed.data;
}
