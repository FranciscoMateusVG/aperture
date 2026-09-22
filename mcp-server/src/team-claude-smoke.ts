import { z } from "zod";

const name = z.string().regex(/^[a-z0-9][a-z0-9_-]*$/).max(31);
export const claudeStartupSmokeSchema = z.object({
  team: name.max(16),
  seat: name,
  expected_generation: z.literal(0),
}).strict();
export type ClaudeStartupSmokeSelectors = z.infer<typeof claudeStartupSmokeSchema>;

// Startup observation and cleanup are NOT mission readiness or tool-call proof.
const diagnostic = z.object({
  action: z.literal("claude_startup_smoke"),
  result: z.object({
    team: name.max(16), seat: name, generation: z.literal(1),
    model: z.literal("claude-sonnet-5"),
    reasoning_observation: z.literal("not_observed"),
    startup: z.literal("verified"),
    cleanup: z.literal("quarantined"),
    mcp_readiness: z.literal("not_run"),
    public_enabled: z.literal(false),
  }).strict(),
}).strict();

export function parseClaudeStartupSmoke(value: unknown, input: ClaudeStartupSmokeSelectors) {
  const result = diagnostic.safeParse(value);
  if (!result.success || result.data.result.team !== input.team || result.data.result.seat !== input.seat) {
    throw new Error("E_CONTROL_UNKNOWN: diagnostic receipt invalid; inspect before any further action");
  }
  return result.data;
}
