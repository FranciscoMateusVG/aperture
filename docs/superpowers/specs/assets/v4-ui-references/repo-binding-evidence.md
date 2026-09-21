# V4 explicit repository binding — UI source checkpoint

Contract: Rex repository-binding addendum v2, SHA-256
`7b55ab843cd02fc68c9279b761e3ae1244bb600934505068c6df202b366be074`.
Parent: `766a44b8048fd98096e49f8d262c58e2f75fe29b`.
This delta does not change native commands, runtime authorization, capabilities,
execution tuples, model budgets, or repository/worktree resolution.

## Implemented boundary

- Repository options come only from native `TeamCatalog.repositories`. No
  TypeScript project-to-repository authority map. Fixture choices are synthetic.
- One option is visibly preselected and explicitly serialized; multiple options
  start unselected, including after a project change. Empty or unavailable
  catalog entries block create with inline explanation.
- Changing project discards a stale selection. Selecting a repository preserves
  the select DOM node, rather than replacing a focused native control.
- Required `repo` key passes the detached input whitelist. Presets cannot supply
  it, and preset editing/saving remains independent of repository availability.
- Create success requires the submitted key, immutable snapshot key, and durable
  creation-request key to agree. Missing, path-shaped, or conflicting echoes are
  rejected without saved callback or editor dismissal.
- Pending/active team context shows the immutable repository key. The UI makes no
  repository edit-permission or worker-launch claim.
- Three fixed native repository errors retain the draft and expose no raw paths
  or backend messages. Native availability/authorization remains authoritative.

## Evidence

Existing Vite SSR / node:test handler-contract fixtures only:

```sh
node --test tests/seat-name-ui.test.mjs tests/team-draft.test.mjs \
  tests/team-contract.test.mjs tests/team-editor.test.mjs tests/teams-area.test.mjs \
  tests/team-runtime.test.mjs tests/team-lifecycle.test.mjs
npm run build
git diff --check
```

Result: **128/128 PASS**, TypeScript/Vite build PASS, diff check PASS.
Fourteen repository cases add to the prior 114 tests. Bidirectional check:
temporarily restoring only changed source files to parent 766a44b, while keeping
new fixtures/tests, gives **13 FAIL / 1 PASS** for the repository-focused cases.
The preserved preset-independence control is the passing case. Source was restored
and the full 128-test suite/build rerun green.

## Limits

These are source/handler assertions, not native DOM or runtime evidence.
WKWebView selection behavior, focus/tab order, modal geometry/pixels, native
catalog availability, live creation/approval, worktree resolution and installed
app journey are **NOT_RUN**. No install, restart, provider, Git mutation against a
catalog repository, or worker launch was performed. The prior layout references
remain references, not screenshots of this repository-selector delta.
