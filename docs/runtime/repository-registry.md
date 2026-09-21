# Runtime repository registry

Repository offers are configuration, not a release. After the mechanism is
installed, GLaDOS can add a repository or edit its display name/enabled status
through the existing authenticated control binary, without rebuilding,
restarting the app, or restarting the MCP consumer.

## Conversation workflow

1. The operator confirms the project, repository key and intended change.
2. `team_list_repositories` returns current entries and `sha256`.
3. `team_save_repository` submits `project`, `repo`, `display_name`, `enabled`,
   and that `expected_sha256`. A stale digest is a conflict, not an automatic retry.
4. Read back the returned entry, then use `team_get_creation_catalog` and the
   separate `team_create` → pending → `team_approve_activation` flow.

Keys resolve only as `$HOME/projects/<repo>`; no path, URL, actor, source or
provenance is accepted from the caller. Enabling requires a safe local clone.
An unavailable clone is never silently substituted with another repository.
For example `eunenem` and `eunenem-engine` are distinct bindings. Project labels
remain the existing five-value taxonomy; they do not select a repository.

## Durable configuration

`~/.aperture/repositories.json` stores schema version 1 and at most 128 entries.
The file is outside the scanned teams directory. The first save materializes
all five legacy seed entries before applying the edit. Reads with no file use
those seeds; a present malformed/unsafe file fails closed, without fallback.
The app never deletes records or remaps existing teams. Edits target one exact
(project, repo) key and change only its display name and enabled status.
The native implementation owns private atomic IO, fsync, validation and CAS.
Do not hand-edit this file or loosen its permissions to recover an error.

The CAS hash covers canonical, ordered configuration, not filesystem
availability. Availability is recomputed every call. Writes hold the catalog
lock; create/approve hold team → catalog (then seat locks on approval), through
publication. No catalog writer acquires a team lock. Contention fails explicitly.

## Existing teams

Disabling a record removes it from creation offers and blocks pending approval.
It does not mutate any `team.json`, stop a worker, or hide an active team.
Stored snapshots are previously admitted immutable bindings. Readers, the
loader and runtime validate their project/key syntax independently of the
mutable catalog; runtime still validates the actual repository and registered
worktree. A broken catalog blocks new admission, not already-running bindings.

## Trust and evidence

Both registry control actions use the existing canonical GLaDOS bearer seam.
The write revalidates that capability immediately before publication. It grants
no new same-UID isolation guarantees. JSON input is strict and cannot supply a
path or authority field. The MCP eligibility check is defense in depth; the
native child remains authoritative.

Rust and MCP consume shared request/response fixtures: Rust invokes the real
control handler and compares its serialized output with the response fixture;
Node validates that same response. Hermetic tests do not prove an installed
journey. Publishing this mechanism still requires paired GUI/MCP/control builds
and an explicit installation window; subsequent registry edits do not.
