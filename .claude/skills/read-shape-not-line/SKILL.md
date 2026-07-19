---
name: read-shape-not-line
description: Shape-change discipline for reading CI failures. Use when a fix's CI re-fails and the failing test/line looks identical to the previous run — the FIRST diagnostic move is to read the actual failure SHAPE (assertion type, locator behaviour, error context), NOT to assume same line = same root cause. Triggers on "CI fails again", "same test failing", "shipped a fix didn't help", "wrong banner", "banner not visible", "CI shape change", "still failing on line N", "fix didn't work".
---

# Read The Shape, Not The Line

When CI re-fails on a fix and the failing test/line is **the same as before**, the natural reflex is: "same symptom, same bug, my fix was wrong, try another fix." That reflex is the bug. Same test + same line can hide N different root causes — each one needing a different fix layer.

This skill is the rule for not iterating on the wrong category of fix.

---

## 1. The Decision Rule (shape > line)

> **The failing line is the address, not the diagnosis. The SHAPE of the failure — which assertion threw, what the locator resolved to, whether the test's setup ran — is the diagnosis.**

A test failing on `lifecycle.spec.ts:91` with `expect(banner).toBeVisible()` and a test failing on `lifecycle.spec.ts:91` with `locator.click()` are two different bugs even if `:91` is literally the same source line. The first means "the element isn't on the page." The second means "the element is on the page but something is intercepting the click." Different fix layers, different mental models, different specialists.

If your second fix attempt doesn't change the failure SHAPE, you're not pivoting — you're guessing harder in the same wrong category. Stop and read the actual error text before shipping another patch.

---

## 2. The Three Discriminators

When CI re-fails on the same test/line, **read these three things before forming a hypothesis**:

| Discriminator | What you're reading | What different values point at |
|---|---|---|
| **1. Which assertion threw?** | The Playwright (or whatever runner) error line: `Error: expect(locator).toBeVisible()` vs `locator.click: Element is outside of the viewport` vs `Timeout 30000ms exceeded while waiting for selector` vs `expect(received).toHaveText(expected)` | `toBeVisible` fail → element missing / wrong selector / data fixture. `locator.click` fail with "intercepts pointer events" → overlay / z-index / modal. `waitForURL` fail → routing / redirect / SPA navigation. `toHaveText` fail → content rendered but wrong copy. Each points at a different fix layer. |
| **2. What did the locator resolve to?** | The locator preamble in the error: `locator resolved to 0 elements` / `locator resolved to 2 elements` / `locator resolved to <div class="banner">…</div>` | `count=0` → fixture / wiring / data not seeded / selector drift. `count>1` → ambiguity (selector too broad, cross-spec pollution rendering a sibling). `count=1, resolved but failing actionability` → overlay, disabled state, off-screen, animating. |
| **3. Was the test's setup/precondition reached?** | beforeEach / beforeAll diagnostic logs (or their absence). Did the setup steps print what they were supposed to? Did the fixture seed the DB? Did the login fixture issue cookies? | If setup logs are missing → failure is **upstream of test** (fixture broken, login flow regressed, container not booted). If setup logs are present and complete → failure is **in test** (assertion / interaction issue). Diagnosing the wrong half wastes cycles. |

**Read all three before deciding what kind of fix the failure needs.** Two of them rarely lie at once. If three say "same as before," only then is your fix actually in the same category.

---

## 3. Worked Example — Surveys-V1 Five-Cycle Progression

This skill exists because Izzy walked the same `lifecycle.spec.ts:91` failure for five CI cycles on PR #300 + PR #315 (`monorepo-incluir`, 2026-05-20/21). Each cycle, the surface report was "banner-not-visible, lifecycle.spec.ts:91 still failing." Each cycle, the actual SHAPE was different.

