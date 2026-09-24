# First-bootstrap recovery for managed Claude seats

Status: source + tests (aperture-pm2rv, 2026-09-24). Closes one concrete failure:
the first normal Claude bootstrap of a seat ended **Quarantined g1** with an
incarnation that was never observed (no status-line sample, no rejection),
the gated root process is Gone, the hub token was revoked at floor g1, and the
g0 runtime attempt expired as `unknown`. This is not a generic restart, not a
mission recovery, and never an automatic retry.

## Entry

`team_bootstrap_seat` with `expected_generation: 1` (0 remains the first start).
The selector carries no authority: the native child proves the state itself.
Recovery is **GLaDOS-only** (`authorize_recovery`); the operator-UI arm that
`authorize_bootstrap` accepts for g0 does not apply.

## Native proof (`RecoveryAdmission`, `team_replacement_native.rs`)

Issued under team lock then seat lock, GLaDOS capability revalidated before and
inside the locks, and **re-run twice** in two phases. Nothing in it comes from
the caller.

- **Pre-admission** (at issue and again under team+seat locks right before the
  deadline admission): `runtime-attempts/<seat>/g1` must be **absent** — one
  admission ever.
- **Pre-reserve** (inside `reserve_recovery`, team lock held by `start_native`,
  seat lock taken for the proof): `runtime-attempts/<seat>/g1` must hold exactly
  the **current** attempt — `admitted.json` and `effects.json` bound to its
  attempt id, no `terminal.json`, no prepared/retirement material
  (`recovery_attempt_open_locked`). Another attempt or a finished one refuses.

The seat lock is **released** after the pre-reserve proof: `OwnerStore::reserve_start`
re-acquires it and applies its own guarantees (expected generation 1,
Stale|Quarantined, fresh nonce, incarnation cleared). The reservation is then
bound to the proof by readback — generation 2, `starting`, the reservation's
nonce digest, the selected tuple, no incarnation, no provisional id — or the
call fails with an unknown outcome. There is no continuous seat lock across the
reserve; the team lock is continuous.

| Fact | Where it is read | Requirement |
|---|---|---|
| Seat classification | registry | Active, same team |
| Team snapshot | `.aperture/teams/<team>/team.json` | one seat entry; Claude harness; exact admitted tuple (`managed_execution_enabled`); typed digest kept for later revalidation, raw digest for the attempt |
| Team state | `.aperture/teams/<team>/state.json` | `active`, generation > 0 (bound to the attempt) |
| Owner | `.aperture/run/owner/<seat>.json` | schema 1, generation 1, `quarantined`, `requested` == snapshot tuple, nonce `None`, `provisional_token_id` `None` **or exactly** the incarnation token id (retained provisional is the factual case; nothing is cleaned to fit), incarnation `observed:false`, `thread_id` empty, tuple equal, processes non-empty and containing the root |
| Processes | `team_process::state` per recorded identity | every recorded process, root included, `Gone`; live, recycled or unreadable → `StopUnverified`. The collector is not used: it admits only Starting/Active owners, and `process_count` in summaries is persisted history, not liveness |
| Token | `.aperture/run/hub-tokens/<seat>.token` | absent |
| Revocation floor | `ws_hub::managed_control::verify_floor(home, seat, 1, token_id)` | floor exactly g1 and the same token digest listed |
| g0 attempt | `.aperture/teams/<team>/runtime-attempts/<seat>/g0/` | `UnfinishedBootstrap::read_locked`: `old_generation 0`, effects `effects_may_have_occurred`, terminal `unknown` or absent-expired, no `reconciled.json` |
| g1 attempt | `.aperture/run/<seat>.g1.claude-attempt.json` (`ClaudeAttempt`) | generation 1, **`mode: normal_positional`**, team generation, raw snapshot digest, token id, root pid/birth, requested model, canonical session uuid |
| g1 launch record | `.aperture/run/managed/<seat>/g1/claude-launch.json` (bounded projection) | generation 1, **`mode: normal_positional`**, session == attempt, token id, typed snapshot digest |
| g1 release | `.aperture/run/managed/<seat>/g1/claude-release.json` | `attempt_sha256` == digest of the typed attempt, root pid/birth. Limit: `launch_sha256` (digest of the private `LaunchRecord` serialization) is not recomputed; the launch record is bound separately by session, token and typed snapshot digest |
| Observation | `.aperture/run/<seat>.g1.claude-{observation,rejected}.json` | both absent (unobserved means neither a sample nor a proven rejection) |
| Single admission | `<seat>.g2.claude-attempt.json`, `run/managed/<seat>/g2` | absent; `runtime-attempts/<seat>/g1` is phase-dependent (see above) |

The **category** (normal bootstrap vs. retired diagnostic) comes from the launch
record and the attempt both being `normal_positional`, never from the tuple or
the seat name: a normal Sonnet seat uses the same tuple a diagnostic used.

## What happens after the proof

- `RuntimeAttempt::begin_bootstrap_recovery` admits at generation 1: the attempt
  is written to `runtime-attempts/<seat>/g1/`; `g0/` is never opened for writing.
  An existing `g1/` refuses with `OutcomeUnknown` (no retry, no reprepare).
- `start_native(..., generation = 1, ...)` → the existing `OwnerStore::reserve_start`
  (which already accepts a Quarantined owner at the expected generation) mints
  **g2**; token provisioning, launch, gate, status-line observation, activation
  and cleanup are the unchanged native paths. New artefacts are
  `<seat>.g2.*` and `run/managed/<seat>/g2/`; all g0/g1 facts remain.
- `capabilities.start` and the MCP preflight admit the recoverable owner
  (`quarantined`, generation 1, `actual: null`, `thread_bound: false`, Claude)
  without requiring `process_count == 0`; the receipt must show generation 2.

## Not covered

Diagnostic seats (smoke/inbox), Active/Starting/Stale owners, observed
incarnations, live or recycled processes, a present token or a floor other than
g1, a present nonce, a missing or diagnostic g1 record, a proven model
rejection, and any second call. Nothing here proves live provider readiness.
