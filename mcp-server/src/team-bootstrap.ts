import { z } from "zod";

const name = z.string().regex(/^[a-z0-9][a-z0-9_-]*$/).max(31);
const tuple = z.object({ harness: z.enum(["codex", "claude"]), model: z.string().min(1).max(128),
  reasoning: z.enum(["low", "medium", "high", "xhigh", "max", "ultra"]).nullable() });
const owner = z.object({ generation: z.number().int().nonnegative(), state: z.enum(["stale", "starting", "active", "quarantined"]),
  configured: tuple, actual: tuple.nullable(), process_count: z.number().int().nonnegative(), thread_bound: z.boolean() });
const seat = tuple.extend({ name, role: z.string().min(1) });
const view = z.object({ snapshot: z.object({ team: name, project: z.string(), repo: z.string(), lead: name, seats: z.array(seat).min(1) }),
  state: z.object({ state: z.enum(["pending", "active", "failed", "archived"]), generation: z.number().int().nonnegative() }),
  seats: z.array(z.object({ configured: seat, observed_owner: owner.nullable() })),
  capabilities: z.object({ start: z.boolean() }) });
export const bootstrapSeatSchema = z.object({ team: name.max(16), seat: name, expected_generation: z.literal(0) }).strict();
export type BootstrapSeatSelectors = z.infer<typeof bootstrapSeatSchema>;
const same = (a: z.infer<typeof tuple>, b: z.infer<typeof tuple>) => a.harness === b.harness && a.model === b.model && a.reasoning === b.reasoning;
export function parseTeamList(value: unknown) {
  return z.object({ action: z.literal("list_teams"), result: z.array(view) }).parse(value);
}
/** Preflight is informational; native authority/locks are still the admission gate. */
export function bootstrapSelection(value: unknown, input: BootstrapSeatSelectors) {
  const teams = parseTeamList(value).result.filter(v => v.snapshot.team === input.team);
  if (teams.length !== 1) throw new Error("E_CONTROL_STALE: team unavailable; inspect before starting");
  const team = teams[0];
  const seats = team.seats.filter(s => s.configured.name === input.seat);
  const snapshots = team.snapshot.seats.filter(s => s.name === input.seat);
  if (seats.length !== 1 || snapshots.length !== 1) throw new Error("E_CONTROL_STALE: seat unavailable");
  const s = seats[0], o = s.observed_owner;
  if (team.state.state !== "active" || !team.capabilities.start || snapshots[0].harness !== "codex" ||
    !o || o.generation !== 0 || o.state !== "stale" || o.actual !== null || o.process_count !== 0 || o.thread_bound ||
    !same(s.configured, snapshots[0]) || !same(o.configured, snapshots[0])) {
    throw new Error("E_CONTROL_STALE: seat is not eligible for first start; no automatic retry");
  }
  return tuple.parse(snapshots[0]);
}
export function parseBootstrapStarted(value: unknown, input: BootstrapSeatSelectors, expected: z.infer<typeof tuple>) {
  const parsed = z.object({ action: z.literal("bootstrap_seat"), result: z.object({ team: name, seat: name,
    generation: z.literal(1), phase: z.literal("started"), owner, blockers: z.array(z.unknown()).length(0) }) }).safeParse(value);
  if (!parsed.success) throw new Error("E_CONTROL_UNKNOWN: bootstrap receipt invalid; inspect before any retry");
  const r = parsed.data.result, o = r.owner;
  if (r.team !== input.team || r.seat !== input.seat || o.generation !== r.generation || o.state !== "active" ||
    !o.actual || !same(o.configured, expected) || !same(o.actual, expected) || o.process_count < 1 || !o.thread_bound) {
    throw new Error("E_CONTROL_UNKNOWN: bootstrap receipt does not prove the requested active owner; no automatic retry");
  }
  return parsed.data;
}
