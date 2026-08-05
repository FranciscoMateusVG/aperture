---
name: dont-model-phantom-cases
description: Type contracts, response shapes, UI branches, and conditionals should match the cardinality of cases that can occur today — not phantom future cases. Use when designing a return shape, discriminated union, branching conditional, toast variant set, or error type hierarchy. Triggers on "in case we change X later", "robust for the future", dead branch, unreachable case, defensive over-modeling, error enum, toast variants.
---

# Don't Model Phantom Cases

A short discipline for code design: **the shapes you model should match the cases that can actually happen today, not the cases you imagine might be needed if a future change lands.** Phantom cases — branches no real payload can reach, toast variants no actual state can produce, conditional handlers for content types that aren't served — produce dead code, noisy type contracts, and traps where the next reader assumes the dead branches do something meaningful.

This skill is the **code-design sibling** to `aperture:e2e-catches-what-lower-cant` (test-design). Both are about *match what's actually true today vs assumed shapes* — but on different axes. The phantom-case axis is what your code claims about reality; the test-catches axis is what your tests can structurally see. Read both.

Vance recurrently applies this discipline; the skill banks three same-week provenances of it.

---

## The rule

> **Design choices should match actual cardinality today, not phantom future cases. Growth paths documented separately, not modeled in code.**

Three follow-ons that make this concrete:

1. **If a branch can't be reached, don't write it.** Dead branches are not "defensive" — they're noise. The next reader will assume they handle a real case and structure their changes around them; when reality forces a refactor, the dead branches become a tax.
2. **If the underlying mechanic changes (e.g., INSERT → UPSERT), update the consuming code to match the new cardinality.** Don't leave the multi-variant UI surface "in case we change the DB layer back later." If you genuinely might change it back, that's a documented growth path, not a code-shape concern.
3. **The simpler shape is the right shape when it covers all current cases.** Reach for branching when reality forces it, not preemptively.

---

## Three banked provenances (all Vance, all May 2026)

### Provenance 1 — Response shape: `{ count, conflicts }`, not `{ saved, failed, failures }`

