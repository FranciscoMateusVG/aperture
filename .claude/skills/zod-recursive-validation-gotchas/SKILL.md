---
name: zod-recursive-validation-gotchas
description: Patterns for applying global invariants (depth caps, fan-out caps, tree-level constraints) to recursive Zod schemas. Use when writing or reviewing `z.lazy` union schemas, adding depth/fan-out caps to tree-shaped types, or shaping validation errors for users. Triggers on `z.lazy`, `superRefine`, `.refine`, `ctx.addIssue`, recursive Zod, generic "audience invalid" errors, accidentally-quadratic validation, AudienceExpression-style schemas.
---

# Zod Recursive Validation Gotchas

Two patterns for adding global invariants (depth caps, fan-out caps, total-size limits) to recursive Zod schemas — `z.lazy(() => z.union([...]))` shapes that admit unbounded tree structures and need a guard at the root.

Both patterns were banked from the same PR (Rex, monorepo-incluir #298, `aperture-nnpb`). They're paired here because **you almost always need both at once**: if you're adding a global invariant to a recursive schema, you need the right composition (Pattern 1) AND the right error shape (Pattern 2).

When you add a new gotcha here, follow the same shape: **Symptom → Cause → Fix → Where this shows up → Source**. Cite the PR + commit; future-you will want to read the diff.

---

## 1. Inner/outer split: keep recursive refs on the un-refined schema

**Symptom:** You add a `superRefine` (or `.refine`) to a `z.lazy(() => z.union([...]))` schema to enforce a tree-level invariant (depth cap, fan-out cap, total-node-count limit). Validation works correctly — pathological inputs are rejected — but parsing is **accidentally quadratic** in tree size, sometimes catastrophically so on inputs the codebase considers "still small." Local benchmarks look fine on toy trees; the timing blows up on real-shaped trees.

**Cause:** `z.lazy(() => z.union([...]))` evaluates the union body every time it's reached during parsing. If your refinement is on the schema that the lazy body refers back to, the refinement effect runs **at every recursion depth**, walking the whole subtree from each node. For a tree of size N, that's an O(N²) walk instead of O(N).

The trap is that the naive way to write it — defining one schema and having it self-reference — looks like the most obvious composition:

```ts
// ⚠️ Quadratic — superRefine runs at every nested .safeParse call.
export const AudienceExpressionSchema: z.ZodType<AudienceExpression> = z
  .lazy(() =>
    z.union([
      MacroLeafSchema,
      // ↓ recursive ref points back at the refined schema
      z.object({ and: z.array(AudienceExpressionSchema).min(1) }),
      z.object({ or: z.array(AudienceExpressionSchema).min(1) }),
    ]),
  )
  .superRefine((expr, ctx) => walkInvariant(expr, ctx, 0, []));
```

Every time the parser descends into an `and` or `or` arm, it re-enters `AudienceExpressionSchema` — which runs the whole `superRefine` walk again from that subtree. The walk doesn't know it's already been done for an ancestor.

**Fix:** Split into two schemas. The **inner** schema is purely structural — the recursive `z.lazy` body, with **internal refs pointing back at the inner schema itself**. The **outer** schema wraps the inner and attaches the `superRefine` exactly once. External callers use the outer; the lazy body uses the inner. The walk fires at the root only.

```ts
// ✅ Linear — superRefine fires exactly once at the root.

// Inner schema: pure structure, recursive refs point to ITSELF.
const innerAudienceExpressionSchema: z.ZodType<AudienceExpression> = z.lazy(
  () =>
    z.union([
      MacroLeafSchema,
      // ↓ recursive ref points at the INNER (un-refined) schema
      z.object({ and: z.array(innerAudienceExpressionSchema).min(1) }),
      z.object({ or: z.array(innerAudienceExpressionSchema).min(1) }),
    ]),
);

// Outer schema: attaches the global invariant. External callers use this.
export const AudienceExpressionSchema: z.ZodType<AudienceExpression> =
  innerAudienceExpressionSchema.superRefine((expr, ctx) => {
    validateAudienceBoundsInto(expr, ctx, 0, []);
  }) as z.ZodType<AudienceExpression>;
```

The mental model: **the inner schema is what the type "is"; the outer schema is what the type "must additionally satisfy at the root."** Internal recursion descends through the inner — bound checks don't re-run. External code that asks "is this a valid audience expression?" goes through the outer — bound checks run once.

**Where this shows up:** any recursive Zod schema where you add a tree-global invariant. Common cases:

- Depth caps (no more than N nesting levels)
- Fan-out caps (no more than N children per combinator / per branch)
- Total-node-count caps (the whole tree must have ≤ N leaves)
- Cycle-detection rules on graph-shaped JSON (rare but real)
- Aggregate-shape rules ("at least one leaf must be of type X")

Any time you find yourself reaching for `.superRefine` on a `z.lazy(() => ...)` schema, ask: **does my refine logic walk into the children, or just inspect the current node?** If it walks (Pattern 2 below — and most tree-global checks do), the inner/outer split is mandatory. If it only inspects the current node, you can attach `.superRefine` directly without the trap — but then you're not really enforcing a tree-global invariant, you're enforcing a per-node one, which the schema body itself can usually express.

**Source:** Rex, monorepo-incluir PR #298 (`aperture-nnpb`), `packages/domains/src/surveys/audience.ts` — `MAX_AUDIENCE_DEPTH = 8` + `MAX_AUDIENCE_CHILDREN = 32` caps on `AudienceExpressionSchema`. See lines 56-100 of the diff for the full inner/outer composition.

---

## 2. Path-aware `ctx.addIssue` over boolean `.refine`

**Symptom:** A validation rejection on a deep tree gives the admin (or end-user) a single, generic error like `"audience invalid"` or `"expression failed validation"`. They can see the input was rejected but have no signal **which node violated which rule**. Time to repair scales with tree size — they have to bisect their own input to find the bad branch.

**Cause:** `.refine(validateFn, errorMessage)` returns boolean and attaches a single error message at the schema's path. If `validateFn` walks a tree and finds a violation 4 levels deep at the third child of an AND combinator, `.refine` can't surface that — the entire schema fails with `errorMessage` at the schema's root path.

**Fix:** Use `superRefine((value, ctx) => ...)` instead, walk the structure yourself, and emit issues via `ctx.addIssue({ code, message, path })`. The `path` array (e.g. `['and', 0, 'and']`) points at the exact node that violated the rule, and Zod's error formatter (or your own) renders that path so the admin reads "AND combinator at `/and/0/and` has 33 children; maximum is 32" instead of "audience invalid."

```ts
// ⚠️ Boolean refine — admin sees "audience invalid" with no path info.
const AudienceExpressionSchema = innerAudienceExpressionSchema.refine(
  (expr) => validateDepth(expr) && validateFanOut(expr),
  "audience invalid",
);

// ✅ Path-aware superRefine — admin sees the exact offending node.
function validateAudienceBoundsInto(
  expr: AudienceExpression,
  ctx: z.RefinementCtx,
  depth: number,
  path: (string | number)[],
): void {
  if (depth > MAX_AUDIENCE_DEPTH) {
    ctx.addIssue({
      code: z.ZodIssueCode.custom,
      message: `Audience expression depth exceeds maximum of ${MAX_AUDIENCE_DEPTH}`,
      path,
    });
    return;
  }
  if (isMacroLeaf(expr)) return;
  if (isAndCombinator(expr)) {
    if (expr.and.length > MAX_AUDIENCE_CHILDREN) {
      ctx.addIssue({
        code: z.ZodIssueCode.custom,
        message: `AND combinator has ${expr.and.length} children; maximum is ${MAX_AUDIENCE_CHILDREN}`,
        path: [...path, "and"],
      });
      return;
    }
    expr.and.forEach((c, i) =>
      validateAudienceBoundsInto(c, ctx, depth + 1, [...path, "and", i]),
    );
    return;
  }
  // …same for or-combinator…
}

export const AudienceExpressionSchema = innerAudienceExpressionSchema
  .superRefine((expr, ctx) => {
    validateAudienceBoundsInto(expr, ctx, 0, []);
  });
```

Three things are happening that boolean `.refine` can't do:

1. **The walker carries `path`** as an accumulating breadcrumb (`[...path, "and", i]`), so the issue's `path` field points at the offending node — not the root.
2. **The walker carries `depth`** as a separate counter, so depth violations are caught the instant they're exceeded (at the descent edge) rather than after-the-fact.
3. **Multiple violations can be reported in one parse** — `ctx.addIssue` doesn't short-circuit. If both depth AND fan-out are violated in different branches, both get surfaced. (Inside one branch, return early after `addIssue` to avoid cascading nonsense.)

The result on the admin side: a UI that can render `audience.and[0].and: AND combinator has 33 children; maximum is 32` and highlight the offending node visually, instead of a generic "validation failed" toast.

**Where this shows up:** any Zod validation that walks a structure to enforce a rule. Common cases:

- Recursive schemas (the audience case above, nested form configs, layered policy documents)
- Discriminated-union validations that need to check cross-arm consistency
- Array schemas with element-position-aware rules ("element 5 must have type X if element 3 has type Y")
- JSON-Schema-derived Zod schemas — passing the original JSON-pointer path through the validator keeps errors aligned with the user's mental model of "where in my JSON"

**The principle:** the **error message is part of the contract.** When you're designing a validator that walks a tree, the path information IS the diff between "rejection" and "useful rejection." Boolean `.refine` is the right tool for atomic per-value checks (`z.string().refine(s => s.length > 0)`); `superRefine` with `ctx.addIssue({ path })` is the right tool for anything that walks.

**Source:** Rex, monorepo-incluir PR #298 (`aperture-nnpb`), same `audience.ts` file. Cipher's S1 follow-up bead proposed `validateDepth(expr): boolean` as a `.refine` — Rex upgraded to the path-aware `superRefine` and banked the principle.

---

## Why both patterns travel together

In real recursive-validation work, you very rarely use one of these without the other:

- If you need a tree-global invariant, you almost always want to surface the offending node's path so the user can fix it → **Pattern 2.**
- If your walker emits `ctx.addIssue` calls that walk into the children, you also need the inner/outer split so the walker isn't re-invoked at every nested parse → **Pattern 1.**

So in practice these are two halves of the same idiom. The PR that banked them (`aperture-nnpb`) used them together: the `innerAudienceExpressionSchema` / `AudienceExpressionSchema` split (Pattern 1) hosts a single `validateAudienceBoundsInto(expr, ctx, 0, [])` call (Pattern 2) at the root.

---

## Adding a new gotcha

When recursive-Zod bites you (or you discover a new principle worth banking), use the same shape:

1. **Symptom** — what the failure looks like (perf cliff, vague error, missed validation, …)
2. **Cause** — the Zod composition rule that produces it
3. **Fix** — the actual code change, with before/after pulled from a real PR
4. **Where this shows up** — the pattern of schema you'd find it in
5. **Source** — agent name, PR + commit SHA, file

Open a PR on the aperture repo. Ping Atlas for a second pair of eyes on the framing if helpful.
