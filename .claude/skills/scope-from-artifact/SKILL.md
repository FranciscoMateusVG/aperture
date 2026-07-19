---
name: scope-from-artifact
description: Scoping discipline — when a feature has an operator source-of-truth artifact (spreadsheet, mockup, written spec, screenshot), scope from THAT, not from the existing schema or code. Use any time you are scoping new work where an operator-facing artifact and an existing implementation/schema both exist. Triggers on: scoping briefs, spec authoring, epic decomposition, Wheatley dispatches, "we already have a table for this" reasoning, "let's reuse the existing model" reasoning.
---

# Scope From the Artifact, Not the Schema

When an operator-facing artifact (spreadsheet, mockup, screenshot, written spec, Notion doc) defines what the feature should do, **the artifact is the ground truth.** Scope from it directly. Do NOT scope from the existing schema or existing code that "looks related" — those carry shape assumptions from prior decisions that may not match what the operator wants.

If the existing schema disagrees with the artifact, **the artifact wins** and you propose a schema change. Going the other way — bending the artifact to fit the schema — produces feature work that compiles and ships, but doesn't do what the operator asked.

---

## The recurring failure mode

The trap looks like this:

1. Operator asks for feature X with a source artifact (a spreadsheet column layout, a Figma mockup, a written description).
2. Scoper opens the codebase, finds an existing table or component that "looks like" the right model.
3. Scoper writes the spec using the existing model's shape — its keys, its joins, its abstractions.
4. Implementation ships. Tests pass. PR merges.
5. Operator opens the live feature and says: **"this is wrong, it has nothing to do with what I asked."**

The implementation matched the spec; the spec matched the schema; the schema didn't match the operator. Every layer downstream of the misread propagates the wrong shape.

---

## The discipline

When dispatching a scoping task — or when scoping yourself — follow this order:

1. **Read the operator artifact FIRST**, before any codebase recon.
   - For spreadsheets: open the actual file. Parse the rows + columns + sheet structure. Note what the axes ARE.
   - For mockups: identify what's on the screen, what's interactive, what's grouped.
   - For written specs: extract the entities, the relations, the actions. Underline anything that implies a data model.
2. **Derive the conceptual data model from the artifact**, not from existing tables.
   - "Rows = X, columns = Y, cells = Z" → that IS the model. Write it down.
   - If the artifact has no obvious axes (free-form copy), enumerate the entities + actions explicitly.
3. **THEN look at the existing schema / code** and ask: does the existing shape match the conceptual model from step 2?
   - If YES → reuse it. Note in the scope that the existing table is the right fit and why.
   - If NO → propose a schema change. Specify the new shape, the migration, and what existing data (if any) gets backfilled.
4. **Never write a spec that bends the artifact to fit the schema.** If you find yourself saying "we'll add a turma_id because the existing table requires one," stop. That's the failure mode. The artifact didn't ask for turma_id; the schema did. Propose schema change instead.

---

## Worked example (banked precedent, 2026-05-27)

**The miss:** 75he "Gestão de Faltas" module. Operator's source-of-truth was the `Gestão de Faltas` sheet in `Planilha - Gestão de Pessoas 2026.1.xlsx`:

  Row 2 headers: `Nomes dos voluntários | DATE_1 | DATE_2 | ... | "28/03 (sem aula)" | ...`

The artifact's axes are unambiguous: **rows = volunteers, columns = program-wide days, cells = present-or-not**. No turma anywhere.

Wheatley scoped without reading the spreadsheet. He looked at the existing tables — `class_meeting_dates` (per-turma) and `volunteer_class_attendance` (volunteer × class meeting) — and wrote the spec around them. The per-turma shape pulled the entire spec the wrong way: turma selector, tipo de sessão filter, per-class-meeting cells.

