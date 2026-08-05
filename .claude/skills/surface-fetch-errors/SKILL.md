---
name: surface-fetch-errors
description: Server Actions and fetch wrappers that catch a network/API failure must surface the error — never silently fall back to empty/default values. Empty-state UI means "API said no data", not "API errored". Use when writing or reviewing try/catch around fetch, Server Action result shapes, or empty-state copy. Triggers on empty-array fallback, `return []`, `Nenhum X encontrado`, `result.success`, silent error swallowing, deceptive empty state, tagged-union result, `error.tsx`, error toast on fetch failure.
---

# Surface Fetch Errors

A discipline for layers that wrap fetch / API calls: **errors and empty are not the same thing, and the difference must be preserved from the wrapper through the page to the user.** If you collapse them — by catching the error and returning `[]` / `null` / `{}` so the page can "just keep rendering" — you produce a UI surface that lies about reality. Sometimes the lie is passive ("no data shown when API broke"). Sometimes it's active ("Notas salvas com sucesso!" / "Nenhum em quarentena. Todos os voluntários acessaram o sistema nos últimos 2 meses."). Both are bugs. The active form is the worst form.

The principle is the sibling of `aperture:observability-as-evidence`. That skill says: don't reason about subsystem A from a span enrichment that represents subsystem B. This skill says: don't conclude "no data" from an action result that represents "fetch errored." Both are about **not trusting the wrong layer's signal as ground truth.** Read both as a pair.

---

## The rule

> **Distinguish "API said no data" from "API errored." Preserve the distinction through every layer. Empty-state UI ("Nenhum X encontrado") renders ONLY when the result is `ok: true && data.length === 0`. When the result is an error, render an error surface — never the empty-state surface, and never a misleading success message.**

Concretely:

1. The fetch wrapper must return a tagged-union result, not silently fall back to a default value.
2. The page-level handler must branch on the error case explicitly.
3. The empty-state UI copy must NOT make a positive claim about reality — it should describe the absence of records, not assert that everyone/everything is in some healthy state.

If you can't tell from the result whether the API errored or returned an empty list, you've already lost. Fix the result shape first; everything downstream depends on that distinction surviving.

---

## The anti-pattern (with code)

The shape that produces the bug — common across the Aperture codebase, will be visible in any `apps/*/src/actions/*.ts` audit:

```typescript
// ❌ Anti-pattern — empty-array fallback inside the try/catch
async function getThings(): Promise<ThingsResult> {
  try {
    const result = await honoGet<{ things: Thing[] }>('/api/things');
    return { success: true, data: result.things };
  } catch (error) {
    console.error('getThings error:', error);
    return { success: false, error: 'Erro ao buscar things' };
  }
}

// Page-level handler:
const things = result.success && Array.isArray(result.data) ? result.data : [];
//                                                                      ^^
//                       The empty array silently swallows the error case.
//                       Page renders as if API returned no things.
//                       Error logged server-side (where nobody reads it),
//                       discarded client-side.
```

Three things go wrong here:

1. **The error becomes empty.** The page's `things.length === 0` check can no longer tell the difference between "API said zero records" and "API blew up."
2. **The error is logged server-side only.** `console.error` in a Next.js Server Action writes to the server log, not to anything the client can render. The user never sees it.
3. **The empty-state UI fires regardless.** Whatever copy lives in the "no things" branch — `"Nenhum thing encontrado"`, or worse, a positive claim — will render even when the fetch failed.

---

## The correct shape (with code)

```typescript
// ✅ Tagged-union result preserves the distinction
type Result<T> =
  | { ok: true; data: T }
  | { ok: false; error: string; status?: number };

async function getThings(): Promise<Result<Thing[]>> {
  try {
    const result = await honoGet<{ things: Thing[] }>('/api/things');
    return { ok: true, data: result.things };
  } catch (error) {
    return {
      ok: false,
      error: extractErrorMessage(error),
      status: extractStatus(error),
    };
  }
}

// Page-level handler MUST branch on error first:
const thingsResult = await getThings();

if (!thingsResult.ok) {
  // Pick one of these — any of them is correct:
  //   - throw and let error.tsx render the error UI
  //   - return <ErrorState error={thingsResult.error} retry={...} />
  //   - redirect to a "service unavailable" page
  // What's NOT correct: silently render empty-state UI.
  return <ErrorState error={thingsResult.error} status={thingsResult.status} />;
}

const things = thingsResult.data;
// Now the empty-state UI is honest:
if (things.length === 0) {
  return <EmptyState message="Nenhum thing encontrado." />;
}
return <ThingsList things={things} />;
```

