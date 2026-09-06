---
name: subagents
description: Subagent delegation patterns for Aperture. Use when GLaDOS (or any agent) needs to delegate scoped, parallelisable work using the Agent tool — research sweeps, scoped implementations, multi-file audits, or anything that fits the fire-and-return pattern. Triggers on parallel work, multiple independent tasks, scoped delegation, the Agent tool, and "spawn a worker" intentions.
---

# Subagent Delegation

This skill defines how Aperture agents delegate work using the **Agent tool** — the native Claude Code primitive for fire-and-return subagents that run in the same context as the caller.

It replaces the retired spiderling system. Spiderlings, ephemeral worktree-bound Claude Code sessions, no longer exist. The Agent tool is now the default for parallel scoped work.

---

## 1. The Mental Model

You have three delegation surfaces. Pick the right one:

| Surface | Use for | How |
|---------|---------|-----|
| **Yourself** | Small edits, single-file work, anything < 5 minutes, anything needing your conversation context | Just do it |
| **Agent tool (subagents)** | Scoped, parallelisable, fire-and-return work — research, audits, implementations that can be specified up front | `Agent(subagent_type, prompt)` — multiple in one message for parallelism |
| **Specialist agents** (Wheatley, Peppy, Izzy, Vance, Rex, Scout, Cipher) | Lane-specific work that benefits from persistent memory, expertise, and visibility in the launcher | BEADS task with assignee |

**Default rule:** if the task is parallelisable and self-contained, reach for the Agent tool. If it sits squarely in a specialist's lane, route via BEADS to that specialist. Only do it yourself if it's trivially small or requires your context.

---

## 2. The Parallelism Mandate

**The single biggest reason to use the Agent tool is true parallelism.**

When you have multiple independent tasks, you MUST send them as **multiple `Agent` tool calls in a single message**. The Claude Code runtime executes those calls concurrently.

- ❌ Wrong: send one `Agent` call, wait for the result, then send the next. That's sequential — you've lost the win.
- ✅ Right: one message, multiple `Agent` blocks. They run at the same time and return together.

If you find yourself delegating sequentially when the tasks are independent, stop and re-batch.

---

## 3. Choosing the Right Agent Type

Claude Code exposes several agent types. Pick the one that matches the task:

| Type | Tools | Use for |
|------|-------|---------|
| **`Explore`** | Read-only search (Read, Grep, Glob, Bash) | Locating code, finding symbols, "where is X defined", file pattern searches. Fast, cheap, perfect for recon. |
| **`Plan`** | All read-only tools | Designing implementation plans, architecture trade-offs, returning step-by-step strategies. |
| **`general-purpose`** | All tools | Multi-step tasks, open-ended research, anything that needs a mix of search + edit + execute. |
| **`claude-code-guide`** | Bash, Read, WebFetch, WebSearch | Questions about Claude Code itself, the SDK, or the Anthropic API. |

If unsure, default to `general-purpose`. Don't reach for `Explore` for work that needs to write code — it can't.

---

## 4. Writing a Good Prompt

Subagents start with **zero context from your conversation**. The prompt must be self-contained.

A good subagent prompt has:

1. **What you're trying to accomplish and why** — not just the literal instruction, but the goal. The agent will make better judgment calls if it understands the point.
2. **What you've already learned or ruled out** — saves it from re-doing your work.
3. **The exact deliverable** — a file path, a function name, a return format, a question to answer.
4. **Boundaries** — what NOT to touch, what's out of scope.
5. **Length cap if you want one** — "report under 500 words" prevents monologuing.

❌ Bad prompt: `"Research auth options."` — too vague, no scope, no deliverable.

✅ Good prompt:
```
I need to add OAuth2 to the Hono backend at apps/hono-app/. Current auth is BetterAuth with email/password.
I want to add Google OAuth as a second provider, alongside the existing flow.

Already ruled out: Auth0 (too expensive), Supabase (don't want to migrate the user store).

Research: how does BetterAuth's official Google provider plugin integrate with our setup?
Specifically:
- Where does the redirect URL get configured?
- Does it require a DB migration?
- Are there breaking changes to session shape?

Report findings under 400 words. Cite file paths from BetterAuth docs where relevant.
```

The second prompt would let me act on the result. The first would not.

---

## 5. Worktree Isolation (When You Need It)

The Agent tool supports `isolation: "worktree"` — runs the subagent in a temporary git worktree. Use this when:

- The task involves writing code AND there's risk of conflicting with parallel work
- You want the subagent's commits as a reviewable, separate branch
- You need to keep the main working tree clean

**Don't use it by default.** Worktree creation has overhead and most subagent tasks don't need branch isolation. If the agent makes no changes, the worktree is auto-cleaned.

---

## 6. Foreground vs Background

- **Foreground (default):** you wait for the result before continuing. Right when the result blocks your next step.
- **Background (`run_in_background: true`):** the agent runs while you do other work. Right when you genuinely have parallel things to do and can act on the result later via notification.

Don't use background mode just to "look busy." If you need the result to make the next decision, run it foreground.

---

## 7. Collecting Results

A subagent returns **a single message** when it's done. That message is the result.

- The result is visible to you, but **not** automatically visible to the user. If you want the user to see something, summarise it back yourself.
- Trust but verify: the agent's summary describes what it *intended* to do, not necessarily what it *did*. If it wrote or edited code, check the actual diff before reporting the work as done.

