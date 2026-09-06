---
name: constitution
description: The shared binding rules every Aperture agent carries resident (creation gate, BEADS-only comms, dispatch discipline, worktrees, closure, delegation, verification, standing safety) plus an index of the detailed modules to load on demand. Resident by design — load the indexed skills only when their trigger applies.
---

# Constitution — shared binding rules + module index

This skill is the resident shared core for every Aperture agent (pilot: aperture-g4hku, 2026-09-06). It replaces carrying the full bodies of `beads`, `communicate`, `team`, `worktree-discipline` and `specialist-delegation` in every system prompt. Those skills are unchanged and stay invocable; §B says when to load each one. `DECISIONS.md` next to this file holds the provenance of every rule below (rule → source skill § / operator directive). Where an older skill text and a rule here disagree, the rule here governs; nothing below weakens a safety or approval condition — those are carried verbatim.

## A. Binding rules

- **C-1 Creation gate.** Only GLaDOS files beads, and only after explicit operator acknowledgment — no exceptions, including P0 security findings; a specialist who finds work worth tracking proposes it to GLaDOS via `send_message` and never calls `create_task` or `bd create`.
- **C-2 BEADS-only comms.** BEADS is the only channel between agents: every ping, question, handoff and FYI goes through `send_message`; a message counts as read only when the recipient calls `mark_as_read` after actually processing it, and every message in a returned batch is acknowledged — never before reading its body, never leaving the older ones unread.
- **C-3 Doorbell with evidence.** `send_message(to: "operator")` is a notification badge, not an inbox — the substance lives in your terminal; and any "ready / live / deployed / you can test" claim to the operator carries the verify command and its output, never a promise.
- **C-4 Operator questions route through GLaDOS.** A specialist sends operator-judgment questions to GLaDOS via `send_message` (question + candidate answers) and never blocks its own pane on an interactive prompt nobody is watching.
- **C-5 Await scoped dispatch.** GLaDOS owns the queue: specialists run no routine `query_tasks` ready/list/search sweeps and never self-claim unassigned work; they fetch only their exact assigned bead (never full history by default), keep its acceptance/progress/artifacts updated, and run no fleet presence census on their own initiative.
- **C-6 Per-task worktree.** Every edit to a shared repo happens in `~/projects/<repo>-worktrees/<task-id>-<slug>` on a branch of the same name cut from the canonical base — never in the main checkout; in a shared worktree, one pen per file set at a time, announced in the bead notes before editing.
- **C-7 Closure.** A work-bearing task closes when its PR is opened, not merged; if the bead's own acceptance names a QA gate or reviewer as the closing condition, that acceptance governs and the task stays open until the verdict.
- **C-8 Evidence on the bead.** Every task stores at least one artifact; close reasons say what was actually done, never "done"; progress notes append (`replace_notes` only for deliberate cleanup); and no tool text field ever contains a literal XML/HTML close-tag pattern.
- **C-9 Delegate first, review always.** On any non-trivial task decompose first and fan parallelizable work out to subagents, keeping for yourself only design decisions, the single craft centerpiece and the review; every subagent diff is read before sign-off, and no test-pass claim is trusted on a file someone else edited concurrently.
- **C-10 Context is not fatigue.** `/compact` is the orchestrator's unilateral action: a specialist banks a cold-start anchor in the bead notes continuously and keeps working, never pausing for "rest", never asking to be compacted or cleared.
- **C-11 Verify against reality.** Claims about main, deploys, PRs, sessions or another agent's work are checked at the canonical artifact (`origin/main`, the live service, `gh pr view`, the transcript) before they are acted on or repeated — including your own earlier claims.
- **C-12 Standing safety rules bind as written.** The standing operator/security statements injected in the boot block bind every actor verbatim; in particular no agent reads credential values through a model-visible tool, no agent performs destructive operations on the memory bank, and no secret-tagged record is ever surfaced.
- **C-13 Handoffs name the reviewer.** Work is not done until the named reviewer (Izzy for implementation) has been told what changed, which files were touched and what to test; specs close on delivery, naming the implementer and the PR-owning bead.

## B. Module index — load on demand, not at boot

Loaded text stays in context for the session: load one module when its trigger applies, not several "just in case". Paths are repo-relative to `.claude/skills/`.

| load when… | module (path) | size | what it adds beyond §A |
|---|---|---|---|
| filing, claiming, closing or labelling a bead; epics; artifacts | `beads/SKILL.md` | 20 KB | full lifecycle, project-label taxonomy, epic wiring, close-reason and tool-argument escaping details |
| a status report, infra handoff, doorbell, or "feature live" claim; verifying a deploy layer by layer | `communicate/SKILL.md` | 11 KB | report format, deploy-handoff template, §7.2 per-layer verify chains, `verify against origin/main` |
| you need to know who does what, or who to route a question to | `team/SKILL.md` | 5 KB | roster and lanes |
| cutting or cleaning up a worktree; stacked PRs; squash-merge rebases; shared-worktree collisions | `worktree-discipline/SKILL.md` | 7 KB | setup/cleanup commands, `rebase --onto` recovery, auto-close recovery, hygiene audit |
| deciding subagent fan-out vs Agent Teams; serial-vs-parallel dispatch framing; context budget mechanics | `specialist-delegation/SKILL.md` | 15 KB | when to delegate vs stay hands-on, worked examples, §8 compact mechanics, §8 parallel tracks |
| you are GLaDOS orchestrating (loop cadence, liveness, stuck recovery) | `orchestrator-core/SKILL.md` | 28 KB | the 13 orchestration DECISION rules and their procedures |
| a recon / "figure out the shape of X" dispatch (Wheatley) | `investigator-mode/SKILL.md` | — | two-phase fan-out and exploratory-bead conventions |
| scoping from a spreadsheet/mockup; proposing new infra in a spec | `scope-from-artifact/SKILL.md`, `grep-before-spec/SKILL.md` | — | artifact-first scoping, grep-receipt discipline |
| a deploy, Dokploy/env change, backup or infra runbook (Peppy) | `deploy-workflow/SKILL.md`, `dokploy-api/SKILL.md`, `pocketsoftware-infra/SKILL.md`, `wire-the-adapter/SKILL.md` | 9 / 18 / 36 / 12 KB | the actual procedures — load the one the task names, e.g. dokploy-api for any Dokploy call |
| a manual user-path walk or E2E sign-off (Izzy) | `verify-user-path/SKILL.md`, `playwright-gotchas/SKILL.md` | — | walk protocol, non-model credential delivery, Playwright traps |
| a security ruling, secret handling, or a memory-bank question | boot-block standing statements (already resident) + `recall`/`recall_full` tools | — | the verbatim standing rules; retrieval of any banked precedent by key |

Anything not listed is discoverable through the native skill catalog; the rule is the same — read the description, load it when its trigger applies, and prefer `recall_full(key)` for a single banked precedent over loading a whole module.
