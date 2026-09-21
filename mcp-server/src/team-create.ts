import { z } from "zod";
import { projectLabelSchema } from "./team-repositories.js";

const id = (max: number) => z.string().min(1).max(max).regex(/^[a-z0-9][a-z0-9_-]*$/);
const text = z.string().min(1).refine(v => [...v].length <= 2000 && Buffer.byteLength(v) <= 8000);
const tuple = z.object({
  harness: z.enum(["claude", "codex"]),
  model: z.string().min(1).max(128),
  reasoning: z.enum(["low", "medium", "high", "xhigh", "max", "ultra"]).nullable(),
}).strict();
const seat = tuple.extend({ role: id(10) }).strict();
export const createTeamSchema = z.object({
  team: id(16),
  project: projectLabelSchema,
  repo: z.string().min(1).max(64).regex(/^[a-z][a-z0-9._-]*$/),
  mission: text,
  acceptance: text,
  preset_id: id(31).nullable(),
  seats: z.array(seat).min(2).max(99),
  lead_index: z.number().int().nonnegative(),
  fallbacks: z.array(tuple).max(16),
}).strict().refine(v => v.lead_index < v.seats.length);
export type CreateTeamSelectors = z.infer<typeof createTeamSchema>;

const hash = z.string().regex(/^[a-f0-9]{64}$/);
const response = z.object({
  action: z.literal("create"),
  result: z.object({
    team: z.object({
      snapshot: z.object({
        schema_version: z.literal(1), team: id(16), project: z.string(), repo: z.string(),
        mission: z.string(), acceptance: z.string(), lead: id(31),
        preset: z.object({ id: id(31).nullable(), sha256: hash.nullable() }),
        seats: z.array(seat.extend({ name: id(31) })), fallbacks: z.array(tuple),
        grants: z.array(z.unknown()).length(0),
        creation_request_id: z.string().uuid(), staging_uuid: z.string().uuid(), created_at: z.string().min(1),
      }),
      state: z.object({ state: z.literal("pending"), generation: z.literal(0), epic_id: z.null() }),
      seats: z.array(z.object({ configured: seat.extend({ name: id(31) }), observed_owner: z.null() })),
      capabilities: z.object({ cancel: z.literal(true), activate: z.literal(true), start: z.literal(false),
        checkpoint: z.literal(false), replace: z.literal(false), archive: z.literal(false) }),
    }),
    creation_request: z.object({
      schema_version: z.literal(1), request_id: z.string().uuid(), team: id(16), project: z.string(), repo: z.string(),
      snapshot_sha256: hash, expected_generation: z.literal(0), created_at: z.string().min(1),
    }),
  }),
});
const fail = (): never => { throw new Error("E_CONTROL_FAILED: pending creation response did not match request; refresh before any retry"); };
const equal = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b);
/** Validate the real Rust envelope; never promote a pending response to activation. */
export function parseCreatedTeam(value: unknown, input: CreateTeamSelectors) {
  const parsed = response.safeParse(value);
  if (!parsed.success) return fail();
  const { team, creation_request: request } = parsed.data.result;
  const s = team.snapshot;
  const counts = new Map<string, number>();
  const expected = input.seats.map(v => {
    const count = (counts.get(v.role) ?? 0) + 1; counts.set(v.role, count);
    return { harness: v.harness, model: v.model, reasoning: v.reasoning, role: v.role,
      name: `${input.team}-${v.role}${count === 1 ? "" : `-${count}`}` };
  });
  if (s.team !== input.team || s.project !== input.project || s.repo !== input.repo ||
      s.mission !== input.mission || s.acceptance !== input.acceptance || s.preset.id !== input.preset_id ||
      (input.preset_id === null ? s.preset.sha256 !== null : s.preset.sha256 === null) ||
      !equal(s.seats, expected) || !equal(s.fallbacks, input.fallbacks.map(v => tuple.parse(v))) ||
      s.lead !== expected[input.lead_index].name ||
      !equal(team.seats.map(v => v.configured), expected) ||
      request.request_id !== s.creation_request_id || request.team !== s.team ||
      request.project !== s.project || request.repo !== s.repo || request.created_at !== s.created_at) return fail();
  return parsed.data;
}