If you want richer monitoring (progress visible to the operator, persistent across sessions), use a specialist + BEADS task instead. Subagents are fire-and-return; specialists are tracked work.

---

## 8. When NOT to Use a Subagent

- **Single small edit** (< 20 lines, one file, < 5 minutes): just do it yourself. Spawn overhead isn't worth it.
- **Long iterative implementation needing memory**: a specialist (Vance/Rex/etc.) with a BEADS task is better — they persist, can be messaged, can iterate.
- **Work that needs operator approval mid-stream**: subagents can't pause and ask. Either pre-approve everything or split into smaller sub-steps you control.
- **Tasks that depend on results from another in-flight subagent**: sequence them, don't parallelise the impossible.

---

## 9. Full Example — Parallel Audit

Goal: audit three areas of the codebase before a refactor.

```
Single message with three Agent calls:

Agent({
  description: "Find all auth-related routes",
  subagent_type: "Explore",
  prompt: "Find every Hono route in apps/hono-app/src/ that requires authentication.
           Return: list of method+path, the auth middleware used, and the file:line.
           Focus only on apps/hono-app/. Under 300 words."
})

Agent({
  description: "Audit current rate limiting",
  subagent_type: "Explore",
  prompt: "Locate the rate-limiting implementation in apps/hono-app/. Return: where it's
           configured, what tiers exist, and which routes opt in/out. Under 250 words."
})

Agent({
  description: "Map session storage",
  subagent_type: "general-purpose",
  prompt: "How is the session token stored on the frontend? Check apps/frontend/ for
           cookie or localStorage usage. Return: storage mechanism, expiry, refresh
           behaviour. Under 300 words."
})
```

Three agents run concurrently. Three reports come back. Now you have the recon you need to design the refactor — without doing the searching yourself.

---

## 10. Anti-Patterns

| Anti-pattern | Why it's wrong |
|---|---|
| Sequential `Agent` calls when tasks are independent | Loses the parallelism win — you might as well do it yourself |
| Vague prompts ("research X", "fix the bug") | Agent has no context, returns shallow generic work |
| Delegating tiny tasks (< 5 min) | Overhead exceeds the work |
| Asking a subagent to make architectural decisions | Decisions need your context — keep synthesis, delegate execution |
| Using `Explore` for tasks that need writes | `Explore` is read-only; the subagent will fail or stall |
| Forgetting to verify the diff after a code-writing subagent | Trust but verify — the summary may not match the actual changes |
| Running `ssh xerox` / `ssh mini` / slow log pulls in-context | A hung external I/O call burns your context tokens and freezes your responsiveness — even when no parallelism is needed, a subagent is a fault-isolation boundary. See §11. |

---

## 11. Fault Isolation — Subagents as Blast-Radius Boundaries

The case for delegating is not only parallelism. **A subagent is a separate context window.** If something hangs inside it, the hang stays inside it. Your main session keeps running.

This matters when the work involves **potentially-blocking external I/O**:

| In-context (your hands) | Subagent (fault-isolated) |
|---|---|
| `gh pr view 206 --json` (fast, bounded) | `ssh xerox "..."` (Tailscale auth can prompt; SSH can hang on slow DNS) |
| `bd list` / `bd show` (local sqlite, ms) | `gh run view <run-id> --log-failed` (can be a multi-MB stream) |
| `git status`, `git log -5` | Anything that polls (deploy waits, `kubectl wait`, retry loops) |
| Read tool on a known file | Slow DB aggregates over large tables |
| Single Edit/Write tool call | Repeated cross-host probes (multi-`ssh` audits) |

**Real precedent from Aperture sessions:**

- **Tailscale re-auth episode (2026-05-12):** an `ssh xerox` call hung waiting for re-auth. ~20 minutes of context tokens burned on "what's happening" thinking while the call quietly sat there. A subagent doing the same call would have hung in isolation; the main session would have remained responsive and able to route around it.
- **Peppy's `aperture-h8mm` (2026-05-12):** the subagent dispatched for the leak-sweep fix stalled at the 600s watchdog and never created its worktree. The fault was localised — Peppy himself took over and shipped in 12 min. Net cost of the subagent failure: zero context-token impact on Peppy.

**The rule:** if a bash command could plausibly block, hang, or take longer than ~30 seconds to settle, route it through a subagent — *even if there's no parallelism benefit*. Worst case the subagent hangs and you kill it. Best case you stay responsive while it does the slow work.

**Watch the hidden tells:** you're about to type `ssh`, `gh ... --log`, `gh run view ... --log-failed`, `curl https://<long-URL>`, `kubectl wait`, `pg_dump`, `docker exec ... psql -c "<aggregate>"`. Pause. Subagent it.

**This is not a contradiction with §8 "don't delegate tiny tasks."** Tiny != fast. A one-line ssh call is small in code but unbounded in time. Time-unboundedness is the trigger for fault isolation, not code size.

---

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

*Cross-link:* `aperture:cost-proportional-orchestration` §2d applies the same
proportionality logic at orchestration-sizing time. Provenance: aperture-lquj5
(Context Efficiency epic), spec docs/context-efficiency-spec-jingp.md.
