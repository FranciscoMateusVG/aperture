---
name: name-the-blast-radius
description: When a PR adds OR changes a production-runtime contract — auth matcher, response shape, API endpoint, dispatch protocol, middleware behaviour, audience matcher, anything that affects how the system behaves at runtime in prod — the PR body must explicitly name the change. Reviewers and operators should be able to read the PR body and know what production capabilities changed, without diffing files. Use any time you're authoring a PR description, reviewing a PR, or noticing that the diff contains more "secondary" changes than the PR body discloses. Triggers on PR-body authoring, runtime-contract change, response-shape migration, snake_case / camelCase translation, new audience matcher, new auth matcher, middleware change, dispatch protocol, undisclosed migration, silent shape change, "the PR body didn't mention X."
---

# Name the Blast Radius

A discipline for PR authors: **if the diff changes a production-runtime contract, the PR body must name it.** Reviewers should read the PR description and know — without opening the diff — what production capabilities are different after merge. Operators should be able to grep merged-PR descriptions and find the moment any contract changed.

The shape this protects against: the PR's primary work is disclosed and well-reviewed; a *secondary* runtime change rides along in the same diff, not mentioned in the body, ships unreviewed for the property that matters most. The PR appears to be about X; it's actually about X *and* Y, where Y is the part with the blast radius.

This skill sits in the same family as `aperture:verify-against-reality` (in `specialist-delegation §6`) — Cipher's principle of checking code against external state. Verify-against-reality is the code-vs-prod check at the END of implementation. This skill is the **diff-vs-PR-body check at the END of authoring** — the same discipline applied to the human-readable artifact reviewers depend on.

---

## The rule

> **When a PR adds OR changes a production-runtime contract, the PR body must explicitly name that change. Reviewers and operators should be able to read the PR body and know what production capabilities changed — without diffing the files themselves.**

If the diff contains it and the PR body doesn't name it, the contract change ships unreviewed.

---

## What counts as "production-runtime contract"

The list below is the enumerated set of things that DEMAND PR-body disclosure. If your diff touches any of these, name it in the body:

| Category | Examples |
|---|---|
| **Auth matcher / gate** | New `requireRole()` gate, widened `requireAdmin` to also accept `secretaria`, new permission check, new audience matcher (e.g. `user_id:<uuid>`) |
| **Response shape** | Field rename (`lastLogin` → `last_login`), nesting change, new/removed field, type change (`string` → `string \| null`), envelope shape migration |
| **API endpoint** | New route, removed route, route path change, method change (`GET` → `POST`), pagination contract change |
| **Dispatch protocol** | Message format change, queue routing change, event-payload shape change, webhook contract change |
| **Middleware behaviour** | New global handler, behaviour change in an existing handler (logging, rate-limit, redirect, error transform), order change in middleware chain |
| **DB contract** | New column with non-trivial semantics, FK change, RLS policy change, schema migration that consumers depend on |
| **Feature-flag default** | Default value change, new flag introduced, flag retirement (existing-consumer impact) |
| **External-service contract** | New external API consumed, payload change to existing external call, auth scheme change to a third-party integration |

If the change is in any of these categories — name it. If you're unsure whether your change qualifies, name it anyway. The cost of over-disclosure is one extra paragraph; the cost of under-disclosure is the blast-radius examples below.

---

## Two banked provenances

### Provenance 1 — PR #315 (Vance, surveys macro isolation), 2026-05-19

**The PR body said:** audience-flush + nav-overlay scope work for the surveys macro isolation feature.

**The diff also contained:** a new production-runtime audience matcher — `user_id:<uuid>` — adding a new way to target individual users via the audience expression system. This is a NEW runtime capability; the audience-eval engine gained a new matcher class.

**What happened:** Cipher caught the new matcher in review and filed an audit-log follow-up so that any use of `user_id:<uuid>` in production gets logged. Without that catch, the matcher would have shipped as **undisclosed runtime capability** — operators would have no idea the audience system could target individual users; security review would have missed the new matcher entirely; the audit-log gap would have shipped as a silent compliance hole.

**The discipline that should have fired at author time:** *"my diff adds a new audience matcher — that's a runtime-contract change. The PR body must name it."*

### Provenance 2 — PR #310 (Rex, pdlq auth-gate widening), 2026-05-20

**The PR body said:** auth gate widening on the pdlq surface, with extensive discussion of the auth changes.

**The diff also contained:** a migration of `/api/users` response shape from BetterAuth-shaped (camelCase: `lastLogin`, `firstName`, etc.) to direct-Kysely (snake_case: `last_login`, `first_name`). This is a RESPONSE SHAPE migration — every consumer of `/api/users` now receives differently-cased keys.

**What happened:** the shape change shipped unreviewed for the property that mattered. Cipher's cross-cut sweep (filed as `aperture-n9gn` P1) found the undisclosed migration broke **5 apps × 6+ consumers** — frontend mappers across the codebase still reading the old camelCase keys, all silently returning `undefined`, all producing wrong-state UI. The Quarentena "Nunca acessou" bug banked in `aperture:walk-the-route` is the visible tip of that wider iceberg; the rest of the fanout was only surfaced because Cipher walked all callers of the changed route after spotting the first symptom.

**The discipline that should have fired at author time:** *"my diff changes the response shape from camelCase to snake_case — that's a runtime-contract change. The PR body must name it."*

### Common shape across both

In both cases:

- The PR's *primary* work was disclosed and reviewed thoroughly.
- A *secondary* runtime-contract change rode along in the same diff.
- The body didn't name the secondary change.
- The secondary change is what created the blast radius — either a silent capability (P1's new audience matcher) or a cross-cut shape migration (P2's 5 contract-mismatch bugs).

The two cases are different specific shapes but the same authoring failure: **the diff was larger than the PR body claimed.**

