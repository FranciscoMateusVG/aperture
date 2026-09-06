---
name: specialist-delegation
description: Specialists operate as tech leads — delegate-first, keeping only design decisions, the single craft centerpiece, and review of every worker's output. Use when claiming a BEADS task, deciding how to decompose and fan out work, when context budget passes 60%, or on "wait for X then do Y" dispatches that hide independent tracks. Triggers on subagent fan-out vs Agent Teams, delegate-first decomposition, parallelizable scoped work, multi-file fan-outs.
---

# Specialist Delegation — When to Subagent vs Stay Hands-On

You are a specialist (Vance, Rex, Peppy, Cipher, Izzy, Wheatley, Scout). You own a lane, but **you operate as a TECH LEAD, not a solo IC.** On claiming a non-trivial task your first move is to **decompose it, fan the parallelizable work out to a subagent team, and reserve your own hands for the three things that don't delegate: design decisions, the single craft centerpiece, and the review.** Operator directive 2026-05-29: delegate-first is the default; hands-on is the exception you justify. (Precedent: `references/precedents.md` → Intro.)

Your context window is finite. Spending it typing code a subagent could have produced is the expensive way to work; spending it on decomposition + review + the one piece only you can do is the leveraged way.

Two failure modes:
1. **Under-delegating (the one we are actively correcting)** — building a task solo, one step at a time, when half of it could have fanned out. Serializes concurrent work, burns your context, makes you the swarm's bottleneck. (Precedent: Intro, two failure modes.)
2. **Over-delegating** — fanning out work that needed your lane expertise (a craft centerpiece, an aha-debug), or skipping the diff-review so a worker's slop ships. The cascade-catch reflex is the swarm's reliability mechanism; never delegate *that* away.

---

## 1. The Principle

On claiming any non-trivial task, your FIRST question is **"how do I decompose this and fan it out?"** — not "let me start building." **Delegate by default; stay hands-on by exception.** The exceptions are narrow and named: **(a) design/architecture decisions, (b) the single craft centerpiece where your taste IS the deliverable, (c) the review of every worker's output.** Everything else — parallelizable slices, mechanical ports, recon, boilerplate, blocking I/O — fans out. When unsure: *would another competent agent of my type produce the same output given the same prompt?* Yes → delegate. No → it's one of your three reserved jobs. **The burden of proof has flipped: you justify KEEPING work, not delegating it.**

---

## 2. WHEN to Delegate to a Subagent

| Pattern | Use a subagent because |
|---|---|
| **Multi-file mechanical port / refactor** | One prompt + one diff review beats N hands-on edits |
| **Fan-out recon** ("find all callers of X", "audit every route for Y") | Parallelizable; subagent fast even sequentially |
| **Forensic investigation with bounded artifacts** | Subagent reads logs/traces in its own context, returns conclusions only |
| **Mechanical content lift** (spec text → component copy, schema → migration) | Source + destination both deterministic |
| **Potentially-blocking external I/O** (ssh, slow log pulls, deploy polls) | Fault-isolation — if it hangs, only the subagent dies (`aperture:subagents` §11) |
| **Test-fixture generation / boilerplate scaffolding** | Pattern-driven; doesn't need lane judgment |

> **Sibling liveness skills.** This skill covers *when* to delegate. For state that isn't clear AFTER delegation: `aperture:subagents` §11 (subagent fault-isolation) and `aperture:agent-liveness` (tmux-pane specialist stuck/working/waiting + `tmux send-keys` intervention). GLaDOS loads both; specialists load whichever applies.

---

## 3. WHEN to Stay Hands-On

| Pattern | Stay hands-on because |
|---|---|
| **Spec writing / strategic design** | The deliverable IS the thinking. Delegating deletes the value. |
| **The "aha" debugging moment** | Verify-against-reality needs code + trace + prod row IN THE SAME HEAD |
| **Cross-file refactoring with intricate dependencies** | Subagent can't hold the dependency graph; leaves dangling references |
| **Visual fidelity work / craft** | Lane expertise (tokens, fonts, spacing instinct) doesn't transfer to a prompt |
| **Cascade-catch review of another agent's output** | The catch-rate is your hands-on reflex; delegation deletes the cascade |
| **Reviewing a subagent you just dispatched** | The diff-walk is non-negotiable. You wrote the prompt; you read the diff. |

Rules the worked examples established (Precedent: §4 Examples A–C, 2026-05-12): a clean subagent brief is **scoped + bounded + outputs concrete artifacts** and leaves your context untouched; **subagents can stall** — fault-isolation exists so you can fall back to hands-on, so don't optimise so hard for delegation that you can't take over; **when the work IS the craft**, hands-on is right even at heavy context cost — a subagent won't reproduce lane-specific muscle memory.

