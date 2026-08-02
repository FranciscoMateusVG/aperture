# Spec: MCP Payload Truncation + Investigation-Scoping Discipline

**Bead:** aperture-jingp (child of epic aperture-lquj5, "Context Efficiency")
**Author:** Wheatley, 2026-08-02
**Status:** PENDING APPROVAL — GLaDOS + operator. Spec only; no implementation until approved.

---

## Deliverable (a) — `bd show` / `query_tasks(mode:"show")` default truncation

### The bug (as root-caused in the epic, confirmed on-file)

`mcp-server/src/beads.ts` line 234–236:

```ts
if (mode === "show" && id) {
  // Always return full detail for a single task — no filtering, no projection
  return runBd(["show", id, "--json"]);
}
```

`list`/`ready` modes already have working projection five lines below (`SUMMARY_FIELDS` / `TRUNCATED_FIELDS` / `summarizeTask` / `projectFields`, lines 159–227). Show bypasses all of it. Observed cost: aperture-jxwbd dumped 82,830 chars from one call; long-lived beads with append-only notes are unbounded.

### NEW finding during this spec's recon — nested dependency amplification

`bd show --json` embeds **full records of every dependency** in the `dependencies[]` array — including the complete description + acceptance criteria of parent epics. Empirical receipt from this session: `show aperture-jingp` (a young bead, ~3.1KB description, 2 short notes) returned **10,381 bytes**, because the full aperture-lquj5 epic record (~4.5KB description alone) rode along inside `dependencies[]`. Any child of a fat epic inherits the epic's full weight on every show. The fix must project nested dependency records too, or the per-bead caps are defeated.

### Proposed fix

**Three-tier projection.** Extend the existing `QueryFields` type from `"summary" | "full"` to `"summary" | "detail" | "full"`. Mode defaults:

| Mode | Default tier | Rationale |
|---|---|---|
| `list` / `ready` | `summary` (unchanged) | scan intent — id/title/status is the payload |
| `show` | **`detail`** (new) | deliberate single-task lookup — higher intent than a list scan, deserves more than 200 chars, but not unbounded history |
| any mode + `fields:"full"` | `full` | exact-state resume, explicit opt-in |

**The `detail` tier** (new function `detailTask`, sibling of `summarizeTask`):

