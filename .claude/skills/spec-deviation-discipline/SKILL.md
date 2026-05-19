---
name: spec-deviation-discipline
description: When implementing from a spec, audit the spec against the system's existing invariants before you start coding. If the spec's prescribed mechanism conflicts with an invariant, deviate to a mechanism that satisfies both intent and invariant — and document the deviation. Use any time you receive a written specification (bead description, design doc, threat model, ticket acceptance criteria, operator brief) and are about to translate it into code. Triggers on bead implementation, spec-to-code translation, "the bead says X but Y conflicts," "trust the spec vs verify the spec," internal-spec-inconsistency, mechanical spec-following, "the spec proposed N options."
---

# Spec-Deviation Discipline

A short discipline for anyone implementing from a written spec. It separates "follow the spec" from "verify the spec is internally consistent" — and gives you the documented escape valve when the second check fails.

This skill is paired with — but distinct from — Cipher's `verify-against-reality` principle (referenced in `aperture:specialist-delegation`). Verify-against-reality is about checking your CODE against external state (database rows, traces, prod). This skill is about checking the SPEC against the system's own invariants **before** you write any code. Both are anti-cargo-cult disciplines; this one runs earlier in the cycle.

---

## The rule

> Before implementing a spec, walk the spec's proposed transformation against every existing invariant of the system it touches. If the transformation conflicts with any invariant, deviate to a mechanism that satisfies both the spec's intent and the invariant; document the deviation in code comment + commit message + PR body.

That's it. Three sentences. The rest of this skill is two war stories illustrating two different ways the conflict shows up in practice, plus a forward-friction check you can apply at spec-read time.

---

## Why this needs to be a discipline

Mechanical spec-following is a tempting default: someone (Wheatley, Cipher, the operator, an upstream agent) did the thinking; you just have to translate words into syntax. The trap is that **the person writing the spec didn't always check the spec against every invariant of the system it'll land in.** Specs are written from the writer's mental model; that model may be partial.

When the prescribed mechanism in the spec conflicts with an actual property of the system — a regex set, a schema constraint, a foreign-key shape, an existing test pin — and you implement it literally, you ship a silent regression. The CI is green (the spec said to do X, you did X), nothing screams, and the bug surfaces weeks later in a context where nobody connects it to the implementation choice.

The discipline is one read-pass over the spec **before** you start coding, asking: "if I do exactly what this says, does the result violate anything else this system promises?" That pass is cheap. The regression it prevents is expensive.

---

## Two conflict modes (war stories)

The two war stories below are the same shape — *spec mechanism vs system invariant, deviate to representable, document* — but they show two different ways the conflict surfaces. Recognizing the shape is what lets you apply the discipline going forward.

### Mode A — Spec's mechanism over-consumes a protected set

