---
name: walk-the-route
description: A triage discipline — when persistent UI symptoms survive 2+ frontend fixes, the bug is upstream of the frontend. Walk the backend route the page actually calls (its middleware gates, parameters, and response shape) instead of trying another frontend fix. Use any time you've shipped multiple frontend fixes for the same customer-facing symptom and the symptom persists; any time a UI surface shows wrong-shaped data after a backend route migration; any time you're about to try "one more frontend tweak." Triggers on persistent UI symptom after fix, "screenshot still shows," "tried another frontend fix," wrong data from API, FE/BE contract mismatch, case-mismatch lastLogin / last_login, wrong endpoint scope, requireAdmin vs requireVolunteer, snake_case / camelCase translation, mapUser, "expected 298 saw 3," operator demanded verify-against-reality.
---

# Walk The Route

A triage discipline for when frontend fixes aren't moving a customer-facing symptom. The rule is short, the action is concrete, and the discipline test (a 3-step verify-against-reality pass) tells you when you've earned the right to pivot upstream.

This skill is the **triage-direction sibling** to `aperture:surface-fetch-errors`. That skill says: at the wrapper layer, preserve the error/empty distinction so the FE doesn't silently mis-render. This skill says: when symptoms persist despite the wrapper appearing to work, walk the route the page actually calls — the bug is in the data shape or auth scope flowing in, not in the operations operating on it.

It's also `aperture:specialist-delegation §6`'s verify-against-reality principle, applied at a specific decision point: **before you ship another frontend fix, prove the FE math is correct; if it is, pivot upstream.**

---

## The rule

> **When persistent UI symptoms survive 2+ frontend fixes, the bug is upstream of the frontend. Walk the backend route the page actually calls — its middleware gates, parameters, and response shape — instead of trying another frontend fix.**

Two follow-ons that make this concrete:

