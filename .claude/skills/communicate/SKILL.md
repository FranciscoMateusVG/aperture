---
name: aperture-communicate
description: Inter-agent communication patterns for Aperture. Use when sending messages to other agents, reporting task status to GLaDOS, requesting infra work from Peppy, or writing status reports. Triggers on agent messaging, status reports, task handoffs, and infra requests.
---

# Aperture Communication Patterns

This skill defines how Aperture agents communicate. Follow it whenever you report progress, hand off work, or coordinate with other agents.

---

## 1. The Golden Rule

**BEADS is the ONLY communication channel between agents.**

Every message between agents — task updates, quick pings, handoffs, questions, FYIs — goes through BEADS. There is no exception. `send_message` to another agent does NOT exist as a separate file-based pattern anymore.

**How it works:**
- You call `send_message(to: "agent", message: "...")` — this writes a BEADS message record
- Delivery is **push**, via the aperture-bus hub: Claude agents receive events on their inbox monitor — a bash-based Monitor running `node ~/projects/aperture/mcp-server/dist/hub-client.js <your-name>` (persistent), which sends the identifying hello frame to the hub at `ws://127.0.0.1:4517` and streams each frame as an event. Codex agents receive injected turns via the app-server bridge. ⚠️ Never use the Monitor tool's native ws source for the inbox — it is receive-only, cannot send the hello, and leaves you as an anonymous socket the hub treats as offline (bug aperture-1qwty)
- Recipient offline? Nothing is lost — on reconnect the hub replays every unread message
- A message counts as **read only when the recipient explicitly calls `mark_as_read` after processing it** — never on delivery. If you receive a message, process it, then mark it read.

**Why:** File-based messages got lost when agents were busy processing. BEADS messages are persistent, have read/unread state, and replay on reconnect until acknowledged.

---

## 2. When to Use What

| Channel | Use for | Example |
|---------|---------|---------|
| **BEADS `update_task`** | All task progress, completions, blockers, findings | "Found the bug — query filter was wrong. Fixed in usuarios/page.tsx" |
| **BEADS `store_artifact`** | Deliverables, files created, URLs deployed | `type: "file", value: "src/auth.ts"` |
| **BEADS `send_message`** | ALL agent-to-agent messages — pings, questions, FYIs, coordination | "Heads up, I changed the DB schema" |
| **`send_message(to: "operator")`** | **Doorbell only** — fires a notification badge on your row in the launcher. The operator then attaches to your tmux to read your scrollback. NOT a chat surface. | "Need your GitHub credentials for this repo" |

**The only recipient that bypasses BEADS:** `operator` — and that's a notification badge, not a message inbox (see §7).

---

## 3. Task Communication Flow

### Starting work
```
update_task(id: "task-id", claim: true)
update_task(id: "task-id", status: "in_progress")
```

### Progress updates (when something notable happens)
```
update_task(
  id: "task-id",
  notes: "Found that the nav link already exists — only the filter needs changing"
)
```

### Completion
```
store_artifact(task_id: "task-id", type: "file", value: "src/components/Auth.tsx")
update_task(id: "task-id", status: "done", notes: "Implemented auth flow. Build passes. Tests green.")
```

### Blockers
```
update_task(
  id: "task-id",
  notes: "BLOCKED: Need DATABASE_URL for production. Waiting on operator."
)
```

### Handoffs (e.g., builder → deployer)
```
update_task(
  id: "task-id",
  notes: "HANDOFF TO PEPPY: Ready for deploy. Repo: /projects/fitt, Branch: main, Port: 3000, Subdomain: fitt.programaincluir.org"
)
```

---

## 4. Status Report Format

When completing a task, your BEADS notes should be structured enough for GLaDOS (or any agent) to understand what happened without asking follow-up questions:

```
What I did: [1-3 bullet points of actual changes]
Files touched: [list key files]
Next step: [what happens now — review needed? deploy? nothing?]
```

