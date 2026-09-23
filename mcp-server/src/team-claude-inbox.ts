import { z } from "zod";
export { claudeStartupSmokeSchema as claudeInboxProbeSchema } from "./team-claude-smoke.js";
import type { ClaudeStartupSmokeSelectors } from "./team-claude-smoke.js";
const name = z.string().regex(/^[a-z0-9][a-z0-9_-]*$/).max(31);
const response = z.object({
  action: z.literal("claude_inbox_probe"),
  result: z.object({
    team: name.max(16), seat: name, generation: z.literal(1),
    model: z.literal("claude-sonnet-5"), reasoning_observation: z.literal("not_observed"),
    startup: z.literal("verified"), kickoff: z.literal("sent"),
    owner_state: z.literal("quarantined"), cleanup: z.literal("verified"), mcp_readiness: z.literal("pending_verification"), public_enabled: z.literal(false),
  }).strict(),
}).strict();
export function parseClaudeInboxProbe(value: unknown, input: ClaudeStartupSmokeSelectors) {
  const parsed = response.safeParse(value);
  if (!parsed.success || parsed.data.result.team !== input.team || parsed.data.result.seat !== input.seat) {
    throw new Error("E_CONTROL_UNKNOWN: inbox probe receipt invalid; inspect without retry");
  }
  return parsed.data;
}
