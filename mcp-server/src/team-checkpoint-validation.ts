import { z } from "zod";

const name = z.string().regex(/^[a-z0-9][a-z0-9_-]*$/).max(31);
const positiveSafeInteger = z.number().int().positive().max(Number.MAX_SAFE_INTEGER);

/** Selectors only; native authentication and the collector derive validation. */
export const validateCheckpointSchema = z.object({
  team: name.max(16),
  seat: name,
  expected_generation: positiveSafeInteger,
  seq: positiveSafeInteger,
}).strict();
export type ValidateCheckpointSelectors = z.infer<typeof validateCheckpointSchema>;

const receipt = z.object({
  action: z.literal("validate_checkpoint"),
  result: z.object({
    team: name.max(16),
    seat: name,
    generation: positiveSafeInteger,
    seq: positiveSafeInteger,
    validation: z.enum(["ok", "divergent", "rejected"]),
  }).strict(),
}).strict();

/** Even an ok validation is not a stop receipt or permission to discard context. */
export function parseCheckpointValidation(value: unknown, input: ValidateCheckpointSelectors) {
  const parsed = receipt.safeParse(value);
  if (!parsed.success || parsed.data.result.team !== input.team ||
      parsed.data.result.seat !== input.seat ||
      parsed.data.result.generation !== input.expected_generation ||
      parsed.data.result.seq !== input.seq) {
    throw new Error("E_CONTROL_UNKNOWN: checkpoint validation receipt invalid; inspect and reconcile before any retry");
  }
  return parsed.data;
}