❌ Bad: `"done"`
✅ Good: `"Updated SECRETARIA filter in admin/usuarios/page.tsx to show only CONVIDADO users. Build passes. Ready for review."`

---

## 5. Monitoring Delegated Work (for GLaDOS)

GLaDOS tracks all delegated work through BEADS:

```
query_tasks(mode: "list")              — see all tasks and their status
query_tasks(mode: "show", id: "...")   — read notes, artifacts, and progress
```

When you delegate to specialist agents, poll BEADS for their task updates. Subagents (Agent tool) return their result directly when done — they don't write to BEADS unless you instruct them to. Messages from agents arrive via BEADS — the hub pushes them to you as they land (Monitor event for Claude, injected turn for Codex).

### 5.1 Presence

The hub broadcasts presence events — `join`, `leave`, `busy`, `idle` — for every connected agent (socket connected = present, disconnect = leave). GLaDOS uses this stream as the **primary liveness signal**; pane-peeking remains a forensic fallback only. Don't infer "agent is dead" from silence when the presence stream says otherwise.

---

## 6. Infra Handoff Requests to Peppy

When you need Peppy to deploy, structure it as a BEADS task note:

```
update_task(
  id: "task-id",
  notes: "DEPLOY HANDOFF TO PEPPY:
  - Repo: /projects/my-app
  - Branch: main
  - Service: my-app
  - Port: 3000
  - Subdomain: myapp.programaincluir.org
  - Env vars: DATABASE_URL, ADMIN_SECRET
  - Notes: Docker Compose, needs PostgreSQL"
)
```

Peppy reads BEADS and picks up deploy tasks. The structured format means no follow-up questions needed.

---

## 7. Operator Communication

**The Chat panel is gone.** There is no surface where the operator reads agent messages. The operator interacts with you ONLY by attaching to your tmux window and typing.

**How to reply when the operator messages you:**
Respond in your terminal — print your answer as your normal turn output. The operator is reading the same tmux pane your work appears in.

**How to alert the operator that you need them:**
Call `send_message(to: "operator", message: "<short reason>")`. This **does not deliver text to a UI** — it only lights up a notification badge on your row in the launcher. The operator will see the badge, attach to your tmux window, and read whatever context is in your scrollback.

So:
- The substance of your communication lives in your terminal output.
- `send_message(to: "operator", ...)` is a *doorbell*, not an inbox. Use it sparingly — only when something actually requires the operator's attention.
- Do NOT use it to "reply." Replies go in your terminal.

Use the doorbell for:
- Questions only the human can answer
- Critical status updates or completion of major milestones
- Blockers that need human intervention

**Default escalation path:** Try to solve it yourself → update BEADS with findings → if truly stuck, message GLaDOS via BEADS → last resort, ring the operator's doorbell.

### 7.1 Evidence-attached doorbell rule — NON-NEGOTIABLE

**Banked precedent: lz9y AI intake, 2026-05-23 — three premature "feature live" claims to operator in 90 minutes, each one wrong at a different layer.**

When you tell the operator "X is ready" / "feature is live" / "you can test now" / "deploy complete" — your message MUST include **evidence attached**, not a promise. Evidence = the canonical verify command + its output (or a verbatim quote from the verify), not a description of what you intended to verify.

| ❌ Promise (banned) | ✅ Evidence (required) |
|---|---|
| "Container has the env var" | `docker exec X env \| grep VAR → VAR=true` |
| "Feature is live, you can test" | `curl https://prod/feature → HTTP 200` + `grep /_next/static/chunks/*.js → "FLAG_NAME":"true"` + the URL operator should open |
| "PR merged and deployed" | PR URL + merge timestamp + deploy SHA + container restart timestamp + `curl` of the new feature endpoint |
| "Backend endpoint works" | `curl -X POST https://api/route -d '{...}' → 200 {...}` |
| "Sidebar entry visible" | bundle-grep for the flag value + a description of the role used + screenshot (or the assertion that the bundle inlined the value) |

