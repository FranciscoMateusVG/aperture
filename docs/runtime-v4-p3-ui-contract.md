# K310B P3 UI contract v1 — source-only freeze

Frozen against runtime core `52d27e3` / tree `96787ed2` and shared P1 contract
v3 `0abf1b00`. Scope: Vance's independent dialogs, typed wrappers and negative
UI fixtures. **These command names are reserved, not registered or usable yet.**
Rex remains the sole integration owner; Peppy owns their native implementation.
All existing `TeamCapabilities.start/checkpoint/replace/archive` remain false.
A successful fixture is not backend evidence and must never enable an action.

## Shared types and data boundary

Reuse P1 `ExecutionTuple`, `OwnerSummary`, `TeamView`, error envelope
`{code: string, message: string}` and exact immutable snapshot/fallback tuples.
No new model aliases or mutable preset authority. Inputs use snake_case and
reject unknown fields. They contain selectors and intent only, never actor,
writer, timestamp, validation, process identity, token, proof or authorization
booleans. Native code supplies time, principal, facts and generated IDs.

```ts
type RuntimeCheckState = "pending" | "verified" | "blocked" | "unknown";
type CheckpointRecovery = "valid" | "stale" | "none";
type ReplacementPhase =
  | "snapshot" | "checkpoint_pending" | "stopping" | "revoking"
  | "reconciling" | "ready" | "starting" | "started"
  | "model_unverified" | "blocked";
interface RuntimeBlocker {
  code: string;          // fixed native category, never raw stderr/provider body
  reference: string;     // validated seat/team/bead/review ID, never caller path
}
interface ReplacementChecks {
  process_stop: RuntimeCheckState;
  revocation: RuntimeCheckState;
  remote_effects: RuntimeCheckState;
}
interface ReplacementView {
  team: string;
  seat: string;
  generation: number;    // owner/incarnation generation, not team generation
  phase: ReplacementPhase;
  checkpoint_recovery: CheckpointRecovery;
  checks: ReplacementChecks;
  owner: OwnerSummary | null;
  blockers: RuntimeBlocker[];
}
interface PreparedReplacementView extends ReplacementView {
  preparation_id: string | null;
}
```

`owner` is ONLY the P1 safe summary: generation/state/since/configured/actual/
process_count/thread_bound. No raw PID, start time, command/cwd, thread ID, token
ID, nonce, bearer, transcript or checkpoint text enters this view. The internal
`StartedReplacement.thread_id` must NOT be serialized directly to the UI.

The native backend can report `verified` only from its completed evidence gate.
No observation means `unknown` (or `pending` for an action actually in progress),
not a zero count or a green checkbox. Empty lists/transcripts cannot verify
remote effects. No new remote-resolution command is defined by this UI freeze.

## Reserved command inputs and outputs

Every invoke uses the P1 envelope `{input: ...}`. No request field is authority.

| Command | Input | Output |
|---|---|---|
| `team_prepare_replacement` | `{team, seat, expected_generation}` | `PreparedReplacementView` |
| `team_start_replacement` | `{team, seat, expected_generation, preparation_id, selection: ExecutionTuple}` | `ReplacementView` |
| `team_archive` | `{team, expected_generation}` | `ArchiveView` below |

For the first two commands, expected_generation is the old **owner** generation.
For archive it is the **team state** generation, as in P1 TeamView. Read the right
field; do not derive one from the other. All names/IDs are exact selectors under
native validation. There is no command accepting a process list or ready proof.

`preparation_id` is a backend-generated opaque selector for the private,
non-cloneable PreparedReplacement, NOT a bearer or an authorization grant. It
must bind the exact team/seat/generation and current authorized principal.
Only a successful native prepare returns it; all blocked outcomes return null.
The native start must consume it once, recheck every gate and current policy,
and refuse stale/missing/consumed or mismatched selectors. Lost backend state
requires fresh preparation, never reconstructing a permit from UI checkboxes.
The existing common control seam carries this operation; no new broker/service.

No checkpoint command is frozen here: the worker tool/Claude hook writer and
lead-validator authentication seam is still being composed with Rex. The UI
must keep Checkpoint now unavailable, not submit a fabricated worker payload.
No new stop-only shortcut or managed watchdog policy is introduced here.

## Replacement dialog and state handling

1. Open is local/read-only. Initially all checks are unknown; show current
   OwnerSummary exactly, or unavailable if absent. Do not infer a stopped worker
   from missing observation. Selection comes from immutable approved tuples.
2. **Prepare / stop / verify** and **Start replacement** are separate actions.
   Capability false or absent disables invocation, with explicit unavailable
   copy. No native command call is made just to discover that it is unwired.
