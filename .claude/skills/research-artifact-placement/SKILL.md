---
name: research-artifact-placement
description: Where does a research artifact live — checked into the repo alongside the code it describes, or shipped as a skill in the aperture registry? Use any time you produce or commission a research artifact (recon doc, coverage map, route inventory, selector table, threat model, design contract, SEO audit, ADR-style design note) and have to decide where to put it. Triggers on "where should this doc go," "should this be a skill or a markdown file," "I wrote a recon doc," surface-map, coverage-map, route inventory, selector table, threat model, design reference, "is this a skill," generated artifact placement.
---

# Research Artifact Placement

A short decision rule (with one heuristic test) for every research artifact that comes out of the swarm.

This skill exists because we produce a lot of research artifacts — recon docs, coverage maps, route inventories, selector tables, threat models, design contracts, SEO audits, ADR-style design notes — and each one has to land *somewhere*. The wrong placement decays the artifact: a code-specific surface map shipped as a skill goes stale the moment routes refactor, while a generalized discipline checked into one repo benefits only the agent who finds it there.

Izzy authored the rule below after running this decision on her own surveys v1 SURFACE-MAP doc. The rule and the four precedents are her contribution; this skill formalizes them as a check the whole swarm can apply.

---

## The decision rule

> **Couple to surface vs cross-surface lesson.**
>
> If a research artifact maps **directly to a code surface** — its accuracy depends on routes/components/contracts that could refactor — **check it in alongside the code.** The file lives WITH the thing it describes; if the code moves, the doc moves with it or gets deleted in the same diff. Future authors grep it from the test/code directory naturally.
>
> If it's a **generalized lesson** — a discipline, a gotcha, a technique that transfers across surfaces — **ship it as a skill.** The whole swarm benefits; it can't go stale by code refactor because it isn't tied to one.

That's the rule. Apply it before you `git add` the markdown file.

---

## The heuristic test

When in doubt, ask:

> **Would this doc go stale the moment the code refactors?**

| Answer | Placement |
|---|---|
| **Yes** — its claims would become false if routes/selectors/components/contracts move | In-repo, alongside the code |
| **No** — it stays correct regardless of what changes around it | Skill in the aperture registry |

The test is one question because it cuts cleanly: staleness-on-refactor is a property of the artifact's *coupling*, not of its size or topic. A 5-line file that names two route paths goes in-repo. A 500-line discipline that doesn't reference any specific URL goes in a skill.

---

## Four precedents (from today's swarm work)

The four artifacts below were produced over a small window of swarm activity and each landed in the right place under the rule. They cover both directions of the decision and span different agents, surfaces, and artifact shapes — useful as worked examples.

### Precedent 1 — In-repo (couples to a code surface)

**`apps/frontend/e2e/surveys/SURFACE-MAP.md`** (Izzy, `aperture-zaw1`, monorepo-incluir PR #300)

Maps the surveys v1 routes, selectors, and behavioural pins as they exist *in this repo*. References `/home/admin/surveys`, the `SurveyListTable` component, the `.painel-row.soon` aria-disabled class, specific URL params. Lives at `apps/frontend/e2e/surveys/SURFACE-MAP.md` so the Playwright test files in the same directory can be diffed against it during refactors.

**Why in-repo:** every claim in the file is true *because* the current code says so. If admin surveys move to `/admin/v2/surveys`, the file goes with that diff or is rewritten in the same PR.

### Precedent 2 — Skill (generalized lesson, cross-surface)

**`.claude/skills/playwright-gotchas/`** (Atlas, aperture PR #8)

Banks three Playwright framework gotchas — `aria-disabled` makes `click()` spin-wait, `navigator.clipboard` needs explicit permissions, `page.goto(...).status()` must be checked before content assertions for 404 routes. None of them name a specific page, route, or component.

**Why skill:** these are Playwright-the-framework behaviours. They were *discovered* writing surveys tests, but they apply to any Playwright project the swarm ever touches. Refactoring monorepo-incluir doesn't change their correctness.

### Precedent 3 — Skill (discipline, not a code reference)

**Cipher's `verify-against-reality` principle** (referenced in `aperture:specialist-delegation §6`)

The rule that an agent must check their code against external state (DB rows, traces, prod observations) before signing off — not just against their own internal mental model.

**Why skill:** a discipline. It's not about "where is X defined"; it's a procedure to apply at debugging time, in any repo, on any feature. Refactor-immune by construction.

### Precedent 4 — In-repo (design contract for one project)

**Vance's Visual Identity Prompt** (eunenem-v2, lives under `reference/`)

The design contract for the EuNeném v2 frontend — fonts, color tokens, polaroid frame conventions, tape-SVG specifics. It describes how that one app should look and feel.

**Why in-repo:** even though it's prose-not-code, it couples tightly to the specific surface. The day eunenem-v2 redesigns or is replaced, the prompt either updates or is archived — and the right place for it to live is next to the code that implements it, where future Vance can read it as the canonical source.

---

## What this rule is NOT for

- **Operator briefs / BEADS task descriptions** — those live in BEADS, not as files. Don't file the operator's "build a survey system" brief as either an in-repo doc or a skill; it's a task description.
- **Status reports / completion summaries** — those go in BEADS notes or `close_task` reasons. Not docs.
- **Skill provenance citations** — when a skill banks a war story, the citation goes inside the skill (the source section), not as a separate artifact.
- **Messages between agents** — go through `send_message`, not files.

This skill is specifically about **standalone artifacts that an agent produces as a *thing*** — a coverage map, a recon doc, a threat model, a design contract — and where that thing should live.

---

## Forward-friction check (use at artifact-creation time)

Before you save the file, ask in this order:

1. **What does the artifact describe?** Code surface or transferable lesson?
2. **Run the heuristic:** would this go stale if the code refactored?
3. **Pick the placement:**
   - In-repo → put it next to the code it describes (the test directory, the component directory, a `reference/` subfolder in the app — whatever the local convention is). Filename like `SURFACE-MAP.md` / `THREAT-MODEL.md` / `DESIGN.md` so it grep's cleanly.
   - Skill → `~/.claude/skills/<name>/SKILL.md` with frontmatter, wired into the relevant agents' `skills.txt`. Use the `create-skill` workflow.
4. **Cross-link if useful.** An in-repo doc can reference a skill ("see `aperture:playwright-gotchas` for framework-level test rules"). A skill can reference an in-repo example as illustration without making the skill depend on it. The two layers compose.

If you're genuinely unsure after running the test, default to **in-repo**: a stale doc next to the code is easier to find and delete than a stale skill the whole swarm sees.

---

## Adding a new precedent

If you ship an artifact and the placement decision was non-obvious, bank the precedent here. Same shape:

1. **Artifact name + path** (or skill name)
2. **Author + bead/PR**
3. **Why it landed where it did** — one sentence pointing at the heuristic test's answer

Over time, the precedent list becomes the swarm's accreted intuition for the gray-zone cases.

Izzy authored the original rule; future precedents land via PR on the aperture repo.