If you can't produce the evidence, you don't ring the doorbell yet. You either:
- Do the verify first, then ring with the output attached, OR
- Ring with "X is *almost* ready — gate N of M still pending: [the missing verify]"

**The "almost ready" framing is fine.** The banned shape is "X is ready" without evidence, because that ranks the operator's attention against a claim that may not hold. Each false-positive ring burns doorbell credibility; over a session, the operator stops trusting the badge.

### 7.2 Multi-layer verify for "feature live" — the canonical chain

**The general principle (project-agnostic):**

> A feature isn't live until **every layer between source and user** is independently verified at the artifact that layer produces. Each layer is a separate failure surface; jointly they're sufficient, individually they're not. Verify each at the layer's OWN artifact, not at a dependency's artifact.

For your specific project + feature kind, you need to:

1. **Enumerate the layers** the feature passes through from source-control to a user clicking it. Common layer types:
   - Source merged (PR / commit / release tag)
   - Build artifact produced (compiled binary / built bundle / packaged container / published package)
   - Build-time configuration baked (env vars inlined into the artifact, feature flags compiled in, native code linked)
   - Artifact distributed (uploaded to registry / pushed to CDN / shipped to app store / published to npm)
   - Runtime environment configured (env vars set, secrets mounted, config files placed, DNS pointed)
   - Service running (process up, port listening, health endpoint green, app store live, package installed)
   - Gate logic resolves correctly (auth check, feature flag, role check, A/B branch)
   - User can reach the surface (URL responds / app opens / CLI command found)
   - User-visible behavior matches expected (the actual artifact at the surface — UI element renders, response payload correct, command output correct)

2. **For EACH layer, identify the canonical probe** — the smallest command/check that interrogates that layer's OWN artifact, not the one upstream of it.

3. **Run every layer's probe; attach every output to the doorbell.**

**The trap to avoid (recurring across many feature kinds):** verifying layer N+1 because it's cheaper, and inferring layer N. Examples of this anti-pattern:
- Checking the env var is set on the running container (layer 5) and inferring the build-time-inlined client bundle has it (layer 3) — the container has the var but the already-built bundle doesn't. Banked 2026-05-23 (Next.js NEXT_PUBLIC_).
- Checking the package is published to the registry (layer 4) and inferring downstream apps will resolve it (layer 8) — peer-deps or lockfiles can pin the old version.
- Checking the container is running (layer 6) and inferring the route exists (layer 8) — a stale image can be running fine while missing the new route.
- Checking the new column exists in the DB (layer 5) and inferring the code that uses it ships in the same deploy (layer 6) — schema + code can drift.
- Checking the API responds (layer 8) and inferring the auth gate resolves correctly (layer 7) — a permissive default can mask a broken gate.

### 7.2.1 Example protocol catalogue (extend per project)

The general principle is universal; the specific probes are project-specific. Start from your project's existing patterns; add a protocol the first time you ship that kind of feature, refine it the next time. Three reference examples below — replace / extend with the protocols your stack actually needs.

**Example A — Web app feature behind a flag (Next.js + Docker + Dokploy, monorepo-incluir):**
1. PR merged → SHA + merge time
2. Build args declared in Dockerfile → `grep "ARG NEXT_PUBLIC_FLAG" Dockerfile` (only NEXT_PUBLIC_ vars need build-arg wiring; server-only env vars skip this)
3. Build args passed via docker-compose → `grep "build.args" docker-compose.yml | grep FLAG`
4. Deploy completed → container restart timestamp
5. Runtime env present (server-only flags) → `docker exec X env | grep FLAG`
6. Build-time bake present (NEXT_PUBLIC_ flags) → `curl prod/_next/static/chunks/*.js | grep "FLAG":"true"` — the canonical layer-3 probe
7. Route responds with auth → `curl prod/page → 200`
8. Sidebar/nav surface renders → bundle-grep OR authenticated screenshot

