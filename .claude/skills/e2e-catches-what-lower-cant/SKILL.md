---
name: e2e-catches-what-lower-cant
description: Choose a faithful test boundary when mocks or lossy fixtures hide a consequential failure. Use for coverage design or tests-green/product-broken triage; not a blanket requirement for browser E2E on ordinary UI changes.
---

# E2E Catches What Lower Tests Can't

Mocks, lossy fixtures and alternate composition roots can bypass the very code that failed. More assertions against that substitute do not add evidence about the real boundary.

## The decision rule

Name the failure and the surface your current test substitutes. Choose the smallest faithful regression: a byte-level unit test, actual composition-root/adapter integration test, or a scoped E2E if lower layers cannot represent the failure. Crossing an adapter boundary alone does **not** require a browser E2E. Ordinary UI/input logic stays unit/component-level.

For an approved primary user journey or consequential reproduced browser-only failure, exercise the actual build/composition and assert the outcome. Byte behavior needs byte assertions; this applies at every layer. No new browser harness, live provider call or production write is authorized by this skill. See `verify-user-path` for the scoped journey/stop contract and `wire-the-adapter` for build-side composition discipline.

The examples below explain why the tests used at the time missed a bug; they are not proof that all possible unit/integration tests are incapable of detecting it.

---

## Two banked modes (3 worked examples)

### Mode 1 — Composition-root gap

**Shape:** the test app and the prod app have *different wiring*. Unit + integration tests construct a test app that injects fake adapters (`InMemoryBlobStorage`, `surveyRepository`, etc.) directly into the route handlers. The prod composition root (`server.ts`, `index.ts`, the entry point) is supposed to wire the *real* adapters into the *real* route mount. If the entry point forgets to do that, the routes ship but the adapter doesn't — every request hits a catch-all 404 (or a no-op default), and no test below E2E ever exercises the prod composition path.

**Worked example A — `aperture-y57q` (PR #132, Vance + Rex)**

Blob-storage adapter never wired in `server.ts`. Originally shipped under `aperture-47hg` (PRs #128/#129 — backend + frontend halves) with the prod composition root missing the adapter wire-up; the fix re-shipping with prod wire-up + a composition smoke test landed as `aperture-y57q` / PR #132 (*"fix(blob-storage): re-ship MinIO migration with prod wire-up + composition smoke"*). Unit + integration tests on the original PRs passed because they injected `InMemoryBlobStorage` directly into the test app. The real Postgres composition test surfaced the gap missed by those injected tests.

**Worked example B — `aperture-3ghh` (PR #303, Rex)**

Survey adapters (`surveyRepository`, `surveyResponseRepository`) never wired in `server.ts`. Same composition-root gap shape. From the PR body:

> Backend unit + integration tests inject the repos directly into the test app, bypassing `server.ts` entirely — only a full prod build (E2E via `next build`) wires through `server.ts` and surfaces the gap.

The two examples are the same shape, surfacing in two different domains within ~48h. That's what triggered the promotion to a banked mode.

**Fix shape:** wire the adapter in the composition root, AND add a fail-fast startup guard so the next miss fails loudly at boot (see `wire-the-adapter` for the full build-side discipline).

### Mode 2 — String-string roundtrip trap

**Shape:** the failure mode is byte-level (BOM, encoding, multi-byte char boundary, …) but the test is string-level. The test apparatus on both sides of the pipe converts bytes → string before assertion, and the conversion is *the same on both sides*. The conversion may itself be lossy — but because the loss happens identically on both sides, the equality check still passes.

**Worked example — `aperture-tx2k` (PR #306, Vance)**

Next.js admin proxy was reading the upstream Hono response via `await upstream.text()`. The hono backend emits a leading UTF-8 BOM (`0xEF 0xBB 0xBF`) on CSV exports specifically so Excel auto-detects UTF-8 on Brazilian users' machines. WHATWG `TextDecoder` (which `.text()` uses) **strips a leading BOM by default**, and `.text()` doesn't expose the `{ ignoreBOM: true }` option.

```diff
- const responseBody = await upstream.text();
+ const responseBody = await upstream.arrayBuffer();
```

Why every test below E2E missed it:

- **Unit tests on the proxy** — passed because the test fixture's "upstream response" was a string already; `TextDecoder` had no BOM to strip from a string.
- **Frontend integration tests** — passed because they assert `response.text() === expected`. Both `expected` and `response.text()` had been through `TextDecoder`; both had the BOM stripped; the equality held with the bug intact.
- **Only Izzy's Q1 E2E** asserted at the byte level against hono directly + against the proxied response, and saw the BOM present in one and absent in the other.

The general lesson: **string-equality on bytes is a lossy assertion.** If the wire-level behaviour matters (encoding, BOM, multi-byte boundaries, trailing whitespace, line endings, …), the assertion has to be at the byte level, and the comparison has to bypass `.text()` / `.toString()` / `String(bytes)` on at least one side.

---

## Diagnostic — when to suspect this class

You see one of these signals:

| Signal | What it means |
|---|---|
| Feature works in dev, 404s in prod | Possible composition-root gap (Mode 1) |
| Unit + integration tests all green; user reports broken behaviour | Test apparatus bypassing the failure surface |
| Encoding / byte / format issue ("mojibake", "weird characters", "Excel can't open this") | Likely a string-string roundtrip trap (Mode 2) |
| "But I tested this!" + a feature that crosses a real adapter boundary | Suspect a fake-adapter masking real-adapter behaviour |
| A PR with `injectIntoTestApp(...)` or `InMemoryX` + a route file modification | The composition-root gap risk surface for that PR |

If you see these signals, inspect what the test bypasses. Cover the real failure boundary at the cheapest faithful layer; do not respond with an automatic E2E campaign.

---

## Coverage check

1. What does this test replace or decode before asserting?
2. Could the observed failure live in that replaced surface?
3. Can a unit/component or integration test exercise the real failing behavior directly?
4. If only a browser journey can, is that journey part of approved acceptance or an approved consequential regression?

A fake provider proves the fake-provider contract, not real-provider integration. A component buffer assertion proves buffer logic, not browser caret feel. Preserve these evidence limits without automatically opening another investigation.
