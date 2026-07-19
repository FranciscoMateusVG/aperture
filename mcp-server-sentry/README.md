# aperture-mcp-server-sentry

Wrap layer that exposes Sentry MCP tools to Aperture agents behind a security
gate stack. Sits between agent MCP clients and the upstream
[`@sentry/mcp-server`](https://www.npmjs.com/package/@sentry/mcp-server).

## Architecture

```
agent ──MCP──> aperture-mcp-server-sentry ──stdio MCP client──> @sentry/mcp-server ──HTTPS──> sentry.io
```

On every tool call the wrap layer applies, in order:

1. **R3** Load allowlist from `~/.config/aperture/sentry-mcp-allowlist.yaml`. Missing/empty → all tools refused.
2. **Agent gate** Caller must be in `agent_default_on` or `agent_opt_in`.
3. **Tool class** Classify as `read` / `mutation` / `attachment`.
4. **R4** Project-allowlist check across 10+ param-name shapes.
5. **R2** Justification check (20–500 chars) for attachment tools.
6. **R1** Operator approval via BEADS message for mutation + attachment tools (10-min timeout).
7. Forward to upstream.
8. **R5** Extract `target_user_id` (cap at 10, mark truncation).
9. Emit audit line `agent.sentry_query` to Loki (params_safe, no PII body).

Every log / audit / error emission passes through **R6** token redaction.

## Installation

```bash
cd mcp-server-sentry
pnpm install
pnpm build
```

The Aperture tmux launcher (via `src-tauri/src/agents.rs`) wires this server
into each agent's per-session MCP config alongside `aperture-bus` and
`mempalace`. Rebuild the Tauri app after the Rust change lands:

```bash
pnpm tauri build
```

## Configuration

| Variable                          | Default                                                              |
|-----------------------------------|----------------------------------------------------------------------|
| `AGENT_NAME` (required)           | injected by agents.rs                                                |
| `SENTRY_ACCESS_TOKEN`             | falls back to `~/.config/aperture/sentry-agent-token` file           |
| `SENTRY_MCP_TOKEN_PATH`           | override path for token file                                         |
| `SENTRY_MCP_ALLOWLIST_PATH`       | override path for allowlist YAML                                     |
| `SENTRY_MCP_UPSTREAM_CMD`         | `npx`                                                                |
| `SENTRY_MCP_UPSTREAM_ARGS`        | `-y @sentry/mcp-server`                                              |
| `LOKI_URL`                        | `http://localhost:3100`                                              |

Reload the allowlist without restarting: `kill -HUP <pid>` on the wrap layer.

## Operational runbook

### R7 — Token rotation cadence

Rotate the Sentry workspace access token at least:

- **Annually** (set a calendar tickler — drift from the boot-time install date)
- **On staff change** — anyone with shell access to xerox who left the team
- **On compromise suspicion** — unusual Sentry audit access patterns, machine compromise, accidental token leak (e.g. token surfaced in a Loki line — see audit-fail alert below)

### R8 — Revocation + re-auth runbook

When you need to revoke + re-issue (≤5 min for experienced operator):

1. **Revoke at Sentry**
   Go to `sentry.io → User Settings → Auth Tokens` → find the workspace token (description `aperture-bus`) → click `Revoke`.

2. **Re-issue**
   Same page → `Create New Token` → name it `aperture-bus-YYYY-MM-DD` → check the same scopes as before (`org:read`, `project:read`, `event:read`, `event:admin` if mutation tools are in use).

3. **Install on xerox**
   ```bash
   ssh xerox 'cat > ~/.config/aperture/sentry-agent-token' <<< 'sntry_NEW_TOKEN_HERE'
   ssh xerox 'chmod 600 ~/.config/aperture/sentry-agent-token'
   ```

4. **Restart aperture-bus + wrap layer**
   Either restart the tmux launcher (full cycle, simplest) or per-agent: in the tmux window, exit and re-launch the agent. The new token is picked up on the next process boot.

5. **Verify**
   From any default-on agent, run `mcp__sentry__search_docs query="dsn"` (no-PII tool) and confirm a typed JSON response. Then check Loki:
   ```bash
   logcli query '{service="aperture-bus-sentry", event="agent.sentry_query"} | json' --since=2m
   ```
   The verification call should land as a single audit line within 2 minutes.

### R9 — Audit-fail alert

A Loki alert rule (see `monitoring/sentry-mcp-audit-fail.alert.yml`) watches for
`audit emission failed` stderr lines from `aperture-bus-sentry`. If any such
line appears, the operator is paged via the standard notification channel —
audit blackouts MUST surface within 5 minutes, not "eventually".

Manual probe:

```bash
logcli query '{service="aperture-bus-sentry"} |~ "(?i)audit.*fail"' --since=5m
```

If hits appear, investigate immediately: Loki ingester down, network split,
or wrap layer crash loop. Recovery is usually `systemctl restart loki` on
xerox (with operator approval).

## Maintenance — keeping R4 honest

The project-param coverage list in `src/gates.ts` (`PROJECT_PARAM_NAMES`)
MUST be reviewed against the upstream `@sentry/mcp-server` tool surface
whenever the upstream package is bumped:

```bash
pnpm view @sentry/mcp-server@latest version
```

If a new tool exposes a project-identifying param shape not in the list,
the defensive regex catches it and DENIES — but better to enumerate. Open
a follow-up bead in `project:aperture` whenever you bump the upstream
version.

## Tests

```bash
pnpm test
```

Five spec files map 1:1 to Cipher's wiring contract:

| File                          | Constraint(s) |
|-------------------------------|---------------|
| `tests/redact.test.ts`        | R6            |
| `tests/allowlist.test.ts`     | R3, R4        |
| `tests/audit.test.ts`         | R5, R2 (no-log) |
| `tests/justification.test.ts` | R2            |
| `tests/approval.test.ts`      | R1 (unit slice)|

End-to-end integration of R1 (BEADS round-trip with a live operator) is
covered by Izzy's smoke-test task `aperture-echr`, which runs against the
real xerox-to-xerox Sentry org after this PR merges.
