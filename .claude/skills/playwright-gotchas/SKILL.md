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

## 5. Visible element inside a modal Radix Dialog retries click forever — body-portaled overlays are mouse-dead (`pointer-events: none` inheritance)

**Symptom:** A dropdown/menu/tooltip rendered inside a modal dialog resolves visible (`toBeVisible()` passes, screenshot shows it), but `click()` retries until timeout — Playwright reports another element (typically the `DialogContent`) "intercepts pointer events." Two banked instances hit the identical signature: **163 retries / 90s timeout**. No amount of `waitFor`, scrolling, or locator tightening helps; `{ force: true }` "works" but is a lie — real users can't click it either, so force-clicking masks a genuine product defect.

**Cause:** Modal Radix Dialogs (via react-remove-scroll) set `pointer-events: none` on `document.body` and re-enable it only on the dialog's own subtree. Anything portaled to `document.body` — react-select's `menuPortalTarget={document.body}`, many tooltip/popover libraries — inherits `none`. **Paint ignores `pointer-events`; hit-testing honors it**: the overlay renders perfectly while `elementFromPoint` falls through to whatever is behind it. A `onPointerDownOutside` preventDefault on the dialog does NOT fix this — it only stops dismissal, it never re-enables the portal's pointer events. The sibling shape (interactive SVG nested *inside* a trigger button) produces the same interception signature for a different reason (the trigger's own content wins hit-testing).

**Fix (product code, not test code):** re-enable pointer events on the portaled subtree — for react-select, one load-bearing line on the `menuPortal` style:

```tsx
styles={{
  ...customStyles,
  menuPortal: (base) => ({
    ...base,
    zIndex: 9999,
    pointerEvents: "auto", // LOAD-BEARING: modal Radix Dialog sets
    // pointer-events:none on document.body; this menu is portaled there
    // and inherits it — visible but mouse-dead without this line.
  }),
}}
```

Do NOT reach for the tempting alternatives: making the Dialog non-modal changes UX semantics, and portaling the menu *into* `DialogContent` breaks react-select's viewport-space coordinate math (DialogContent carries a `transform`, which becomes the containing block for fixed/absolute descendants).

**Test-side rule:** when a visible element inside a modal won't take a click, the FIRST hypothesis is pointer-events inheritance, not a flaky locator. Keep the spec strict (no `force: true`, no locator softening) — the strict click IS the regression test for the product fix. Assert the real interaction end-to-end (click → state change → persistence), exactly what caught both instances.

**Where this shows up:** any body-portaled floating UI (react-select menus, custom tooltips, popovers from non-Radix libraries) rendered while a modal Radix Dialog is open; also icon controls nested inside other interactive elements (same interception signature, different mechanism — see the clear-button case).

**Source:** Izzy (strict E2E discovery, production compose, both instances) + Vance (product fixes), monorepo-incluir PRs #770 (nested-interactive clear X in `userSearch.tsx`, `aperture-l7w9r`) and #772 (`pointerEvents:"auto"` on the menuPortal in `courseRestrictionsDialog.tsx`, commit 858099ff). Both verified by literal-click re-runs on production compose — 2/2 pass, no force-click.

---

## 6. `boundingBox()` samples entry animations — poll to geometric settle before asserting element sizes

**Symptom:** A size assertion (`boundingBox().height >= 44` for touch targets, width checks, position checks) fails with a value ~2-10% *smaller* than the CSS declares — e.g. a `min-h-[46px]` control "renders" 43.96px. The readings **vary between runs and between nodes in the same run** (42.03–42.56 for one node; ratios 0.950 vs 0.956 for two nodes measured together). Escalating the declared size doesn't converge: bumping 44→46px just moves the measured value to 46×0.955 ≈ 43.9 — still "under."

