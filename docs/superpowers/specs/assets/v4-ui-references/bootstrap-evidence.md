# First-start bootstrap — UI source checkpoint

Parent: `84ca65a1cee313b4c94e4647d321760d93aa43d3`.
Frozen native contract: Peppy BEADS `6i8016`, confirmed by Rex `v8xa1m`.

- First start calls only `team_bootstrap_seat({input:{team,seat,expected_generation:0}})`.
  No actor, tuple selection, fallback, preparation, thread, PID or token is sent.
- CTA and wrapper require active team, explicit start capability, observed stale
  owner generation 0 and requested tuple matching the immutable seat snapshot.
  Team lifecycle generation is not substituted for owner generation.
- BootstrapView is strict: team/seat/generation/phase/owner/blockers only.
  Started requires active owner at a new generation, exact snapshot requested and
  actual tuples, matching returned owner generation, and no blockers.
  Starting/blocked do not assert success. No stop/remote checks are invented.
- Actual click handler locks duplicates while awaiting native evidence.
  Valid responses trigger authoritative team refresh; no local generation/model
  update is performed. Blockers use fixed copy.
- Unknown, timeout, transport or malformed responses disable stale actions and
  require an explicit read refresh. No automatic start retry or rollback claim.
- Generation-0 replacement prepare/start is rejected before invoke, including in
  the existing lifecycle dialog. First start never takes the legacy agent route.
- Archive remains present and capability-gated; its collector is still required
  for final V4 acceptance. This checkpoint does not waive it.

## Focused evidence

Existing Vite SSR / node:test fixture layer, no new harness:

```sh
node --test tests/seat-name-ui.test.mjs tests/team-draft.test.mjs \
  tests/team-contract.test.mjs tests/team-editor.test.mjs tests/teams-area.test.mjs \
  tests/team-runtime.test.mjs tests/team-lifecycle.test.mjs
npm run build
git diff --check
```

**157/157 PASS**, including 29 new bootstrap cases; build and diff check PASS.
Restoring changed source files to parent 84ca65a with the new bootstrap tests
produces **28 FAIL / 1 PASS** (the stale-action negative control still passes).
This pins the new feature's absence as well as the existing g0 prepare rejection;
it does not constitute runtime testing. Restoring new source returns all 157 green.

Native command registration/DTO composition, process launch, model observation,
WKWebView focus/geometry/pixels, installed app and primary live journey remain
**NOT_RUN**. No backend edits, install, restart or worker launch performed.