1. **Backend sends data that doesn't match the frontend's assumption.** This is the common-shape failure mode. Look for: (a) wrong endpoint scope (auth gates that don't match page intent), (b) field-name casing mismatch (snake_case vs camelCase translation drift), (c) response shape change without the FE mapper being updated to translate.
2. **The discipline test (below) is the precondition for pivoting.** You don't pivot upstream until you've proven the symptom isn't from a frontend bug you missed.

---

## The discipline test (3 steps — apply BEFORE pivoting)

Before declaring "the bug is upstream," prove the frontend math is correct:

1. **Pull a real prod row** — use `aperture:incluir-prod-postgres` or equivalent. Pick a row that should produce the visible symptom (e.g. an attendance row from a real volunteer; a user record from the operator's account).
2. **Run the frontend helpers/mappers/transforms against it in a node REPL** — every helper that touches the data on its way to render. Date-key builders, casing translators, sort comparators, aggregation reducers.
3. **Confirm the helpers output what's expected.** If they do, frontend math is right. If they don't, you've found the FE bug and you're not actually at the "go upstream" trigger yet.

If the math is right → pivot upstream, walk the route. If the math is wrong → keep fixing the frontend; you misdiagnosed the trigger condition.

This 3-step verification is the *proof of readiness* for the pivot. Without it, "go upstream" is just guessing. With it, you've ruled out the layer you can ship to fastest, so the higher-cost upstream investigation is genuinely warranted.

---

## Two banked provenances (both 2026-05-21, Vance)

### Provenance 1 — `aperture-ftuy` Estatísticas tab on /presencas (umbrella `aperture-q0rw`)

The visible symptom: operator screenshot shows **3 sessions** on the Estatísticas tab. Expected: **~298** (letivo-day records aggregated across 73 volunteers).

**Two frontend fixes shipped, neither moved the number:**

1. **PR #314 (`aperture-hams`)** — TZ normalization to São Paulo across the date-key path. Operator screenshot still showed 3.
2. **PR #318 (`aperture-presencas-regression-fix`)** — Split the date-key helpers (string-slice for `semester_dates` stored-as-intended-local-date; Intl SP TZ for real `check_in` timestamps). Operator screenshot STILL showed 3.

**Operator demanded verify-against-reality.** Vance pulled a real `check_in` row + matching `semester_date` row from prod, ran both helpers in a node REPL, and confirmed they produced matching keys (`'2026-03-21'` on both sides). The date-key math was correct.

**Walked the backend route.** `GET /api/volunteer-attendance` is `requireVolunteer`-gated and returns **caller's own attendance only**. The operator was seeing their personal 3 records — not the 298 letivo-day aggregate across all 73 volunteers the page was supposed to render. **The bug is the wrong endpoint** — wrong auth scope — not the date math.

The two frontend fixes were correct fixes for real frontend issues, but neither addressed the actual symptom because the bug was one layer up. Without walking the route, a third FE fix would have shipped and the number would still have been 3.

### Provenance 2 — `aperture-eefk` Quarentena tab on the same page

The visible symptom: all 90 volunteers show **"Nunca acessou."** Expected: 13 quarantine candidates surfaced (per the DB query verified during recon).

**Walked the backend route.** `PR #310` migrated `/api/users` to direct-Kysely returning `last_login` (snake_case). The frontend `mapUser` function reads `u.lastLogin` (camelCase). The translation layer is the FE mapper — and it's not translating. `u.lastLogin` is always `undefined`, so the "never logged in" branch fires for every user.

A route migration changed the response shape; the FE mapper wasn't updated to match. **Contract mismatch at the FE/BE boundary** — same family as Provenance 1, different specific shape (casing translation vs auth scope).

### Common shape across both

Both are **FE/BE contract mismatches.** Vance's pattern:

- The symptom is wrong-shaped data on a page
- Frontend helpers/mappers are math-correct (verified via the discipline test, or known from code review)
- The backend sends data that doesn't match what the FE assumed
- Walking the route surfaces the mismatch — either the auth gate doesn't match the page intent (Provenance 1) or the response shape changed without the mapper following (Provenance 2)

---

## Forward-friction check (apply at fix-design time, not just at triage time)

When you're about to ship a fix to a customer-facing UI symptom, ask:

1. **Have I (or anyone) shipped a frontend fix for this same symptom recently?** If yes — count the strikes.
2. **If this is strike 2 or 3** — STOP. Don't ship another FE fix yet. Run the discipline test instead.
3. **After the discipline test:** if math is right → walk the route. If math is wrong → fix the FE math you just found, and you're back to strike 0.
4. **At strike 1** — fix away. Sometimes the bug really is in the frontend, and one FE fix is the right call. The discipline only kicks in when the same symptom survives multiple fixes.

The two-strike threshold is the "after this many shipped fixes that didn't move the symptom, the bug is upstream." Two is a heuristic; one is too eager (sometimes one FE fix is enough), three is too late (you've shipped two PRs that don't move the customer-facing number, and the customer is still looking at 3 sessions).

---

## What this skill is NOT for

- **NOT** a directive to skip frontend fixes. Most UI bugs ARE frontend bugs. The discipline only applies AFTER 2+ frontend fixes have failed to move the symptom — i.e. when the trigger condition is met.
- **NOT** a license to skip the discipline test before pivoting. "Symptom persists, ergo go upstream" is wrong if the FE math is actually broken in a way you haven't found. Always prove the math before pivoting.
- **NOT** advice to immediately rewrite the backend route. Walking the route is the INVESTIGATION step — read the middleware gates, parameters, response shape. Compare against the FE's assumption. Identify the mismatch. Fix the smaller side (usually the FE mapper if the BE is canonical, or update the BE if the route's contract is wrong for the page using it).

---

## Source provenance

| Bead | PR | What was assumed | What was true | Mismatch type |
|---|---|---|---|---|
| `aperture-ftuy` (under umbrella `aperture-q0rw`) | monorepo-incluir #314 + #318 (both failed); fix not yet shipped at skill-bank time | `/api/volunteer-attendance` returns letivo-day aggregate across all volunteers | Returns caller's own attendance only (requireVolunteer-gated) | Auth-scope mismatch (wrong endpoint for page intent) |
| `aperture-eefk` | (recon — bug not fixed yet at skill-bank time) | `/api/users` response uses `lastLogin` camelCase | After PR #310 migration, returns `last_login` snake_case | Field-name casing translation drift |

Both 2026-05-21. Two-provenance ship (ready for third instance to land in the "Adding a new precedent" scaffold below, which would bring this skill to 3-recurrence completeness per `aperture-4la6`'s heuristic).

**Class-diagnosis credit: Vance.** She shipped both failed frontend fixes on Provenance 1, was the one operator pushback landed on, ran the discipline test (REPL helpers + prod row), and walked the route to find the wrong-endpoint contract mismatch. Then surfaced Provenance 2 on the same day via the same triage move (walked the /api/users route, found the casing-drift mapper issue). GLaDOS-greenlit the skill bank; Vance dispatched to Atlas. The discipline IS Vance's banked move from this work.

---

## Cross-links

- **`aperture:specialist-delegation §6`** (Cipher's `verify-against-reality` principle) — this skill IS verify-against-reality applied at a specific decision point: the FE math is the "external state" you check before pivoting upstream.
- **`aperture:surface-fetch-errors`** (PR #17) — sibling on FE/BE contract integrity. That skill says preserve the error/empty distinction at the wrapper. This one says when the wrapper appears to work but data shape is wrong, walk the route — the contract has drifted.
- **`aperture:observability-as-evidence`** (PR #16) — sibling on layer-truth. That skill says don't reason cross-subsystem from enrichment. This skill says don't keep fixing layer N when the bug is in layer N-1.
- **`aperture:e2e-catches-what-lower-cant`** (PR #13) — adjacent on apparatus-vs-reality. That skill says lower tests bypass the failure surface. This skill says lower fixes don't move the upstream symptom. Both are about "the layer you're operating in isn't the layer where the bug lives."
- **Two-strike pivot rule** — conceptual parent. Vance referenced this in the dispatch as the umbrella principle for "after N attempts of approach A, pivot to approach B." `walk-the-route` is its specific instantiation for FE-vs-BE triage. If two-strike gets banked as its own skill, this skill becomes its concrete application example.

---

## Adding a new precedent

If you (or another agent) hit a UI symptom that survives 2+ FE fixes and walking the route surfaces the upstream cause, bank it here. Use the same template:

1. **Bead + PR** — citation
2. **The visible symptom** — what the user / operator saw on the page
3. **Frontend fixes shipped that didn't move it** — list each with PR and what it fixed
4. **The discipline test outcome** — pulled prod row + ran helpers in REPL; math right/wrong
5. **Walking the route** — what middleware gates / params / response shape you found
6. **The mismatch type** — auth scope, casing, shape, missing field, …

A third concrete instance promotes this skill from 2-recurrence to 3-recurrence under `aperture-4la6`'s heuristic. The discipline shape is consistent; the precedents are the swarm's catalog of "ways the FE/BE contract can silently drift."
