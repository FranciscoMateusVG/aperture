---
name: playwright-gotchas
description: Playwright E2E gotchas banked by Aperture testing agents — quirks that cost ~10 minutes each to re-derive. Use when writing or debugging `*.spec.ts`, `playwright.config.ts`, or anything under `e2e/`, especially when a test hangs or asserts the wrong layer. Triggers on `navigator.clipboard` in tests, `aria-disabled` interactions, `context.grantPermissions`, hanging actionability checks, `notFound()` route tests, "test is timing out and I don't know why".
---

# Playwright Gotchas

Small framework quirks that bite. Each entry is one banked failure mode + the fix + a citation back to the test where we banked it.

This skill exists because three Aperture agents touch Playwright periodically — **Izzy** writes the e2e coverage, **Vance** uses it for visual-regression fixtures, **Scout** uses it for mobile-web flows — and they kept paying the same 10-minute re-derivation tax. One paragraph each up front is cheaper than re-discovering the same gotcha six months from now.

When you add a new gotcha, follow the same shape: **Symptom → Cause → Fix → Source citation → Code example**. Cite the test that earned the lesson; future-you will want to read the surrounding context.

---

## 1. `aria-disabled='true'` makes `.click()` spin-wait forever

**Symptom:** You call `await element.click()` on a button or row that has `aria-disabled="true"`. The test times out on actionability — Playwright reports the element is "not enabled" and waits indefinitely, even though the DOM node is visible and the underlying `<a>` or `<button>` is structurally clickable.

**Cause:** Playwright's actionability check treats `aria-disabled="true"` as **not enabled** (same as the native `disabled` attribute). The default `click()` waits for actionability before dispatching, so it never fires.

**Fix:** When the test intentionally needs to dispatch a click on an aria-disabled element — usually to verify that the React `onClick` handler calls `preventDefault()` and the user doesn't navigate — pass `{ force: true }` to bypass actionability:

```ts
// Test 6 — "em breve" rifa row is aria-disabled and clicking does not navigate.
const soonRow = page.locator(".painel-row.soon");
await expect(soonRow).toHaveAttribute("aria-disabled", "true");

// Default click() hangs forever on aria-disabled. Use { force: true } to
// simulate a real user managing to click a "disabled" link and assert the
// React onClick still calls preventDefault.
const urlBefore = page.url();
await soonRow.click({ force: true });
await page.waitForTimeout(300);
expect(page.url()).toBe(urlBefore);
```

**Where this shows up:** anywhere the UI surfaces "em breve" / "coming soon" / feature-flagged-off controls with `aria-disabled="true"` and a JS-level `preventDefault`. If you're testing the click-prevention contract itself, you NEED the click to dispatch — `{ force: true }` is the right call. If you're testing that the user can't interact at all, assert on `aria-disabled` and DON'T attempt the click.

**Source:** Izzy, eunenem-v2 PR #5 (`aperture-cw6b`), `e2e/painel.spec.ts` — Test 6.

---

## 2. `navigator.clipboard` requires `context.grantPermissions(['clipboard-read', 'clipboard-write'])`

**Symptom:** A test exercises a "copy to clipboard" button (or any code that calls `navigator.clipboard.writeText` / `readText`). The test fails with a confusing error far from the call site — often the component's `try/catch` swallows the rejection, the visible UI state never updates, and the assertion times out on "expected `.copied` class to appear" with no indication that clipboard was the root cause.

**Cause:** Chromium headless does not grant clipboard permissions by default. `navigator.clipboard.writeText` rejects with a permission error; well-written components catch it and continue silently, so the symptom looks like "the copy button doesn't work" rather than "permissions denied."

**Fix:** Grant clipboard permissions on the browser context **before** navigating:

```ts
// Test 4 — share-link copy flow.
test("share-link copy button toggles to copiado, fires toast, reverts", async ({
  page,
  context,
}) => {
  // Grant clipboard-write so navigator.clipboard.writeText succeeds.
  // Component still progresses on rejection (try/catch in onCopy), but
  // granting it removes a flake source on Chromium headless.
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);

  await page.goto(PAINEL_PATH, { waitUntil: "networkidle" });
  // … assert on .copied class, toast, revert-after-timeout, etc.
});
```

**Where this shows up:** copy-link buttons, "copy code" snippets, share-sheet flows, anything that round-trips through the Clipboard API. Grant both `clipboard-read` and `clipboard-write` — read is occasionally needed for round-trip assertions, and granting both has no downside.

**Source:** Izzy, eunenem-v2 PR #5 (`aperture-cw6b`), `e2e/painel.spec.ts` — Test 4.

---

## 3. For 404 assertions, check `page.goto(...).status()` BEFORE asserting visible content

**Symptom:** You're testing a Next.js route that calls `notFound()` for invalid slugs. You navigate to `/painel/this-does-not-exist` and assert on the visible content of the page. The test passes — but it's actually asserting against the rendered output of the not-found page, not verifying that the **HTTP status** was 404. A future regression where the route serves the wrong content with a 200 status would slip past.