Three PRs merged on the wrong shape (#438 curation, #447 frontend matrix) plus one in-flight backend (#444). Operator opened the live page and said: *"this is wrong and has nothing to do with the excell sheet."*

**The cost:** 3 merged PRs needing rework, ~1 day of specialist work undone, P0 reschedule, schema redesign required (`program_days` + `volunteer_attendance` to match the per-day shape the artifact actually called for).

**What would have caught it:** opening the spreadsheet at scoping time, reading sheet 2 (`Gestão de Faltas`), seeing the two header groups (`Dias de organização` + `Dias de aula`) and the per-day cell structure. 5 minutes. Catches the entire cascade.

---

## The recursive trap — scope-from-artifact alone is not enough

**Banked precedent 2026-05-27 (the same day this skill was created):** the discipline correctly fired on the 75he Faltas miss — we read the spreadsheet, derived the volunteer-by-day model, refactored the schema. Shipped #450 + #452 with `program_days` and `volunteer_program_attendance` tables.

Then operator opened the live page and asked: *"how come there are no days? there is literally a table called `semester_dates` with active semester...."*

`semester_dates` already had **14 rows for the active 2026.1 semester** — populated, curated, semester-FK'd. And the student-attendance system at `apps/gestao-de-pessoas/src/app/(dashboard)/presencas/` was ALREADY using `semester_dates` as the date axis for a volunteer × date matrix. Same conceptual problem, working pattern in the same monorepo. We built a parallel system.

The original miss was scoping from the wrong schema (`class_meeting_dates` per-turma) — schema-side error. This second miss was scoping from the artifact but **not checking whether existing tables/components already solved the same problem in the right shape** — schema-side blindness.

### The two disciplines must BOTH fire

1. **`scope-from-artifact`** (this skill) — read the operator's source-of-truth artifact, derive the conceptual data model from it. Don't bend the model to fit existing schema.
2. **`pre-scoping-grep`** (BEADS memory `pre-scoping-grep-removes-already-decided-questions`) — after deriving the conceptual model, grep the codebase for existing tables, components, or patterns that already solve the same conceptual problem.

These are NOT redundant. They catch opposite failure modes:

| Failure mode | What scope-from-artifact catches | What pre-scoping-grep catches |
|---|---|---|
| Wrong-schema-pulls-spec-wrong | ✅ (forces re-deriving from artifact) | ❌ (codebase grep would just confirm the wrong schema) |
| Duplicate-of-existing-good-shape | ❌ (artifact says nothing about what already exists) | ✅ (grep surfaces the existing solution) |

### The required two-pass procedure for any scoping task

**Pass 1 — Artifact-grounded conceptual model:**
- Read the operator artifact (spreadsheet, mockup, screenshot, spec doc)
- Derive: rows = X, columns = Y, cells = Z, entities = ...
- Write this section FIRST in the scoping note

**Pass 2 — EXHAUSTIVE existing-schema enumeration (NON-NEGOTIABLE):**

This is where the discipline has repeatedly failed. The trap is to grep for the CONCEPT (e.g. "attendance"), find the FIRST candidate that looks similar, and stop. **That's not enough.** The recurring miss shape is: a candidate exists but isn't THE existing table — there's another table with a plainer name that's the real owner of the data.

**MANDATORY at scope-time, before drafting any new schema or query:**

1. **List every table in the database.** For Postgres prod:
   ```bash
   ssh xerox "docker exec compose-override-solid-state-port-349ude-postgres-1 psql -U incluir -d incluir_hono -c '\\dt'"
   ```
   Read the WHOLE list. Don't filter by prefix.

2. **For each table that matches the conceptual entity even loosely**, inspect schema + rowcount:
   ```bash
   docker exec ... psql -c '\d table_name'
   docker exec ... psql -c 'SELECT count(*) FROM table_name;'
   docker exec ... psql -c 'SELECT * FROM table_name LIMIT 5;'
   ```
   Looking at "attendance" → check every table containing `attendance`, `presen`, `frequ`, `check_in`, `register`. Don't stop at the first match.

3. **For each candidate**, ask: is this row-data the operator would consider "the existing data for X"? A 1265-row table that's been live for months IS the answer. A new empty 0-row table you just created is NOT.

4. **List every component / page / route file** that touches the same axes:
   ```bash
   grep -rln 'attendance\|presen\|table_name' apps/ --include='*.ts' --include='*.tsx'
   ```
   Read the actual code paths. Find what already works, mirror its shape.

5. **Write Pass 2 results in the scoping note BEFORE Pass 3.** Required section template:
   ```
   ## Pass 2 — Existing-schema enumeration
   - Tables matching conceptual entity (from \dt):
     - table_a (rowcount: N) — purpose: ...
     - table_b (rowcount: N) — purpose: ...
     - table_c (rowcount: N) — purpose: ...
   - Components/pages touching this domain:
     - file_a.tsx — does ...
     - file_b.ts — does ...
   - Conclusion: <use table_X with these JOINs / extend table_Y / propose new table_Z because no existing table fits>
   ```

**Pass 3 — Reconcile:**
- If an existing table holds the data → reuse it. Even if it has fewer fields than your ideal schema. Joining > inventing.
- If multiple candidate tables exist → pick the one with REAL DATA. A 1265-row table beats a 0-row table every time.
- If the existing tables genuinely don't match → propose a schema change AND explicitly enumerate (in the scoping note) every existing candidate you ruled out and why.
- If you find a working precedent for the same conceptual problem (e.g. student attendance matrix → mirror it for volunteer attendance), call it out in the scope as "PATTERN TO MIRROR" and reference the file paths.

### Banked precedent #2 (2026-05-27, third attempt at Faltas)

After the first two misses were corrected, the third refactor (`aperture-0uch` → PR #453) STILL got the existing-schema enumeration wrong. The chain:

1. **Attempt 1**: Built per-turma schema (`class_meeting_dates`, `volunteer_class_attendance`). Operator caught: spreadsheet has no turma axis.
2. **Attempt 2**: Built `program_days` + `volunteer_program_attendance`. Operator caught: `semester_dates` already existed with 14 rows of curated dates.
3. **Attempt 3**: Refactored to use `semester_dates` for dates BUT created a new `volunteer_semester_attendance` table for the cell data. Operator caught: `volunteer_attendance` had been on prod the whole time with **1265 rows of real check-in data** (timestamps, GPS coordinates, the works).

Each refactor cost ~2 hours of specialist time. The recursive failure was always the same: enumerated SOME tables matching the conceptual entity, stopped at the first candidate, didn't check what was actually populated with real data.

**The actual fix on attempt 3 was a JOIN**, not a new schema:
```sql
SELECT v.id, sd.date,
       EXISTS(SELECT 1 FROM volunteer_attendance va
              WHERE va.volunteer_id = v.id
                AND va.check_in::date = sd.date
                AND va.deleted_at IS NULL) as present
FROM volunteers v
CROSS JOIN semester_dates sd
WHERE v.status = 'active' AND v.deleted_at IS NULL
  AND sd.semester_id = (SELECT id FROM semesters WHERE is_active = true)
  AND sd.deleted_at IS NULL
ORDER BY u.name, sd.date;
```

No new tables. No lifecycle filter. Just a JOIN of three existing tables.

Operator's framing at the close: *"i mean, you and wheats should start looking AT WHAT WE ALREADY HAVE before creating crazy tables or data."* Banked verbatim — that's the rule.

The three soft-marked dead tables (`volunteer_class_attendance`, `volunteer_program_attendance`, `volunteer_semester_attendance`) all stay in the schema per operator's keep-everything stance. Operator will clean up later. The lesson is in the cost paid, not in dropping the artifacts of the failure.

### Anti-pattern: skipping Pass 2 after Pass 1 succeeds

When the artifact-derived model is crisp and obvious, it's tempting to dive straight into schema design. **Don't.** The recursive trap is: the model is right, the schema you propose is also right in isolation, but a parallel-and-redundant schema already exists. You waste hours building what's already built.

The two-pass procedure is cheap (10-15 min of grep). Skipping it costs hours of rework.

### Banked discipline (mandatory for all scoping going forward)

Every scoping note must have BOTH sections, in order:

```
## Source artifact (Pass 1)
- Path / link: ...
- Conceptual model derived from artifact: ...

## Existing-solution sweep (Pass 2)
- Tables grepped: ...
- Components grepped: ...
- Existing solutions found: ...
- Reuse decision: <reuse / extend / replace because X> 
```

If the second section is empty or missing, the scope is incomplete. Don't dispatch implementation without it.

---

## When this is NOT the trap

Some features don't have an operator-facing source-of-truth artifact — they're internal infra, refactors, or implementation-detail choices. In those cases:

- Existing schema and code ARE the ground truth.
- Pre-scoping-grep discipline (see the related BEADS memory `pre-scoping-grep-removes-already-decided-questions`) applies — read the codebase before asking the operator questions the code already answers.

The artifact-wins rule only fires when an operator-facing artifact exists and disagrees with the existing implementation. If you're not sure whether an artifact exists, **ask the operator** before scoping.

---

## The orchestrator's side (GLaDOS)

Before dispatching a scoping task to Wheatley (or any scoper):

- **Name the artifact explicitly in the dispatch.** "Open `Planilha - Gestão de Pessoas 2026.1.xlsx`, sheet `Gestão de Faltas`, and scope from that." Don't leave it implicit.
- **Forbid implicit-schema scoping.** Add to the dispatch: "Scope from the artifact, not from the existing schema. If the existing tables don't match the artifact's shape, propose a schema change."
- **Review the scope before approving.** Read it asking: does the spec's data model match the artifact's axes? If the spec talks about `turma_id` when the artifact has no turma column, that's the smoke signal.
- **When operator says "this is wrong, has nothing to do with the artifact"** — that's banked precedent of this exact failure. Stop further work on the wrong shape immediately, re-dispatch scoping with explicit artifact grounding.

---

## The scoper's side (Wheatley + anyone scoping)

Your scoping note MUST have this section near the top:

```
## Source artifact
- Path / link: <exact location>
- Sheet / page / section: <which part>
- Conceptual model derived from the artifact:
    - Rows: <axis>
    - Columns: <axis>
    - Cells / interactions: <semantics>
    - Entities: <list>
```

This section comes BEFORE the "audit existing code" section. The order matters. Codebase recon comes AFTER the artifact-grounded model, never before.

If you find that the existing schema doesn't match the artifact:

- Say so explicitly in the scoping note: "Existing `X` table does NOT match the artifact's shape because Y."
- Propose the schema change with rationale.
- Do NOT silently bend the spec to fit existing tables.

---

## Related disciplines

- **`cipher-verify-reality`** (BEADS memory) — verify against the actual artifact, not the model of the artifact. This skill is the scoping-layer recursion of that principle.
- **`pre-scoping-grep-removes-already-decided-questions`** (BEADS memory) — companion: when scoping, grep the codebase for already-answered questions before asking the operator. Different problem, same family (artifact-grounded vs. implementation-grounded).
- **`verify-against-origin-main-not-local`** (BEADS memory) — verify against canonical reality, not local cache. Same family.

The unifying frame: **at every layer (scoping, implementation, verification, claims-to-operator), check against the ground truth artifact appropriate to that layer.** For scoping, the ground truth is the operator's source-of-truth artifact. For implementation, it's the spec. For verification, it's the deployed surface a real user touches. The recursion is the discipline.