3. During a real in-flight prepare promise, show pending, not verified. Render
   phase changes only if actually received from the native integration; this
   freeze does not invent a polling/event endpoint. Without progress evidence,
   retain pending until the returned result or error.
4. Start is enabled only when capability.replace is true, returned phase=ready,
   all three checks=verified, no blockers, non-null preparation_id, and the
   displayed team/seat/owner generation still match the prepared result.
   Checkpoint none/stale is an explicit warning, not automatically a blocker:
   native inventory/reconciliation decides safe recovery.
5. Start sends only the approved selection plus bound selectors. An explicit
   operator confirmation is required for changes to harness/reasoning/budget;
   client confirmation is intent, not backend authority or a new grant.
6. On D1 model_unverified/blocked, display failure and actual-model-unavailable
   honestly; no success toast, task dispatch, second start or g+2 auto-retry.
   A fresh returned OwnerSummary is authoritative, never increment generation
   locally. A failed start may consume a generation; do not assume rollback.
7. Closing the dialog does not undo an already initiated prepare/stop. Before
   execution, Cancel only closes locally. During execution, no cancellation RPC
   exists in this freeze; do not claim that closing rolls back native effects.
8. On invoke/transport error, retain unknown outcome rather than claiming the
   process is stopped or the operation had no effect. No automatic retry.

Existing core errors: `E_GENERATION_MISMATCH`, `E_STOP_UNVERIFIED`,
`E_UNOWNED_PROCESS`, `E_REVOCATION_UNVERIFIED`, `E_REMOTE_UNCERTAIN`,
`E_REPLACEMENT_AUTHORIZATION`, `E_FRESH_THREAD_UNVERIFIED`,
`E_MODEL_UNVERIFIED`, `E_START_CLEANUP_UNVERIFIED`, `E_RUNTIME_IO`.
Unknown future error categories render a generic failure, never success.

## Archive dialog

```ts
interface ArchiveChecks {
  reconciliation: RuntimeCheckState;
  reviews: RuntimeCheckState;
  metrics: RuntimeCheckState;
  process_stop: RuntimeCheckState;
  revocation: RuntimeCheckState;
  remote_effects: RuntimeCheckState;
  worktrees: RuntimeCheckState;
}
interface ArchiveView {
  team: string;
  generation: number;  // team state generation
  state: "pending" | "blocked" | "archived" | "unknown";
  checks: ArchiveChecks;
  blockers: RuntimeBlocker[];
}
```

Opening the dialog supplies no authoritative checklist; start unknown. Archive
capability false keeps it unavailable. A native command always collects fresh
reconciliation itself under team/seat locks; the UI cannot submit checks,
dispositions, audits, task lists or booleans as accepted evidence. `archived`
requires completed shared journal plus canonical archive verification; a write
attempt or an empty blocker list is insufficient. Failed/ambiguous transport or
journal state is unknown, never rolled-back/archived by inference. No auto retry,
evidence deletion, identity reuse or client-side rollback action.

Existing blocker codes include `E_RECONCILIATION_INCOMPLETE`,
`E_RECONCILIATION_COVERAGE`, `E_CREATION_GATE_VIOLATION`,
`E_UNFINISHED_SEAT_WORK`, `E_DISPOSITION_MISSING`,
`E_COMPLETED_WITHOUT_EVIDENCE`, `E_CANCEL_UNAPPROVED`,
`E_TRANSFER_UNACCEPTED`, `E_OPEN_CHILDREN`, `E_REVIEW_MISSING`,
`E_METRIC_UNMET`, `E_SEAT_COVERAGE`, `E_STOP_UNVERIFIED`,
`E_REVOCATION_UNVERIFIED`, `E_REMOTE_UNCERTAIN`,
`E_WORKTREE_UNPROTECTED`, `E_GENERATION_MISMATCH`; shared journal/IO failures
retain their fixed categories. References must be safe IDs, never raw evidence.

## Minimum independent UI fixtures (no native effects)

- Every capability false/missing: no invoke and no success state.
- Prepare and start are separate; ready without permit, any unknown/blocked
  check, mismatched seat/generation, or changed selection cannot auto-start.
- None/stale checkpoint remains a visible warning; not misrepresented as valid.
- Unknown remote work never displays 0-in-flight or green by empty array.
- Model failure/cleanup failure/transport loss never retries or increments g.
- Archive pending/blocked/unknown, missing reviews/worktree proof and journal
  failure cannot display archived; dialog close does not assert rollback.
- Neither rendering nor command serialization exposes forbidden owner fields
  or accepts forged actor/proof/validation/timestamp fields.

Positive returned-result rendering may use explicitly labelled fixture data,
not a production fallback or a way to enable unwired capabilities. Backend
adapters, durable permit binding, authority, collectors and lifecycle effects
still require implementation and exact-head review before registration.