**War story (aperture-sjon, PR #296, Rex):** Cipher's S1 follow-up bead said *"use `raw.trimStart()` before the formula-prefix check"* in the CSV-export escape function. The existing prefix regex matched `[=+\-@\t\r]` — which **includes** `\t` and `\r`. `trimStart()` strips all whitespace including `\t` and `\r`, so a payload like `\tabc` would post-trim to `abc` (not a formula char) and emit unguarded. The literal spec would have silently regressed the `\tabc` / `\rabc` cases the original regex correctly guarded.

The deviation: instead of `trimStart()`, use a regex with an **explicit non-formula-whitespace skip set** — `[space (0x20), NBSP (U+00A0), LF (\n)]`, deliberately NOT `\t` / `\r`. Same intent (leading whitespace shouldn't hide a formula prefix), different mechanism (one that doesn't eat the chars we need to keep guarding).

```ts
// ✅ Skip leading SAFE whitespace, then check the formula-prefix set.
//    Deliberately excludes \t (	) and \r () from the skip set
//    because they're themselves formula-prefix triggers — trimming them
//    would regress \tabc / \rabc cases.
const FORMULA_PREFIX = new RegExp(
  `^[\\u0020\\u00A0\\u000A]*[=+\\-@\\t\\r]`,
);
```

The intent (don't let a leading space hide a formula) is preserved; the protected set (chars that are themselves formula triggers) is preserved. **Both invariants satisfied.**

**Signal phrase for the trap:** the spec proposes a transformation that operates on a set, and the protected set the system already cares about **intersects** with what the transformation removes/modifies.

### Mode B — Spec's mechanism isn't representable in the type system

**War story (aperture-736g, PR #301, Rex):** Cipher's bead for defence-in-depth read-side re-validation said *"On parse failure, log + return null/empty audience (defensive — better to NOT show a survey than to silently mis-evaluate it)."* The bead offered two options: return null OR return an "empty audience."

On inspection, **"empty audience" wasn't representable in the system.** `AudienceExpressionSchema` requires AND/OR combinators to have `.min(1)` children, and there's no `{ macro: 'never' }` sentinel. The fan-out + depth caps shipped in PR #298 (`aperture-nnpb`) made the picture even tighter — there's no construction of the schema that means "empty."

The deviation: pick the representable option (return null), and choose caller behaviour per method so the spec's INTENT (corrupt audience → survey is invisible) is preserved:

| Method | Behaviour on corruption |
|---|---|
| `list` | Filter null rows out (admin still gets stable pagination via `total`) |
| `findById` | Return null → admin gets 404 → the right "investigate this" signal |
| `update` / `activate` / `close` / `archive` / `duplicate` | Return null |
| `create` | **Throw.** We just wrote a row we'd validated; a round-trip corruption is a contract violation — 500 is right |

The spec's intent (don't silently mis-evaluate) is preserved. The mechanism is the one the type system actually admits. **Both intent and invariant satisfied.**

**Signal phrase for the trap:** the spec lists multiple options ("return null OR an empty X"), and one of the options assumes a value shape that the schema/type/contract doesn't admit.

---

## Documenting the deviation — three places, every time

When you deviate, the discipline is to leave a trail in three places so the next reader doesn't reverse it:

1. **Code comment** — at the point of deviation. Says *what* the spec proposed, *why* it conflicted, *what* you did instead. Short. Names the conflicting invariant.
2. **Commit message** — separate paragraph in the body explaining the deviation. The PR author may not be the bead's author; the commit reader may not have the bead open.
3. **PR body** — an explicit "Deviation from spec" or "Deviation from bead" section calling out the change so reviewers don't have to bisect the diff to find it.

The three-place rule isn't paranoia — it's how the deviation survives the inevitable churn (file refactors lose comments, squash-merges lose commit history, closed PRs are hard to find). At least one of the three trails almost always survives.

---

## Forward-friction check (read this at spec-receipt time, before coding)

When you receive a spec, ask:

1. **What sets/structures does the spec's prescribed transformation touch?**
2. **What other invariants does the system maintain over those sets/structures?** (regex, schema, type contract, FK shape, RLS rule, existing test pin, …)
3. **Walk the transformation against each invariant. Any conflict?**
   - If yes → deviate, satisfy both intent and invariant, document in 3 places.
   - If no → implement as specified.

You do this pass once, at spec-read time, before you write any code. The cost is 30 seconds to 5 minutes; the cost of skipping it and silently regressing an invariant is hours-to-days to track down later.

If you find a conflict, this is also the moment to **flag it to the spec author** (Cipher, Wheatley, the operator). They may have known about the conflict and there's context you're missing; or they didn't, and your catch becomes a contract amendment. Either way, the deviation gets explicit blessing before you commit.

---

## What this skill is NOT

- **NOT** an excuse to deviate from the spec because you'd rather solve it differently. The deviation must be triggered by an actual conflict with a system invariant — not by aesthetic preference, not by "I think this is cleaner," not by "I disagree with Cipher's design." Those go in a follow-up bead or a PR review comment, not in a silent deviation.
- **NOT** a license to skip the spec-author handoff. When you find a conflict, you flag it and document the resolution. Silent deviation, even one that's technically correct, breaks the contract that specs are an honest record of the design.
- **NOT** a replacement for `verify-against-reality`. This skill catches spec-vs-system conflicts at *implementation start*. Verify-against-reality catches code-vs-prod conflicts at *implementation end*. Both are needed.

---

## Source provenance

| Mode | Bead | PR | Agent | What conflicted |
|---|---|---|---|---|
| A: spec mechanism over-consumes protected set | `aperture-sjon` | monorepo-incluir #296 | Rex | `trimStart()` strips chars (`\t`, `\r`) that are themselves in the formula-prefix protected set |
| B: spec mechanism not representable | `aperture-736g` | monorepo-incluir #301 | Rex | "Empty audience" isn't representable — schema requires `.min(1)` children, no sentinel macro |

Both originated from Cipher's S1 follow-up cohort on the surveys v1 epic — that cohort is a useful natural-experiment dataset: same agent (Rex) implementing, same spec-author (Cipher), same week, two different deviation shapes from the same root cause class.

Rex authored the rule itself — quoted verbatim at the top of this skill — and confirmed verbatim use for the documented version.

---

## Adding a new conflict mode

If you hit a third shape of spec-vs-invariant conflict that doesn't fit Mode A or Mode B, bank it. Use the same template:

1. **Mode name** — what kind of conflict (over-consumes / not-representable / wrong-direction / etc.)
2. **War story** — bead ID, PR, agent, what the spec proposed, what invariant it conflicted with, what you did instead
3. **Signal phrase** — the verbal pattern that identifies this mode at spec-read time

The point of the modes is forward-friction: at spec-read time, you should be able to ask "does this look like Mode A?" "does it look like Mode B?" — and the more modes we accumulate from real precedent, the better that pattern-match becomes.