| Cycle | Same line? | Assertion that threw | Locator resolved to | Actual root cause | Fix layer |
|---|---|---|---|---|---|
| 1 | `:91` | `expect(banner).toBeVisible()` | count=0 | Surveys repository never wired in `server.ts` (composition gap) | Infrastructure — wire-the-adapter |
| 2 | `:91` | `expect(banner).toBeVisible()` | count=0 | Layout Server Component's `getActiveSurveysForMe` fetch silently swallowed by `Promise.allSettled` | Application — error visibility / fail-loud at layout |
| 3 | `:91` | `expect(banner).toBeVisible()` | count=0 | (Suspected) `SurveyDiscoverySurface` shim render-path bug — **actually wrong root cause; fix didn't move the needle** | Misdiagnosis — cycle wasted because shape wasn't re-read |
| 4 | `:91` | `expect(banner).toBeVisible()` | count=0 | Cross-spec audience pollution rendered a DIFFERENT survey's banner; the title-filtered locator returned 0 | Test isolation — fixture / cleanup |
| 5 | `:91` | **`locator.click: subtree intercepts pointer events`** | count=1, resolved | Nav-drawer wrapper at `z-50` intercepted the click — the banner IS visible now, the next step in the same test is what's failing | Frontend — z-index / layering |

**The misread that cost cycles:** in cycles 1–4 the surface symptom was "same banner-not-visible." In each one the assertion AND the locator resolution were identical (`toBeVisible` failing on `count=0`). So discriminators 1 and 2 said "same shape." The thing that changed across the cycles was discriminator 3 — the *upstream cause* of `count=0`. Cycle 5 was the cleanest tell: discriminator 1 changed (assertion type flipped from `toBeVisible` to `click`-with-intercept), discriminator 2 changed (`count=0` → `count=1, resolved`). The error text screamed "different bug now" and was easy to miss because the line number was unchanged.

**The lesson:** when discriminators 1+2 stay the same across N cycles, the bug is upstream-of-the-DOM (fixture, wiring, data, isolation) and you need to instrument the setup path — not keep poking at the assertion. When discriminator 1 OR 2 flips, you're staring at a different bug; treat it as such even if line number is identical.

---

## 4. The Rule of Two — When To Stop Fixing And Pivot To Investigation

**Two consecutive CI failures with the SAME shape (same assertion + same locator resolution + same setup state) after two different fixes = wrong category of fix. Stop iterating. Pivot to active investigation.**

What "active investigation" means in practice:

- **DOM dump on failure.** Add `page.screenshot()` + `page.content()` to a `test.afterEach(async ({ page }, testInfo) => { … })` that fires on `testInfo.status !== testInfo.expectedStatus`. Look at the actual HTML the browser had when the assertion ran.
- **Structured diagnostic instrumentation.** `console.log` in the suspected upstream layer (the fixture, the layout fetch, the audience filter). Make the failure self-describe what state it ran against.
- **Bisect by re-running with one variable forced.** Run the spec with `test.only` to remove cross-spec pollution. Run against a hand-seeded DB to remove fixture-drift. Run with the suspected overlay removed to remove layering interference.
- **Read the trace.** Playwright trace files (`--trace on`) capture every network call, every locator resolution, every DOM snapshot. Open the trace before shipping fix #3.

Two same-shape fails ≠ "my third fix will be the right one." It means the diagnosis category is wrong. Pivoting takes 15 minutes; another wrong-category fix attempt takes a full CI cycle (10-25 minutes) plus context-switch cost when it fails again.

---

## 5. Forensic Drill — When CI Says "Same Test Failing"

The 90-second triage when you get the "CI re-failed" notification and the line number matches the last cycle:

1. **Open the failure log. Find the exact assertion line.** Not the test name — the `Error:` line under it.
2. **Compare assertion type vs last cycle.** Did `toBeVisible` become `click`? Did `toHaveText` become `toHaveCount`? If yes → DIFFERENT BUG. Treat as a fresh diagnosis.
3. **Compare locator resolution vs last cycle.** Did `count=0` become `count=1, intercepted`? Did the resolved HTML snippet change? If yes → DIFFERENT BUG.
4. **Compare beforeEach / fixture logs vs last cycle.** Did the login step print? Did the seeding step print? If a setup log is missing this cycle that was present last cycle → fixture regression, not the test.
5. **If all three discriminators match last cycle:** your fix didn't move the needle. Do NOT ship fix #3 in the same category. Pivot per §4.

