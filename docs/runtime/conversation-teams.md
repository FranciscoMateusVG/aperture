# Conversation-led temporary teams

Operator direction (2026-09-21): coordination is GLaDOS, Wheatley and Peppy;
project workers belong to temporary mission teams. Preset templates remain an
internal implementation detail, not an operator editor.

## Creation boundary

1. GLaDOS reads `team_get_creation_catalog` (the existing native catalog;
   repository offers come from the runtime registry, see
   [`repository-registry.md`](./repository-registry.md);
   repositories, roles and execution tuples are not inferred from labels).
2. The operator confirms the mission, acceptance, repository, seats, lead and
   exact execution tuples. This is not permission to file new BEADS tasks
   without the existing acknowledgement gate.
3. `team_create({input: ...})` calls the **same** authenticated control binary.
   The child authenticates the canonical GLaDOS capability; caller actor, grants
   and generated provenance fields are forbidden. Capability identity is
   revalidated before staging and publication.
4. The result is **Pending, generation 0**, with no worker owner/token/process.
   MCP verifies the pending state and exact immutable echoed input. Creation
   neither activates the team nor starts a harness.
5. `team_approve_activation` remains separate and checks the active epic/project
   against the pending request. The operator approves the composition once;
   GLaDOS handles the subsequent starts, not individual launcher clicks.
6. GLaDOS reads `team_list`, then calls `team_bootstrap_seat` for each eligible
   Codex seat, sequentially, with its exact team/seat and owner generation 0.
   Each child retains the native 170s budget / 180s parent watchdog, capability
   revalidation, reservation, fresh session, exact observation and D1 cleanup.
   Success requires an active observed owner and bound process/thread. On any
   blocker or unknown outcome stop the sequence and inspect; never auto-retry.
   Already-running seats are not bootstrapped again. Claude stays blocked.
7. Only after the required seats are active/observed does GLaDOS dispatch the
   approved mission tasks. Teams UI is for inspection: compact seat rows and
   collapsed mission/diagnostics, not a manual per-worker startup checklist.

On an unknown create outcome, list pending requests before retrying. A name
collision is not a successful retry. No automatic replay, activation or launch
is introduced.

## Mission completion and publication

Started is not ready, and PR-open ends nothing. A team mission is concluded by
GLaDOS only on agreed evidence; the lead template (`roles/*/prompt.md.tmpl`)
binds every seat to the same contract:

1. **Handoff wakes QA.** A reviewable state is handed to the reviewer seat by
   BEADS message with the immutable head SHA, files, PR URL/base and exact
   commands. A note on a bead is evidence, not a handoff. The handoff stays open
   until the reviewer's receipt message names that SHA.
2. **Verdict wakes lead and root.** The reviewer sends PASS or HOLD (SHA and
   repro) by message to the lead and to GLaDOS; a verdict only in notes is not
   a verdict.
3. **PR-open ends nothing.** An implementation bead subject to review closes only
   after handoff receipt and verdict are recorded; a review bead closes on its
   own verdict and evidence. Lead and epic responsibility continue: the epic
   closes only on verdict plus publication readback under the policy frozen at
   creation.
4. **Publication is explicit.** Release target, actor and authority are frozen
   in the mission acceptance at `team_create`. Nobody infers permission to merge
   or promote to main/prod; an unnamed step is a blocker for GLaDOS, not a default.
5. **No tools is a blocker.** If aperture-bus messaging tools are not callable in
   a seat, that seat records `BLOCKER: messaging tools unavailable` when bead
   tools exist and reports only through an authorized BEADS surface; it never
   invents a channel or claims a report was sent without one. A notes-only
   handoff is a capability gap, never a completed handoff. Functional readiness
   before business dispatch collects that error (aperture-g4nyg).

## Stop a seat without replacement

GLaDOS uses `team_stop_seat({input:{team,seat,expected_generation}})` to stop and
revoke one exact current seat only when the native collector proves a current,
validated checkpoint. The input contains selectors only: no caller-supplied
checkpoint proof, discard/force option, actor or timeout. The native child
revalidates identity, process ownership and checkpoint evidence before signals;
MCP authorization alone is not that proof.

A verified receipt echoes the exact team, seat and unchanged owner generation,
with `phase: "ready"`, `checkpoint_recovery: "valid"`, `owner_state: "active"`
and no blockers. The persisted owner stays Active until archive; its stored
process count is not rewritten to zero and is not a live-process claim. No
replacement, new generation, bootstrap or archive is implied. `team_archive`
remains a separate explicit action with its reconciliation gates.

Stop uses the existing isolated control process group and fixed 180-second
parent watchdog. Timeout or an invalid/mismatched receipt means UNKNOWN, not
success, rollback or permission to retry. Preserve native attempts/evidence,
inspect and reconcile explicitly; do not replay the stop automatically. Missing
or stale checkpoint evidence blocks the native action rather than discarding
context. Source tests do not establish a live stop/archive journey.

## Compatibility and limits

GUI, MCP and `aperture-team-control` must be built/published from the integrated
head. An old binary cannot handle the new `catalog` / `create` /
`list_repositories` / `save_repository` / `list_teams` / `bootstrap_seat` /
`stop_seat` actions. Existing
operator-native creation remains available internally; the preset editor is
removed from the launcher surface. No standing manifest, history or task is
removed or reassigned by the UI change.

Claude managed launches remain disabled/pending their existing implementation
and observed-model gates. A tuple appearing in the catalog is not evidence of
an installed working harness. The current test journey uses Codex only.

Tests share the exact request fixture between Node and Rust serde/native
creation, rather than a frontend-only envelope mock. Source/component evidence
is not installed user-journey evidence.
