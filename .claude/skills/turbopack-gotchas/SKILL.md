---
name: turbopack-gotchas
description: Turbopack vs tsc divergence gotchas — bundler-enforced constraints tsc accepts, so typecheck is green but `next build` fails. Use when writing or debugging Next.js code touching "use server", "use client", server actions, or server components. Triggers on `next build` errors that don't reproduce under `tsc --noEmit`, "Server Actions must be async functions", `Ecmascript file had an error`, server-action module organization.
---

# Turbopack Gotchas

Small bundler-enforced rules that bite. Each entry is one banked failure mode where **`tsc --noEmit` accepts the code but `next build` (Turbopack) refuses it** — plus the fix, plus a citation back to the PR where we banked it.

This skill exists because Turbopack enforces a stricter subset of "what's valid in this file" than the TypeScript compiler does. Type-clean code that passes CI's typecheck step can still fail the build step, and Turbopack's error messages often point at a **symptom location** that's far from the **root cause** at the top of the file — so the obvious debugging path (chase the type error in the function body) is wrong.

When you add a new gotcha, follow the same shape: **Symptom → Cause → Fix → Where this shows up → Source.** Cite the PR + commit that earned the lesson; future-you will want to read the diff.

---

## 1. `"use server"` module exports must ALL be async

**Symptom:** `next build` fails with:

```
Build error occurred
Ecmascript file had an error
    Server Actions must be async functions.
> 165 | export function parseStatusFilter(raw: string | undefined): SurveyStatus | undefined {
      |        ^^^^^^^^
```

The error line points at a **synchronous helper function declaration** somewhere inside the file (e.g. line 165), not at the `"use server"` directive on line 1. If you read the message at face value, the natural debugging path is "what's wrong with `parseStatusFilter`'s signature?" — chasing type issues that don't exist. Meanwhile `pnpm typecheck` (which runs `tsc --noEmit`) is **green** because tsc has no opinion about server-action semantics.

**Cause:** Next.js + Turbopack enforce that **every export from a module with a top-level `"use server"` directive is an async function** (because every export is treated as a server action callable from a client component). A sync helper sitting next to your `async function createSurvey(...)` exports breaks the contract, and Turbopack rejects the whole file at build time. `tsc` doesn't know about the `"use server"` semantics and happily compiles the sync function.

The error attribution (function signature line, not directive line) is the trap — Turbopack reports where the violation is, not the contextual reason WHY it's a violation. Without context, you debug the symptom site.

**Fix:** Extract the sync helper(s) into a **sibling non-directive module**. Keep the `"use server"` file purely for async server actions; put pure helpers, types, constants, and synchronous narrowers next door.

Before — single `"use server"` file with a sync narrower mixed in:

```ts
// apps/frontend/src/backend/surveys/adminSurveys.ts
"use server";

export const listSurveys = wrapServerAction(/* ... */);
export const createSurvey = wrapServerAction(/* ... */);
// ... more async server actions ...

// ⚠️ Build-time error: this is a sync export in a "use server" module
const STATUS_VALUES: readonly SurveyStatus[] = ["draft", "active", "closed"];

export function parseStatusFilter(raw: string | undefined): SurveyStatus | undefined {
  if (!raw) return undefined;
  return (STATUS_VALUES as readonly string[]).includes(raw)
    ? (raw as SurveyStatus)
    : undefined;
}
```

After — sync helper moved to a sibling module:

```ts
// apps/frontend/src/backend/surveys/adminSurveys.helpers.ts
// NOTE: no "use server" directive. Pure helpers for the UI.

import { type SurveyStatus } from "@repo/domains";

const STATUS_VALUES: readonly SurveyStatus[] = ["draft", "active", "closed"];

export function parseStatusFilter(
  raw: string | undefined,
): SurveyStatus | undefined {
  if (!raw) return undefined;
  return (STATUS_VALUES as readonly string[]).includes(raw)
    ? (raw as SurveyStatus)
    : undefined;
}
```