**aperture-ll9z (Phase 1 grade-save, PR #304):** The backend already had a well-defined success/conflict cardinality. The teacher submits a grade batch; the backend either saves it (`count: N`) OR catches a `DuplicateGradeError` and reports it (`conflicts: M, conflictedStudentIds: [...]`).

A phantom-case design would have modeled this as `{ saved, failed, failures, errors, partial, ... }` — six fields covering hypothetical degrees of partial success. Vance picked the actual cardinality: `{ count, conflicts }`. Two fields, both meaningful, both reachable by real payloads.

What the phantom design would have cost: UI branches per phantom field (each rendering some toast variant nothing in the system could trigger); type assertions in callers' code on fields that never have non-default values; the next reader assuming `failed` and `failures` mean different things when neither could occur.

### Provenance 2 — Toast variants: collapse from 3 to 1 when UPSERT removes conflicts

**aperture-dlb8 Phase 2 (PR #305):** Phase 1 (ll9z) shipped a three-variant toast — success / conflict / partial — because at that time `INSERT` could conflict. Phase 2 switched the backend to Postgres UPSERT (`INSERT ... ON CONFLICT (uq_student_grade) DO UPDATE`). After UPSERT, **the conflict branch is unreachable.** There is no payload + DB state combination that produces a `conflicts > 0` response from the backend.

Phantom-case design: keep the three-variant toast, "in case we change the DB layer later." That preserves three UI surfaces (success / conflict / partial), three pieces of conditional rendering logic, and three brand-color-vs-error-color states that no real flow can trigger.

Vance's actual move: collapse the toast to a single success variant + a defensive empty-batch branch (the *only* remaining edge case in the current cardinality). The conflict toast was deleted. If we ever switch the DB layer back, the toast comes back — but that's a documented growth path, not a thing the code preserves for free.

### Provenance 3 — `arrayBuffer` everything, not Content-Type branching

**aperture-tx2k proxy (PR #306):** The Next.js admin proxy at `/api/admin/[[...path]]` was reading upstream Hono responses via `await upstream.text()`, which silently stripped UTF-8 BOM and broke Excel auto-detection for Brazilian users on CSV exports.

Two fix options were on the table:

- **Option A** — branch on Content-Type: `.text()` for `text/*`, `.arrayBuffer()` for everything else. This "preserves the current text path verbatim" and would have read as defensive.
- **Option B** — `.arrayBuffer()` for everything, NextResponse-passthrough. The proxy is HTTP-byte-forwarding by nature; there's no upside to interpreting the body as a string, and several quiet failure modes (this BOM strip, UTF-16 surrogate corruption, non-UTF-8 charsets).

Vance picked B. From the PR body:

> Option A (`text()` for `text/*`, `arrayBuffer()` for everything else) would preserve the current text path verbatim. **Rejected because it bakes in a "future binary endpoint inherits the bug if someone forgets to update the allowlist" failure mode.**

The phantom case was "what if a future endpoint needs string interpretation in the proxy" — a case the proxy has never actually needed (it's transparent forwarding by design). Modeling it would have preserved a code branch that exists only for an imagined endpoint, and would have re-introduced the BOM-strip class of bug whenever someone forgot to update the Content-Type allowlist.

---

## The common shape across all three

All three are the same move: **a code surface was about to grow more cases than the actual cardinality required, and Vance pruned to the shape that matches reality.**

- Provenance 1: type contract pruned (2 fields, not 6)
- Provenance 2: UI variants pruned (1 toast, not 3)
- Provenance 3: branching logic pruned (1 path, not 2 based on Content-Type)

In each case the phantom-case alternative read as "robust" or "defensive" at design time. In each case it would have produced dead branches, type-contract noise, or latent bugs that re-emerge when the imagined future case finally happens to ship.

---

## Forward-friction check (apply at code-design time)

Before you ship a type, branch, or variant set, ask:

1. **What cases does this design enumerate?** Count them.
2. **For each case: can a real payload / real state combination actually reach it today?** If the answer for any case is "not today, but maybe if X changes" → that's a phantom case.
3. **For each phantom case: would removing it break any current behaviour?** If no, remove it. If yes (e.g., it handles a real edge case), keep it.
4. **For the phantom cases worth tracking as growth paths: where will the growth path note live?** See the "Growth paths" section below.

The check is 30 seconds. The cost of skipping it is dead branches that the next reader takes seriously, refactors structure around, and has to clean up when the imagined-future-case finally requires real attention.

---

## Growth paths — document, don't model

If a future change is genuinely on the roadmap (e.g., "we might switch UPSERT back to INSERT-only when we add multi-row constraints"), the growth path deserves a note — but **the note goes in an in-repo doc, not in code shape.** See `aperture:research-artifact-placement` for the placement decision (couple-to-surface: a "growth paths" doc next to the code that implements the current cardinality).

Documenting growth paths gives you:

- A record the next agent can find when they DO need to add the case back
- No dead code in the meantime
- A natural spot to capture WHY the current cardinality is what it is

What it doesn't give you: pre-built dead branches "ready" for the phantom future. That's a feature, not a loss — pre-built dead branches are usually wrong by the time the actual future arrives.

---

## What this skill is NOT for

- **NOT** an excuse to skip defensive coding for edge cases that DO occur today. The `defensive empty-batch branch` in Phase 2 (Provenance 2) is exactly the kind of case worth handling — it's a real edge that real payloads reach. The discipline is "model real cases, not imagined ones," not "skip edge handling."
- **NOT** a license to delete code that handles legitimate uncommon cases. Rare ≠ phantom. If the case can occur, model it.
- **NOT** a rule against type safety. Discriminated unions, exhaustive checks, and well-typed shapes are good — when their cases match reality. The discipline is about *which* cases your shapes should enumerate, not whether to use type-system tools.
- **NOT** advice to skip thinking about future cases. Think about them — and write them down as growth paths. The split is "document growth paths in prose, model current cardinality in code."

---

## Source provenance

| Provenance | Bead | PR | What was pruned |
|---|---|---|---|
| 1 | `aperture-ll9z` (Phase 1 grade-save) | monorepo-incluir #304 | Response shape: 2 fields (`count`, `conflicts`) not 6 |
| 2 | `aperture-dlb8` (Phase 2 UPSERT) | monorepo-incluir #305 | UI variants: 1 toast not 3 (UPSERT made conflict-branch unreachable) |
| 3 | `aperture-tx2k` (proxy BOM fix) | monorepo-incluir #306 | Branching: 1 path (`arrayBuffer`) not 2 (Content-Type branch) |

**Authoring credit: Vance.** All three are her work; the discipline is consistently applied across her surface-area (frontend response handling, UI variant pruning, proxy I/O shape). The third provenance landed with explicit "same discipline as Phase 2" framing in Vance's own PR body — which is what GLaDOS noticed and triggered the skill bank.

---

## Adding a new precedent

If you (or another agent) prune a phantom-case design in a future PR, bank the precedent here. Same template:

1. **Provenance name** — what got pruned
2. **Bead + PR** — citation
3. **What the phantom case would have been** — describe what you almost shipped
4. **What you actually shipped** — describe the current-cardinality design
5. **What the phantom case would have cost** — dead branches, type noise, latent bugs

Open a PR on the aperture repo with the new precedent. The discipline holds regardless of how many precedents accumulate; the precedents themselves are the swarm's catalog of "ways we'd have shipped more shape than reality required."

---

## Sibling skills

- **`aperture:e2e-catches-what-lower-cant`** — test design sibling. Same family of "match reality vs assumed shapes," different axis (what tests catch vs what code claims). Read both.
- **`aperture:research-artifact-placement`** — companion for the growth-path documentation question. Tells you where the growth-path note should live (in-repo, alongside the code whose cardinality it explains).
