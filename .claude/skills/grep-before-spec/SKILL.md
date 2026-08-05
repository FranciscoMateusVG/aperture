---
name: grep-before-spec
description: Before proposing new infrastructure in a recon-phase architecture doc, grep-confirm the codebase doesn't already have it — every infra-adding proposal must include a grep-receipt. Design-spec drift from code-reality ships as implementation rework. Use when writing recon docs, scoping beads, design sketches, or epic visions that add clients, adapters, middleware, rate limiters, queues, or integrations. Triggers on "add Redis", "new adapter", "from scratch", "let's introduce X" without grep-receipt.
---

# Grep Before Spec

A discipline for the recon phase. The principle is one sentence; the failure mode shows up three times in a single epic; the fix is a small evidence obligation that shifts the catch from "implementation rework" to "recon-time correction."

This skill is the **recon-side companion** to `aperture:spec-deviation-discipline` (PR #11). That skill is about catching internal inconsistencies in a written spec; this one is about catching spec drift from the codebase reality the spec is supposed to operate on. Both are anti-cargo-cult disciplines applied to specs; they catch different classes of error.

---

## The principle

> **When a recon-phase architecture proposal would add new infrastructure (a client, adapter, middleware, rate limiter, queue, integration, schema), grep-confirm that the codebase doesn't already have it BEFORE writing the spec. Include the grep-receipt in the recon doc.**

Two framings (GLaDOS-named):
- **Failure-mode:** *"design-spec drift from code-reality is a recon-phase failure that ships as implementation-phase rework."* The spec proposes adding X; the codebase already has X; implementation either ships the redundant addition (waste) or refactors to use the existing X (the rework GLaDOS named).
- **Fix-shape:** *"any architecture proposal that adds infra MUST include a grep-receipt that the codebase doesn't already have it."* The grep-receipt is the small evidence step that converts the spec from "what should we build" to "what should we build, given what we already have."

---

## Three banked provenances (all `aperture-lz9y` epic, 2026-05-22)

The pattern surfaced three times in a single epic's design phase. Same root cause across all three: a recon-phase agent (Wheatley) drafting architecture without grep-confirming whether the proposed infra already exists. Shipping-phase agents (Cipher security review, Rex implementation recon) caught each instance because they were forced to actually read the relevant code.

### Provenance 1 — `aperture-cea7` A11 (Cipher's catch)

**Spec said:** Wheatley's Wave 1 rate-limiting design proposed adding a "small in-memory LRU" rate limiter from scratch.

**Codebase already had:** A full sliding-window rate-limiter adapter wired in production. Both Redis (`apps/hono-app/src/adapters/rate-limit/rate-limit-redis.ts` — Cipher variant B Lua) and memory variants exist. `REDIS_URL` already wired in `server.ts:110-184`. The two-bucket-dispatcher template from `error-ingest-rate-limit.ts` was sitting there waiting to be mirrored.

**Caught by:** Cipher during S1 security review (`aperture-cea7` AXIS 3). The verdict explicitly FAILED Wheatley's "small in-memory LRU" wording and required the spec change before Wave 2 ships.

**Cost of the catch:** ~5 minutes of grep at recon time would have saved the spec being filed with the wrong abstraction. Without Cipher's catch, Rex would have implemented the in-memory limiter, shipped it, then refactored to the existing Redis adapter at PR review.

### Provenance 2 — `aperture-fjjk` recon #1 (Rex's catch, mailer pattern)

**Spec said:** Wheatley's OpenAI integration sketch said *"follow the mailer pattern for OpenAI client DI"* — implying the mailer uses dependency injection and the OpenAI client should mirror that pattern.

**Codebase already had:** The mailer is implemented as a **module singleton**, NOT dependency-injected. The correct pattern to follow for service injection in this codebase is `AppDependencies` (the standard DI container used by other services).

**Caught by:** Rex during implementation recon for B2 (`aperture-fjjk`). He started writing the OpenAI client following the mailer pattern, hit the module-singleton structure, realized the spec's reference was wrong, and pivoted to `AppDependencies`.

**Cost of the catch:** Rex's grep-receipt would have shown the mailer's singleton shape directly. The spec's reference assumed DI by parallel; the parallel was false.

### Provenance 3 — `aperture-fjjk` recon #2 (Rex's catch, rate-limit adapter)

**Spec said:** Wheatley's OpenAI sketch implied new rate-limit infrastructure was needed for the AI endpoints.

**Codebase already had:** The same Cipher Lua variant B sliding-window adapter from Provenance 1 (`apps/hono-app/src/adapters/rate-limit/`). Rate-limit infra is already a first-class primitive in the codebase; the AI endpoints just need to wire two new buckets to it (`report:draft:user:<user_id>` + `report:turn:user:<user_id>`), not introduce new infra.

**Caught by:** Rex during the same implementation recon as Provenance 2. Adjacent catch — once he was reading the existing infra for the OpenAI client wiring, he also surfaced the rate-limit reuse.

**Cost of the catch:** Provenance 1's grep would have caught this if extended; Provenance 3 confirms the pattern is "Wheatley's spec missed the same infrastructure twice from two different angles."

### Common shape across all three

- **Recon-phase agent** (Wheatley) drafts an architecture proposal that adds new infra
- **Codebase already has the relevant primitive** wired and in production
- **No grep-receipt** in the recon doc to demonstrate the absence-check was performed
- **Shipping-phase agent** (Cipher S1 review, Rex implementation recon) catches the drift because they're forced to actually read the relevant code
- **Implementation-phase rework** is averted ONLY because the catch happened pre-code; without it, the rework happens at PR review

---

## The grep-receipt discipline

When a recon doc proposes adding infrastructure, include a small "grep-receipt" section that demonstrates the absence-check. Format:

```markdown
### Existing infrastructure audit

Before specifying `<new-infra>`, audited the codebase for existing primitives:

- **`grep -r '<keyword-1>' apps/hono-app/src/`** — no matches
- **`grep -r '<keyword-2>' apps/hono-app/src/`** — found at `<path>:<line-range>`, evaluated for fitness:
  - Shape: `<one-line-description>`
  - Fit for `<our-need>`: yes / no — `<one-sentence-reason>`
- **`find apps/hono-app/src/adapters/`** — listed `<adapters>`; none cover `<our-need>`

**Conclusion:** Codebase does not have `<new-infra>` in a form usable for `<our-need>`. Spec proceeds with adding it.

(OR: **Conclusion:** Codebase already has `<existing-infra>` at `<path>`. Spec changes to reuse, not add.)
```

The grep-receipt is short — usually 3–8 lines. The cost is 2–5 minutes at recon time. The value is converting the spec from "what should we build" to "what should we build given what already exists," which catches every shape of the three banked provenances above.

What to grep for:
- The proposed infra name (`OpenAI`, `RateLimit`, `Mailer`, `Redis`, …)
- The proposed environment variables (`OPENAI_API_KEY`, `REDIS_URL`)
- The proposed adapter folder (`apps/hono-app/src/adapters/`)
- The composition-root wiring point (`server.ts`, `app.ts`)
- Any pattern the spec invokes by name ("follow the mailer pattern" → grep mailer to confirm what its pattern actually is)

---

## Forward-friction check (apply at recon-design time)

Before you publish a scoping bead, design sketch, or recon doc that proposes adding infrastructure, ask:

1. **What new infra does my spec propose?** List each item by name (client, adapter, middleware, schema, env var, …).
2. **For each item: does the codebase already have it (or a close-enough primitive)?** Grep for keywords; check `apps/hono-app/src/adapters/`, `server.ts`, `.env.example`, the relevant route files.
3. **For each item I found in the codebase: is the existing version usable for my need?** If yes, the spec changes to reuse, not add. If no, the spec proceeds with the addition AND explains why the existing version doesn't fit.
4. **Include the grep-receipt in the recon doc.** Even a 3-line grep summary is sufficient — the point is to demonstrate the check was performed, not to write a full audit.

The cost is 2–5 minutes per recon doc. The benefit is catching the three provenances above at the right phase (recon, not implementation).

---

## What this skill is NOT for

- **NOT** a directive to grep before every line of every spec. The discipline applies specifically to proposed infrastructure additions — clients, adapters, middleware, rate limiters, schemas, env vars, integrations. A spec that doesn't add infra doesn't need the grep-receipt.
- **NOT** a license to skip writing the spec until the grep is done. The grep-receipt is part of the spec, not a precondition for starting. You can draft the spec and add the receipt as you write; the receipt just needs to be in the final document.
- **NOT** about retroactive blame for past recon drift. The discipline is for the next recon doc, not for litigating the previous one.
- **NOT** a substitute for shipping-phase code review. Cipher's security verdict + Rex's implementation recon catches still happen at their respective phases; this skill just shifts the recon-side catches that should have happened earlier.

---

## Source provenance

| Bead | Catch agent | What was proposed | What existed | Cost averted |
|---|---|---|---|---|
| `aperture-cea7` A11 | Cipher (S1 review) | "small in-memory LRU" rate limiter | Redis + memory adapters in `apps/hono-app/src/adapters/rate-limit/`, `REDIS_URL` already wired | Implementation + PR-time refactor (Rex would have built the LRU, then rewritten to Redis) |
| `aperture-fjjk` recon #1 | Rex (B2 implementation recon) | "Follow mailer pattern for OpenAI client DI" | Mailer is module singleton, not DI; correct pattern is `AppDependencies` | Wrong-pattern implementation, then refactor to AppDependencies |
| `aperture-fjjk` recon #2 | Rex (B2 implementation recon) | New rate-limit infra for AI endpoints | Same Cipher Lua variant B adapter as Provenance 1 | Same — would have re-implemented existing primitive |

**Class-diagnosis credit (3-agent shape):**
- **Cipher** — caught instance #1 during S1 security review
- **Rex** — caught instances #2 + #3 during implementation recon
- **GLaDOS** — named the categorical observation + dispatched the bank, after PR #23 noted multi-agent class-diagnosis at 3-recurrence saturation

This is the **4th banked instance of multi-agent class-diagnosis** (after PR #21 Wheatley+Vance, PR #22 Vance+GLaDOS, PR #23 Cipher+GLaDOS) AND the **first banked instance of 3-agent class-diagnosis** — extending the pattern's shape beyond strict pairs. Banked as a refinement to anchor #21 on `aperture-4la6`: the multi-agent class-diagnosis pattern admits 2-agent AND 3-agent shapes.

**3-recurrence ship** under aperture-4la6's promotion-by-recurrence heuristic. Three independent provenances of the same root cause in a single epic's design phase; banking immediately is the right call before more instances accumulate.

---

## Cross-links

- **`aperture-4la6` anchor #29b** (parallel-security-review-catches-design-drift) — same root-cause family, different mechanism. 29b is about review-cycle topology (S1 runs PARALLEL to B1-B3 design, not after); this skill (anchor #29a, the saturated half) is about scoping-recon hygiene (grep-receipt at recon-write time). Both prevent the same class of drift at different phases.
- **`aperture:spec-deviation-discipline`** (PR #11) — adjacent. That skill catches internal inconsistencies in a written spec; this skill catches spec drift from codebase reality the spec is supposed to operate on. Composite-mesh pair on "verify the spec against something concrete."
- **`aperture:filed-p2-becomes-prereq`** (PR #23) — adjacent. Same Cipher-Rex-GLaDOS dispatch shape. Both about catching consequences of recon-phase decisions that ship as implementation-phase rework.
- **`aperture:specialist-delegation §6`** (Cipher's verify-against-reality) — parent. This skill is verify-against-reality applied at recon-write time: check your spec against codebase reality before publishing.
- **`aperture:investigator-mode`** (Wheatley-only recon discipline, PR #7) — when investigator-mode merges, a reciprocal cross-link from there to this skill closes the loop on Wheatley's recon-side discipline.

---

## Adding a new precedent

If you (or another agent) catch another instance of the recon-spec-drift-from-codebase pattern, bank it here. Same template:

1. **Bead + catch agent** — citation
2. **What the spec proposed adding** — the proposed infra
3. **What the codebase already had** — the existing primitive (with file path + line range)
4. **Cost averted** — what the catch saved (implementation-phase rework, PR refactor, etc.)
5. **What grep would have surfaced** — the keywords/files that would have caught it at recon time

The pattern is at 3-recurrence saturation today on a single epic. New precedents from other epics would confirm the pattern generalizes beyond `lz9y`'s design phase — useful for skill body refinement.