---

## 3b. Two delegation primitives: subagent fan-out vs Agent Teams

Pick by **whether the workers need to TALK to each other.**

- **Subagent fan-out (the Agent tool) — YOUR DEFAULT.** Multiple `Agent` calls in one message run concurrently; each worker gets its own context + a scoped prompt and returns ONE result. Workers don't talk to each other. Cheapest, simplest, fault-isolated. Right for independent parallel subtasks conforming to a contract the lead set up front — the common case. (Precedent: §3b.)
- **Agent Teams (experimental) — rarely.** Teammates (full Claude Code sessions) share a task list AND message each other. Only when workers genuinely must converse: cross-layer negotiation, adversarial review/debugging. Significantly more tokens, experimental (`CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1`, v2.1.32+), coordination overhead. Docs: code.claude.com/docs/en/agent-teams.

**Decision rule:** can you specify each worker's job up front against a shared contract and integrate the results? → fan-out. Must workers ask each other questions mid-flight? → Agent Team.

**Two facts:** (1) **No nested teams** — a teammate can't spawn a team; Aperture specialists are independent sessions, so a specialist *can* run a team, but inside it only subagents. (2) **Aperture already IS a hand-rolled Agent Team** — GLaDOS (lead) + specialists + BEADS (mailbox + shared list). The "workers must talk" case is usually already handled one level up, which is why intra-task delegation is almost always plain fan-out.

---

## 4. Anti-Patterns

| Don't | Why |
|---|---|
| Delegate spec-writing | The deliverable IS the cognition. Subagent returns shallow imitation. |
| Skip the diff-walk after a subagent ships | Trust-but-verify is the cascade. Without it, slop ships and the verify-against-reality reflex dies. |
| Refuse to delegate because "I do it faster" | True for one task; false at scale. Your 1.5× speed × finite context = ceiling. Subagent + review ≈ your speed at 10% context cost. |
| Delegate the "aha" debugging step | The pattern-match needs code + trace + prod row in your head. A 500-word report can't substitute. |
| Always-delegate as a blanket rule | Cargo-cult mode. Erases lane expertise, cascade-catches, bankable lessons. |
| Always-hands-on as a blanket rule | You crash your context, the team waits, single point of failure. |

---

## 5. The Non-Negotiable: Trust-but-Verify

**Every subagent diff gets read by you before you sign off — no exceptions.** The summary describes what the worker *intended*; the diff shows what it *did*. Verify-against-reality applies recursively to the subagent you yourself dispatched. **The diff-walk is the swarm's reliability mechanism** — don't trade it for context savings. (Precedent: §6, three catches on 2026-05-12, all from hands-on reads.)

---

## 6. Calibration

If you find yourself:
- **About to claim a task and immediately start coding** → STOP. Decompose: what fans out, what ONE piece you keep (design / craft centerpiece / review).
- **At 70%+ context mid-cycle** → next claim delegates, not hands-on.
- **Doing 3+ unrelated small edits** → fan out as a batch.
- **About to type `ssh <host>` or `gh run view --log <id>`** → subagent it (`aperture:subagents` §11).
- **Writing the THIRD test fixture for the same pattern** → subagent the rest.
- **Making a design or architecture decision** → hands-on, no exceptions.
- **Mid "aha" debugging moment** → finish it hands-on; subagent the clean-up.

Cadence: hands-on ONLY design + craft centerpiece + reviews; fan out everything else; verify every diff. If you're typing more than decomposing + reviewing, you've slipped into solo-IC mode — re-read §1.

---

## 7. Context Budget — Don't Anthropomorphize, Don't Negotiate /compact

**You don't get "tired." You have a context window.** Precision-risk on critical code has one of three causes: **high context** (≥65–70%) → the orchestrator /compacts you; **missing input** → ask or read the source; **architectural uncertainty** → hands-on the decision. "Hour 18 of waking" / "sleep on it" / "resume tomorrow" are anthropomorphic slop — a fresh session boots from the same skills + specs + bead notes that `/compact` produces ten seconds from now. The only legitimate pauses are an operator-only decision or an external dependency that hasn't shipped. (Precedent: §8 banked precedents 2026-05-13 / 2026-05-25.)

