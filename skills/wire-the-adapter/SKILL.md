---
name: wire-the-adapter
description: How to introduce a new port/adapter without shipping a half-wired feature. Use when designing or reviewing infrastructure work — new repositories, blob storage, message queues, external clients, anything where tests inject a fake and prod needs the real thing. Triggers on port/adapter design, composition root work, dependency injection, "tests pass but prod 503s", DDD infrastructure layer, mock vs real adapter.
---

# Wire The Adapter

The single most common way infrastructure work ships broken: **tests inject the in-memory adapter, prod has nothing constructed at all, every endpoint returns 503/null/silent failure, and nobody notices because every test is green.**

This skill is the rule for not doing that.

---

## 1. The Anti-Pattern (with the war story)

On 2026-05-08, PRs #128 + #129 shipped the MinIO migration in `monorepo-incluir`. The work was done well by every reasonable measure:

- **Domain-Driven Design shape:** correct aggregates, value objects, ports, use cases
- **89 unit tests** + **24 integration tests** covering every code path
- **Code review** by GLaDOS approved the architectural shape

What got shipped to production:

```ts
// apps/hono-app/src/http/app.ts — the dependency interface
/**
 * aperture-47hg PR-B — optional blob storage adapter for the new
 * attachment upload routes. Wire createMinioBlobStorage() in
 * production; tests inject createInMemoryBlobStorage().
 */
readonly blobStorage?: BlobStorage;
```

That `Wire createMinioBlobStorage() in production` was a TODO. **It was never wired.** The function existed in `blob-storage-minio.ts` but was never called from the prod entrypoint. Every test injected `InMemoryBlobStorage` and passed. Prod had `deps.blobStorage === undefined` and every upload route hit:

```ts
if (!deps.blobStorage) return c.json({ error: 'Blob storage unavailable' }, 503);
```

The bug was caught when the operator tried to upload a file. The fix was a one-line revert of the entire migration.

**The dumb part:** the TODO comment proves the author *knew* the wiring was needed. The integration tests provided false confidence — they all injected the mock. Nothing in CI exercised the prod composition.

---

## 2. The Forensic Signature

Spot this anti-pattern in code review or self-review by looking for any of:

| Smell | What it usually means |
|---|---|
| **Optional dependency in `AppDependencies`** (`readonly blobStorage?: BlobStorage`) | Prod might never construct it; tests are the only callers |
| **Routes guarded by `if (!deps.x) return 503`** | The author admitted "this dep might not exist" — and there's no smoke test catching the case where it doesn't |
| **`TODO: wire X in production` comments** | Self-evident. If you wrote it, complete it in the same PR. |
| **Every test file imports `createInMemoryX`** with zero references to `createRealX` outside the adapter file itself | The real adapter has no test exercising it through the composition root |
| **Adapter factory function exists and exports cleanly but `grep -rn "createMinioBlobStorage()"` returns 0 calls** | It's never instantiated anywhere. It's dead code in prod. |
| **PR title / description claims "Wave N complete" but `server.ts` / composition root is unchanged** | The wiring layer was skipped |

If you see two or more of these in the same PR, the prod composition is almost certainly broken.

---

## 3. The Rules

### Rule 1 — Composition root in the same PR

Any PR that introduces a new port + adapter pair MUST include the line that constructs the real adapter and passes it into `createApp` (or your equivalent composition root). Not in a follow-up. Not in a TODO. **Same PR.**

```ts
// apps/hono-app/src/http/server.ts (or wherever you bootstrap)
import { createMinioBlobStorage } from '../adapters/blob-storage/blob-storage-minio.js';

const blobStorage = createMinioBlobStorage();  // ← THIS LINE

const app = createApp({
  // ...other deps
  blobStorage,  // ← AND THIS
});
```

If your PR doesn't have those two lines, it's not done.

### Rule 2 — Required dependency types in production composition

Don't ship `readonly blobStorage?: BlobStorage` (optional) to production paths. The optionality is for **tests only** — if a code path can't function without the adapter, the type should be required at the prod composition root:

```ts
// AppDependencies — the type that createApp accepts
export interface AppDependencies {
  // ...
  readonly blobStorage?: BlobStorage;  // OK — tests can omit it
}

// But the prod composition function should NOT make it optional:
export function bootstrapProd(env: Env): AppDependencies & { blobStorage: BlobStorage } {
  return {
    // ...
    blobStorage: createMinioBlobStorage(env),  // required at this layer
  };
}
```

The optional type lives in the test surface; the prod surface tightens it. Ship both.

### Rule 3 — Smoke test that exercises the real composition

Unit tests with `InMemory` are necessary but not sufficient. **Add a smoke test that:**

1. Boots the prod-shape server (no mocks, real env, real adapters)
2. Hits the actual endpoint
3. Asserts NOT 503 / NOT null

The cheapest way: a Vitest test that spins up a Testcontainers MinIO + Postgres, calls `bootstrapProd(testEnv)`, runs `createApp(deps)`, and POSTs to the route. If the wiring is missing, this test fails. Mock-based integration tests can't.

```ts
// example: apps/hono-app/tests/smoke/blob-storage-wired.test.ts
test('production composition exposes a working blob storage', async () => {
  const env = await spinUpTestEnv();      // real testcontainers minio + pg
  const deps = bootstrapProd(env);
  const app = createApp(deps);

  const res = await app.request('/api/files/staging-uploads', {
    method: 'POST',
    body: validMultipartBody(),
    headers: validAuthHeaders(),
  });

  expect(res.status).not.toBe(503);
  expect(res.status).toBeGreaterThanOrEqual(200);
  expect(res.status).toBeLessThan(300);
});
```

If your CI doesn't have something like this for every adapter you ship, ship it before the feature.