---

## The PR-body disclosure template

Once you've identified a runtime-contract change in your diff, name it explicitly. The template:

```markdown
## Summary
<the primary work — same as you'd write today>

## Runtime contract changes
- **<change name>** — <one-sentence description of what's different at runtime>
  - **Category:** auth matcher / response shape / endpoint / dispatch / middleware / DB / flag / external (pick one)
  - **Consumers affected:** <which code paths or external systems need to know>
  - **Migration plan:** <how downstream consumers handle the change — or "none, behaviour is purely additive">
```

If your PR has multiple runtime-contract changes, list each one separately. Each row is a few lines of disclosure that gives reviewers and operators visibility into exactly what's different after merge.

If the section reads empty — i.e. you have no runtime-contract changes to disclose — omit it entirely. Don't write `## Runtime contract changes` followed by `None`; just leave it out. The presence of the section signals that there IS a change to know about.

---

## Forward-friction check (apply at PR-author time, BEFORE opening the PR)

Before you hit "Create pull request":

1. **Walk your diff one more time.** Not the message you're about to commit — the actual `git diff`.
2. **For each changed file, ask: does this touch any of the "production-runtime contract" categories above?** If yes, the change must be named in the PR body.
3. **Compare your PR body draft against your answers in step 2.** Anything in your diff that qualifies as a contract change should be either (a) named in the Summary as the primary work, or (b) called out in a `## Runtime contract changes` section.
4. **If you find a gap** — add the disclosure. Don't open the PR until the body matches the diff.

The cost is 30 seconds to 5 minutes per PR. The cost of skipping it is what the two banked provenances show: silent capabilities, cross-cut bug waves, P1 incidents that traced to "the PR body didn't mention X."

---

## What this skill is NOT for

- **NOT** a directive to bloat every PR description. If your PR doesn't change a runtime contract, the disclosure section doesn't exist. The skill is about *naming changes that exist*, not adding ceremony to PRs that don't have them.
- **NOT** a license to mix unrelated contract changes into one PR because "I'll just list them all." Multiple contract changes in one PR is still bad practice — split them if you can. The disclosure rule applies regardless; smaller PRs make the disclosure both easier and more reviewable.
- **NOT** a replacement for the diff itself. Reviewers still read the diff. The discipline is that the PR body should let them know *what to look for* in the diff — not let the diff carry the burden of being the only signal.
- **NOT** about commit messages. Commit messages are a separate discipline. The PR body is the reviewer-facing artifact; that's the surface this skill protects.

---

## Source provenance

| Bead | PR | What was disclosed | What was NOT disclosed | Blast radius |
|---|---|---|---|---|
| (no dedicated bead — referenced from Cipher's review thread) | monorepo-incluir #315 (Vance) | Audience-flush + nav-overlay scope work | New `user_id:<uuid>` audience matcher (runtime capability) | Would have shipped as silent capability without Cipher's catch + audit-log gap |
| `aperture-n9gn` | monorepo-incluir #310 (Rex) | Auth-gate widening (extensive discussion) | `/api/users` response shape migration camelCase → snake_case | 5 apps × 6+ consumers broken (Cipher's cross-cut sweep); Quarentena "Nunca acessou" is the visible tip |

**Class-diagnosis credit: Cipher.** She flagged both instances during review — first on #315 (caught the user_id macro and demanded the audit-log follow-up), then on #310 (caught the silent shape migration during cross-cut analysis). The pattern she named — "PR adds production-runtime capability without PR-body disclosure" — is the principle this skill banks. GLaDOS dispatched the bank after the second instance.

**Two-provenance ship** under the single-provenance-for-categorical-principles trigger from `aperture-4la6`. The principle is categorical enough (one sentence, universal applicability to every PR) that single-provenance would have justified the bank; two provenances within 48h is the stronger evidence. The "Adding a new precedent" scaffold below is for the third instance, which would promote this to 3-recurrence completeness.

---

## Cross-links

- **`aperture:specialist-delegation §6`** (Cipher's verify-against-reality principle) — this skill IS verify-against-reality applied to the PR-body artifact: check the body against the actual diff before opening.
- **`aperture:walk-the-route`** (PR #19) — sibling on FE/BE contract integrity. That skill catches the undisclosed contract AFTER the bug ships (operator screenshot → walk the route → find the silent shape migration). This skill catches it BEFORE: the PR body should have named the shape change, the cross-cut bugs wouldn't have happened.
- **`aperture:surface-fetch-errors`** (PR #17), **`aperture:observability-as-evidence`** (PR #16), **`aperture:e2e-catches-what-lower-cant`** (PR #13) — all in the FE/BE/observability contract-integrity mesh. This skill is the AUTHORING-time guard; the others are debugging / reasoning / testing guards.
- **`aperture:spec-deviation-discipline`** (PR #11) — adjacent on "documented changes are part of the contract." Spec-deviation says: when you deviate from a written spec, document the deviation in 3 places. This skill says: when you change a runtime contract via a PR, document the change in the PR body. Both about preserving the chain of disclosure.

---

## Adding a new precedent

When you (or a reviewer) catch a PR that shipped a runtime-contract change without PR-body disclosure, bank it here. Same template:

1. **Bead + PR** — citation
2. **What was disclosed** — what the PR body claimed to be about
3. **What was NOT disclosed** — the runtime-contract change that wasn't named
4. **Blast radius** — what happened (or would have happened) because the change shipped without disclosure
5. **Category** — which row of the "what counts as production-runtime contract" table

A third concrete instance promotes this skill from 2-recurrence to 3-recurrence under `aperture-4la6`'s heuristic. The categorical principle holds regardless; precedents are the swarm's catalog of "ways the diff-vs-body gap can produce real production cost."
