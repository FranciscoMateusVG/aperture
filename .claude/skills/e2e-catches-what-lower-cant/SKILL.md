---
name: e2e-catches-what-lower-cant
description: A class of bug — silent corruption / silent omission — that unit and integration tests structurally cannot catch because the test apparatus bypasses the failure-prone surface. Only end-to-end tests exercising the real prod composition path with byte-level assertions surface them. Use when designing test coverage for a new feature, when triaging a "tests passed but prod broke" bug, when deciding whether unit + integration coverage is "enough," or when an E2E catches something the lower layers missed. Triggers on composition-root gap, "tests pass but feature 404s in prod," test apparatus mismatch, dependency injection masking, string-string roundtrip, BOM strip, silent encoding loss, "why didn't unit tests catch this," `injectIntoTestApp`, fake adapter, `InMemoryX`, response.text() roundtrip, byte-level assertion.
---

# E2E Catches What Lower Tests Can't

A class of bug that **structurally cannot be caught** by unit or integration tests, no matter how thorough — because the lower layers' test apparatus bypasses the failure-prone surface. The bug only surfaces at end-to-end, exercising the real prod composition path with byte-level assertions.

This skill exists because the swarm hit this class three times in two days (May 2026) on monorepo-incluir — twice as a composition-root gap, once as a string-string roundtrip trap. Each instance unblocked customer-visible features that would otherwise have shipped silently broken. The common pattern earned its skill slot on the third recurrence per `aperture-4la6`'s promotion heuristic.

This skill is the **test-side companion** to `aperture:wire-the-adapter`. That skill is about how to introduce a port/adapter without shipping it half-wired (build-side discipline). This one is about why unit + integration tests miss the half-wiring (test-side mechanism). Read both — they describe the same class of failure from opposite ends.

---

## The umbrella principle

> **Lower test layers (unit, integration) routinely substitute test-friendly apparatus for the real prod surface — fake adapters, in-memory stores, string-shaped fixtures, mocked composition roots. When the bug lives IN the swapped-out surface, no lower test can catch it, no matter how exhaustive its assertions. Only end-to-end tests that exercise the real prod composition path with byte-level assertions surface this class.**

The failure mode is always:

- **Silent corruption / silent omission** — the return value is wrong, but well-typed; the route is missing, but no error is raised at startup; the bytes drop, but the string equality on both sides of the test pipe still passes.
- **No exception, no 500, no test failure** at any layer below E2E.
- **Customer-visible feature shipped broken** if E2E hadn't been run before merge.

---

## The decision rule (call this at coverage-design time)

If a feature crosses an adapter boundary OR has byte-level / encoding / format behaviour that matters:

> **Your E2E must (a) exercise the real prod composition path, and (b) assert at the byte level wherever byte-level behaviour matters.**

If your E2E only exercises a test-app composition or only asserts at the string-equality level, your coverage has a structural gap — and that gap is exactly where this class of bug lives. Unit and integration coverage cannot close it, no matter how deep.

(Forward-friction check below for the longer version. The decision rule is the one-line callable.)

---

## Two banked modes (3 worked examples)

### Mode 1 — Composition-root gap

**Shape:** the test app and the prod app have *different wiring*. Unit + integration tests construct a test app that injects fake adapters (`InMemoryBlobStorage`, `surveyRepository`, etc.) directly into the route handlers. The prod composition root (`server.ts`, `index.ts`, the entry point) is supposed to wire the *real* adapters into the *real* route mount. If the entry point forgets to do that, the routes ship but the adapter doesn't — every request hits a catch-all 404 (or a no-op default), and no test below E2E ever exercises the prod composition path.

**Worked example A — `aperture-y57q` (PRs #128/#129, Vance + Rex)**

Blob-storage adapter never wired in `server.ts`. Unit + integration tests passed because they injected `InMemoryBlobStorage` directly into the test app. Only E2E (real Postgres path) surfaced the gap.

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

If you're seeing any of these, the next move is **not** "write more unit tests" — those are structurally incapable of catching this. The next move is "what does my E2E exercise, byte-level, against the real composition path?"

---

## Forward-friction check (apply at test-design time, not bug-triage time)

Before you sign off on a feature's test coverage, ask:

1. **What does my test apparatus substitute for the real prod surface?** (Fake adapter? String fixture? Mock composition root? In-memory store?)
2. **Could the bug I'm worried about live IN the thing I substituted?** If the answer might be yes, that bug is invisible to your test.
3. **Does my E2E exercise the real composition path?** I.e. does the E2E actually go through `server.ts` / `index.ts` / your prod entry point, or does the E2E also wire test adapters?
4. **Does my E2E assert at the byte level where byte-level behaviour matters?** Or does it round-trip through `response.text()` / `.toString()` and lose the byte-level signal?

If the answers to (2) is "yes" and (3) or (4) is "no, I never assert against that," your coverage has a structural gap. The fix is to extend the E2E — not to deepen the unit/integration layer that can't see the failure.

The cost of this pass is 30 seconds per feature. The cost of skipping it is a customer-visible regression that ships, gets caught in prod, and requires an emergency PR (every banked instance above unblocked or hot-fixed a customer-visible feature).

---

## What this skill is NOT for

- **Routine bug-finding** — most bugs ARE catchable at unit/integration. This skill is specifically for the structural-blind-spot class.
- **Excuse to skip unit + integration tests** — they catch a different (and broader) class. The argument is "you need BOTH unit/integration AND E2E with the right substrate," not "E2E replaces lower layers."
- **General E2E enthusiasm** — adding more E2E without targeting byte-level / composition-root assertions doesn't help. The skill is specifically about *what* the E2E should assert.

---

## Source provenance

| Mode | Bead | PR | Agent | Failure mode |
|---|---|---|---|---|
| 1: Composition-root gap | `aperture-y57q` | monorepo-incluir #128/#129 | Vance + Rex | Blob-storage adapter not wired in server.ts |
| 1: Composition-root gap | `aperture-3ghh` | monorepo-incluir #303 | Rex | Survey adapters not wired in server.ts |
| 2: String-string roundtrip | `aperture-tx2k` | monorepo-incluir #306 | Vance | Next.js proxy `.text()` strips BOM via TextDecoder |

**Class-diagnosis credit: Izzy.** Both composition-root gaps and the BOM strip were *caught* by Izzy's E2E suites (her Q1 surveys E2E specifically, on the most recent two). Banking the abstract class — recognizing that y57q, 3ghh, and tx2k all share the same root structural cause (test apparatus bypasses the failure surface) — is Izzy's contribution to this skill. The individual provenances are Rex's + Vance's; the skill's reason for existing is Izzy's.

---

## Adding a new mode

If you hit a third structural shape (beyond composition-root gap and string-string roundtrip), bank it as a new mode. Use the same template:

1. **Mode name** — what test-apparatus-vs-prod-surface mismatch produces the bug
2. **Shape** — abstract description of how the substitution masks the failure
3. **Worked example(s)** — bead ID, PR, agent, what the test apparatus did, why the bug lived in the gap
4. **Fix shape** — what the E2E coverage needs to do to catch this mode

Open a PR on the aperture repo with the new mode. The skill's umbrella principle should hold regardless of how many modes accumulate; the modes themselves are the swarm's accreted catalog of "ways your test apparatus can lie to you."

---

## Sibling skill — code design

This skill is about **test design**: what each test type structurally catches. There's a sibling skill on **code design**: `aperture:dont-model-phantom-cases` — what cases the code itself should actually handle vs phantom future cases. They're complementary; both about "match what's actually true today, not assumed shapes." Read both.