**Example B — Backend API endpoint (any HTTP service):**
1. PR merged
2. Deploy reached the running service (restart timestamp or revision id)
3. Endpoint exists → `curl -I prod/api/route` → expected method-allowed status (not 404)
4. Endpoint with auth → `curl -X POST -H "auth: ..." -d '...' prod/api/route` → expected payload shape
5. Downstream side effects → live query of the DB / queue / log that should reflect the action

**Example C — Library / SDK release (any package registry):**
1. Version tag pushed
2. Package published → registry shows the version (`npm view @org/pkg versions`, `cargo search`, `pip show`, etc.)
3. Downstream consumer can resolve → in a fresh project: install the version, import a known-new symbol
4. Downstream consumer's lockfile updated (peer-dep / engines / minimum-version constraint resolves correctly)
5. Smoke test exercising the new symbol passes in the consumer

**For your project's other feature kinds** — mobile app store release, native binary distribution, CLI tool, browser extension, scheduled job, message-queue consumer, IaC change, DNS change, etc. — file the protocol the FIRST time the feature kind ships, in this catalogue, with the same shape: layers + per-layer probe + canonical artifact. Future agents can then run the existing protocol instead of re-deriving it.

### 7.2.2 The meta-rule for any feature kind

> The verify chain for a feature must probe EVERY layer between source and user-visible behavior. The most-skipped layer is "build-time inlining" — anywhere a value gets baked into an artifact at build time, runtime env-check is NOT the right probe; the artifact itself is.

If you don't have a protocol for the feature kind you just shipped, you're not ready to claim "live" — you're ready to author the protocol (in §7.2.1) and then run it. Future-you (and every other agent) gets the durable benefit.

### 7.3 Verify against ORIGIN/main, not your local checkout

**Banked precedent: lz9y AI intake recon, 2026-05-23 — orchestrator grepped local working tree and concluded "frontend doesn't exist," then filed 3 duplicate beads as if greenfield. Vance's subagents caught the duplication, but only after wasted dispatches.**

Before claiming "X is missing" or "X was never built," verify against the CANONICAL reality, not a potentially-stale local mirror:

- File-system claims → `git fetch && git ls-tree origin/main --name-only | grep X` (NOT `find ~/projects/X` on a possibly-stale local clone)
- Code-content claims → `git show origin/main:path/to/file` (NOT `cat` on local)
- Deployed-state claims → curl the prod URL or `docker exec` on the live container (NOT the local dev server)
- Bead-state claims → `bd list --status=open` after `bd dolt pull` (NOT a cached list from session start)

If your local was last `git pull`'d more than ~1 hour ago, treat it as stale and `git fetch` before any claim about main's contents. The same recursion applies that Cipher and Atlas codified: "verify against reality" needs to be applied at the RIGHT artifact layer — local-stale-clone is not the reality you're claiming about.

---

## 8. Codex Agents

> **If you are a Codex agent** (your model starts with `codex/`), everything in this skill applies to you directly: Codex agents now call the aperture-bus MCP tools themselves — `send_message`, `get_messages`, `mark_as_read`, and the BEADS task tools. Inbound messages arrive as injected turns via the app-server bridge; process them, then acknowledge with `mark_as_read` like any other agent.
>
> See the rewritten `codex-comms` skill for Codex-specific mechanics (bridge behavior, turn injection, MCP registration). The old `@@BEADS@@` pane-scraping protocol is **retired** — never emit that command-block pattern; it is no longer intercepted by anything.

---

## 9. Don't Spam

- Don't send the same update twice
- Don't update BEADS every 5 minutes unless something changed
- DO update BEADS if a task is taking longer than expected
- DO update BEADS immediately if you're blocked — silence is worse than a blocker report
- One BEADS update per significant milestone, not per line of code