This takes about 90 seconds once you know the shape. Faster than re-reading the spec, faster than re-running locally, faster than shipping another wrong patch.

---

## 6. Anti-Patterns to Reject

| Anti-pattern | What to say |
|---|---|
| "Same line failing, my fix didn't take, I'll try the obvious next thing" | "Read the assertion type first. If it's different, it's a different bug. If it's the same, you need DOM evidence, not another guess." |
| "It's still banner-not-visible, must be the same root cause" | "`toBeVisible` fails for at least 4 different reasons (missing / hidden / detached / count=0). Which one is THIS run?" |
| "I'll just bump the timeout" | "Timeout-bumping is the universal anti-fix. If the element genuinely needs longer, you have a perf bug; if the element will never appear, you have a wiring bug. Bumping hides both." |
| "Let me re-run CI, sometimes it just flakes" | "Then prove flake — re-run and check whether the shape is identical or different across runs. If two re-runs fail in the same shape, it's not flake, it's a real bug repeating." |
| "It's the same test, same line, must be a small tweak away" | "Five cycles of 'small tweak away' is how the surveys-v1 epic went. Read the shape." |

---

## 7. Sibling Skills (Cross-Links)

This skill lives in a small family of "how to read what your test suite is telling you" skills. Use them together:

- **`e2e-catches-what-lower-cant`** — sibling skill on what E2E catches that unit/integration cannot. That one is about WHY E2E earned its slot; THIS one is about HOW to read what E2E catches when it surfaces a regression. If your unit/integration suite is green but E2E is red and you're tempted to dismiss it as "E2E flake," read the E2E skill first; then read THIS skill before deciding what to fix.
- **`aperture:playwright-gotchas`** — Pattern 1 ("actionability failure for a known reason") is a downstream consequence of misreading shape change. If you've already shipped a wrong-category fix because you didn't read the new assertion type, that gotcha catalogue will tell you what each actionability error actually means at the DOM level.
- **`research-artifact-placement`** — when active investigation per §4 produces DOM dumps, trace files, or structured logs, that skill governs where the artifacts land so the next person to walk this bug doesn't have to rebuild the evidence.

---

## 8. Adding A New Precedent

When you walk a multi-cycle CI debug and the failure shape changed across cycles (or stayed the same in a way that taught you something), add a row to §3's table. The format:

```markdown
| Cycle | Same line? | Assertion that threw | Locator resolved to | Actual root cause | Fix layer |
```

And add a one-paragraph "the misread that cost cycles" note explaining which discriminator(s) flipped vs which stayed constant, and what category of fix the shape was actually pointing at. Real precedents make this skill keep paying for its slot.

If the precedent surfaces a new failure mode (a discriminator we don't have a row for in §2's table — e.g., a runner whose error text doesn't surface locator resolution cleanly), update §2 first, then add the precedent. The discriminator table is the load-bearing piece; precedents reinforce it.

---

## 9. Closing Thought

The line number in a CI failure is a coordinate, not a diagnosis. Two tests can fail on the same coordinate for four, five, or N different reasons — and they often do, especially in E2E suites where the failure surface is the rendered DOM and the upstream causes are anywhere in the stack between the database and the pixel.

If you find yourself shipping fix #2 with the same mental model as fix #1, stop. Read the error. Read the locator preamble. Read the setup logs. The shape will tell you what category of fix to write. Don't let "same line, same test" trick you into writing the same wrong fix twice.

---

## Provenance

Banked by Atlas on 2026-05-21 from a principle articulated by Izzy after five CI cycles on PR #300 + PR #315 (`monorepo-incluir`, surveys-v1 lifecycle epic, 2026-05-20/21). The 5-cycle progression in §3 is the source material; Izzy's pattern recognition across those cycles is what earned this skill its slot in the shared library.

Promotion trigger: 5 instances of the same misread ("same line = same bug") in one feature epic — single-provenance, but five instances within one debug arc clears the recurrence bar for promotion to a shared skill rather than an agent-local note.