**Cause:** `page.goto(...)` returns a `Response | null` whose `.status()` tells you the actual HTTP status. If you skip that and go straight to content assertions, you're testing rendering, not routing. Next's `notFound()` returns 404, but a misconfigured route, a missing rewrite, or a regression that just renders a "not found" body with a 200 status will all pass content-only assertions.

**Fix:** Capture the Response from `page.goto` and assert on `.status()` **first**, then content:

```ts
// Test 1 — route resolution + 404.
test("GET /painel/helena → 200; any other slug → 404", async ({ page }) => {
  const ok = await page.goto("/painel/helena", { waitUntil: "domcontentloaded" });
  expect(ok, "page.goto must return a Response").toBeTruthy();
  expect(ok!.status(), "Helena slug must resolve to 200").toBe(200);
  await expect(page.locator(".painel-app")).toBeVisible();

  // Negative case — unknown slug returns 404 via notFound().
  const notFound = await page.goto("/painel/this-slug-does-not-exist", {
    waitUntil: "domcontentloaded",
  });
  expect(notFound).toBeTruthy();
  expect(
    notFound!.status(),
    "Unknown slug must 404 (notFound() in src/app/painel/[slug]/page.tsx)",
  ).toBe(404);
  // 404 surface should NOT contain the app shell.
  await expect(page.locator(".painel-app")).toHaveCount(0);
});
```

**Where this shows up:** any route that uses Next's `notFound()`, `redirect()`, or a custom 404 page. Also relevant for testing auth-gated routes that return 401/403 — same pattern: capture the Response, assert on `.status()` first, then optionally on rendered content.

**Note on `waitUntil`:** `"domcontentloaded"` is usually fine for status checks. If the page renders content that depends on client-side hydration, you may also want `"networkidle"` for the content assertions — but the status check itself only needs the document.

**Source:** Izzy, eunenem-v2 PR #5 (`aperture-cw6b`), `e2e/painel.spec.ts` — Test 1.

---

## 4. E2E spec FORMAT is caught by nothing until a branch-cut reds the full-repo biome — run `biome check --write` as the LAST step before commit

**Symptom:** You (or a teammate) cut a fresh branch off `staging`, touch something unrelated, and the CI `lint` step (`pnpm check`'s full-repo `biome`) fails — pointing at an `e2e/*.spec.ts` file **you never edited**. It's an *inherited* red: an e2e spec landed on `staging` unformatted, and now every branch cut after it fails full-repo biome until someone reformats the file. Locally the author's per-file `biome check` looked clean and CI on the author's PR was green — so nothing flagged it before merge.

**Cause:** Playwright specs live outside every format-catching gate. In the engine repo: Playwright is **not run by CI** (`aperture-zn1ud`), the app-level `tsc` **excludes `e2e/`**, and the root `tsc` covers `src/` only. So an e2e spec's *formatting* is validated by exactly one thing — the full-repo `biome` in the `lint` step — and that only trips when a branch is cut and biome walks the whole tree, inheriting the unformatted file's red onto an innocent branch. The classic self-own: running a **read-only** `biome check` (easy to do with `| tail -1`, which hides the complaint) after your *final* edits instead of `biome check --write`, OR running `--write` early and then making one more edit whose formatting never gets applied.

**Fix:** Make `biome check --write` the **last** step before `git commit` on any e2e spec — after the final edit — and confirm it changed nothing:

```bash
# LAST thing before committing an e2e spec — after every edit is done:
biome check --write e2e/my-gate.spec.ts     # or: pnpm lint --write <file>
biome check e2e/my-gate.spec.ts             # must say "No fixes applied"
git diff --stat e2e/my-gate.spec.ts         # must be EMPTY (write changed nothing)
git add e2e/my-gate.spec.ts && git commit ...
```

Two rules that make it stick:
- A **read-only** `biome check` is not enough — it reports but doesn't fix, and a truncated view (`| tail -1`) can hide the one line that matters.
- Running `--write` and *then* making another edit **doesn't count** — the later edit's formatting is unapplied. Re-run `--write` after the *last* change.

**Where this shows up:** any repo where Playwright specs sit outside the typecheck + test gates (engine `e2e/`, and any app whose `tsconfig` excludes the e2e dir). If your spec's only format gate is a whole-repo linter that runs on branch-cut rather than per-file on your PR, this bites. It is a **gate-coverage** gotcha, not an API quirk — the spec runs fine; it's the *format* that rots silently and lands on someone else's branch.

**Source:** Izzy, engine PR #381 (`aperture-8r5kp`, W2 enforcement gate) — merged unformatted after a read-only `biome check` followed my final edits; became the 4th inherited-staging-red of the fblrt wave, fixed by Rex in #384. Banked `aperture-103mj`.

---

## Adding a new gotcha

When Playwright bites you in a way that took >10 minutes to figure out and would bite someone else the same way, bank it here. Use the same shape:

1. **Symptom** — what you saw, in the failure mode the next agent will encounter
2. **Cause** — the underlying framework behaviour
3. **Fix** — the actual code change, with a short code example
4. **Where this shows up** — the pattern of UI/test scenarios that trigger it
5. **Source** — agent name, PR / bead ID, file, and which test earned the lesson

Then open a PR on the aperture repo with the edit. Ping Atlas if you'd like a second pair of eyes on the framing.