### Rule 4 — Composition test in unit suite

For every port the app exposes, add a test that pins the composition contract:

```ts
test('createApp without blobStorage returns 503 from upload routes', async () => {
  const app = createApp({ /* deps without blobStorage */ });
  const res = await app.request('/api/files/staging-uploads', { method: 'POST', ... });
  expect(res.status).toBe(503);
});

test('createApp with blobStorage returns 2xx from upload routes', async () => {
  const app = createApp({ /* deps */, blobStorage: createInMemoryBlobStorage() });
  const res = await app.request('/api/files/staging-uploads', { method: 'POST', ... });
  expect(res.status).toBeGreaterThanOrEqual(200);
  expect(res.status).toBeLessThan(300);
});
```

This pins the contract — any future regression that drops `blobStorage` from prod deps fails the second test. Cheap to write, structural protection.

---

## 4. Code Review Checklist (orchestrators + reviewers)

When reviewing a PR that introduces a new port/adapter pair, ask **all** of these. Don't approve until each is answered with a concrete file:line reference, not "we'll add it later."

- [ ] **Where is `createXAdapter()` constructed in production code?** (Get a file path. If the answer is "in tests" or "TODO" — block.)
- [ ] **Where is the smoke test that boots the prod composition and hits the endpoint?** (Different from integration tests with mocks.)
- [ ] **Is `deps.x` typed as required or optional in the prod bootstrap function?** (Should be required.)
- [ ] **What happens if `deps.x` is undefined at runtime?** (If "503" — there should be a unit test pinning that, AND a unit test pinning the success path.)
- [ ] **Has the author run the binary against real adapters end-to-end before merging?** (Local docker-compose up, real MinIO, real upload — not the test suite.)

If you can't tick all five, the PR ships half-broken. No exceptions for "it's just infrastructure," "the tests cover it," or "we'll smoke test in staging." Staging is too late if your auto-deploy goes straight to prod.

---

## 5. Author Self-Review (before opening the PR)

Run this once before you open the PR:

```bash
# 1. Does the adapter factory get called outside its own file?
grep -rn "createMinioBlobStorage\|createXAdapter" --include='*.ts' apps/ | grep -v "/blob-storage-minio.ts"

# 2. Is the composition root touched?
git diff main -- apps/hono-app/src/http/server.ts apps/hono-app/src/composition.ts

# 3. Are there any TODO comments mentioning "wire" or "production" in the diff?
git diff main | grep -iE "TODO.*wire|TODO.*prod|wire.*production"
```

If #1 returns nothing, you didn't wire it. If #2 is empty, you didn't bootstrap it. If #3 returns matches, you wrote a TODO instead of completing the work.

---

## 6. The "Tests Pass But Prod 503s" Fingerprint

If a bug surfaces in prod that fits this pattern, the diagnosis is almost always:

1. Open the prod logs — find the 503 / null / silent error
2. Find the route handler — look for the `if (!deps.x)` guard
3. Grep for the adapter factory — count call sites in non-test files
4. If count is zero in non-test files: confirmed. The bug is missing wiring.

This takes about three minutes once you know the shape. Faster than re-running tests, faster than rebuilding the container, faster than reading the code top-down.

---

## 7. Why DDD Makes This Worse (and What To Do)

DDD's strength — clean separation between domain, application, and infrastructure — is also where this bug hides. Tests at the domain and application layer don't need real adapters. Tests at the infrastructure layer test the adapter in isolation. The composition root is the seam where it all comes together, and it's often the least-tested part of the codebase.

**The discipline:** treat the composition root as a first-class layer with its own tests. It's where the bug lives, so it deserves the test coverage. A 50-line `bootstrapProd` function should have a test file next to it.

---

## 8. Anti-Patterns to Reject in Review

| Anti-pattern | What to say |
|---|---|
| "I'll wire it in a follow-up PR" | "No. Same PR. The follow-up gets de-prioritised and the feature ships broken." |
| "The integration tests cover it" | "Integration tests inject the mock. They don't catch missing prod wiring. Add a smoke test that boots real composition." |
| "It's just infrastructure, low risk" | "The aperture-47hg incident was 'just infrastructure.' It bricked file uploads in prod for the entire user base." |
| "I tested it manually on my laptop" | "Then turn that manual test into a CI step. If you can do it once, the runner can do it on every commit." |
| "Adding a smoke test is a separate task" | "Yes — and that task is part of this PR. Bundle them." |

---

## 9. Closing Thought

If you find yourself writing `// TODO: wire X in production`, stop. That comment is the bug. Either complete the wiring before you push, or don't open the PR. Future-you will not remember to come back. CI will not catch it. Code review may not catch it. The next person to find it will be a real user, in production, getting an error toast. Don't let that be how the work ships.

---

## 10. Read the test-side companion: `aperture:e2e-catches-what-lower-cant`

This skill is the **build-side** discipline for the composition-root-gap failure class — *how* to introduce a port/adapter without shipping it half-wired. The **test-side companion** is `aperture:e2e-catches-what-lower-cant`, which explains *why* unit + integration tests cannot catch the gap (they inject fakes directly into the test app, bypassing the prod composition root entirely) and *what* E2E coverage must do to surface it (exercise the real composition path, assert at the level where the failure lives).

Read both as a pair. They describe the same class of failure from opposite ends — the build side codifies the wiring discipline you apply when introducing the adapter; the test side codifies the coverage discipline that catches the gap when the wiring discipline slips. Bug class recurrence on monorepo-incluir (`aperture-y57q` blob-storage, `aperture-3ghh` survey adapters) is what earned the test-side skill its slot under aperture-4la6's promotion-by-recurrence heuristic; both incidents are also case studies for the build-side discipline in this skill.
