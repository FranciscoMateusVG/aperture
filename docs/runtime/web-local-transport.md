# Local browser transport — F1 candidate, not a deployment

Implementation bead: aperture-b2ajt; frozen runtime base: abeb9aedadfa10cfc6d2c42bdaeb0d74ee112c24.
The binding security contract is Cipher's PRE-F1 replacement contract on aperture-ed2uf, accepted by root in u07hd3. This document is not security or QA approval.

## Composition and authority

`aperture-server` composes the existing Rust engine/state with a finite Axum router. It never invokes the GUI `run()` or initializes bd/dolt. The rebuilt Tauri fallback and server both acquire `.aperture/run/daemons.lock` before initialization/mutation; the second controller fails closed with recorded pid/birth metadata. That does not make an already-installed older app lock-aware. Running either on real state is outside this delivery.

The shared daemon entry point still has the legacy supervision and shutdown semantics. F1 does **not** claim restart survival, identity-based daemon adoption, immutable release binding or launchd integration; those remain F2. No installer/publication/service changes occur here. `ui/current` is read by the server, not created or deployed by these tests.

There are 20 registered web routes: 19 commands plus permanent bootstrap denial. Shared native wrappers preserve the domain engine, replacement permit store and Active-only terminal binding. Session middleware constructs only `operator_ui`; input never selects an actor. Native errors retain bounded codes but raw native messages/paths are not returned to the browser. Legacy string failures receive a fixed web error. Team archive remains a read-only checklist. Other GLaDOS/seat controls, generic invoke, argv and shell have no routes.

Bootstrap is **new web policy**, not parity with the existing g0 engine: the no-state/no-body handler always returns HTTP 403 `E_WEB_AUTHORITY_DENIED`, with fixed message, before JSON/domain parsing or engine invocation. This does not change native/MCP bootstrap authority.

## HTTP/session boundary

Production binds exactly `127.0.0.1:4519`; only Host `127.0.0.1:4519` is accepted, including for static assets. Rebinding/localhost/duplicate authorities fail. Browser writes require exact Origin and `Sec-Fetch-Site: same-origin`; authenticated reads require same-origin fetch metadata and bearer. There is no CORS, ambient cookie authority, forwarding-header trust or WebSocket/HMR on this listener.

`aperture-server open` reads the boot-scoped private capability into memory, sends it in an authorization header to `/session/mint`, and passes only a 30-second exchange URL to `/usr/bin/open` without a shell. The native mint path rejects Origin and all browser fetch metadata. The capability is never accepted by API routes. Capability publication reuses the native private atomic writer with no-follow/exclusive temporary creation, mode 0600 and validated private directories. It is rotated before listening. Child open stdout/stderr are suppressed to avoid URL echo.

The page removes the fragment synchronously before any await, redeems the exact exchange JSON under a 1 KiB body limit, and stores the bearer in one versioned sessionStorage key. Authentication precedes application initialization/tmux bootstrap/polling. Refresh reuses the session. 401 clears storage and reports an ended session; writes are never automatically retried, including on transport loss. Logout revokes server-side before client deletion. Link issuance is parent-session-bound until redemption; revocation or restart invalidates unredeemed links.

Credentials use 32 CSPRNG bytes/base64url without padding. In-memory records contain digests and use constant-time comparisons. Sessions have 12-hour idle/24-hour absolute limits; exchange redemption is mutex-atomic. Limits: 64 outstanding exchanges, 16 mints/minute per server and 256 sessions. All stores are boot-local and disappear on restart. Tab duplication can copy sessionStorage; this is browsing-context storage, not an uncopyable tab identity.

API JSON is limited to 16 KiB, depth 12, strings 8192 bytes, arrays 128 and object fields 32 before domain validation; native domain limits remain authoritative within those caps. Web DTOs reject unknown fields, including execution tuple extras. Path and body selectors must agree. The web tmux session selector is restricted to the existing `aperture` session and window selector to a bounded native window id. The adapter is an explicit command map, not a generic server invoke endpoint.

Every response receives the Cipher CSP (default none, self-only scripts/styles/connect, no frame ancestors/base/form/objects/workers), nosniff, no-referrer, DENY framing, same-origin COOP/CORP and restrictive Permissions-Policy. Session/API/static responses are no-store. No HSTS or service worker. Fixed palette CSS classes replace inline agent color styles. Static files are bounded, extension-allowlisted and canonicalized inside the resolved UI root.