```ts
// apps/frontend/src/backend/surveys/adminSurveys.ts
"use server";

// Only async exports here.
export const listSurveys = wrapServerAction(/* ... */);
export const createSurvey = wrapServerAction(/* ... */);
// ...

// Note: parseStatusFilter (the synchronous URL-param narrower used by
// the list page) lives in adminSurveys.helpers.ts — a "use server"
// module may only export async functions (Turbopack enforces this).
```

```ts
// apps/frontend/src/app/home/admin/surveys/page.tsx
import { listSurveys } from "@/backend/surveys/adminSurveys";
import { parseStatusFilter } from "@/backend/surveys/adminSurveys.helpers";
```

**Where this shows up:** any time you co-locate URL-param narrowers, type guards, constants, or other sync utility code next to your server actions because they "belong with" that domain. Common offenders:

- `parseXxxFilter` / `parseXxxParam` URL narrowers used by the page that calls the server actions
- `STATUS_VALUES` / `ROLE_VALUES` / other readonly constant tables
- Sync zod schemas that the server actions reference (move to a shared schemas module)
- Type predicates (`isAdminSurvey`, `isActiveStatus`, …)
- Constant-derived utility functions (e.g. `displayLabelFor(status)`, `colorForRole(role)` — sync mapping lookups that read from a constant table) — same trap shape, same fix

The rule of thumb: **a `"use server"` file is for server actions and only server actions.** Anything that doesn't `await` something or perform a side effect on the server belongs in a sibling `*.helpers.ts` / `*.schema.ts` / `*.types.ts`.

**Forward-friction check (use this to prevent the bug from being authored in the first place):** when you're about to add a non-action export to a `"use server"` file, ask yourself —

> **Is this awaitable?**

If no, sibling module. Don't try to make it async to satisfy the rule (forcing `async function parseStatusFilter(...)` so the build passes is wrong — it pollutes the type signature for every caller and obscures the fact that this is a pure URL-param narrower). Extract instead.

**Note on the error trap:** the misleading line-number attribution is the part you'll see again. If you ever see `Server Actions must be async functions` pointing at what looks like a perfectly valid function declaration, **don't debug that line — look at the top of the file for `"use server"`** and move the sync export elsewhere.

**Source:** Vance, monorepo-incluir PR #289 (`aperture-26bp`), commit `41d28a0` — `fix(surveys): extract sync parseStatusFilter from "use server" module`. Files touched: `apps/frontend/src/backend/surveys/adminSurveys.ts` (removed), `apps/frontend/src/backend/surveys/adminSurveys.helpers.ts` (added, 20 LOC), `apps/frontend/src/app/home/admin/surveys/page.tsx` (import retarget).

---

## Why these gotchas are worth banking

`tsc --noEmit` and `next build` (Turbopack) check different invariants. CI runs typecheck first, so a code-clean PR often makes it through typecheck-gate before the build-gate catches the bundler-only violation — meaning the failure surfaces late, in a step where the error messages are tuned for runtime correctness rather than developer guidance. Every minute spent re-deriving "oh right, server actions must be async" is a minute the next agent doesn't have to spend.

When CI shows you a build failure on a function declaration you swear is fine, check this skill first.

---

## Adding a new gotcha

When Turbopack (or any Next.js bundler-side rule) bites you in a way that took >10 minutes to figure out and would bite someone else the same way, bank it here. Use the same shape:

1. **Symptom** — what the error message + line attribution actually look like, ideally with the misleading parts highlighted
2. **Cause** — the underlying bundler/runtime rule, and why tsc doesn't catch it
3. **Fix** — the actual code change, with before/after examples pulled from a real PR
4. **Where this shows up** — the pattern of code organization that triggers it
5. **Source** — agent name, PR + commit SHA, file(s) touched

Then open a PR on the aperture repo with the edit. Ping GLaDOS to bank it as a skill precedent.