The shape preserves the error/empty distinction at every layer:

- The wrapper returns `{ ok: false, error, status }` on failure, not `[]`.
- The page checks `!result.ok` BEFORE checking `data.length`.
- The empty-state UI only renders when `result.ok && data.length === 0` — i.e. the API actually said "no records."

---

## Four anti-patterns to enumerate (catch them at review time)

| Anti-pattern | What it produces | Fix |
|---|---|---|
| Silent `try/catch` with empty-array fallback | Page renders empty-state UI on error | Return tagged-union result; branch on `ok` |
| Error logged server-side only, no client surfacing | User sees broken UI with no error signal | Return the error in the result; render an error UI on the client |
| Empty-state UI that doesn't branch on error vs empty | "Nenhum X" fires regardless of why the data is missing | Page-level `if (!result.ok)` before the empty-state branch |
| **Deceptive empty-state copy** (positive claim variant) | "Notas salvas com sucesso," "Todos os voluntários acessaram o sistema nos últimos 2 meses" — UI makes a POSITIVE assertion that's the inverse of reality | (a) Fix the result shape so the error branch can render something else; (b) Even on the empty branch, prefer factual ("Nenhum X encontrado") over claims ("Todos os X estão Y") |

### The deceptive-copy variant deserves its own paragraph

The worst form of this pattern isn't "page hides the error." It's **the page makes a positive claim that's the inverse of reality.**

Two banked instances, both from 2026-05-20:

- `aperture-ycko` (grade-save): backend silently catches `DuplicateGradeError`, frontend toast displays the request-size count — *"Notas salvas com sucesso! 1 aluno(s) atualizado(s)."* DB had zero new rows. The teacher walks away believing the grade was saved.
- `aperture-qz4b` (Quarentena tab): `volunteerData=[]` (because `/api/users` returned 403 silently), the empty-state copy reads *"Nenhum voluntário em quarentena. Todos os voluntários acessaram o sistema nos últimos 2 meses."* DB has 13 quarantine candidates. The admin walks away believing the team is healthy.

The active-falsehood variant is more dangerous than passive silence because **the user takes action based on the false claim.** The teacher won't re-enter the grade. The admin won't investigate why nobody's been flagged. The lie compounds.

**The rule for empty-state copy:** describe *the absence of records* ("Nenhum X encontrado"), not *the state of the system* ("Todos os X estão Y"). Never claim a positive property of the data that you couldn't verify from an empty result.

---

## Three concrete provenances (3/3, all 2026-05-20)

### Provenance 1 — `aperture-ycko` (grade-save silent data loss)

Frontend caught the action result, displayed a request-size count in a toast, didn't call `router.refresh()`. Backend additionally swallowed `DuplicateGradeError` in a silent `try/catch`. Operator-reported P1. The toast read *"Notas salvas com sucesso! 1 aluno(s) atualizado(s)"* while the DB had zero new rows. Phase 1 fix (PR #304) made the failure VISIBLE; Phase 2 (PR #305) switched the backend to UPSERT to make it WORK. The skill banks the test-time discipline that would have caught the lying toast pre-merge.

### Provenance 2 — `aperture-kzw0` → `c0qt` (/presencas zero-data)

Frontend Server Actions caught 404 + 403 from real API failures, returned `[]`, page rendered misleading empty states across all 3 tabs of the `/presencas` admin surface. Closed as duplicate of `c0qt` for the primary case; the secondary 403 finding split out to `qz4b`.

### Provenance 3 — `aperture-qz4b` (Quarentena tab deceptive empty state)

When `/api/users` returned 403 silently (`requireAdmin()` gate vs gestão-de-pessoas user — see `observability-as-evidence` for the related authz layer confusion), the volunteerData fetch fell back to `[]`. The Quarentena tab's empty-state copy: *"Nenhum voluntário em quarentena. Todos os voluntários acessaram o sistema nos últimos 2 meses."* DB had 13 quarantine candidates verified during recon. The single worst empty-state-copy example in the banked set — a positive claim that's the exact inverse of the truth.

