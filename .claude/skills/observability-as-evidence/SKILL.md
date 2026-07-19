---
name: observability-as-evidence
description: A discipline for reasoning from observability data — traces, logs, metrics, span enrichments. Each enriched field represents ONE specific subsystem's view of the world; reasoning about another subsystem's behaviour from it is a category error. Use any time you're triaging from a Tempo trace, Loki log, Grafana panel, or any enriched observability output — especially before drawing a conclusion that depends on which subsystem a field came from. Triggers on Tempo trace, span enrichment, span attribute, Loki log labels, `user.role`, BetterAuth, institutional permissions, authz from observability, recon from traces, "bonus finding worth confirming," cross-subsystem inference, enriched field provenance.
---

# Observability as Evidence

A short discipline for anyone reading observability data (Tempo traces, Loki logs, Grafana panels, span enrichments, metric labels). The rule is one sentence, the war story is concrete, and the failure mode generalizes far beyond the specific case that earned it the skill slot.

This skill is `aperture:specialist-delegation §6`'s **verify-against-reality applied recursively to observability data.** That principle says "check your code against external state — DB rows, traces, prod observations — before signing off." This skill is the next layer down: **even when you're using traces and logs as your external state, the data itself has provenance, and the provenance matters.** A field on a span is *one subsystem's worldview*. It is not ground truth about every subsystem.

---

## The rule

> **Enriched observability fields (span attributes, log labels, metric tags) represent ONE specific subsystem's view of the world. Reasoning about ANOTHER subsystem's behaviour from them is a category error. If an enriched field seems to answer your question, prove it answers your question by tracing the enrichment back to its source.**

That's it. The skill is one sentence + one war story + a forward-friction check at trace-read time.

---

## The failure mode (concrete)

Observability instrumentation is enriched by middleware at well-defined points in the request lifecycle. The enrichment captures the subsystem's state *at that point* — typically auth context, request shape, or domain-specific tags injected by handlers. A given enriched field is faithful to its source subsystem but says nothing about what other subsystems think.

The trap is that the field LOOKS authoritative. `user.role` on a Tempo span reads like a global property of "the user." It isn't. It's the value of one column from one subsystem (BetterAuth, in the precedent below), captured at request-time, faithful to that one slice. The institutional view of the user — what teams they're on, what permissions they have, what surface they're allowed to use — lives in entirely different tables and is never reflected in that enrichment.

When you reason about behaviour A from a field that represents subsystem B's truth, the answer is usually subtly wrong in a way that doesn't trip any alarms. The recon framing looks reasonable, the trace "supports" the framing, and the conclusion downstream agents build on the framing is silently incorrect.

---

## War story — `aperture-3hhp` (Wheatley, 2026-05-20)

**The investigation:** Operator reports a P1 bug — Fernanda Souza hits 403 on the `/presencas` page. Wheatley dispatches recon: trace pull from Tempo + code recon.

**The trace:** Subagent C pulled Tempo trace `755d732bf8a85b0e2a45c889dc4b1710` and surfaced the enriched span attribute `user.role=user`.

**The miss:** Wheatley flagged `user.role=user` as a "bonus finding worth confirming" but didn't follow up. The initial recon framing was *"Fernanda is a voluntario [user.role=user]; voluntarios can't list users [requireAdmin gate on /api/users]; ergo the bug is voluntario-can't-list-users."* That framing built three downstream conclusions on the implicit premise that `user.role` represents Fernanda's institutional position.

**Operator pushback:** "Fernanda is on the gestão-de-pessoas team. She should have a gestão role, not voluntario."

**Re-recon (Subagent A, DB query via `aperture:incluir-prod-postgres`):**

- `user.id`: `c2fc927e-c216-4b16-a7b9-88356b771c02`
- `user.role` (BetterAuth column): `'user'` ← the field the Tempo span surfaced
- `volunteers.status`: `'active'`
- `volunteer_permissions`: **`['gestao_de_pessoas', 'secretaria']`** ← the field the Tempo span did NOT surface
- Cross-check: one of 6 active gestao_de_pessoas team members in the system

Fernanda *is* on the gestão-de-pessoas team, exactly as the operator said. The original recon's framing — *"Fernanda is a voluntario"* — was schema-flattening: treating "her BetterAuth role is 'user'" as equivalent to "her institutional position is unprivileged." Two completely different authz layers.

**The actual bug** (re-framed): the `/presencas` page is intended for gestão-de-pessoas team members. It calls `/api/users` which is gated by `requireAdmin()` (checks `user.role === 'admin'`, ignores `volunteer_permissions` entirely). One of the page's intended consumers triggers a call that's gated wrong. The bug is a **contract mismatch** between the FE call site and the BE auth gate — *not* a user-doesn't-have-permission bug.

**Why the trace looked authoritative:** BetterAuth middleware enriches every authenticated span with `user.id` + `user.role`. That's its job — it's the auth-layer telling Tempo what the auth-layer knows. The `volunteer_permissions` join table is never consulted by BetterAuth, never makes it onto the span. The Tempo enrichment is faithful to *BetterAuth's* worldview; treating it as the *system's* worldview was the category error.

---

## Common shapes (where this generalizes)

The 3hhp case is one instance of a broader pattern. Watch for the same trap in:

| Enrichment | Subsystem it represents | Easy to mistake as |
|---|---|---|
| `user.role` (BetterAuth) | System-level auth role | Institutional / domain / team role |
| `tenant_id` (multi-tenant middleware) | Routing-layer tenant binding | Business-layer tenant relationships |
| `request.session.userId` (session middleware) | Session-layer identity | Authenticated business identity |
| `feature_flag.X` (flag-eval middleware) | Flag-service worldview | Actual feature exposure to user |
| `experiment.bucket` (A/B middleware) | Bucketing-service decision | What user actually saw in UI |
| Loki `level=error` from app A | App A's classification | Cross-app error severity |
| OTel `service.name=foo` | Service-mesh registration | Actual code identity (in monorepos with shared services) |

The common shape: **a single string-typed field that LOOKS like a global property but is really one subsystem's local truth.**

---

## Forward-friction check (apply at observability-read time)

Before you draw a downstream conclusion from an enriched field, ask:

1. **Which subsystem injects this enrichment?** (Auth middleware? Tenant resolver? Flag service? Session layer?)
2. **Does that subsystem actually know the answer to my question?** Or does the question belong to a different subsystem?
3. **If different — where does the other subsystem's truth live?** (Join table? Other span? Out-of-band query?)
4. **Can I cheaply verify against that other source before committing to the downstream conclusion?** (DB query, secondary trace, code grep)

If you skip the check, you risk shipping a recon framing that's faithful to one subsystem and wrong about the actual question. The check is 30 seconds to 5 minutes; the cost of skipping is a downstream investigation chain built on the wrong premise — every conclusion past that point inherits the error.

---

## "Bonus findings worth confirming" — DON'T DEFER

A specific failure mode worth naming: when a recon dispatcher (the orchestrator agent, or you yourself when synthesizing) labels an observability field as a "bonus finding worth confirming" but doesn't immediately confirm it, that field becomes implicit-premise material. Downstream conclusions silently rest on the assumption that the bonus finding *would* have confirmed.

In `aperture-3hhp`, the Tempo `user.role=user` was exactly this kind of bonus finding. It was flagged, deferred, and then the recon framing implicitly relied on it. The deferred verification never happened; the conclusion shipped on the assumption.

**The rule:** *if an observability field is load-bearing for any downstream conclusion in your recon, it's not a "bonus finding worth confirming" — it's the premise, and the verification is critical-path.* Either verify it now, or remove the downstream conclusion that depends on it.

If you find yourself writing "worth confirming" or "would be useful to verify" about a field you're about to reason from, stop. The note is the bug. Confirm before continuing, or change the framing to not depend on the unverified field.

---

## What this skill is NOT for

- **NOT** a directive to distrust all observability data. The data is real and useful; the discipline is about provenance, not paranoia. Use traces and logs aggressively — just verify the layer.
- **NOT** a replacement for traces and logs as primary investigation tools. They surface evidence faster than DB queries or code reads. The discipline applies to *interpretation*, not to *use*.
- **NOT** about correctness bugs in the instrumentation. If a span enrichment is wrong about its OWN subsystem, that's a separate class of bug. This skill is about correct enrichment being mis-applied to the wrong question.

---

## Source provenance

| Bead | PR / Trace | Agent | What was enriched | What was inferred (wrong) | What was actually true |
|---|---|---|---|---|---|
| `aperture-3hhp` | Tempo trace `755d732bf8a85b0e2a45c889dc4b1710` | Wheatley | `user.role=user` (BetterAuth) | "Fernanda is a voluntario; can't list users" | Fernanda has `volunteer_permissions=['gestao_de_pessoas','secretaria']`; gate-vs-page is a contract mismatch |

**Single-provenance ship** per GLaDOS — the principle is categorical enough that one war story articulates the discipline cleanly. If a second instance surfaces (a different agent making the same category-error from a different enrichment field), bank it as a new precedent under the "Adding a new precedent" scaffold below.

**Class-diagnosis credit: Wheatley.** Wheatley made the original miss, recognized the failure mode after operator pushback, and articulated the discipline in `aperture-3hhp`'s "Banked lesson" section. The skill exists because he banked the lesson from his own miss — that meta-move (recon agent bankrolling their own framing failure as a swarm-wide discipline) is itself worth noting.

---

## Cross-links

- **`aperture:specialist-delegation §6`** (Cipher's `verify-against-reality` principle) — this skill is that principle applied recursively to observability data. Verify-against-reality: check your code against external state. This skill: even when the external state IS observability data, that data has provenance — check the provenance against the question you're answering.
- **`aperture:e2e-catches-what-lower-cant`** — sibling on test-apparatus-vs-reality. That skill is about test apparatus bypassing the failure surface. This skill is about observability apparatus surfacing one subsystem's truth and being mistaken for global truth. Same family ("apparatus has a worldview that may not match what you're reasoning about"), different domain.
- **`aperture:investigator-mode`** (when PR #7 merges — Wheatley-only recon discipline) — observability-as-evidence is the data-trust layer Wheatley's recon workflow depends on. When investigator-mode lands, a reciprocal cross-link from there to here lets future recon dispatches surface this skill at the right moment in Wheatley's process.

---

## Adding a new precedent

If you (or another agent) hit a different observability enrichment / different subsystem-confusion shape, bank it here. Use the same template:

1. **Bead + PR / trace ID** — citation
2. **Enriched field + injecting subsystem** — what was surfaced and by whom
3. **What was inferred (wrong)** — the recon framing that depended on the implicit premise
4. **What was actually true** — the ground-truth answer once the right subsystem was queried
5. **How the discrepancy was caught** — operator pushback? sibling agent? self-catch on review?

The principle's umbrella stays one sentence. The precedents are the swarm's accreted catalog of "ways an enriched field can lie about a question it doesn't actually answer."