Residuals remain those accepted at design gate: same-user account access, extensions/OS compromise, same-origin XSS and brief short-lived exchange exposure in open argv/history. None grants GLaDOS authority.

## Isolated evidence and limits

All command output is retained under this task worktree's `tmp/f1/`; build target is `tmp/f0-20260927T1400/cargo-target`, never the runtime checkout. Native Node is `/Users/franciscomateus/.volta/tools/image/node/22.22.3/bin/node`, prefixed only in the process PATH. Root mtbb6l authorized normal Cargo fetch for named transport dependencies; lock comparison shows nine new packages and no removed/upgraded existing package versions. Root o6fr6y previously authorized task-local offline frozen pnpm preparation.

Commands (verification, **not** runtime launch):

```sh
CARGO_NET_OFFLINE=true CARGO_TARGET_DIR="$PWD/tmp/f0-20260927T1400/cargo-target" cargo check --manifest-path src-tauri/Cargo.toml --lib --bin aperture-server
CARGO_NET_OFFLINE=true CARGO_TARGET_DIR="$PWD/tmp/f0-20260927T1400/cargo-target" cargo test --manifest-path src-tauri/Cargo.toml --lib
CARGO_NET_OFFLINE=true CARGO_TARGET_DIR="$PWD/tmp/f0-20260927T1400/cargo-target" cargo test --manifest-path src-tauri/Cargo.toml --lib web_
PATH="/Users/franciscomateus/.volta/tools/image/node/22.22.3/bin:$PATH" pnpm test
PATH="/Users/franciscomateus/.volta/tools/image/node/22.22.3/bin:$PATH" pnpm build
```

The TCP suite uses the production Axum router on an ephemeral loopback port with synthetic state, no daemon. It covers Host/Origin/fetch metadata/preflight, all 20 routes behind auth, constant denial g0/g1/g2 including malformed/oversized bodies with byte/tree-identical native state, nonexistent authority routes, unknown fields, bounded errors, concurrent redemption/replay/expiry, refresh credential reuse, logout/link invalidation and same-home restart/open-capability rotation. Domain lifecycle remains covered by existing tests; these are not live lifecycle evidence. UI component/parser tests run through the injected HTTP adapter as well as the existing factories.

### Browser gate: RUNNER_BLOCKED, not PASS

A separately ignored native Chrome fixture was authorized by root q3hjke. It uses only the existing Chrome 154.0.8037.57, fresh HOME/profile, private ephemeral loopback servers, a copy of packaged UI with only the canonical-origin literal adjusted for its fixture port, and inert backend responses for UI reads/session bootstrap. Production UI bytes and origin remain unchanged. External-script negatives target a second loopback fixture, not an Internet host. Chrome background networking/DNS are disabled; profiles are removed and owned process groups are bounded by a watchdog.

- `browser-csp-01.log`: wall-clock timeout; original pipe collection suspected.
- `browser-csp-02.log`: wall-clock timeout after concurrent pipe draining; backpressure not established as the cause.
- `browser-csp-03.log`: already launched when root STOP rgzcjk arrived; preserved as a third execution, not relabeled as a read. Timeout cleanup killed only the owned Chrome group. Bounded/redacted diagnostics show inline/external CSP violations but do not establish all assertions.
- `browser-csp-04-native-timeout.log`: one correction/run expressly authorized in 6vouvw, replacing virtual time with native Chrome timeout; also timed out. **STOP browser: no automatic further execution.**

The required browser marker/positive-control/frame assertions remain intact. QA f4vm0q identified a weak event oracle; root ud0su1 authorized strengthening it to parse the listener's root `data-csp` blockedURI set (inline, eval, exact external URL), with a synthetic regression that rejects misleading DOM substrings. This stronger oracle has not completed in a browser. Partial console CSP violations are not a full CSP/refresh/UI/framing PASS. Desktop/mobile visual journeys and real managed lifecycle are NOT_RUN.

Before merge: QA and Cipher must review the immutable composed SHA, exact diff and evidence. Browser runner/evidence remains a blocking gate unless the authorized decision owner explicitly records another disposition; no fabricated PASS or self-approval. Runtime publication, migration, launchd, live witness and Tauri removal remain separately gated.
