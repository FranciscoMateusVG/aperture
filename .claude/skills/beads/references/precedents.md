# BEADS Discipline — Precedents & Worked Examples

Companion to `SKILL.md`. Every block below was moved out of `SKILL.md` verbatim on 2026-09-06 so the boot-injected skill carries rules, not stories. Each heading names the SKILL.md section the block came from; the rule each story established still lives in `SKILL.md`.

---

## §0 — Operator directive on the creation gate (2026-07-29)

**Operator directive, 2026-07-29.** Bead creation is not an execution-layer action. It's a direction-layer decision — "does this deserve to exist as tracked work at all" — and direction lives with the operator and GLaDOS, nobody else.

> **Operator + GLaDOS are the brains and the direction. Specialist agents get shit done. That's the whole split.**

---

## §0 — The 300-bead noise incident behind the raised filing bar (2026-07-29)

Banked from a real incident (2026-07-29): the board accumulated 300+ open beads, most of them cosmetic nits, speculative "candidate" explorations, and parked decisions (`[operator-decides]`, `v1.1 OPTION: ...`) that were never going to be prioritized over real work. They didn't help — they buried the real P0s and P1s under noise, and gave specialists a plausible-looking-but-wrong item to self-start on instead of what actually mattered. GLaDOS bulk-closed all 307 of them in one pass. The lesson: **filing has to cost something, or it will be used for everything.**

---

## §3 — The raul-fitt incident: prior-art-check before kickoff epics (2026-08-23)

**Before filing a project-kickoff epic, run `aperture:prior-art-check` first** — a cheap sweep (local projects grep, git remote check, BEADS closed-history search) for whether the "new project" you're about to file is actually new. Banked from the raul-fitt incident (2026-08-23): GLaDOS nearly re-dispatched three months of already-verified work because a "new project" request was taken at face value and a design-tool link the operator handed over turned out to be a stale snapshot that had silently diverged from real, already-shipped local work. Run the sweep before `create_task`, not after a specialist has already spec'd against the wrong ground truth.

---

## §3 — Stale wiring instruction in older versions of this doc (verified 2026-07-29)

**⚠️ Older versions of this doc said to wire the epic as `blocked-by:<child>` via `bd dep add`/`bd create --deps`. That is WRONG for the current `bd` CLI — it rejects it outright: `Error: epics can only block other epics, not tasks`. The actual mechanism is structural parent-child, set via `--parent` on the child, NOT a dependency edge on the epic.**

---

## §3 — Worked example: epic dependency wiring on aperture-vsr9k (verified 2026-07-29)

Worked example (bd CLI, verified 2026-07-29 on aperture-vsr9k + 3 children):

```bash
# 1. File the epic, no children yet
bd create "Incluir Novas Features — autonomous Notion intake pipeline" \
  --type epic --priority 2 --label project:incluir \
  --description "VISION: ..." \
  --acceptance "≥3 end-to-end Notion→merged-PR cycles..." \
  --json
# → returns id: e.g. aperture-abcd

# 2. File a child task during scoping
bd create "Build Notion-API → BEADS sync worker" \
  --type task --priority 1 --label project:incluir \
  --json
# → returns child id: e.g. aperture-efgh
bd label add aperture-efgh project:incluir

# 3. Wire it under the epic — this is what actually gates the epic's close
bd update aperture-efgh --parent aperture-abcd

# (optional) sequential deps between siblings use plain bd dep add with bare IDs —
# NOT a "blocked-by:X" string as a single positional arg, that silently fails to
# persist. Positional order is (blocked-issue, blocking-issue):
bd dep add aperture-ghij aperture-efgh   # ghij is blocked by efgh, default type "blocks"
```

---

## §4 — notes-append fix history (aperture-e8qp)

(Fixed in aperture-e8qp. Earlier sessions document the old replace-by-default behaviour and the read-modify-write workaround — that workaround is no longer needed.)

---

## §4 — QA-gate acceptance override: three specialists in one session (2026-08-28)

**Banked precedent (2026-08-28, three separate specialists — Rex, Wheatley, Vance — hit this exact gap in one session):** all three closed a bead citing their own shipped PR / passing local gates, while the bead's own acceptance criteria said "Izzy re-gates / walks the journey before this ships." In each case Izzy's real re-gate came back with a genuine finding the specialist's own testing had missed (a prompt-cap edge case, a curriculum-scope violation, an a11y contrast miss) — proving the QA step wasn't ceremony, it was catching real gaps. GLaDOS caught the premature close each time, reopened, and the specialist self-corrected without pushback once it was pointed out — the instinct to apply the general PR-open rule is strong and needs an explicit override, not just good faith.

---

## §4 — Stacked-PR auto-close precedents (2026-05-14)

Precedents (2026-05-14): PR #237→#245 (Vance, aperture-abfs), PR #242→#244 (Rex, aperture-q2we).

---

## §4 — Tool-argument escaping: real precedent (2026-05-12)

**This footgun has bitten multiple agents and subagents in a single day.**

**Real precedent (2026-05-12):**
- Peppy's `aperture-z5ow` subagent: close_reason quoted "the `</reason>` field" → bead record has the truncated close-reason + a `</reason>` close-tag bleed visible in the persisted record.
- Multiple GLaDOS sessions: descriptions that documented THIS skill's warning by quoting the close-tag pattern produced the very bug they were warning about.

---

## §6 — Full Example Sequence

```
# 1. Find work
query_tasks(mode: "ready", project: "incluir")
# → task-456: "Add usuarios page to Secretaria nav"

# 2. Claim
update_task(id: "task-456", claim: true)
update_task(id: "task-456", status: "in_progress")

# 3. Work, log a discovery mid-way
update_task(
  id: "task-456",
  status: "in_progress",
  notes: "Nav link already exists — scope reduced to filter change only"
)

# 4. Store artifacts
store_artifact(task_id: "task-456", type: "file", value: "apps/frontend/src/app/home/admin/usuarios/page.tsx")

# 5. Close with summary
close_task(
  id: "task-456",
  reason: "Updated SECRETARIA filter to show only CONVIDADO users. Build passes."
)

# 6. Report to GLaDOS
send_message(to: "glados", message: "task-456 closed. Filter scoped down — nav was already there.")
```

---

## §7 — Filing a New Task — Complete Example

```bash
bd create "Add rate-limit middleware to /api/otel/v1/traces" \
  --description "Public OTLP ingestion endpoint has no auth or rate-limit. Add a per-IP rate-limit (60/min) plus body-size cap (1MB) before the proxy hands off to the backend. Without this we're a free relay for whoever finds the URL." \
  --type task \
  --priority 1 \
  --label project:incluir \
  --acceptance "Anonymous requests above 60/min return 429; bodies above 1MB return 413; existing legitimate traffic unaffected" \
  --json
```

That's a well-shaped task. Future agents claiming it know what to build, why it matters, and exactly when they're done.
