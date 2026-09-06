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
- Recipient offline → nothing lost; the hub replays every unread message on reconnect. `send_message`'s reply tells you the recipient's presence (offline / busy / idle); read it.
- The monitor reconnects by itself after a hub blip (`HUB_RECONNECTING` → `HUB_RECONNECTED`). Restart it only if it EXITS: code 4000 = a newer monitor replaced you (don't start another); code 4001 = hello rejected (token/name).
- A message is **read only when the recipient calls `mark_as_read` after processing it** — never on delivery. Process, then mark.

**Why:** file-based messages got lost when agents were busy. BEADS messages persist, carry read/unread state, and replay until acknowledged.

---

## 2. When to Use What

| Channel | Use for | Example |
|---------|---------|---------|
| **`update_task`** | All task progress, completions, blockers, findings | "Found the bug — query filter was wrong. Fixed in usuarios/page.tsx" |
| **`store_artifact`** | Deliverables, files created, URLs deployed | `type: "file", value: "src/auth.ts"` |
| **`send_message`** | ALL agent-to-agent messages — pings, questions, FYIs, coordination | "Heads up, I changed the DB schema" |
| **`get_presence`** | Who's online / busy / idle before dispatching or waiting on someone | `get_presence()` → "rex busy since 14:02" |
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

`query_tasks(mode: "list")` for all tasks and status; `query_tasks(mode: "show", id)` for notes, artifacts, progress. Poll BEADS for specialist updates. Subagents (Agent tool) return their result directly and don't write to BEADS unless instructed. Agent messages arrive via the hub push (Monitor event for Claude, injected turn for Codex).

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

**Escalation path:** solve it yourself → update BEADS with findings → message GLaDOS via BEADS → last resort, ring the operator.

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

**If you are a specialist:** operator-judgment questions go to GLaDOS via `send_message`, never to a blocking interactive prompt in your own pane. Her pane is the one surface the operator reads; yours is not. (Precedent: §7.4 eunenem 26wof — a question sat blocked on-screen, unnoticed.)

- Genuine product/strategic ambiguity → `send_message(to: "glados", message: "<question + candidate answers>")`, note "blocked on operator input via GLaDOS" in the bead, pivot or wait.
- Do NOT use a multi-choice/selector tool that blocks your turn waiting for a keypress in your pane — it resolves only if the operator happens to be attached to YOUR window.
- GLaDOS relaying the answer (BEADS message, occasionally a keystroke relay into an open prompt) is the real go signal.
- Exception: the operator is already attached to your pane and actively interacting with an on-screen prompt (`agent-liveness §4`) — a live human takes precedence. That's about not corrupting their input, not a license to design around them showing up.

Mirror of `aperture:agent-liveness` (GLaDOS reading YOUR pane): the operator's attention is scarce and GLaDOS-mediated. Design assuming you never have direct access to it.

---

## 8. Codex Agents

If your model starts with `codex/`, everything here applies directly: you call `send_message`, `get_messages`, `mark_as_read`, and the BEADS task tools yourself; inbound messages arrive as injected turns via the app-server bridge — process, then `mark_as_read`. Mechanics in `codex-comms`. The old `@@BEADS@@` pane-scraping protocol is **retired** — never emit it.

---

## 9. Don't Spam

- Don't send the same update twice; don't update every 5 minutes unless something changed.
- DO update when a task runs longer than expected; DO update immediately when blocked — silence is worse than a blocker report.
- One BEADS update per significant milestone, not per line of code.