- **All scalar/meta fields pass through**: id, title, status, priority, issue_type, assignee, owner, labels, timestamps, created_by, parent, acceptance_criteria (acceptance criteria are the "definition of done" — an agent showing a bead to work it needs these complete; they are authored-once and bounded in practice).
- **`description`: head-truncated at 4,000 chars.** Descriptions are authored-once work briefs; nearly all fit (jingp's own is ~3.1KB and was needed in full to do this task — 200 chars would have been useless). Cap protects against pathological cases only. Marker appended on truncation: `…[description truncated: 4,000 of N chars — fields:"full" for complete]`.
- **`notes`: TAIL-truncated at 3,000 chars** — this is the key design choice. Notes are an append-only chronological log; the *most recent* entries are what a "check this bead" or post-/compact resume actually needs. List-mode's head-truncation is right for identification; show-mode wants the tail. Prefix marker on truncation: `[notes truncated: showing last 3,000 of N chars (M total entries) — fields:"full" for complete history]`.
- **`dependencies[]`: each nested record passed through `summarizeTask`** (the existing 200-char summary), plus `dependency_type`. A show call is about THIS bead; deps are context pointers — anyone needing a dep's full state shows it directly.
- **`_truncated: true`** flag whenever any field was cut, same convention as summary mode, so callers programmatically know more exists.

**Why 4,000/3,000 and not 200:** show is a deliberate lookup (the caller already knows which bead they want — intent is high); the failure mode being fixed is the unbounded *history* tail, not the bounded *brief*. Worst-case detail payload ≈ 8–10KB vs. today's unbounded (82.8KB observed). Typical beads come through byte-identical to today (under caps → no truncation, no markers). Constants live next to `TRUNCATE_AT` and are trivially tunable.

**Opt-in mechanism: reuse the existing `fields` param.** It's already in the query_tasks zod schema; show currently ignores it. Wire `fields:"full"` on show mode → today's exact behavior (unconditional `bd show --json` passthrough). `fields:"summary"` on show → summarizeTask (cheap existence/status check). Zero new API surface; tool description text updated to document the tiers.

### Backward-compat check

| Consumer | Impact | Verdict |
|---|---|---|
| Specialist cold-start anchors (specialist-delegation §8: "bank state in bead notes, read after /compact") | Anchors are the most *recent* notes → tail-truncation preserves them. Oversized anchors surface `_truncated` + the caller escalates to `fields:"full"`. | Safe; capability retained via opt-in |
| GLaDOS watch-protocol progress reads | Reads recent progress → tail suffices | Safe |
| Workflows parsing show JSON | Output stays valid JSON; markers are inside string values; `_truncated` is additive. Naive string-matching on notes content could see the marker text — called out for reviewers, considered acceptable | Safe with note |
| Direct `bd show --json` via Bash | Unaffected — this changes only the MCP wrapper. CLI remains the raw escape hatch | N/A |
| Codex-backed agents (rex/izzy/cipher via codex-bridge) | Same MCP tool surface → same benefit; no separate path found in skeleton check of codex-bridge.ts | Safe |

### Deployment

`just build-mcp`; mcp-server is a per-agent stdio subprocess, so rollout lands on next agent restart — non-disruptive, same pattern as the skills.txt fix.

### Verification plan (before/after numbers for the epic's acceptance)

1. Before: `query_tasks(show, aperture-jxwbd)` → record byte count (epic documents 82,830).
2. After rebuild: same call → expect ≤ ~10KB with `_truncated: true` and tail-notes visible.
3. Regression: show on a small bead → byte-identical fields to today, no markers.
4. Opt-in: `fields:"full"` → byte-identical to pre-fix output.

---

## Deliverable (b) — Sibling MCP tool audit

Method (and live demonstration of deliverable (c)): `ls` + `wc -l` for the file inventory (10 files, 2,323 lines), one rg pass for function skeletons + return statements, one rg pass for tool registration, then exactly two targeted reads (beads.ts §155–337, index.ts handler blocks). Two one-line empirical probes (`wc -c` on live tool output). Total ≈ 8 tool calls. Findings:

| Tool | Verdict | Detail |
|---|---|---|
| `query_tasks` (show) | **BUG** — the known one, plus nested-dependency amplification (new, see (a)) | fix per (a) |
| `update_task` | **SIBLING CONFIRMED** | Handler returns raw `bd update --json` echo = the FULL task record (full description + full accumulated notes + labels + meta). Empirically measured this session: **4,259-byte echo for a one-line note append** on a young bead; scales with total record size. This tool fires on EVERY progress note, claim, and status change — the highest-frequency mutation in the fleet. On an 82KB-notes bead, every append echoes ~82KB back to the writer. Arguably worse than show in aggregate: cost = record size × every mutation by every agent. |
| `close_task` | **SIBLING CONFIRMED** | Same full-record echo via `bd close --json`. Once per task lifecycle, but fires exactly when notes history is at its maximum. |
| `store_artifact` | **SIBLING CONFIRMED** | Delegates to `bd update --append-notes` → same full echo, plus the handler *prepends* `Artifact stored: type:value` before it. |
| `create_task` | Clean-ish | Echoes the just-created record — content is what the caller just authored, bounded. Optionally give it the compact-ack treatment for consistency; not required. |
| `search_tasks` | **CLEAN** | Projection wired, summary default — the reference implementation alongside list. |
| `query_tasks` (list/ready) | **CLEAN** | The reference implementation. |
| `get_messages` | Clean by design, one note | Full message content IS the payload (truncating would break comms). Unbounded replay (`-n 0`) after long offline could be large; replay-until-ack is the intended feature. Optional future hardening: cap per call at N messages with a "M more pending — call again" marker. Not in this fix wave. |
| `mark_as_read` | **CLEAN — and the model fix pattern** | Discards the bd echo entirely, returns a one-line ack. This is what the mutation tools should do. |
| `send_message` | CLEAN | Fixed-string ack (verified live this session). |
| `get_identity`, `list_objectives`, `update_objective` | CLEAN | Small bounded payloads. |
| codex-bridge.ts / ws-hub.ts / hub-notify.ts / hub-client.ts | Out of scope | Transport machinery, not payload-returning MCP tools. Skeleton-checked only, per the discipline this spec exists to bank. |

### Proposed fix for the three confirmed siblings (update_task / close_task / store_artifact)

Replace the raw echo with a **compact ack**, mirroring mark_as_read:

- `update_task` → `Updated <id> (<status>): <what changed — e.g. "note appended (312 chars)", "claimed by wheatley", "status → in_progress">`
- `close_task` → `Closed <id>: <first 120 chars of reason>`
- `store_artifact` → `Artifact stored on <id>: type:value` (drop the trailing echo it currently appends)

The writer already knows what they wrote; echoing their own mutation back — riding on the entire accumulated record — is pure waste. Error paths unchanged (bd failures still surface via the existing catch). If reviewers want mutation results to remain inspectable, alternative: return `summarizeTask(record)` instead of a string ack — still bounded, slightly richer. My recommendation is the string ack; anyone needing post-mutation state calls show (which, post-(a), is cheap).

Backward-compat: no skill or workflow found that parses update/close echo output (the callers treat it as fire-and-forget; today's usage in every agent transcript reads the ack visually at most). Risk is low; flagged for reviewer confirmation.

---

## Deliverable (c) — Discipline amendment: "Gather Cheap, Escalate Targeted"

### Placement decision (as tasked: argued, my call)

**Primary: `aperture:subagents`, as new §12.** The banked failure happened at *brief-authoring time* — a dispatcher wrote "read all 5 design docs in full, do not truncate for brevity" into a subagent prompt. The subagents skill is what's loaded at exactly that moment, and its §4 ("Writing a Good Prompt") is the natural neighbor. **Secondary: short cross-link subsection in `cost-proportional-orchestration` (§2d)** — that skill's audience (orchestrator at sizing time) applies the identical proportionality logic, but to information gathering instead of agent count. Full text lives in ONE place (subagents) to avoid duplication drift; the cross-link is three sentences.

### Ready-to-land text — subagents SKILL.md, new §12

```markdown
## 12. Scope the Brief: Gather Cheap, Escalate Targeted

Investigation dispatches — and your own inline recon — default to **skeleton-first**:
grep headers, signatures, status lines, titles + first paragraphs. Deep-read ONLY the
specific artifact Phase 1 flags as load-bearing for the actual question. "Read
everything in full / be exhaustive" is an anti-default: it must be justified by the
question, never by thoroughness anxiety.

**The two-phase shape:**

1. **Phase 1 — gather cheap.** Enumerate + skim structure: `ls` + `wc -l`, `rg -n` for
   headers/signatures/return statements, doc titles, status lines. Cost scales with
   STRUCTURE, not content.
2. **Phase 2 — escalate targeted.** Deep-read only the items Phase 1 flagged. Escalation
   is cheap and encouraged when a flag fires; blanket deep-reads are the waste.

**Brief-authoring rules (for the dispatcher):**

- State the QUESTION the dispatch must answer, not the corpus it must consume.
  ✅ "Return a one-line summary of each design doc's purpose"
  ❌ "Read all 5 design docs in full, don't truncate for brevity"
- Match depth mandate to output size. A deliverable of N one-liners NEVER justifies
  N full-document reads.
- The phrases "in full", "exhaustively", "don't truncate" are red flags in a brief.
  Keep them only when the deliverable IS the full content (a migration touching every
  line, a byte-level audit).
- Give the subagent an output budget ("report in ≤ 30 lines"). Output budgets
  discipline input gathering.

**Reconciliation with investigator-mode's depth mandate:** investigator-mode §3 says
"enumerate ALL instances, don't stop at the first." That is a COVERAGE rule for
Phase 1 — enumerate the full checklist cheaply — not a mandate to deep-read every
item. Enumerate everything; deep-read selectively. The two disciplines compose:
breadth at the skeleton layer, depth only where flagged.

**Worked example (banked 2026-08-02, the Hermes dispatch):** a project-history
research subagent was dispatched with an explicit "read all 5 design docs in full,
do not truncate for brevity" brief — for a task whose deliverable was a short list
of one-line summaries. Cost: **148,614 tokens, 51 tool calls, 6.7 minutes.**
Titles + first paragraphs would have produced the identical deliverable for roughly
5–10k tokens. ~15× overspend, purchased by one sentence in the brief.

**Counter-example (same repo, same week):** the aperture-jingp MCP-payload audit —
10 source files, 2,323 lines to check for an unbounded-payload pattern. Method:
skeleton greps + two targeted reads + two one-line empirical probes. Found the known
bug, three sibling instances, and one previously-unknown amplification finding, in
~8 tool calls.

**Forward-friction check (at brief-writing time, or before your own recon):**

1. What QUESTION does this investigation answer? One sentence.
2. What is the smallest artifact set that answers it? (Headers? One function body?)
3. Does the brief mandate reading anything the question doesn't need?
4. Does the output size justify the input mandate?
5. About to write "in full / exhaustive / don't truncate"? Justify it or delete it.
```

### Ready-to-land text — cost-proportional-orchestration, new §2d

```markdown
### 2d. Information-gathering proportionality

The §2a sizing table applies to READING, not just agent count: size the read to the
question, not to the available corpus. Investigation dispatches and inline recon
default to skeleton-first (grep structure, then deep-read only what's flagged) —
the full discipline, brief-authoring rules, and the banked 148k-token Hermes worked
example live in `aperture:subagents` §12 ("Scope the Brief: Gather Cheap, Escalate
Targeted"). An exhaustive-read mandate in a dispatch brief is the information-layer
version of the 7-agent fan-out on a CRUD task.
```

---

## Implementation checklist (post-approval, for whoever GLaDOS dispatches)

1. `beads.ts`: add `"detail"` to `QueryFields`; add `detailTask()` + `DETAIL_DESCRIPTION_CAP = 4000` / `DETAIL_NOTES_TAIL = 3000`; route show mode through it (respecting `fields`); summarize nested `dependencies[]`.
2. `beads.ts`: update_task/close_task/store_artifact → compact string acks.
3. `index.ts`: update query_tasks + update_task/close_task/store_artifact tool description text to document the tiers and the ack shape.
4. `just build-mcp`; verify per the plan in (a); report before/after on aperture-jxwbd to the epic.
5. Skill PRs: subagents §12 + cost-proportional-orchestration §2d as drafted above.

Estimated size: (1)+(2)+(3) is a single small PR, one file pair, ~80–120 lines changed. (5) is a docs-only PR.