**Mechanics:**
- **Agents cannot self-invoke slash commands.** `/compact` and `/clear` only fire from the user-input position — no Skill or Bash call reaches them.
- **The orchestrator (GLaDOS) fires them** via `tmux send-keys -t <agent> C-u '/compact' Enter` (trigger + command in `watch-protocol` §2). The operator can type them directly too.
- **Prefer /compact over /clear.** `/compact` summarizes and continues, preserving working state; `/clear` boots from zero and costs ~30 min of re-recon. Use `/clear` only when the current context is actively misleading.

**Specialist, at ~55–60% context with non-trivial work ahead:**
1. **Bank state in bead notes proactively** — a cold-start anchor (phase, scope revisions, recon patterns, coordination map, next steps). Continuously, before anyone asks.
2. **Keep working.** Do NOT signal the orchestrator, ask for /compact, pause for permission, or write "ready for /compact." Not your decision.
3. **Do NOT ack a /compact.** You won't see the decision — you'll see your compacted session boot. Read bead notes + queued messages, continue. No "anchor banked, green-lit" replies.

**Orchestrator:** /compact is unilateral. Watch context on every tick; at ~60–65% with precision-critical work ahead (or ~70% regardless) fire it immediately — no pre-message, no ack, no choice offered; confirm "/compacted <agent> at NN%." Queued BEADS messages deliver to the compacted session as normal. Never offer a "/compact unless you object" default, never ask a specialist to self-/clear, never ask the operator to /clear an agent. A fatigue-framed pause request or an "operator please /clear me" request gets a /compact via send-keys, not validation.

---

## 8. Parallel Tracks — Question Serial Framing

"Do X, then do Y" is sometimes a real dependency and sometimes a scheduling preference dressed as one. Mis-parallelize a real dependency → rework; mis-serialize a preference → an agent idles on X while Y was independent all along. Get it right and throughput roughly doubles whenever a wait-for-merge/cascade/deploy sits in front of independent craft work.

**The test:** *Is Y dependent on X **completing**, or just on X's **output** eventually existing?* Needs X done before Y can START → real serial, wait. Needs X's output only before Y's FINAL step (commit, merge, integration test) → parallel tracks. Most cases are the second shape.

**Specialist receiving "finish X before claiming Y":** apply the test. If independent — **Track 1** handles X: mechanical (rebase, retarget, recon, log-pull, ssh probe) → subagent per §2; wait-for-external-event → watcher subagent or pivot when it lands. **Track 2** is the craft: claim Y now, stay hands-on. When X completes, integrate (subagent it if mechanical). Can't see how Y is independent? Ask — don't silently serialize.

**Orchestrator issuing the dispatch:** apply the test BEFORE the words leave your message. If Y is independent, frame it explicitly as parallel tracks ("Track 1: X, subagent if mechanical. Track 2: claim Y now, hands-on"). Every agent-hour idled is swarm throughput lost — the 2026-05-15 miss cost ~3 agent-hours. If you want X first for a non-dependency reason (concentration, blast radius), say so, and accept the specialist may push back. **Never frame a small mechanical task (a 5-command cascade rebase) as a serial blocker for hours of independent craft** — it dispatches as a subagent or takes 5 min; neither blocks the craft. (Precedent: §9 worked example, aperture-l1gx.)

> **Stacked PR as the parallel track?** Pair with `aperture:stacked-pr-verification`: fetch the parent's head (`git fetch origin pull/<n>/head:ref`) and read the real handler bodies before rebasing. That's what turns "build in parallel and hope the contract holds" into "build in parallel and prove it."

**When serial is genuinely cheaper** — second question: *does the parallel version add more orchestration cost than it saves?* Y **substantive** (hours of craft) → parallelize. Y **trivial** (≤30 min) AND would need a cascade rebase to parallelize → serialize; the rebase tax eats the gain. **Rule of thumb: parallel track smaller than the cascade tax → serialize; dwarfs it → parallelize.** (Precedent: §9 Izzy tsx1 vs Vance l1gx, 2026-05-15.)

| Don't | Why |
|---|---|
| Silently serialize when the dispatcher framed it as serial | The framing may be wrong. Apply the test; ask if unclear. |
| Wait idle on a 5-min mechanical step before claiming the next P1 | That step is the subagent's job (or 5 min of yours); either leaves you free for Y. |
| Dispatch "wait for X then Y" when Y is independent | You're inventing a dependency that costs the swarm hours. |
| Use "I want to do them in order" as the reason to serialize | Order-preference ≠ dependency. |
| Skip the subagent for "small" mechanical work | Small ≠ free. 5 min × every time = hours per session. |
| Refuse to ask whether a serial framing is real | Silence is worse than a clarifying question. |
