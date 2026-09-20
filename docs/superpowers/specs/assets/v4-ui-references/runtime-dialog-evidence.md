# P3 dialogue source slice — capabilities remain false

Task aperture-zfmd5. Parent P1 UI head b85c24a6 (draft PR #73). This work lives on `aperture-zfmd5-runtime-dialogs`, not on the PR #73 branch. Integration owner Rex; native owner Peppy.

## Frozen contract

Peppy `docs/runtime-v4-p3-ui-contract.md` at 16b29cbf3003fe5511cfc9a71a204a6d7b7e9567, SHA-256 609782d0fbcb15ae33c9424766fc4ebf595b1be78bbe2ebb7d07950e70b8ead9. Follow-up cpfcu2 pins: view generation equals included owner generation; started generation must be greater than the old expected generation, without a frontend +1 assumption. Archive generation is canonical readback, not frontend arithmetic.

## Source implemented

- Separate Prepare / stop / verify and Start replacement; opening a dialog is read-only.
- False/missing capability or missing authoritative owner generation means **zero invokes**. Reserved command names are not registered by this slice; no capability is made true anywhere in production code.
- Opaque backend preparation selector never displayed or treated as authorization. Ready requires all native checks verified, no blocker, exact seat/team/current owner generation and an immutable-policy selection. Selection changes invalidate local preparation; start consumes it locally before awaiting to prevent duplicate submissions. Native enforcement remains authority.
- Changed model/harness/reasoning/budget intent requires explicit local confirmation, which never becomes a JSON grant. Only snapshot and exact snapshot fallbacks are offered, not the entire catalog.
- Pending native promise is not fabricated phase progress. Missing data stays unknown; stale/none checkpoints warn rather than automatically blocking backend-approved recovery.
- Private owner fields are rejected at the boundary. Serialization is an exact selector/intent allowlist; no actor, proof, PID, thread, token, timestamp, arbitrary path or authority flags.
- Failed/model-unverified/ambiguous outcome retains unknown/failure and requires an explicit state refresh. No automatic retry or locally incremented generation.
- Closing does not claim cancellation/rollback. A completed native response after close can cause only an authoritative read refresh, never a second lifecycle call.
- Archive has one **Verify and archive** CTA: the reserved command is effectful, not a read-only readiness probe. Returned archived requires verified checks and no blockers; transport/journal ambiguity never implies rollback or completion. No new endpoint or cleanup operation.
- Checkpoint now and separate Stop remain unavailable: those auth/runtime seams are not frozen by this contract.

## Evidence

Existing node:test + Vite SSR/FakeElement handler-contract layer, no new dependency/framework/browser harness:

`node --test tests/seat-name-ui.test.mjs tests/team-draft.test.mjs tests/team-contract.test.mjs tests/team-editor.test.mjs tests/teams-area.test.mjs tests/team-runtime.test.mjs tests/team-lifecycle.test.mjs`

→ **114/114 PASS**, including 31 P3 wrapper/render/handler tests. `npm run build` and `git diff --check` PASS. Positive fixtures are explicitly synthetic test-only responses, never fallback production data.

These checks do not prove native dialog focus, geometry, pixel parity, Tauri adapters, durable permits, provider model observation or actual team lifecycle effects. All are **NOT_RUN**. No runtime command was called; no app install/setup/restart occurred. This slice requires independent source review and later exact backend composition before any live capability is enabled.
