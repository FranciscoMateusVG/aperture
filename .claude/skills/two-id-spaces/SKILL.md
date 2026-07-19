---
name: two-id-spaces
description: In incluir's data model, a volunteer has TWO identifiers — `users.id` (auth/session identity, BetterAuth) and `volunteers.id` (program participation PK). API contracts MUST declare which ID space they expect at every boundary. Frontends generally have `user.id` from session; `volunteer.id` requires a lookup. Use any time you're designing or consuming an API contract whose parameters, response fields, or join semantics could plausibly reference either user-auth identity OR volunteer-domain identity. Triggers on `users.id`, `volunteers.id`, `user_id`, `volunteerId`, BetterAuth session, volunteers join, "ID-space mismatch", `/api/users` vs `/api/volunteers`, `lastLogin` vs `last_attendance`, FE sends user.id BE expects volunteer.id, 404 from POST body, "wrong scope".
---

# Two ID Spaces

A discipline for incluir API contracts. The rule is one sentence; the failure mode appeared three times in one day; the fix is a small explicitness obligation at every contract boundary that crosses the user-auth / volunteer-domain divide.

This skill is the **identity-layer companion** to the FE/BE contract-integrity mesh (`aperture:walk-the-route` / `aperture:name-the-blast-radius` / `aperture:surface-fetch-errors`). Those skills protect against different layers of contract drift (debug-time / author-time / runtime collapse); this one protects against a specific *kind* of contract drift — the wrong identifier space crossing a route boundary.

---

## The rule

> **In incluir's data model, a volunteer has TWO identifiers — `users.id` (auth/session identity, BetterAuth-managed) and `volunteers.id` (program participation PK). API contracts MUST declare which ID space they expect at every boundary. Frontends generally have `user.id` from session; `volunteer.id` requires a lookup (typically `users.id` → `volunteers.user_id` → `volunteers.id`).**

The contract boundary is wherever an ID crosses the FE/BE wire: route path parameters, query string keys, POST/PATCH body fields, response payload structure, downstream joins inside the BE handler. At each of these, the consumer and the producer must agree on which space the ID belongs to.

---

## The incluir-specific identifier table

| ID | Where it lives | Where you get it | When to use |
|---|---|---|---|
| `users.id` | `users` table (BetterAuth-managed) | Session (`session.user.id`); the FE has this by default for any authenticated user | Auth gates, session lookups, BetterAuth-domain queries (last_login, role, email) |
| `volunteers.id` | `volunteers` table (program-participation PK) | Lookup: `volunteers.user_id = session.user.id → volunteers.id` | Program-participation joins, attendance records, audience matchers, volunteer-domain operations |

When an API contract crosses the boundary, declare which side it expects. **Default assumption is wrong** — both look like UUIDs, both come from the same database, and a typo or copy-paste between layers produces a 404 (best case) or silently-wrong data (worst case).

---

## Three banked provenances (all 2026-05-21)

### Provenance 1 — `aperture-ftuy` (Estatísticas /by-semester scope)

`GET /api/volunteer-attendance` was gated `requireVolunteer` and returned the caller's own attendance only — when the page expected the letivo-day aggregate across all 73 volunteers in the semester. The bug surfaced as "operator sees 3 sessions instead of 298." The root cause was that the route's auth gate + scope were calibrated for `users.id`-space (caller-only) when the page semantics demanded `volunteers.id`-space-aware aggregation (semester-wide).

The fix lived at the BE side (introduce `/by-semester/:id` that takes a semester ID and aggregates across the volunteer-domain). The discipline: at API design time, the route's response semantics should match the page's question — caller-scoped data has different identity assumptions than aggregate data.

### Provenance 2 — `aperture-eefk` (Quarentena `lastLogin` mapping)

`/api/users` migration in PR #310 (pdlq) switched from BetterAuth-shaped (camelCase) to direct-Kysely (snake_case). The Quarentena tab's `mapUser` function read `u.lastLogin` (camelCase); after migration the response shape became `last_login` (snake_case). Field always undefined; every volunteer rendered as "Nunca acessou."

The casing-translation drift is the surface symptom (and the immediate fix). The deeper observation: the page wants **volunteer-activity data** ("when did this volunteer last attend a session") but the API gives **user-auth data** ("when did this user last log in"). Even with the field name resolved, the conceptual identity is misaligned — `users.last_login` (BetterAuth's notion) is not the same as `volunteers.last_attendance` (program-participation's notion). The pdlq migration exposed a long-standing ID-space-layer confusion at the route-choice level.

### Provenance 3 — `aperture-kruq` (staff-on-behalf-of POST + DELETE/PATCH confusion)

Two distinct bugs in the same surface:

1. **POST handler treats `body.volunteerId` as a `volunteers` PK.** The frontend sends `user.id` (because that's what's in the session). 404 on every call.
2. **DELETE `/api/users` vs PATCH `/api/volunteers/inactivate`.** The destructive-vs-soft-delete distinction maps to the wrong ID space — DELETE on a user removes the auth identity (cascading consequences); PATCH on a volunteer marks the program participation inactive. Confusion in either direction blasts the wrong semantic surface.

Both bugs are the same shape: the FE's natural identifier (`user.id`) doesn't match the BE's expected identifier (`volunteers.id`), and the contract didn't declare which side it expected.

### Common shape across all three

- The FE has `user.id` from session; the BE expects `volunteer.id` (or vice versa)
- The contract boundary didn't make the expectation explicit
- The mismatch produces 404 (best case), wrong-scope data (medium), or silently-wrong data via mapper drift (worst)
- The fix is a small explicitness obligation at the contract: declare which ID space, lookup if you have the wrong one, document the contract in the PR body (`aperture:name-the-blast-radius`)

---

## Forward-friction check (apply at API-contract-design time)

