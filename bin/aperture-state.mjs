#!/usr/bin/env node
// aperture-state — report this agent as busy or idle to the hub.
// Called by Claude Code hooks (UserPromptSubmit/PreToolUse -> busy, Stop -> idle).
// Never fails the hook: any error is swallowed so Claude keeps working.
//
// Usage: aperture-state busy|idle   (agent from APERTURE_AGENT)
// Env:   APERTURE_AGENT, APERTURE_HUB_HTTP (default http://127.0.0.1:4518)

const state = process.argv[2];
const agent = process.env.APERTURE_AGENT;
const hub = (process.env.APERTURE_HUB_HTTP ?? "http://127.0.0.1:4518").replace(/\/$/, "");

if (!agent || !["busy", "idle"].includes(state)) process.exit(0);

try {
  await fetch(`${hub}/presence/${encodeURIComponent(agent)}`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ state }),
    signal: AbortSignal.timeout(2000),
  });
} catch {
  // hub down: nothing to report to
}
process.exit(0);
