---
name: filed-p2-becomes-prereq
description: Security/audit/cleanup follow-ups filed as "P2 nice-to-have" consistently become hard prereqs when the next feature on the same surface ships. File follow-ups at the right priority, and audit accumulated follow-ups before designing on a touched surface. Use when filing a follow-up bead or scoping a feature on a surface with outstanding follow-ups. Triggers on P2 nice-to-have, "we'll get to it later", "blocks the next feature", audit-log gap that became a gate.
---

# Filed P2 Becomes Prereq

A discipline for the lifespan of follow-up beads. The principle is observed across 6+ banked instances in two weeks; the rule applies on two sides (bug-author + feature-designer); the forward-friction check is small and worth applying at both ends.

This skill sits in the same family as `aperture-4la6` anchors #19 ("catches have tails" — Cipher) and #27 ("ship surface, document deeper" — Vance). All three are about "the bug-fix has a tail of work; document the tail." This skill specifically names the failure mode where the tail was filed as P2 nice-to-have and then turns out to be the next thing that needs to ship.

---

## The principle

> **An earlier security/audit/cleanup follow-up filed as "P2 nice-to-have" consistently turns out to be a hard prereq when the next feature on the same surface ships.** The "we'll get to it later" framing is structurally wrong on surfaces that see active feature work — the follow-up is not optional; it's pre-positioned for the next feature whether anyone realizes it or not.

Two framings of the discipline (apply both):

1. **Bug-author side** — when filing a follow-up bead, recognize the surface it's on. If the surface is likely to see future feature work, the follow-up is probably a hard prereq, not a nice-to-have. P2 with default framing is wrong; reframe to P1 with explicit "blocks future X on Y surface" language.
2. **Feature-designer side** — when designing a new feature on a surface that has outstanding follow-ups, audit those follow-ups for hidden prereq dependencies BEFORE designing. The earlier P2 may be the gate your new design assumes is already closed. The audit is 5 minutes; the cost of missing it is feature work that hits the gate mid-design and stalls.

---

## Six banked provenances

All from monorepo-incluir, May 2026. The pattern is consistent enough that recurrence is essentially structural rather than coincidental:

| Bead chain | Surface | The follow-up | The feature that needed it |
|---|---|---|---|
| `pdlq` | /api/users response shape | Snake_case migration cleanup filed P2 | Multiple downstream consumers (5 apps × 6+) broke; became prereq for #322 |
| `ftuy` | /presencas Estatísticas | FE/BE contract for /by-semester filed P2 | Estatísticas fix needed the route shape declared |
| `kruq` | staff-on-behalf-of | ID-space declaration filed P2 | POST handler needed the contract pinned before shipping |
| `#336` | (additional ID-space work) | (per Cipher's enumeration) | Became prereq |
| `wbg9` | (5th instance per GLaDOS) | Surface follow-up filed P2 | The next feature on that surface needed it |
| `cea7 → jy16 → B2-blocker chain` | report-intake epic (Wave 1, 2026-05-22) | Audit follow-up from cea7 | Blocking jy16 which is a B2 blocker — the chain rolled forward fast |

**Class-diagnosis credit (multi-agent shape, third banked instance of that anchor):**
- **Cipher** — flagged the pattern as a recurring class while writing the cea7 audit, named two candidate skill names (`filed-p2-becomes-prereq` / `follow-up-rolls-forward`), and dispatched the bank
- **GLaDOS** — caught the 5th instance on wbg9 + named the categorical observation: *"filed P2 follow-up becomes prereq for the next feature on the same surface"*

---

## The bug-author side discipline

When filing a follow-up bead, before defaulting to P2:

1. **Identify the surface the follow-up lives on.** Is it a route, page, file, module, schema, or domain that's likely to see active feature work in the next 1–4 weeks?
2. **If yes — the default priority is wrong.** Reframe the bead:
   - **Priority:** P1, not P2
   - **Title:** include "blocks future X on Y surface" so the prereq nature is visible from `bd ready`
   - **Description:** name the specific feature/scope the follow-up gates, even if speculative ("if anyone adds another consumer of /api/users, they will need this resolved first")
3. **If no — P2 might be right.** Surfaces that are stable (low-touch infra, completed migrations, legacy code with no active work) admit nice-to-have follow-ups. The discipline is to prove the "no" rather than assume it.

The cost of over-filing P1 is small (a P1 bead with no claimant just sits in the queue). The cost of under-filing P2 is a hidden prereq that lands as a feature-blocker mid-cycle.

---

## The feature-designer side discipline

When designing a new feature on a surface that has previous touches:

1. **Pull the surface's follow-up debt** — `bd list --label project:incluir | grep <surface>` or equivalent. Specifically look for beads with "follow-up" / "P2" / "we'll get to it" framing on the same route, file, page, or schema.
2. **For each follow-up, ask: does my new feature assume this gate is already closed?** Common gate categories:
   - Auth/permission contracts (does my feature assume the auth gate handles X?)
   - Response shape contracts (does my feature consume the field shape from the open follow-up?)
   - Audit-log expectations (does my feature need the audit event the follow-up was meant to add?)
   - Rate-limit / capacity (does my feature volume assume the limit that follow-up was going to raise?)
   - Schema/migration (does my feature use the column the follow-up was going to add?)
3. **For each assumed-closed gate that's actually open — your feature has a prereq.** Either:
   - Promote the follow-up to a blocker in your epic
   - Land it as the first child task
   - Document in your feature's PR body (`aperture:name-the-blast-radius` — the follow-up is itself part of the contract change)

The audit is 5 minutes; missing it produces feature work that stalls when CI / Cipher review / E2E surfaces "wait, this assumes X is already done — but X is the open P2 we filed three weeks ago."

---

## Forward-friction check (apply at both ends)

**Bug-author at filing time:**

> Is this follow-up on a surface that's seeing active feature work? If yes — file as P1 with explicit "blocks future X" framing. If no — prove the "no" before defaulting to P2.

**Feature-designer at scoping time:**

> Pull the follow-up debt for this surface. For each open follow-up, does my new feature assume it's already closed? If yes for any — it's a prereq, not a nice-to-have.

Both checks take 5 minutes. Both have been observed to catch the failure mode banked above. The 6-recurrence rate suggests the pattern is structural; the discipline doesn't fully solve it but it shifts the catch from "feature stalled mid-cycle" to "feature scoped knowing the prereq" — which is materially cheaper.

---

## What this skill is NOT for

- **NOT** a rule against P2 follow-ups in general. P2 is a real priority for legitimately stable surfaces. The discipline is "prove the stability before defaulting to P2," not "ban P2."
- **NOT** a rule that every follow-up must be promoted to P1. The discipline applies on surfaces with active feature work; surfaces in maintenance mode genuinely admit nice-to-have items.
- **NOT** an excuse to inflate priorities for visibility. The discipline is "the follow-up is structurally a prereq" — not "I want this work done sooner so I'll call it P1."
- **NOT** about retroactive blame for past P2 filings that became blockers. The discipline is for the next filing + the next feature design, not for litigating the previous one.

---

## Bonus precedent (related but distinct pattern — single provenance for future watch)

`aperture-cea7` A11 caught a different but adjacent shape: Wheatley's recon spec for Wave 1 rate-limiting said to add Redis-backed rate-limiting "from scratch," but `REDIS_URL` was already wired in `server.ts`. The spec didn't catch existing infrastructure.

**Pattern shape:** "design-spec-says-add-X-from-scratch-but-the-codebase-already-has-X." Two parallel lessons:
- Scoping recon should grep for existing infrastructure before specifying inferior alternatives
- The security verdict (Cipher) catches this cheaply when filed in parallel with the implementation bead (B1) rather than serially

Single-provenance — filed as a 4la6 watch-list candidate for future-second-instance promotion. Distinct shape from filed-p2-becomes-prereq but adjacent in the "scoping recon misses important precedent on the surface" family.

---

## Cross-links

- **`aperture-4la6` anchor #19** (Cipher's "catches have tails" pattern) — adjacent. That's about REVIEWER catches generating follow-up beads; this skill is about those follow-up beads becoming prereqs for the next feature. The full life-cycle: review catches → tail bead filed → bead becomes prereq → feature prep audits it.
- **`aperture-4la6` anchor #27** (Vance's "ship surface, document deeper") — sibling on author-side discipline. That anchor is about naming the deeper architectural question when shipping a surface fix; this skill is about ensuring those deeper questions get priorities that match their future-blocking nature.
- **`aperture:spec-deviation-discipline`** (PR #11) — adjacent. That skill is about verifying spec internal consistency; this skill is about verifying spec against accumulated follow-up debt on the surface it touches.
- **`aperture:name-the-blast-radius`** (PR #20) — adjacent. When you find that a follow-up IS a prereq for your feature, the discipline of disclosing the contract change applies — the follow-up is part of what's changing.
- **`aperture:specialist-delegation §6`** (verify-against-reality, Cipher's principle family) — parent.

---

## Adding a new precedent

If you (or another agent) hit a new instance of the filed-P2-becomes-prereq pattern, bank it here. Same template:

1. **Bead chain** — citation
2. **Surface** — which route / page / file / module the follow-up lives on
3. **The follow-up** — what was filed at P2
4. **The feature that needed it** — what work later turned the follow-up into a prereq
5. **Cost of the surprise** — how much work stalled / had to rebase / had to wait

Six instances at saturation now. New precedents reinforce the pattern; an absence of new precedents would be a signal that the discipline is being applied successfully at filing time (the catch shifted upstream).

---

## Future synthesis note

The "ship surface, document deeper" anchor #27 (Vance) and the "catches have tails" anchor #19 (Cipher) and this skill are all on the same axis at different points: catches → follow-ups → prereqs. At 4la6's eventual synthesis cycle, watch whether these three collapse into one "discipline of follow-up-bead lifespan" piece or stay as three sibling skills. The principles are distinct enough today that three separate framings is the right shape; that may change as more instances refine the picture.
