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
- **Superseded by P4 contract qihtdz/v35l6x:** Archive uses **Check archive readiness**, a read-only checklist. Complete inventory remains pending approval/archival by GLaDOS through separate authenticated control; this UI cannot perform it. No new endpoint, authority fields or cleanup operation.
- Checkpoint now and separate Stop remain unavailable: those auth/runtime seams are not frozen by this contract.

## Evidence

Existing node:test + Vite SSR/FakeElement handler-contract layer, no new dependency/framework/browser harness:

`node --test tests/seat-name-ui.test.mjs tests/team-draft.test.mjs tests/team-contract.test.mjs tests/team-editor.test.mjs tests/teams-area.test.mjs tests/team-runtime.test.mjs tests/team-lifecycle.test.mjs`

→ **114/114 PASS**, including 31 P3 wrapper/render/handler tests. `npm run build` and `git diff --check` PASS. Positive fixtures are explicitly synthetic test-only responses, never fallback production data.

These checks do not prove native dialog focus, geometry, pixel parity, Tauri adapters, durable permits, provider model observation or actual team lifecycle effects. All are **NOT_RUN**. No runtime command was called; no app install/setup/restart occurred. This slice requires independent source review and later exact backend composition before any live capability is enabled.

## Bounded preparation-expiry correction (parent c6eb8e3)

Frozen follow-up: Rex BEADS ecqk3j, root dikw3t. The fixed
`E_PREPARATION_EXPIRED` category means completed Prepare facts still hold:
the previous worker remains stopped/revoked, but its opaque Start permit is
invalid. Start is **blocked**, not an unknown transport outcome.

The UI keeps the completed preparation evidence, discards the expired selector,
and requires explicit refresh/reprepare. It does not mutate the native DTO to
fabricate a blocked response, restore a worker, retry, or reset a generation.
Selection cannot erase this evidence while refresh is required. Transport loss
after Prepare remains unknown and does not inherit the expiry-specific claim.

Existing handler regression: completed Prepare -> Start expiry -> blocked evidence
-> disabled Start/Prepare -> explicit refresh -> fresh Prepare. Separate transport
negative control. Pre-fix expiry regression **RED**, transport control **PASS**;
current full source suite **159/159 PASS**, Vite build and diff-check PASS.
No DTO/layout/backend change. Native expiry/stop/revocation, WKWebView and installed
runtime behavior remain NOT_RUN; this is source evidence only.

## P4 read-only archive copy correction (parent d131e810)

Rex v35l6x / native source 20765139827d9ba4def46e6792acd133757ff755
supersedes the original effectful UI contract. The existing team_archive command
projects a read-only checklist. Its complete inventory returns pending; approval
and actual archive require the separate authenticated GLaDOS control action.

This bounded delta changes only display copy/CTA and focused assertions. It does
not alter DTOs, selectors, wrappers, capabilities, hashes, proofs, authority or
native code. The dialog says **Check archive readiness**; pending explicitly
awaits GLaDOS. Opening/closing/checking never claims to request archival.
Capability false still produces zero invokes. The prior illustrative references
are not installed screenshots and do not override this corrected contract.

Three copy regressions fail on parent d131e810 and pass here: read-only CTA,
complete checklist awaiting GLaDOS, and in-flight checking rather than archiving.
Full existing source suite **161/161 PASS**, build and diff-check PASS.
Native composition, final archive control, browser/WKWebView and installed runtime
remain NOT_RUN; no capability was enabled and no live action was performed.
