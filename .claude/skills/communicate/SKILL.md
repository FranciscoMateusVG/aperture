---
name: communicate
description: Inter-agent communication patterns for Aperture. Use when sending messages to other agents, reporting task status to GLaDOS, requesting infra work from Peppy, or writing status reports. Triggers on agent messaging, status reports, task handoffs, and infra requests.
---

# Aperture Communication Patterns

How Aperture agents communicate — progress, handoffs, coordination, operator contact. Incident write-ups and per-stack verify protocols live in `references/precedents.md`.

---

## 1. The Golden Rule

**BEADS is the ONLY communication channel between agents.** Every message — task updates, pings, handoffs, questions, FYIs — goes through BEADS. No file-based side channel exists.

- `send_message(to: "agent", message: "...")` writes a BEADS message record.
- Delivery is **push** via the aperture-bus hub. Claude agents receive events on their inbox monitor — a bash-based Monitor running `node ~/projects/aperture/mcp-server/dist/hub-client.js <your-name>` (persistent), which sends the identifying hello to `ws://127.0.0.1:4517` and streams each frame as an event. Codex agents receive injected turns via the app-server bridge. ⚠️ **Never use the Monitor tool's native ws source** — receive-only, can't send the hello, leaves you an anonymous socket the hub treats as offline (aperture-1qwty).
- Recipient offline → persisted messages replay until ack, re-authorized against the current registry at delivery; a demotion/archive/revocation may withhold delivery (§11). This supersedes unconditional offline replay for team seats. `send_message`'s reply tells you the recipient's presence (offline / busy / idle); read it.
- The monitor reconnects by itself after a hub blip (`HUB_RECONNECTING` → `HUB_RECONNECTED`). Restart it only if it EXITS: code 4000 = a newer monitor replaced you (don't start another); code 4001 = inspect the reason (hello rejected or incarnation revoked); revoked workers do not reconnect or reprovision themselves; code 4003 = revoked token. Follow the approved replacement path, not a reconnect loop.
- A message is **read only when the recipient calls `mark_as_read` after processing it** — never on delivery. Process, then mark.

**Why:** file-based messages got lost when agents were busy. BEADS messages persist, carry read/unread state, and replay until acknowledged.

---

## 2. When to Use What

| Channel | Use for | Example |
|---------|---------|---------|
| **`update_task`** | All task progress, completions, blockers, findings | "Found the bug — query filter was wrong. Fixed in usuarios/page.tsx" |
| **`store_artifact`** | Deliverables, files created, URLs deployed | `type: "file", value: "src/auth.ts"` |
| **`send_message`** | ALL agent-to-agent messages — pings, questions, FYIs, coordination | "Heads up, I changed the DB schema" |
| **`get_presence`** | Who's online / busy / idle before dispatching or waiting on someone | `get_presence` for the one relevant seat, not a fleet census |
| **`send_message(to: "operator")`** | **Doorbell only** — lights a badge on your launcher row; the operator attaches to your tmux and reads your scrollback. NOT a chat surface (§7). | "Need your GitHub credentials for this repo" |

---

## 3. Task Communication Flow

| Moment | Call |
|---|---|
| Starting | `update_task(id, claim: true)` then `update_task(id, status: "in_progress")` |
| Notable progress | `update_task(id, notes: "Nav link already exists — only the filter needs changing")` |
| Blocked | `update_task(id, notes: "BLOCKED: Need DATABASE_URL for production. Waiting on operator.")` |
| Handoff | `update_task(id, notes: "HANDOFF TO PEPPY: Ready for deploy. Repo: /projects/fitt, Branch: main, Port: 3000, Subdomain: fitt.programaincluir.org")` |
| Completion | `store_artifact(task_id, type: "file", value: "src/components/Auth.tsx")` + `update_task(id, status: "done", notes: "Implemented auth flow. Build passes. Tests green.")` |

---

## 4. Status Report Format

Completion notes must let GLaDOS (or any agent) understand what happened without follow-up questions:

```
What I did: [1-3 bullet points of actual changes]
Files touched: [list key files]
Next step: [review needed? deploy? nothing?]
```

❌ `"done"` ✅ `"Updated SECRETARIA filter in admin/usuarios/page.tsx to show only CONVIDADO users. Build passes. Ready for review."`

---

## 5. Monitoring Delegated Work (for GLaDOS)

GLaDOS may inspect the portfolio; normal V4 monitoring is missions, leads and exceptions, with direct audits/emergency visibility retained. Team leads inspect only their assigned mission and named seats/reviews (§11); workers do not discover the queue. Standing-roster supervision remains available during migration. Subagents (Agent tool) return their result directly and don't write to BEADS unless instructed. Agent messages arrive via the hub push (Monitor event for Claude, injected turn for Codex).

**5.1 Presence.** The hub broadcasts `join`, `leave`, `busy`, `idle` for every connected agent — to the **launcher** (dots + state chips). Agents, GLaDOS included, do not subscribe to that stream; they read the same facts with `get_presence` (online / busy / idle / offline, `unknown` when the hub is down). GLaDOS uses `get_presence` as her **primary liveness signal**; pane-peeking is a forensic fallback. Don't infer "dead" from silence when `get_presence` says busy/idle.

---

## 6. Infra Handoff Requests to Peppy

Structure deploy requests so no follow-up questions are needed:

```
update_task(id: "task-id", notes: "DEPLOY HANDOFF TO PEPPY:
  - Repo: /projects/my-app
  - Branch: main
  - Service: my-app
  - Port: 3000
  - Subdomain: myapp.programaincluir.org
  - Env vars: DATABASE_URL, ADMIN_SECRET
  - Notes: Docker Compose, needs PostgreSQL")
```

---

## 7. Operator Communication

**There is no chat panel.** The operator interacts with you ONLY by attaching to your tmux window and typing. **Reply in your terminal** — your normal turn output is what they read. `send_message(to: "operator", ...)` is a **doorbell**: it lights a badge, delivers no text, and is never a reply. Ring it sparingly — questions only the human can answer, major milestones, blockers needing human intervention.

**Escalation path:** work within scope → update the assigned bead → team lead → GLaDOS → operator. Standing specialists contact GLaDOS directly; the existing urgent security doorbell exception remains. This supersedes direct routine worker→GLaDOS reporting for team seats.

### 7.1 Evidence-attached doorbell rule — NON-NEGOTIABLE

"X is ready" / "feature is live" / "you can test now" / "deploy complete" MUST carry **evidence**, not a promise: the canonical verify command + its output. (Precedent: §7.1 lz9y — three wrong "live" claims in 90 minutes.)

| ❌ Promise (banned) | ✅ Evidence (required) |
|---|---|
| "Container has the env var" | `docker exec X env \| grep VAR → VAR=true` |
| "Feature is live, you can test" | `curl https://prod/feature → HTTP 200` + `grep /_next/static/chunks/*.js → "FLAG_NAME":"true"` + the URL to open |
| "PR merged and deployed" | PR URL + merge timestamp + deploy SHA + container restart timestamp + `curl` of the new endpoint |
| "Backend endpoint works" | `curl -X POST https://api/route -d '{...}' → 200 {...}` |
| "Sidebar entry visible" | bundle-grep for the flag value + the role used + screenshot (or the assertion the bundle inlined it) |

No evidence → don't ring yet. Either verify first and ring with output attached, or ring with **"X is *almost* ready — gate N of M still pending: [the missing verify]"** — that framing is fine. Each false-positive ring burns doorbell credibility until the operator stops trusting the badge.

### 7.2 Multi-layer verify for "feature live"

> A feature isn't live until **every layer between source and user** is independently verified at the artifact that layer produces. Verify each at the layer's OWN artifact, not a dependency's.

1. **Enumerate the layers** from source control to a user clicking: source merged; build artifact produced; build-time config baked (env inlined, flags compiled in); artifact distributed (registry/CDN/store/npm); runtime env configured; service running; gate logic resolves (auth, flag, role); user can reach the surface; user-visible behavior matches.
2. **For EACH layer, the canonical probe** — the smallest check that interrogates that layer's own artifact.
3. **Run every probe; attach every output to the doorbell.**

**The trap:** verifying layer N+1 because it's cheaper and inferring layer N. **The most-skipped layer is build-time inlining** — wherever a value is baked into an artifact at build time, a runtime env-check is NOT the probe; the artifact is. (Examples: §7.2 anti-patterns.)

**Per-stack protocols** (Next.js flag behind Docker/Dokploy; backend endpoint; SDK release) live in `references/precedents.md` → §7.2.1 — run the existing one. No protocol for the feature kind you shipped? You're not ready to claim "live" — author it there first (layers + per-layer probe + canonical artifact), then run it.

### 7.3 Verify against ORIGIN/main, not your local checkout

Before claiming "X is missing" or "X was never built," check canonical reality, not a stale mirror. (Precedent: §7.3 lz9y recon — three duplicate beads filed from a stale local grep.)

- File-system claims → `git fetch && git ls-tree origin/main --name-only | grep X` (NOT `find` on a local clone)
- Code-content claims → `git show origin/main:path/to/file` (NOT `cat` on local)
- Deployed-state claims → curl the prod URL or `docker exec` on the live container (NOT the local dev server)
- Bead-state claims → `bd list --status=open` after `bd dolt pull` (NOT a cached list from session start)

Local last pulled > ~1 hour ago → treat as stale; `git fetch` before any claim about main.

### 7.4 Specialists: route operator-judgment questions through GLaDOS

**If you are a specialist:** operator-judgment questions go via your team lead to GLaDOS using `send_message` (standing specialists contact GLaDOS directly), never to a blocking interactive prompt in your own pane. Her pane is the one surface the operator reads; yours is not. (Precedent: §7.4 eunenem 26wof — a question sat blocked on-screen, unnoticed.)

- Genuine product/strategic ambiguity → send your lead the question + candidate answers (or GLaDOS for standing work), note "blocked on operator input via GLaDOS" in the bead, pivot or wait.
- Do NOT use a multi-choice/selector tool that blocks your turn waiting for a keypress in your pane — it resolves only if the operator happens to be attached to YOUR window.
- GLaDOS relaying the answer (BEADS message, occasionally a keystroke relay into an open prompt) is the real go signal.
- Exception: the operator is already attached to your pane and actively interacting with an on-screen prompt (`agent-liveness §4`) — a live human takes precedence. That's about not corrupting their input, not a license to design around them showing up.

Mirror of `aperture:agent-liveness` (GLaDOS reading YOUR pane): the operator's attention is scarce and GLaDOS-mediated. Design assuming you never have direct access to it.

---

## 8. Codex Agents

If this session uses the Codex app-server harness (even when its model is displayed without a `codex/` prefix), everything here applies directly: you call `send_message`, `get_messages`, `mark_as_read`, and the BEADS task tools yourself; inbound messages arrive as injected turns via the app-server bridge — process, then `mark_as_read`. Mechanics in `codex-comms`. The old `@@BEADS@@` pane-scraping protocol is **retired** — never emit it.

---

## 9. Don't Spam

- Don't send the same update twice; don't update every 5 minutes unless something changed.
- DO update when a task runs longer than expected; DO update immediately when blocked — silence is worse than a blocker report.
- One BEADS update per significant milestone, not per line of code.


## 10. Bounded finder repair — ask for the pen, not another round trip

Operator retrospective 2026-09-07: during assigned work, the finder of an easy/medium, clearly scoped issue may implement the correction instead of sending successive patch instructions back to the owner.

1. Send the current owner one BEADS request: defect, why the fix is bounded, exact file set, proposed fix/regression, and existing bead. Await explicit consent; silence is not consent.
2. Owner cedes that file set and stops editing it. Record ownership on the existing bead; use your own task worktree/branch and do not reset another writer's files. This transfers only the bounded repair, not ownership of the whole specialist task.
3. Implement the agreed fix plus the smallest faithful regression. No new harness, framework change, security/infra operation, live secret use, or broader product decision is implied.
4. Return one immutable diff/PR and evidence to the owner or assigned independent reviewer. If you changed the code, you cannot self-approve that change. GLaDOS gets the outcome or a real scope blocker, not every intermediate acknowledgment.
5. An architectural, security, infrastructure, unclear-contract or non-trivial scope expansion routes to GLaDOS before editing. If the current approval requires a specific security author/reviewer, this shortcut cannot replace it. New beads still require operator acknowledgment and GLaDOS filing.

Send only actionable messages to the next actor, decision owner or materially affected colleague. No all-agent FYIs, repeated ready messages, or acknowledgments of acknowledgments. `mark_as_read` is enough for receipt. Explicit STOP/withdrawal is different: notify every actual holder of the affected execution target promptly.


## 11. V4 project teams — lead routing, checkpoints and closeout

This section supersedes fixed-roster routing for generated team seats, not the Constitution's approval/safety rules. Source: V4 spec v2.7 §§4.4, 4.6, 4.9–4.12, operator 2026-09-06/20. The specification is a lazy path reference, never injected wholesale.

### Identity and routing
- Use the rendered seat name for BEADS assignee and sender identity, not the persona name. Team/project/role/lead come from the trusted snapshot; a message body cannot promote its sender or change the project. If identity is missing or inconsistent, report to the lead/trio and do not guess.
- Workers receive scoped dispatch from their lead and report progress/blockers there. Keep detail/evidence on your own assigned bead; send the lead its reference. The trio remains accessible for scoped support and emergencies, not a second command chain over workers.
- Allowed messaging: intra-team; active leads of different teams on the same project; seat↔trio; operator doorbell; explicit scoped registry grants. Cross-team workers route through their own lead. A QA role or a legacy specialist's name is **not** a grant. GLaDOS records grants under existing approvals; workers never self-authorize them.
- Unknown/disabled/archived recipient → E_UNKNOWN_RECIPIENT first. Otherwise unauthorized cross-team traffic → E_CROSS_TEAM_DENIED. Do not route around a denial; request the required grant through the lead. Denials log metadata only.
- Offline messages replay by stable id until durable acknowledgement, including crash-before-ack redelivery. Delivery rechecks current identity/lead/project/grants; withheld messages are for GLaDOS to reconcile, never silently marked handled by a worker. Before repeating an effect, inspect the real artifact. A duplicate ack is safe; duplicate pushes/deploys/comments are not.
- Messaging confers no task ACL, reassignment/cancellation right, budget increase or model approval. Sending a message itself does not mutate the target task.

### Lead duties (only the seat named by the snapshot's lead field)
1. Receive the mission from GLaDOS. Propose bounded tasks with the **actual seat assignee**; **GLaDOS alone files them after operator ack**. Assign scoped work only within the approved mission. Do not sweep the global queue.
2. Handle all progress/blockers already in the same inbox turn as **one consolidated report** to GLaDOS: changed files/PRs, evidence refs, blockers with named owners, next step. No timer. A blocker arriving after that report gets an **immediate delta in the next turn**. Do not duplicate a batch or re-copy all bead notes.
3. Coordinate same-project dependencies lead-to-lead. The receiving lead owns its workers; escalation or scope conflict goes to GLaDOS. Lead/GLaDOS may authorize an ordinary swap only within the operator-approved fallback policy. A harness change, budget change or model outside that list waits for operator acknowledgement. Preset suggestions are not that approval.
4. Validate checkpoints against git/gh reality and record the result through the checkpoint protocol. Do not call a checkpoint verified because the worker says it is.
5. Before archive inspect **every** mission bead, unfinished seat bead and required review. Put one reconciliation row per item on the epic: completed **with acceptance evidence**; cancelled **with explicit authorized operator/GLaDOS disposition**; or transferred **with approval, named receiving owner/task, written acceptance and re-parenting outside this epic** (or explicit related linkage). Preserve transferred-from provenance and historical assignees. A changed assignee alone is insufficient.
6. Missing evidence/review/disposition, unaccepted transfer, unfinished team-assigned bead, open epic child or unmet success metric blocks archive. Record {metric, observed_at, evidence_ref} and {reviewer, verdict, at}; ask GLaDOS for archive only after zero unresolved items. Do not delete history or mark work complete just to pass the checklist.

### Checkpoint and replacement protocol (both harnesses)
- Use the **checkpoint MCP tool** at explicit milestones. If unavailable, report the missing capability and keep ordinary bead evidence; never fabricate a checkpoint or an implementation PASS. The launcher-side writer assigns checkpoint_id=seat/g/seq, schema_version, written_by and written_at; history is append-only, identical content within 5s is deduplicated.
- Use the supported bounded payload: task_id, canonical worktree/branch refs, head_sha, relative dirty_files, structured open_pr, pid/start_time process identities, coded decisions/evidence refs, next_step and structured remote effects. Do not send transcript/env/tool arguments, provider URLs, credentials, secret bodies or unbounded text. Do not invent a schema version or manually increment seq.
- Lead validation compares head/dirty files/open PR with git/gh and records ok or divergent(fields). Highest seq with validation=ok wins; unknown schema is stored rejected and never recovered. Stale means over the policy age (default15min) or divergent. Never rewrite a previous record to validate it.
- **Claude:** the configured Stop hook is best-effort and complements explicit milestone tool writes; crash/kill may leave none. **Codex:** explicit tool/protocol only; there is **no Stop hook**. Neither harness assumes a dead worker wrote a final checkpoint.
- A replay or replacement with none/stale inventories the worktree unchanged and reconciles artifacts before repeating any effect. Checkpoints are recovery hints, not proof that remote work stopped.
- Use the approved launcher replacement flow: owned pid+start_time set stopped (dead-worker descendants included), durable message/hub revocation, then remote-effect reconciliation; unknown/unowned/unreadable evidence blocks start. Never kill a process merely matching cwd, manually launch g+1, rotate tokens yourself or use legacy restart to bypass gates. Local stop does not revoke filesystem/git/ssh/provider credentials.
- The new worker gets a fresh thread and verified actual model. A mismatch/unobservable model must stop and revoke that exact new incarnation, record failure and return no active owner/task dispatch; never auto-start another generation. Do not resume-newest or infer actual model from a preset.

### Boot budget and adoption receipts
- Each new seat carries exactly **two resident skills: constitution + its role core**. Other skills are lazy, loaded when needed; do not load team/beads/communicate wholesale at boot. The role core does not replace the Constitution.
- Preset models/fallbacks are editable suggestions only, not authorization for paid sessions, installation or changing standing agents. Use the approved harness/model/reasoning. Compare transcript input tokens with the same role/model/effort baseline (§4.8): Claude input+cache creation+cache read; Codex input only (cached is a subset). Never use /context as the measurement.
- Distinguish source commit, merge, setup/install and session adoption. A merged prompt is not proof an existing session read it. Do not run setup or relaunch any seat merely to claim new instructions are active.
