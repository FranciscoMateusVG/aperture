# Managed Claude seats — exact model literals

Status: source + tests only (aperture-wzayo, 2026-09-24). Catalog admission is
**not** proof of live harness support: each model still needs its own explicit,
operator-acknowledged smoke on a real seat before anyone claims it "works".

## Authorized literals

| Model | Exact Claude API id | Provenance (2026-09-24) |
|---|---|---|
| Claude Sonnet 5 | `claude-sonnet-5` | unchanged; 4 real receipts in `~/.aperture/run/*.claude-observation.json` show `actual_model == "claude-sonnet-5"` |
| Claude Fable 5.1 | `claude-fable-5-1` | platform.claude.com/docs/en/models/overview ("Claude API ID" and alias rows); Claude Code 2.1.281 bundle model table `{id:"claude-fable-5-1",family:"fable"}` |
| Claude Opus 5 | `claude-opus-5` | platform.claude.com/docs/en/models/opus-5/overview ("Model ID: `claude-opus-5`", status *Active (legacy)*, released 2026-07-24); Claude Code 2.1.281 bundle model table `{id:"claude-opus-5",family:"opus"}` |

Not authorized (deliberately): `claude-opus-5-5` (the current Opus per the docs —
a separate operator decision, never a silent substitution), `claude-fable-5`
(legacy), the CLI aliases `opus` / `sonnet` / `fable`, and any `[1m]` context
suffix (a Claude Code shorthand, not part of the API id). Reasoning is always
`None` for Claude tuples.

Anthropic documents every dateless id from the 4.6 generation on as a pinned
snapshot, so no date suffix is ever appended.

## Where the policy lives

Single source: `src-tauri/src/team_claude_launch.rs`
`CLAUDE_MODELS` + `is_exact_claude_model`. Consumers:

- **Launch** — `exact_tuple` (plan, preflight); `--model` argv and the
  `AGENT_MODEL` env are taken from the validated tuple, never from a constant.
- **Gate** — `validate_record_at` rederives the requested tuple from the
  hash-verified team snapshot (configured seat tuple or a declared Claude
  fallback) via `snapshot_claude_tuple`; the record's own argv cannot choose it.
- **Observation** — `team_claude_observation.rs`: the status-line `model.id`
  must be an allowlisted literal **and** equal `attempt.requested_model`;
  a different allowlisted model is `E_MODEL_UNVERIFIED`, not a fallback.
- **Policy / catalog** — `teams.rs` `managed_execution_enabled` (capabilities
  start/replace, native replacement admission) and `execution_catalog()`.
- **Kickoff** — `team_claude_kickoff.rs` compares the attempt's requested model
  with the owner's requested tuple.
- **Open** — `team_terminal.rs` `claude_owner_valid` and
  `src/services/team-terminal.ts` `canOpenSeat` (`CLAUDE_EXACT_MODELS`).
- **MCP** — `mcp-server/src/team-bootstrap.ts` `CLAUDE_EXACT_MODELS` /
  `supported()`; `team_bootstrap_seat` tool text.

Parity between the Rust array and both TypeScript arrays is enforced by
`exact_model_literals_agree_across_rust_mcp_and_frontend`.

## Deliberately unchanged

The retired startup/inbox diagnostics (`team_claude_smoke`, `team_claude_inbox`,
`smoke_tuple` in native replacement, their MCP schemas and fixtures) stay pinned
to `claude-sonnet-5`. Legacy aliases in old snapshots remain readable and still
never launch. `AGENT_MODEL` is consumed only by `get_identity` (display).