All three landed within the same day (2026-05-20), promoting the candidate from 2/3 to 3/3 under aperture-4la6's recurrence heuristic. Same-day promotion was unusual; the pattern is identical enough across instances that GLaDOS dispatched the skill immediately rather than wait for a 4th hit.

---

## Forward-friction check (apply at action-design time + at review time)

Before you ship a fetch wrapper, action, or page handler:

1. **Does my fetch wrapper distinguish error from empty in its return value?** (Tagged-union, separate fields, anything that the caller can branch on.) If it collapses both into a default value, that's the bug.
2. **Does my page-level handler branch on the error case explicitly, BEFORE checking the data length?** If the first thing the page does is `data.length === 0 ? <EmptyState /> : ...`, the error case is invisible.
3. **Does my empty-state UI fire only when the API actually said "no records"?** Not "I don't have data." Not "the fetch result is falsy." Specifically `ok: true && data.length === 0`.
4. **Does my empty-state copy describe the absence, or claim a positive state?** "Nenhum X encontrado" describes; "Todos os X estão Y" claims. Prefer the descriptive form. Audit any positive-claim copy as a potential deceptive-empty-state instance.

If you find yourself writing `return []` inside a `catch` block, stop. That's the bug; everything downstream inherits it.

---

## What this skill is NOT for

- **NOT** a rule against `try/catch` in fetch wrappers. The wrapper SHOULD catch — it just needs to preserve the error in the return shape, not swallow it.
- **NOT** a directive to throw raw errors out of Server Actions. Tagged-union results are fine; `error.tsx` boundaries are fine; redirect-on-error is fine. Any approach that lets the page-level handler differentiate error from empty works.
- **NOT** about replacing every empty state with an error state. Real empty results (API said zero records) DO need the empty-state UI. The discipline is about the distinction, not about hiding empty states.
- **NOT** a rule against logging errors server-side. Continue logging — just don't let the log be the ONLY surface; the user-facing UI needs to surface it too.

---

## Cross-link with `observability-as-evidence`

The two skills are sibling disciplines. Same family: *don't trust layer-N signals as ground truth.*

- `observability-as-evidence`: don't reason about subsystem A's behaviour from a span enrichment that represents subsystem B's worldview
- `surface-fetch-errors`: don't conclude "no data" from an action result that represents "fetch errored"

Both warn against the same family of errors — treating one layer's output as authoritative for a question that lives in a different layer. Bidirectional cross-link is being added to observability-as-evidence in a follow-up amend so the pair is discoverable from either entry.

---

## Source provenance

| Bead | PR | Agent | Failure mode |
|---|---|---|---|
| `aperture-ycko` | monorepo-incluir #304 (Phase 1 fix) | Rex (backend) + Vance (frontend) | Toast lies about save success while DB unchanged |
| `aperture-kzw0` / `aperture-c0qt` | (recon — bug not fixed yet at skill-bank time) | Wheatley (recon) | Server Actions silently catch 404+403, return `[]` |
| `aperture-qz4b` | (recon — bug not fixed yet at skill-bank time) | Wheatley (recon) | Quarentena tab makes inverse claim when /api/users 403s |

**Class-diagnosis credit: Wheatley.** He filed the original watch-list candidate (under aperture-4la6) at 2/3, then surfaced the third provenance during the qz4b /presencas recon and dispatched the skill bank. The meta-move (filing a watch-list candidate at 2/3, then catching the third instance in his own recon work) is exactly the discipline aperture-4la6's promotion heuristic was designed to enable.

---

## Adding a new anti-pattern variant

If you hit a new shape of error-as-empty collapse, bank it here. Same template as the four anti-patterns above:

1. **Anti-pattern name** — what shape the code takes
2. **What it produces** — the broken UX
3. **Fix** — what the right shape is

Open a PR on the aperture repo with the new variant. The rule's umbrella (preserve the error/empty distinction) stays one sentence; the variants are the swarm's catalog of "ways this distinction can collapse."

---

## Follow-up: codebase audit (filed separately)

Wheatley flagged at dispatch time that a codebase audit of `apps/*/src/actions/*.ts` across the incluir monorepo will likely surface 10+ existing instances of the anti-pattern. The audit is filed as a separate task — bank the discipline first, then sweep the existing instances against it.