Before you ship a route handler, fetch wrapper, or page-level fetcher:

1. **Identify every ID that crosses the boundary** — path params, query keys, body fields, response shape, join semantics inside the handler.
2. **For each ID, ask: which space?** `users.id` or `volunteers.id`? (Or a different domain ID — class.id, course.id, etc.)
3. **Compare against the caller's available data.** The FE typically has `session.user.id`. If the contract expects `volunteer.id`, either:
   - The BE handler resolves the lookup (`users.id` → `volunteers.user_id` → `volunteers.id`); document this in the route
   - OR the FE must do the lookup first (via a precursor `/api/volunteers/by-user` call); document this in the route + the FE
4. **Document the choice in the PR body** per `aperture:name-the-blast-radius` — the contract change is a runtime contract that reviewers + operators need to see.

If you find yourself defaulting to "it's a UUID, both columns have UUIDs, should be fine" — stop. The ID space matters; both UUID columns are functionally distinct primary keys. The fact that they LOOK the same is exactly what makes the bug class hard to catch.

---

## What this skill is NOT for

- **NOT** a rule against using `users.id` in any API contract. Most BetterAuth-domain APIs correctly use `users.id` — auth gates, session lookups, role checks. The discipline only applies when the contract crosses the user-domain / volunteer-domain boundary.
- **NOT** a rule that backends should always resolve the lookup. Resolving server-side has the right ergonomics for many cases (FE sends what it has, BE handles the translation), but sometimes the FE doing the lookup is the right shape (e.g., when subsequent operations also need the resolved ID — saves repeated round-trips). The discipline is "declare which side it expects," not "always resolve at the BE."
- **NOT** generalizable beyond identifiers that look alike but mean different things. The principle (be explicit about which identity space a contract crosses) IS general; the specific incluir-shape (`users.id` vs `volunteers.id`) is the banked case.

---

## Source provenance

| Bead | PR(s) | What was mismatched | How it surfaced |
|---|---|---|---|
| `aperture-ftuy` | monorepo-incluir #314/#318 (failed FE fixes), #323/#324 (correct fix pair) | Auth-scope: route returned caller-only when aggregate expected | Operator screenshot: 3 sessions instead of 298 |
| `aperture-eefk` | monorepo-incluir #310 (pdlq migration) | Response shape + identity layer: page wants volunteer-activity, route returns user-auth | All 90 volunteers showing "Nunca acessou" |
| `aperture-kruq` | (recon, fix not yet shipped) | POST body: FE sends user.id, BE expects volunteers.id; DELETE/PATCH on wrong identity surface | 404 on staff-on-behalf-of operations |

**Class-diagnosis credit (multi-agent shape):**
- **Vance** flagged all three provenances on respective beads during her /presencas FE work today
- **GLaDOS** named the categorical pattern + dispatched the bank

Same multi-agent class-diagnosis shape as `aperture:hold-the-dependent-merge` (PR #21): Vance brings provenances from hands-on work, GLaDOS names + dispatches. Banked as instance #2 of the "multi-agent class-diagnosis" anchor on `aperture-4la6`.

**3-recurrence ship** under `aperture-4la6`'s promotion-by-recurrence heuristic. Three independent provenances of the same root cause on the same day; the principle is categorical, the recurrence rate is high, banking immediately was the right call.

---

## Cross-links

- **`aperture:walk-the-route`** (PR #19) — debug-time sibling. That skill catches the FE/BE contract mismatch AFTER ship (operator screenshot → walk the route → find the mismatch). This skill prevents the specific case where the mismatch is in the ID space at API-design time.
- **`aperture:name-the-blast-radius`** (PR #20) — author-time sibling. PR body must disclose contract changes that cross ID spaces; this is one of the categories under that skill's `## Runtime contract changes` section.
- **`aperture:surface-fetch-errors`** (PR #17) — silent-collapse sibling. If the route returns the wrong ID space's data, the FE mapper may silently return undefined (the eefk shape) — that skill's discipline (preserve error vs empty) protects against the silent-collapse failure mode.
- **`aperture:hold-the-dependent-merge`** (PR #21) — adjacent on FE/BE contract integrity at merge time.
- **`aperture:observability-as-evidence`** (PR #16) — same family on layer-truth. That skill's banked war story (3hhp's `user.role=user` BetterAuth-enrichment vs `volunteer_permissions=['gestao_de_pessoas']` institutional-overlay confusion) is conceptually the same kind of confusion: BetterAuth user-layer signal mistaken for volunteer-layer truth. Different surface (span enrichment vs API contract), same family.
- **`aperture:specialist-delegation §6`** (verify-against-reality, Cipher's principle) — parent.

The composite-mesh property: `two-id-spaces` + `walk-the-route` together protect the full life-cycle of the ID-space failure mode — author-time (this skill) catches it before ship; debug-time (walk-the-route) catches it after ship if author-time missed.

---

## Adding a new precedent

If you (or another agent) hit a new instance of the two-ID-spaces failure mode, bank it here. Same template:

1. **Bead + PR(s)** — citation
2. **What was mismatched** — which ID space the FE had vs which the BE expected
3. **How it surfaced** — 404, wrong-scope data, silent-mapper-undefined, …
4. **Where the contract didn't declare** — path param? body field? response shape? join semantics?
5. **Fix shape** — BE resolves lookup? FE does precursor call? Route signature change?

The principle holds at 3-recurrence saturation today. The precedents are the swarm's catalog of "ways the user.id ↔ volunteer.id confusion can ride into a PR."

Future work: if a similar shape surfaces with a different pair of ID spaces (e.g., `students.id` vs `users.id`, or `enrollment.id` vs `class.id`), file as a sibling skill or extend this one — the principle generalizes, the specific incluir-domain table grows.
