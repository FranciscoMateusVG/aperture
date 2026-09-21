# Conversation-led temporary teams

Operator direction (2026-09-21): coordination is GLaDOS, Wheatley and Peppy;
project workers belong to temporary mission teams. Preset templates remain an
internal implementation detail, not an operator editor.

## Creation boundary

1. GLaDOS reads `team_get_creation_catalog` (the existing native catalog;
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
   against the pending request. Subsequent starts use the existing native gates.

On an unknown create outcome, list pending requests before retrying. A name
collision is not a successful retry. No automatic replay, activation or launch
is introduced.

## Compatibility and limits

GUI, MCP and `aperture-team-control` must be built/published from the integrated
head. An old binary cannot handle the new `catalog` / `create` action. Existing
operator-native creation remains available internally; the preset editor is
removed from the launcher surface. No standing manifest, history or task is
removed or reassigned by the UI change.

Claude managed launches remain disabled/pending their existing implementation
and observed-model gates. A tuple appearing in the catalog is not evidence of
an installed working harness. The current test journey uses Codex only.

Tests share the exact request fixture between Node and Rust serde/native
creation, rather than a frontend-only envelope mock. Source/component evidence
is not installed user-journey evidence.