**Cause:** `boundingBox()` returns the box at the instant of the call — it does NOT await animation settle (only `click()`'s actionability check waits for stability). Radix/shadcn dialogs and popovers enter with `zoom-in-95`: content starts at `transform: scale(0.95)` and eases to 1.0 over ~200ms. Measuring "immediately after open" samples mid-easing, so every px value in the subtree reads ×0.95–0.99 depending on timing. The tell-tale signatures, in diagnostic order:

1. **Ratio ≈ the animation's start scale** (0.95 for `zoom-in-95`).
2. **Variance across runs for the same node** — a static cause (root font-size, zoom, CSS transform) gives a constant ratio; only a time-varying cause gives a range.
3. **Different ratios for different nodes in the same run** — impossible for any static page-wide scale.

Beware plausible-but-wrong static explanations. In the banked incident the ~0.955 ratio was twice misattributed: first to root-font rem scaling (px classes shrank too — refuted), then to "Tailwind v4 normalizes arbitrary values" (build was Tailwind v3.4.3, and the emitted CSS contained `min-height:46px` verbatim — refuted at the artifact layer). Check the emitted CSS in `.next/static/**/*.css` before believing any build-time transformation theory.

**Fix (harness side):** poll the actual clickable node until its geometry is stable across consecutive frames, THEN assert. Do not weaken the threshold; do not add a fixed sleep (flaky under load).

```ts
// Poll until settled: two consecutive identical boundingBoxes.
async function settledBox(locator: Locator) {
  let prev = await locator.boundingBox();
  await expect
    .poll(async () => {
      const cur = await locator.boundingBox();
      const stable =
        prev && cur && Math.abs(cur.height - prev.height) < 0.01 &&
        Math.abs(cur.width - prev.width) < 0.01;
      prev = cur;
      return stable;
    }, { timeout: 2000 })
    .toBe(true);
  return prev!;
}
```

Prefer polling real default-motion behavior over globally forcing `reducedMotion: 'reduce'` — polling exercises the production animation AND proves the settled target. (Forcing reduced motion is a valid separate spec: it tests your `prefers-reduced-motion` support.)

**Where this shows up:** any size/position assertion on content inside a Radix Dialog, AlertDialog, Popover, Tooltip, or dropdown that uses tailwindcss-animate entry effects (`zoom-in-*`, `slide-in-*`); any component library with scale-based mount transitions; screenshot comparisons taken immediately after open.

**Source:** Izzy (strict touch-target gate, production compose) + Vance (hypothesis from ratio variance + artifact-layer CSS audit), monorepo-incluir atomic branch b7d0c361 (`aperture-h7406`). Settled re-measure confirmed 45.41–46.00px on nodes that had "measured" 42–43.9px mid-animation; 20/20 suite green after the harness moved to settle-polling. Two px-bump escalations (44→46→proposed 48) were avoided by diagnosing the measurement instead of chasing it.

---

## 7. jsdom green is NOT evidence for focus/composition claims — the divergence catalogue

**Symptom:** A rendered jsdom test (RTL + user-event) passes a keyboard/focus/menu assertion, but the identical interaction fails deterministically in a real browser — or vice versa. Worse: a whole suite stays green across a product fix AND its pre-fix revert (false-green — it pins nothing).

**Cause:** jsdom + user-event simulate focus, tab order, and event delivery with their own models, which diverge from real browser + real component-library composition in at least SIX documented ways (all from one surface, the Visibilidade combobox saga, monorepo-incluir PR #794, beads 5gaph/qeraa/kmuiv/4fkma, ten QA rounds):

1. **Tab order without the trap**: jsdom's user.tab() computed a tab order that ignored Radix FocusScope — real Tab landed on the DialogContent root (`role=dialog tabindex=-1`), never the button jsdom reached.
2. **Portal event delivery**: keydown events on inputs inside a Radix Portal (body-appended) never reached React handlers in jsdom — React root-container delegation doesn't span body portals there. The handler test was silently vacuous until the Portal was removed from the wrapper.
3. **user-event's tab simulation fights handler focus()**: after a keydown handler calls focus(), user.keyboard("{Tab}") runs its OWN tab-order move and overrides it.
4. **FocusScope confounds every in-trap signal**: it preventDefaults BOTH Tab directions (so fireEvent's return can't be attributed to your handler) and wraps Shift+Tab onto your own target element (so a focus spy can't discriminate caller).
5. **Library-internal input state**: react-select's visible input cleared on blur/menu-close in the real composition in ways neither a controlled `inputValue` prop nor an `onInputChange` return-value covered in every path — and native `fill('')` produced input events that a synthetic-layer mirror missed entirely.
6. **Effect-flush races are real-browser-fast**: an intent flag cleared only in a useEffect left an ~11ms window that Playwright's real event speed hit 3/3 deterministically — jsdom's act() batching never exposed it.

**Fix (a discipline, not a line of code):**
- **Authority layering**: pure-logic machine tests < bare-render handler-contract pins < Dialog-wrapped rendered tests < REAL Playwright against the production composition. Each layer pins only what it can attribute; label each test's honest scope in a comment; the real-browser gate is the ONLY authority for end-to-end keyboard/focus claims.
- **Bare-render handler-contract pins** for jsdom: assert YOUR handler's contract (fireEvent return = your preventDefault; a focus/state spy = your routing) OUTSIDE the confounding trap — never document.activeElement inside it.
- **Problem-shaped regressions, proven bidirectionally**: when a fix lands, the new test must FAIL on the pre-fix commit (check the component out at the old SHA and run) and pass on the fix. A suite that passes both ways pins nothing — QA proved a 7/7 suite false-green exactly this way.
- **Design lessons that ended the saga**: sync state from NATIVE events (onInputCapture on a wrapper — catches keyboard/paste/cut/IME/Playwright fill below any library mediation); derive UI gates from EVERY state that renders what they gate (an alert rendered by `retryPending` means `retryPending` belongs in the menu gate); clear intent flags SYNCHRONOUSLY in the event path, never only in effects.

**Where this shows up:** any keyboard/focus/menu assertion involving Radix Dialog/FocusScope, portals, react-select or other libraries with internal input state, or intent flags cleared in effects. If your assertion involves `document.activeElement` inside a focus trap in jsdom — stop, it is not evidence.

**Source:** Izzy (ten rounds of strict real-browser gating, trace-driven postmortems, the false-green catch) + Vance (fixes, handler-contract pin pattern, bidirectional RED/GREEN proof), monorepo-incluir PR #794 (final head dc7c7c08), beads aperture-5gaph/qeraa/kmuiv/4fkma, 2026-08-13/14. Final authority run: isolated production-compose 2/2 first-attempt, no retries.

---

## toBeVisible() passes on overflow-clipped elements

**Symptom:** a dropdown/popover assertion passes in Playwright (`toBeVisible()` green) but the element is genuinely invisible to a real user — clipped by an ancestor's `overflow` and unreachable by tap/click.

**Cause:** `toBeVisible()` checks CSS visibility properties (`display`, `visibility`, `opacity`, zero-size) but does NOT check whether the element is actually clipped out of the visible viewport by an ancestor's `overflow: hidden`/`auto`. A container with `overflow-x: auto` implicitly forces `overflow-y: auto` too (a CSS quirk — `overflow-x`/`overflow-y` can't independently be `visible` if the other axis is scrollable), which silently clips a dropdown positioned to expand vertically. The element is present, has nonzero size, and is technically "visible" by Playwright's definition — but a real user cannot see or reach it.

**Fix:** for any dropdown/popover/menu assertion, use a hit-test instead of `toBeVisible()`:

```ts
// Honest assertion — proves the element is actually reachable at that point
const box = await dropdown.boundingBox();
const hit = await page.evaluate(([x, y]) => {
  const el = document.elementFromPoint(x, y);
  return el?.closest('[data-testid="dropdown"]') != null;
}, [box.x + box.width / 2, box.y + box.height / 2]);
expect(hit).toBe(true);
```

`elementFromPoint` returns what's actually rendered at that coordinate — if an ancestor's overflow clips the dropdown, a sibling or the clipping container itself will be hit instead, and the test correctly fails.

**Where this shows up:** any dropdown/popover/tooltip that expands beyond its trigger's immediate container, especially inside a horizontally-scrollable region (`overflow-x: auto`) where the y-axis clip is implicit and easy to miss in review.

**Source:** Vance, eunenem-engine PR #79 (aperture-acr3t, mobile popover-not-showing bug), 2026-08-28. Root cause was exactly this pattern — a horizontal-scroll container's implicit vertical clip — and the bug had been shipping invisibly because the existing test suite asserted `toBeVisible()` and passed.

---

## Adding a new gotcha

When Playwright bites you in a way that took >10 minutes to figure out and would bite someone else the same way, bank it here. Use the same shape:

1. **Symptom** — what you saw, in the failure mode the next agent will encounter
2. **Cause** — the underlying framework behaviour
3. **Fix** — the actual code change, with a short code example
4. **Where this shows up** — the pattern of UI/test scenarios that trigger it
5. **Source** — agent name, PR / bead ID, file, and which test earned the lesson

Then open a PR on the aperture repo with the edit. Ping Atlas if you'd like a second pair of eyes on the framing.
