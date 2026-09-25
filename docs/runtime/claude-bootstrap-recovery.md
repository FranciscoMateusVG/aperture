# Bootstrap recovery for managed Claude seats

Status: source + tests (aperture-pm2rv, 2026-09-24; widened to g2→g3 on
2026-09-25 under one explicit operator authorization). Closes one concrete
failure shape at exactly two generations: a normal Claude bootstrap of a seat
ended **Quarantined** with an incarnation that was never observed (no
status-line sample, no rejection), the gated root process is Gone, the hub
token was revoked at a floor equal to that generation, and the bootstrap
admission that led there expired as `unknown`. Generation 1 is the first
bootstrap; generation 2 is that first bootstrap's one explicit recovery that
failed the same way. Nothing later is recoverable. This is not a generic
restart, not a mission recovery, and never an automatic retry.

## Entry

`team_bootstrap_seat` with `expected_generation: 1` or `2` (0 remains the first
start; 3 and above are refused before any lock or read). The selector carries
no authority: the native child proves the state itself, for exactly the
selected generation — a quarantined g2 owner never proves under selector 1 and
vice versa. Recovery is **GLaDOS-only** (`authorize_recovery`); the operator-UI
arm that `authorize_bootstrap` accepts for g0 does not apply.

## Native proof (`RecoveryAdmission`, `team_replacement_native.rs`)

Issued under team lock then seat lock, GLaDOS capability revalidated before and
inside the locks, and **re-run twice** in two phases. Nothing in it comes from
the caller. Below, `g` is the selected generation (1 or 2) and `g+1` the
generation the reserve mints (2 or 3).

- **Pre-admission** (at issue and again under team+seat locks right before the
  deadline admission): `runtime-attempts/<seat>/g<g>` must be **absent** — one
  admission ever per generation.
- **Pre-reserve** (inside `reserve_recovery`, team lock held by `start_native`,
  seat lock taken for the proof): `runtime-attempts/<seat>/g<g>` must hold
  exactly the **current** attempt — `admitted.json` and `effects.json` bound to
  its attempt id, no `terminal.json`, no prepared/retirement material
  (`recovery_attempt_open_locked`). Another attempt or a finished one refuses.

The seat lock is **released** after the pre-reserve proof: `OwnerStore::reserve_start`
re-acquires it and applies its own guarantees (expected generation `g`,
Stale|Quarantined, fresh nonce, incarnation cleared). The reservation is then
bound to the proof by readback — generation `g+1`, `starting`, the reservation's
nonce digest, the selected tuple, no incarnation, no provisional id — or the
call fails with an unknown outcome. There is no continuous seat lock across the
reserve; the team lock is continuous.

| Fact | Where it is read | Requirement |
|---|---|---|
| Seat classification | registry | Active, same team |
| Team snapshot | `.aperture/teams/<team>/team.json` | one seat entry; Claude harness; exact admitted tuple (`managed_execution_enabled`); typed digest kept for later revalidation, raw digest for the attempt |
| Team state | `.aperture/teams/<team>/state.json` | `active`, generation > 0 (bound to the attempt) |
| Owner | `.aperture/run/owner/<seat>.json` | schema 1, generation `g`, `quarantined`, `requested` == snapshot tuple, nonce `None`, `provisional_token_id` `None` **or exactly** the incarnation token id (retained provisional is the factual case at both g1 and g2; nothing is cleaned to fit), incarnation `observed:false`, `thread_id` empty, tuple equal, processes non-empty and containing the root |
| Processes | `team_process::state` per recorded identity | every recorded process, root included, `Gone`; live, recycled or unreadable → `StopUnverified`. The collector is not used: it admits only Starting/Active owners, and `process_count` in summaries is persisted history, not liveness |
| Token | `.aperture/run/hub-tokens/<seat>.token` | absent |
| Revocation floor | `ws_hub::managed_control::verify_floor(home, seat, g, token_id)` | floor exactly `g` and this incarnation's token digest listed (a floor still at `g-1`, or a list holding only earlier tokens, proves nothing) |
| Previous admission | `.aperture/teams/<team>/runtime-attempts/<seat>/g<g-1>/` | `expired_bootstrap_locked`: `old_generation g-1`, effects `effects_may_have_occurred`, terminal `unknown` or absent-expired, no `reconciled.json`. For g1 this is the g0 first start; for g2 it is the g1 recovery admission. A g2 proof judges g1 only — g0 is history the g1 recovery already judged |
| g attempt | `.aperture/run/<seat>.g<g>.claude-attempt.json` (`ClaudeAttempt`) | generation `g`, **`mode: normal_positional`**, team generation, raw snapshot digest, token id, root pid/birth, requested model, canonical session uuid |
| g launch record | `.aperture/run/managed/<seat>/g<g>/claude-launch.json` (bounded projection) | generation `g`, **`mode: normal_positional`**, session == attempt, token id, typed snapshot digest |
| g release | `.aperture/run/managed/<seat>/g<g>/claude-release.json` | `attempt_sha256` == digest of the typed attempt, root pid/birth. Limit: `launch_sha256` (digest of the private `LaunchRecord` serialization) is not recomputed; the launch record is bound separately by session, token and typed snapshot digest |
| Observation | `.aperture/run/<seat>.g<g>.claude-{observation,rejected}.json` | both absent (unobserved means neither a sample nor a proven rejection) |
| Single admission | `<seat>.g<g+1>.claude-attempt.json`, `run/managed/<seat>/g<g+1>` | absent; `runtime-attempts/<seat>/g<g>` is phase-dependent (see above) |

The **category** (normal bootstrap vs. retired diagnostic) comes from the launch
record and the attempt both being `normal_positional`, never from the tuple or
the seat name: a normal Sonnet seat uses the same tuple a diagnostic used.

## What happens after the proof

- `RuntimeAttempt::begin_bootstrap_recovery` admits at generation `g`: the
  attempt is written to `runtime-attempts/<seat>/g<g>/`; earlier directories are
  never opened for writing. An existing `g<g>/` refuses with `OutcomeUnknown`
  (no retry, no reprepare). Only 1 and 2 are accepted.
- `start_native(..., generation = g, ...)` → the existing `OwnerStore::reserve_start`
  (which already accepts a Quarantined owner at the expected generation) mints
  **`g+1`**; token provisioning, launch, gate, status-line observation,
  activation and cleanup are the unchanged native paths. New artefacts are
  `<seat>.g<g+1>.*` and `run/managed/<seat>/g<g+1>/`; every earlier fact
  (g0/g1 for a g1 recovery; g0/g1/g2 for a g2 recovery) remains byte-identical.
- `capabilities.start` and the MCP preflight admit the recoverable owner
  (`quarantined`, generation 1 or 2, `actual: null`, `thread_bound: false`,
  Claude) without requiring `process_count == 0`; the receipt must show
  generation `g+1` for the selector that was sent.

## What stays g0-only

The historical smoke/reconcile handle (`UnfinishedBootstrap::read_locked`,
used by `reconcile_stopped_claude_smoke`) reads `runtime-attempts/<seat>/g0`
only. The recovery's previous-admission proof uses a private parameterized
reader (`read_locked_at`) through `expired_bootstrap_locked`, which returns no
handle — nothing can be recorded or reconciled through it — and accepts 0 or 1
only. The old reconciler is not liberalized.

## Not covered

Diagnostic seats (smoke/inbox), Active/Starting/Stale owners, observed
incarnations, live or recycled processes, a present token or a floor other than
`g`, a present nonce, a missing or diagnostic `g` record, a proven model
rejection, a quarantined g3 or later, and any second call at the same
generation. Nothing here proves live provider readiness.
