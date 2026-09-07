# Aperture V4 — Project Teams: seats, incarnations, presets, safe replacement

**Bead:** aperture-nap3u. **Author:** Wheatley. **Status:** v2.7 (adds §4.10 cross-team lead-to-lead communication, §4.11 framework-awareness prerequisite and §4.12 GLaDOS lead-of-leads operating model, operator-requested 2026-09-06; v2.5 added §4.9) — revised against GLaDOS's architecture batch, Izzy's testability HOLD (12 findings) and her narrow recheck (4 bounded fixes), plus GLaDOS's precision corrections; the resolution table is §11. Gates 4 and 7 in §8 were **approved by the operator on 2026-09-06** (relayed via GLaDOS); the remaining gates are recommended defaults pending final spec approval. **Implementation:** not before the operator resumes Thursday (token reset); nothing in this document authorizes code, installs or a demo before then.

Baseline: master `4fb8cad` (v3.2.0 released and canonical 2026-09-06). **verified** = read in that tree by a read-only recon pass; **hypothesis** = not exercised end-to-end; **GATE** = an unresolved product choice that changes tests and must be decided (with the recommendation given) before the phase that depends on it starts.

---

## 1. Vision and vocabulary

Aperture keeps a **permanent coordination trio** — GLaDOS (orchestration), Wheatley (planning/specs), Peppy (infra/runtime) — and adds **temporary, mission-scoped project teams**. A team is created from an **editable preset**, runs until its acceptance is met and reviewed, and is archived. The vision is *trio + project teams*; today's standing specialists are a transitional arrangement (§5).

| Term | Meaning | Persistence |
|---|---|---|
| **Role** | Reusable persona + skill set (backend, frontend, qa, security, mobile, planner…). Repo template. | Permanent, versioned |
| **Seat** | One named position on one team, bound to one role: `<team>-<role>[-<n>]`. Owns the BEADS assignee, hub identity, registry folder, tmux window. | Lives as long as the team |
| **Incarnation** (worker) | One concrete session filling a seat: harness (Claude/Codex) + model + reasoning + thread id + **generation number** `g = 1, 2, …` | Replaceable |
| **Team** | Mission, seats, lead seat, acceptance, fallback policy. Backed by an epic bead + `team:<name>` label. | pending → active → archived |
| **Preset** | Template team, editable in the UI. Instantiation takes a **snapshot**. | Permanent |
| **Checkpoint** | Structured, reconcilable record of a worker's state; append-only history per seat. | Per seat |
| **Lead** | The seat the operator and GLaDOS contact; owns mission delegation, consolidated reporting and pre-archive reconciliation (§4.9); validates checkpoints; authorizes swaps within policy. | Per team |

Operator preferences already recorded: editable presets are the default flow; snapshot-on-create; start-blank available; personas kept; the full-stack example is illustrative only.

---

## 2. What we can reuse — verified naming compatibility vs required changes

The recon establishes **naming compatibility**, not "reuse unchanged": every subsystem below accepts a new seat name, but the registry, revocation, lifecycle and recipient logic in §4 are **new** code.

| Area | Finding (master `4fb8cad`) | Status |
|---|---|---|
| Agent definition is folder-driven | `just setup` loops `agents/*/` (justfile:55); reads `manifest.json`, `prompts/<name>.md`, `skills.txt`, `resident.txt`; `agent_loader.rs:71-171` has no allowlist | **verified** |
| Hub identity accepts any name | tokens provisioned per boot per folder name (`agents.rs:197`, `hub_auth.rs:77`); `ws-hub.ts:234-275` checks only the token file | **verified** |
| Name charset today | `hub_auth.rs:15-25`, `ws-hub.ts:68`: `^[a-z0-9][a-z0-9_-]{0,63}$`; `presence-hint.ts:52`: `^[a-z0-9_-]{1,32}$`; `sun_path` caps Codex sockets ≈62 chars | **verified**, inconsistent → §4.1 canonical rule |
| tmux / watchdog / codex-bridge / BEADS assignee | window = folder name, `@id` targeting (`tmux.rs:91-112`); watchdog roster = loaded agents; bridge discovers by manifest model prefix; `assignee` is a free string in `beads.ts` | **verified** in this repo; `bd` binary behaviour with a new assignee = **hypothesis** (P0 test) |
| **Hard blocker** | `PERMANENT_RECIPIENTS` allowlist, `mcp-server/src/index.ts:57` (+ prose at `:72-73`) | **verified blocker** |
| Frontend | `roster.ts` sorts known names first; `AgentCard.ts:26-32` renders `agent.name` unescaped assuming a fixed list | **verified**; escaping required |
| Docs coupling | `team` skill and `prompts/glados.md` name the roster; each prompt hard-codes its own inbox line; Codex prompts must not carry Claude monitor instructions (aperture-1socy lesson) | **verified** |
| Resident/lazy + Constitution | Claude: Peppy pilot PASS (leszs). Codex: Rex in flight (PR #62) | **verified / pending** |
| Resume-newest risk | `codex-bridge.ts:359-360` binds newest thread heuristically (its own comment at `:256` warns) | **verified risk** → §4.5 step 5 |
| End-to-end new-seat messaging | acceptance verified, delivery blocked by the allowlist | **hypothesis** (P0 test) |

---

## 3. UX — what the operator sees

The launcher gains a **Teams** area; the fixed trio stays as the "Coordination" group.

### 3.1 Presets library
Preset cards (`name`, mission placeholder, seat chips `backend · gpt-6-astra`). CTAs: **New team from preset**, **Edit preset**, **Duplicate**, **New blank preset**. Editing never touches existing teams.

### 3.2 New Team wizard
```
┌ New team ─────────────────────────────────────────────────────────┐
│ Preset: fullstack ▾                       Team name: [fitt-relaunch]│
│ Mission: [Rebuild raul-fitt landing + booking …]                  │
│ Acceptance: [Izzy walks journey; Lighthouse ≥ 90; PR merged]      │
│ Seats                        role      harness  model      reasoning│
│ ● fitt-relaunch-backend  ★   backend   codex    gpt-6-astra  high  │
│ ● fitt-relaunch-frontend     frontend  claude   opus         —     │
│ ● fitt-relaunch-qa           qa        claude   sonnet       —     │
│ [+ add seat]   ★ = lead (radio)                                    │
│ Fallback policy: operator-approved list [gpt-5.6-sol, gpt-5.6-terra]│
│                          [Cancel]  [Create team]                   │
└────────────────────────────────────────────────────────────────────┘
```
Seat names are derived and validated live against §4.1; a collision with any existing registry folder, archived team or reserved principal is refused inline with the reason. **Create** runs the pending → active protocol of §4.4.

### 3.3 Grouped sessions view
Team group header: mission, lead, epic bead (or `pending`), open PRs, `Archive…`. Seat card: role, lead badge, harness + **actual model** (from the session, §4.5 step 5), turn state (`busy`/`idle`/`offline`/`rate-limited`/`dead`), context % (from transcript usage), generation `g`, checkpoint age, CTAs `Open`, `Checkpoint now`, `Replace worker…`, `Stop`.

### 3.4 Replace worker dialog
```
┌ Replace worker: fitt-relaunch-backend (g=2) ──────────────────────┐
│ Current: codex · gpt-6-astra · high · rate-limited (since 03:12)   │
│ New:     harness [codex ▾] model [gpt-5.6-sol ▾] reasoning [high ▾]│
│ Checkpoint: g2#7 · 00:04 ago · verified ✔ (or: none valid — stale) │
│ Stop verification:                                                 │
│   ☐ checkpoint requested / timed out (fallback noted)              │
│   ☐ owned process tree stopped (N pids, start-time matched)        │
│   ☐ hub/message authority revoked (socket closed, code 4001)       │
│   ☐ remote effects reconciled (0 in-flight)                        │
│   ☐ worktree inventoried (no changes made)                         │
│ Policy: gpt-5.6-sol is on the approved fallback list → lead/GLaDOS │
│                              [Cancel]  [Stop, verify, then start]  │
└────────────────────────────────────────────────────────────────────┘
```
The confirm CTA is disabled until every box is green; any red box shows the blocking reason and **no new worker is created**.

### 3.5 Archive
Checklist gate (§4.7). Archive is a single state transition with an explicit rollback; nothing is deleted.

---

## 4. Architecture

### 4.1 Names (GATE #7 — approved by operator 2026-09-06)
Proposed canonical seat-name rule (a technical bound, offered as the recommended default), one literal regex used by hub, Rust, presence-hint and the UI: **`^[a-z0-9][a-z0-9_-]{0,30}$` — total length 1–31 characters.** Rationale: 31 stays under the tightest existing gate (presence-hint's 32), under the ≈62-char `sun_path` ceiling with room for `~/.aperture/run/` + `.sock`, and leaves headroom for generated suffixes. Team name ≤ 16, role ≤ 10, optional `-<n>` (n ≤ 99) → `16 + 1 + 10 + 3 = 30 ≤ 31`; the wizard enforces the per-part limits so the composed name can never exceed 31. **Collision handling:** the composed name must not equal (a) an existing registry folder (active or archived team), (b) a coordination-trio name, (c) reserved principals (`operator`, `watchdog`, `shared`), (d) any `_`-prefixed name; duplicates within a team append `-2`, `-3` only when the operator adds two seats of the same role. Boundary fixtures: length 1, 31 (pass), 32 (fail), leading `-`/`_`/digit-then-upper, `.`/`:`/space/`<` (fail), `t1-backend`, `team-fullstack-lead` (pass). **Defense-in-depth seam for the `<` fixture:** the loader (`agent_loader.rs`) applies the same regex and rejects the folder with a warning; the UI additionally escapes `agent.name` before `innerHTML`; the visual fixture injects an invalid name **below the loader** through the `list_agents` test double so the escaping path is exercised even though a real folder can never carry it.

### 4.2 Where things live
| Thing | Location | Notes |
|---|---|---|
| Role templates | repo `roles/<role>/{prompt.md.tmpl, skills.txt, resident.txt}` | reviewed like skills; templates render the seat's own inbox instructions per harness (Codex: bridge-delivered; Claude: Monitor) |
| Presets | repo `teams/presets/` (shipped) + `~/.aperture/teams/presets/` (operator-edited) | GATE #1 (recommendation: runtime dir) |
| Team snapshot + state | `~/.aperture/teams/<team>/team.json` (+ `state.json`: `pending|active|archived`, `generation`) | runtime, never source |
| Seat definitions | `~/.claude/aperture/<seat>/…` with a `TEAM` marker file | written by the launcher only via the staging protocol below; `just setup` skips dirs carrying the marker |
| Checkpoints | bead note **and** `~/.aperture/teams/<team>/checkpoints/<seat>/<g>-<seq>.json` | file mirror makes recovery independent of Dolt |
| Ownership record | `~/.aperture/run/owner/<seat>.json` + `<seat>.lock` | §4.3 |

**Staging protocol (P1 failure semantics, crash-consistent):** create everything under `~/.aperture/teams/.staging/<uuid>/`, validate (names, budgets, template rendering, no path escapes — §8). Nothing is moved into the registry while the team is `pending`: seat dirs stay in staging until GLaDOS confirms the epic id (**move timing = at the pending → active transition**, never at create). The transition is a **journaled multi-rename**: (a) write `journal.json` `{team, uuid, moves:[{from,to}], step:0}` and fsync it **before** the first rename; (b) perform the renames in journal order, updating `step` (fsync) after each — `step` is a **hint only**: recovery never trusts it and instead **reconciles physical state per move** (a move is *done* iff `to` exists and `from` does not; *not done* iff `from` exists and `to` does not; both or neither present → `E_JOURNAL_INCONSISTENT`, team marked `failed`, nothing further moved), which covers a crash between a rename and its step fsync; (c) write each seat's `.complete` marker; (d) rename `state.json` to `active`; (e) delete the journal. **Visibility rule (loader, `just setup`, `list_agents`):** a team seat is loadable **only if** its `.complete` marker exists **and** the owning team's `state.json` reads `active` **and** no `journal.json` exists for that team — markers alone are insufficient because they precede the state flip; a seat failing any of the three is reported as `pending` and never booted or listed. **Crash points and outcomes:** before (a) → only staging exists, cleaned on next launcher start; between (a) and (d) → recovery reconciles physically and **rolls forward** if every not-done move still has its `from`, otherwise **rolls back** every done move (inverse renames) and marks the team `failed:<n done>`; after (d) → team is active, journal deletion is idempotent. Fixtures (P1 gate): kill after a rename but **before its step fsync**; kill after all `.complete` markers but **before the state flip**; kill after the state flip but before journal deletion — in every case `list_agents` shows **zero** `t1-*` seats until the team is durably active, and the tree is either fully pre-activation or fully active. Any validation failure before (a) leaves **only** the staging dir, which is removed; the source checkout is never written. Errors are enumerated: `E_NAME_INVALID`, `E_NAME_COLLISION`, `E_PRESET_INVALID`, `E_BUDGET_EXCEEDED`, `E_STAGING_IO`, `E_CREATION_GATE_PENDING`, `E_LOCK_HELD`. The team is **pending** until GLaDOS confirms the epic bead id (creation gate unchanged: the launcher sends GLaDOS a BEADS message; GLaDOS files after operator ack). A pending team may boot nothing; if GLaDOS is offline or refuses, the operator sees `pending: <reason>` and may cancel (rollback = remove the team dir and the marked seat dirs; residue = none; the snapshot copy is kept under `.staging/rejected/<uuid>` for 7 days for inspection).

### 4.3 Durable ownership (open requirement A — recommendation, not validated)
Requirement: at most one **writable incarnation** per seat, enforced across the GUI launcher, any headless CLI path and launcher/hub restarts, with durable recovery. Recommended design (an **implementation gate** until a P1 test validates it on APFS):
- `~/.aperture/run/owner/<seat>.lock` held with an **OS advisory lock** (`flock`) by the launcher process for the lifetime of its ownership operations; the lock is the single-writer fence between concurrent GUI/CLI actors.
- `~/.aperture/run/owner/<seat>.json` `{generation, incarnation:{pid, start_time, thread_id, harness, model}, since, writer}` written **under the lock** with an **expected-generation check** (read → compare `generation == expected` → write temp → `fsync` → `rename` → `fsync(dir)`); a mismatch fails with `E_GENERATION_MISMATCH` and no write. Rename gives atomic replacement of the visible file; durability comes from the fsyncs; the lock supplies the compare-and-set — none of the three alone is sufficient, and this spec does not claim rename is a CAS.
- Restart semantics: on launcher start, every owner record whose `pid`/`start_time` do not match a live process is marked `stale` (visible in the UI) and is never reused implicitly; a new generation must be explicitly started through §4.5.
- The hub caches ownership for display only. BEADS notes and JSON rows edited read-then-write are **not** claims.
- Deferred: a Dolt-backed lease for multi-machine; not needed for the single-Mac deployment.

### 4.4 Checkpoints (open requirement C)
Two writers exist — the `checkpoint` MCP tool (worker-initiated, both harnesses, at explicit milestones) and, **on Claude only**, the Stop hook (harness-initiated, best-effort; Codex has no equivalent hook and uses the tool/protocol exclusively) — so the record carries `checkpoint_id = <seat>/<g>/<seq>` with `seq` assigned by the launcher-side writer service, `schema_version`, `written_by`, `written_at`. Rules: append-only; the **highest `seq` with `validation: ok`** is the current checkpoint; a record with an unknown `schema_version` is stored but marked `rejected` and never used for recovery; identical content re-submitted within 5 s is deduplicated by content hash (idempotent). Lead validation compares `head_sha`, `dirty_files`, `open_pr` against `git`/`gh` and stores `validation: ok | divergent(<fields>)`. **A Stop-hook checkpoint is not guaranteed** (crash, kill, hook timeout): recovery must work from `none` or `stale` (age > policy, default 15 min, or `divergent`) — the replacement then inventories the worktree as-is and treats the previous checkpoint as a hint. Replayed messages are idempotent acknowledgements only; **any side effect (commit, push, deploy, PR comment) is repeated only after reconciling the real artifact** shows it did not happen.

### 4.5 Safe stop-before-replace (open requirement B — honest scope)
**Guarantee offered:** *the replacement starts only after the previous incarnation's locally owned processes are stopped and its message/hub authority is revoked; in-flight remote effects are reconciled or the swap is blocked.* Aperture has **no tool-level write fencing**: filesystem/git/ssh/deploy/provider credentials are not revoked by stopping a process. The UI label is therefore "**message/hub authority revoked**", never "tool authority".

Protocol (fake-clock deadlines; every step emits a structured event for tests):
0. **Ownership snapshot first.** Before any kill, record the owned process set: the tmux pane's process group, every descendant by `ppid` walk, each with `pid + start_time + cmdline + cwd`. Reparenting after the parent dies cannot hide processes that were recorded here. Processes merely *matching* the worktree cwd/cmdline but **outside** this set are **evidence, not authority**: they are listed for the operator and block the swap if unowned (`E_UNOWNED_PROCESS`), never killed.
1. **Checkpoint request.** State source is the hub turn-state plus the harness error stream: `busy` → request checkpoint, wait `T_ckpt` (default 90 s on the fake clock); `rate-limited` (signature: provider 429 / `rate_limit` error in the last 60 s of the session's error stream, TTL 60 s) → same request, then proceed using the last valid checkpoint flagged `stale`; `dead` (window gone or pane pid missing) → no request, proceed with the last valid checkpoint or `none`; timeout → proceed with `stale`/`none`. A missing checkpoint never blocks the swap; it changes recovery (§4.4).
2. **Stop the owned tree.** `SIGTERM` to the recorded set (children first), wait `T_term` (10 s fake clock), `SIGKILL` survivors, then verify every recorded `pid + start_time` is gone. **Dead workers go through this step too** — descendants of a dead CLI are the common orphan case. Unreadable process table or a survivor after `SIGKILL` → `E_STOP_UNVERIFIED`, swap blocked.
3. **Revoke message/hub authority — durably and immediately.** The launcher (a) writes a revocation record `{seat, generation, thread_id, token_id, at}` to `~/.aperture/run/revoked/<seat>.json` (durable across hub restarts; the hub loads it at start), (b) tells the hub over its control channel to **close the known socket(s) now** with close code **4001 `revoked`** (not on the next frame), (c) deletes the token file. Oracle: the old socket receives close 4001 within 1 s; a reconnect with the old token is rejected with 4003 `revoked-token`; the hub restarted afterwards still rejects it; the new incarnation gets a **new token id** provisioned only after (a)–(c) complete. Deleting the token file alone is not revocation proof.
4. **Reconcile remote effects.** From the checkpoint's `running_procs`/`decisions` and the seat's recent tool calls (transcript metadata only), list in-flight remote work (ssh sessions, deploys, CI runs, provider jobs). Each item is `finished`, `cancellable → cancelled`, or `unknown`; any `unknown` blocks the swap (`E_REMOTE_UNCERTAIN`) until the lead/operator marks it resolved. Provider keys are not rotated by this protocol; if the previous incarnation held one, that is called out.
5. **Start g+1** with an explicitly selected harness/model/reasoning and a **new thread id** (never resume-newest); write the owner record under the lock with expected generation `g`; verify the actual model from the new session (Codex `thread/start` response; Claude first assistant `message.model`) and display it; mismatch → the card shows `model unverified` and the lead is notified.

Blocked swap **invariants** (tested): no new owner record, no new token, no new thread, no new window; old worktree byte-identical; the failure reason and evidence are shown.

### 4.6 Permissions, recipients, messaging semantics
- **Recipient validation** (P0): `send_message.to` is valid iff the name is in the **enabled seat registry** (the same folders `agent_loader` loads, `enabled:true`, plus `operator`) — existence in the registry, **not** a live token, defines a recipient. Messages to an **enabled, never-booted** or **stopped-between-incarnations** seat are stored in BEADS and replayed on that seat's next hello (existing replay path); messages to an **archived/disabled/unknown** seat fail closed with `E_UNKNOWN_RECIPIENT`. A seat can only send as its own authenticated identity (hello token ↔ `AGENT_NAME` must match; mismatch closes with 4002).
- **BEADS**: a new seat must round-trip `update_task(claim)` and `assignee` through the real `bd` binary (P0 test) — the hub round-trip alone does not prove it.
- **Authority** (GATE #4 — approved by operator 2026-09-06): swaps whose target model is on the team's **operator-approved fallback list** are authorized by the lead or GLaDOS; any other model, harness change, or budget change needs operator acknowledgement (the dialog waits). Create team: operator via UI, epic filed by GLaDOS after ack. Archive: GLaDOS after the checklist. Delete: nobody.
- **Lead as contact:** GLaDOS dispatches to the lead; seats keep the Constitution (no sweeps, await scoped dispatch, process-then-ack).

### 4.7 Archive — one canonical state
Archived seats: manifest `enabled:false` **and** the seat folder moved to `~/.claude/aperture/_archived/<team>/<seat>/` (the `_` prefix is already skipped by the loader, `agent_loader.rs:93`) **and** `team.json` moved to `~/.aperture/teams/archive/<team>/`. `just setup` never touches `_`-prefixed dirs, so nothing is resurrected. Preconditions: the lead's **reconciliation record** (§4.9) is complete and every item resolved; epic success-metric evidence recorded as `{metric, observed_at, evidence_ref}`; all seat beads closed **or transferred per §4.9** (an approved transfer removes active ownership from the closing team without erasing history); review recorded `{reviewer, verdict, at}`; zero live processes for every seat (owner records all `stale` or absent); worktrees clean **or** carrying a `.aperture-protected` marker file. Archive **blocks** on any unresolved or unowned work item, any missing required review, or any transfer not yet accepted by its named owner. Archive uses the **same journaled multi-rename** as §4.2: journal written and fsynced first; renames in order with `step` updates; `_archived/<team>/<seat>/.complete` markers written last (a seat dir without the marker is neither loadable nor considered archived); `state.json = archived`; journal deleted. Recovery reconciles **physical** from/to state per move exactly as in §4.2 (the journal `step` is a hint only); roll forward if every not-done move still has its source, else roll back to the exact pre-archive tree (oracle: byte comparison of the seat dirs and `team.json` against the pre-archive manifest of paths + sha256 written into the journal). Visibility: a seat counts as archived only when its `_archived/.../.complete` marker exists **and** `state.json` reads `archived` **and** no journal exists; until then the loader treats the team as `archiving` and boots nothing. Fixtures mirror §4.2 (kill before step fsync; kill after markers before state flip). Manual rollback = the same inverse-rename script driven by that manifest. Same-name reuse is refused while an archived team of that name exists (rename the archive first).

### 4.8 Constitution and context budget — measured, not promised
Per-seat structure: `resident.txt` = `constitution` + one role core; everything else lazy; no `/context` inside measured windows. Budgets are **acceptance thresholds on measured tokens**, with a **provider-specific formula** (the two APIs report cache differently and must not be summed the same way): **Claude (Anthropic)** — first request total = `input_tokens + cache_creation_input_tokens + cache_read_input_tokens` (three disjoint fields); **Codex (OpenAI)** — first request total = `input_tokens` only, because `input_tokens_details.cached_tokens` is a **subset** of `input_tokens` and is reported alongside, never added; `total_tokens` (which includes output) is never the compared figure. Both: deduplicate streamed records by message id, fresh thread proven by id, and **the same role, model and reasoning effort as the baseline** — never compare across roles, models or providers. Baselines and thresholds: Claude — Peppy pilot fresh first request 60,260 (Opus, 2f847855) → seat target **≤ 1.10 × = 66,286**; Codex — Rex at 5266ad0, thread 01a07950 (gpt-5.6-sol/high): `input_tokens` 34,208 (of which `cached_tokens` 7,552) → seat target **≤ 1.10 × = 37,629**. Any figure that summed Codex cached tokens on top of input (e.g. the earlier 34,338/43,393 pairs) is superseded. **First-task growth ≤ 1.5 × the first request is a proposed budget, not an empirical finding**; it is measured on a fixed fixture task (defined in P1: one scoped backend task with exactly one lazy skill load, no `/context`) so successive runs are comparable. Byte caps on prompt/resident remain gates, not the acceptance.

---

### 4.9 Lead accountability (addendum, operator-requested 2026-09-06)
The lead's responsibilities are explicit, not implied:

1. **Mission delegation.** The lead receives the mission from GLaDOS, decomposes it and **proposes** the seat tasks; **GLaDOS alone files the beads, after operator acknowledgement** (the creation gate is unchanged — the lead never creates beads). Every filed task keeps its **actual seat as assignee** (`t1-backend`, not the lead) so ownership and BEADS history stay truthful.
2. **Consolidated reporting.** Workers report progress and blockers **through the lead**; the lead sends GLaDOS consolidated progress/blocker reports in the standard shape (what changed, files/PRs, blockers with owner, next step). Batching rule — **same-turn coalescing, no timer**: when the lead handles its inbox, every blocker/progress item already received in that turn is coalesced into **one** report; a blocker that arrives after that report was sent yields an **immediate delta report** in the next turn (one report per batch, never one-per-milestone and never two for the same batch). Workers still write their own bead notes/artifacts; the lead's report links to them rather than restating them. GLaDOS may still read any seat bead directly.
3. **Pre-archive reconciliation.** Before requesting archive the lead walks **every** mission bead, seat bead and required review and records a **reconciliation record** on the epic bead, one line per item, each with exactly one disposition:
   - **completed** — the bead's own acceptance criteria are met with evidence (PR/artifact/receipt cited); a bead is never marked completed to satisfy the archive gate;
   - **cancelled** — only by an explicit, authorized disposition under the **existing** approval rules (operator acknowledgement, relayed and recorded on the bead by GLaDOS with the reason); this introduces no unilateral cancellation authority for the lead or for GLaDOS — the lead may propose, never self-authorize;
   - **deferred (transferred)** — an explicit, **approved and accepted** handoff to a **named owner and task outside the closing team** (approval under the existing operator/GLaDOS rules; acceptance by the receiving owner in writing, BEADS message referenced in the record). The handoff reconciles **structure as well as assignee**: the bead is re-parented out of the closing team's epic to the receiving owner's epic/task (or explicitly left as a `related` link when it has no new parent), `discovered-from`/`transferred-from:<seat>` provenance and the full history stay on the bead, and the closing epic's open-child gate and its mission success metric are re-evaluated afterwards — changing the assignee alone never satisfies `bd close` on an epic with open children nor an unmet success metric. Until approval and acceptance both exist the item counts as unresolved.
   The `all seat beads closed` gate of §4.7 is preserved by this definition: a transferred bead is no longer owned by any seat of the closing team, so the team's seat beads are all closed **or transferred**, and nothing is deleted or rewritten.
4. **Archive request.** The lead requests archive only when the reconciliation record shows zero unresolved items; the archive checklist (§3.5/§4.7) displays the record and **blocks** on: any item without a disposition, any `deferred` without recorded approval **and** acceptance, any required review missing, any **unfinished** bead still assigned to a seat of the closing team (closed historical beads keep their original seat assignee — history is never rewritten), any open child still parented under the closing epic, or an unmet mission success metric.

**Fixtures (P4 gate):** (a) one seat bead left open with unmet acceptance → archive blocked with that bead named; (b) a bead marked completed without evidence → reconciliation validator flags `completed-without-evidence`, archive blocked; (c) a proposed cancellation without an operator/GLaDOS disposition note → blocked; (d) a transfer to a named external owner **without** approval or acknowledgement → blocked; the same transfer **with** both → passes only once the bead is also re-parented out of the closing epic: assignee changed, `transferred-from` note and full prior history intact, closing epic shows no open child for it, original team's history untouched; a transfer that changes assignee but leaves the bead parented under the closing epic → blocked; (e) lead report fixture: two blockers already in the lead's inbox when it processes → GLaDOS receives **exactly one** consolidated report naming both blockers and owners; a third blocker delivered after that report → exactly one **delta** report naming only the third (assert total reports = 2, no duplicates, deterministic under a fake-clock sequence: deliver A,B → lead turn → deliver C → lead turn); each worker's own bead notes still carry the original detail; (f) creation fixture: the lead proposes a decomposition with seat assignees → GLaDOS files after the operator ack → each bead has `created_by = glados` and `assignee` = the actual seat (never the lead); a bead created by any other actor for the team → flagged by the reconciliation validator as `creation-gate-violation`.

### 4.10 Cross-team communication — lead-to-lead only (addendum, operator-requested 2026-09-06)
Several feature teams may work the **same project** concurrently. Coordination between them is restricted to their **leads** and runs over BEADS with identities taken from the trusted registry, enforced **server-side** in the aperture-bus MCP + hub — never by prompt text alone.

**Identity.** Every seat's `team.json` snapshot carries `project` (the canonical `project:<name>` label), `team`, and the seat's `role`; `is_lead` is derived from the team's lead field. The aperture-bus MCP resolves the **sender** from its own authenticated identity (`AGENT_NAME` ↔ hub token, §4.6) and the **recipient** from the enabled seat registry — a message never carries self-declared project/role/lead claims; any such fields in the payload are ignored, not trusted.

**Authorization rule (evaluated per message, at send time and again at delivery):** `send_message(to)` from a team worker or lead is allowed iff one of:
1. **Intra-team:** sender and recipient are seats of the same team (workers route everything else through their own lead);
2. **Lead-to-lead, same project:** sender is the lead of team A, recipient is the lead of team B, and `A.project == B.project`, both teams `active`;
3. **Explicitly authorized paths:** any seat ↔ the coordination trio (GLaDOS, Wheatley, Peppy) and GLaDOS ↔ anything (oversight is not reduced by this section); `operator` doorbell; and **explicit scoped grants** recorded in the trusted registry (`team.json` `grants: [{from: <seat|team>, to: <seat>, scope: "message", by: glados, at}]`, filed by GLaDOS under the existing approval rules) — e.g. a QA seat's grant to receive from the seats it verifies. There is **no blanket permission by role name**: "QA" as a role grants nothing; only a recorded grant does.
**Precedence with §4.6:** recipient validation runs first — an unknown, disabled or archived recipient fails with `E_UNKNOWN_RECIPIENT` exactly as in §4.6, before any cross-team rule is consulted. Only for a **valid principal** does the cross-team rule apply; everything not allowed above then **fails closed** with `E_CROSS_TEAM_DENIED` and a reason code (`nonlead`, `cross_project`, `team_not_active`, `no_grant`): a worker addressing another team's seat, a lead addressing another team's **worker**, any lead addressing a lead of a **different project**, or a message that would need a grant that does not exist. Denials are logged as metadata only (sender, recipient, reason — no body).

**What the channel does and does not grant.** Lead-to-lead messages are coordination: sharing interfaces, sequencing, blockers, review requests. They confer **no authority**: the receiving lead owns delegation inside their own team; a message cannot reassign or cancel another team's task, add scope, or change any seat's model/budget (those follow §4.6/§4.9 and the operator's approval rules). **BEADS task access remains Constitution-governed** (C-1 creation gate, assigned-bead discipline, GLaDOS-owned queue): this section introduces **no task ACL architecture** and claims no existing server-side task enforcement (`mcp-server/src/index.ts` applies no caller/assignee/team check on `update_task`/`query_tasks`/`search_tasks` today — verified). What is testable now is the invariant that **`send_message` itself never mutates any bead**; task-scope enforcement, if ever wanted, is a separate future decision, not part of V4.0.

**Offline delivery and replay.** A message to an enabled-but-offline lead is stored (§4.6) under its **stable message id** and, on that lead's next hello, **re-validated against the current registry before delivery**. Delivery is **replay-until-ack** by stable message id (the existing unread-replay + `mark_as_read` semantics): a message is replayed on every hello until the recipient acknowledges it; a recipient that **crashes before acking** is redelivered the same id and must reconcile against artifacts before repeating any effect (§4.4). This spec makes **no once-only delivery claim** — "once" holds only *after* the ack is durably recorded. Then: if the recipient is no longer a lead (demoted), the team is archived/disabled, the projects no longer match, or the sender's authorization no longer holds, the message is **not delivered** and is marked `withheld:<reason>` for GLaDOS to see. Role changes therefore cannot retain privileges: authorization is a property of the current registry, never of the stored message.

**Fixtures (P0/P3 gate, deterministic, test hub + registry fixture with two same-project teams `p1-a`, `p1-b` and one other-project team `p2-c`):** (i) lead `p1-a-lead` → lead `p1-b-lead` delivered and acked; reverse direction likewise; (ii) worker `p1-a-backend` → `p1-b-frontend` → `E_CROSS_TEAM_DENIED nonlead`; worker → other team's lead likewise denied; worker → `p9-nope` (unknown) → `E_UNKNOWN_RECIPIENT`, never a cross-team code (precedence); (iii) lead `p1-a-lead` → lead `p2-c-lead` → denied `cross_project`; (iv) payload carrying forged `role: lead` / `project: p1` from a worker → fields ignored, denied on registry truth; forged hello (token of one seat, name of another) → hub closes 4002 as in §4.6; (v) `p1-b-lead` offline: message stored with its stable id; lead comes back → delivered; crash the lead **before** it acks → next hello redelivers the same id (replay-until-ack); lead acks → no further replay on subsequent hellos; a forced duplicate replay of an acked id → idempotent ack, **no duplicated effect**; (vi) same as (v) but `p1-b` lead is changed to another seat before the hello → message `withheld:nonlead`, not delivered, visible to GLaDOS; (vii) GLaDOS → any seat and any seat → GLaDOS/Wheatley/Peppy/QA remain deliverable throughout; (viii) a lead-to-lead message whose body "reassigns" or "cancels" a task → the target bead's assignee/status/notes are byte-identical before and after delivery and ack (the channel mutates nothing); no server-side task ACL is asserted or invented — BEADS access stays Constitution-governed.

### 4.11 Framework-awareness prerequisite (operator-raised, docs-only)
Before any team boots, the prompts and skills that seats and the trio carry must agree with this specification. Prerequisite work (docs/prompts only, no runtime change):
1. **Contradiction review** of the trio prompts (`prompts/{glados,wheatley,peppy}.md`), the role templates (`roles/*/prompt.md.tmpl`) — including **explicit lead-specific guidance** (a lead template section covering §4.9 delegation/reporting/reconciliation, §4.10 routing and §4.12 escalation) — and the resident/indexed skills (`constitution`, `beads`, `communicate`, `team`) for: team identity (seat names, project label), message routing (§4.6/§4.10 — workers via their lead, leads to leads, trio paths, scoped grants), task ownership (actual seat assignee, GLaDOS-only creation), reporting (§4.9 batches), checkpoints (§4.4 schema, who writes, who validates), archive (§4.7/§4.9 reconciliation) and the §4.12 responsibility split. Every contradiction gets a DECISION row in the constitution's `DECISIONS.md` (provenance: this spec, operator 2026-09-06) and the losing text is marked superseded in place, per the existing constitution discipline.
2. **Harness-specific behaviour tests, fresh sessions:** for each harness (Claude, Codex) one fresh seat boots and must (a) know its team/project/lead and route a cross-team request through its lead, (b) never start the other harness's inbox mechanism (extends `codex-startup-instructions.test.mjs`), (c) write a valid checkpoint by the harness-specific mechanism — **Claude:** the Stop hook (best-effort, may be missing after a crash) plus explicit milestone `checkpoint` tool writes; **Codex:** the explicit `checkpoint` tool/protocol only (there is no Stop hook on that harness and none is assumed) — never relying on a dead worker to have written one, (d) refuse to create beads — measured on transcript usage, no `/context` in the window.
3. **Budgets preserved:** the Constitution stays resident and unchanged in scope; per-seat boot stays within §4.8 thresholds; **the specification itself is never injected into any prompt** — seats get the constitution + role core, and this document is referenced by path for on-demand reading only. Security posture (§7 security acceptance, credential rules) unchanged.

### 4.12 GLaDOS as lead of team leads — operating model (addendum, operator-requested 2026-09-06)
| Layer | Owns | Does not own |
|---|---|---|
| **Operator** | direction and priorities; budget and model approvals; acknowledgement for every bead creation; gate decisions | day-to-day execution |
| **GLaDOS** (lead of leads) | the mission portfolio; team formation and lead appointment (from presets, §3); inter-team dependencies, escalations and conflicts; closeout oversight (reconciliation review, archive); **GLaDOS-only bead filing after operator ack** (leads propose batches, §4.9.1 — no new autonomy grants); routine monitoring at the level of **missions, leads and exceptions**; direct audits and emergency visibility into any seat at any time | ordinary in-policy recovery inside a team; worker-level task supervision |
| **Team lead** | mission execution; delegation to seats; ordinary in-policy recovery (swaps within the approved fallback list, §4.6); consolidated reporting (§4.9.2); pre-archive reconciliation (§4.9.3); lead-to-lead coordination (§4.10) | bead creation; cancellations or transfers without the existing approvals; anything outside policy → escalate to GLaDOS |
| **Worker seat** | scoped task execution and evidence on its own beads; checkpoints; reporting through its lead | cross-team messaging; queue discovery; self-claim |
| **Wheatley / Peppy** (coordination trio) | planning/spec support and shared infra/runtime support **to GLaDOS and to leads on request** — coordination roles, not a second command chain over workers | directing workers; owning missions |

Escalation path: worker → lead → GLaDOS → operator; GLaDOS may short-circuit downward for audits and emergencies. Nothing in this section changes the creation gate, the approval rules in §4.6/§4.9, or the Constitution.

---

## 5. Migration from the fixed roster (transitional)

| Phase | Change | Rollback |
|---|---|---|
| M0 | Registry-derived recipients (fail closed); roster unaffected | revert one file + `just build-mcp` |
| M1 | Roles extracted from the five specialists into `roles/`; the specialists keep running as **standing seats — explicitly temporary** | roles dir unused |
| M2 | Teams backend + UI; first team runs beside the roster | archive the team |
| M3 | Specialists retired or re-homed into teams so the runtime matches the vision (**trio + project teams**); timing is the operator's | none needed |

GLaDOS/Wheatley/Peppy stay permanent, fixed-name seats. No history rewrite, no worktree deletion, no registry wipe.

---

## 6. Phases, owners, gating tests (Thursday onward)

| Phase | Work | Owner | Depends on | Gating tests (deterministic) |
|---|---|---|---|---|
| P0 | Registry-derived recipients; one canonical regex in hub/Rust/hint/UI/loader; `agent.name` escaping; `just status` from `agents/*/`; `just setup` skips `TEAM`-marked and `_` dirs | Rex | — | recipient: unknown/disabled → `E_UNKNOWN_RECIPIENT`; enabled-never-booted → stored + replayed on first hello; new-name seat hub round-trip; **real `bd` claim/assignee round-trip**; regex boundary fixtures (§4.1); loader rejects invalid folder; UI escapes injected `<` name |
| P1 | Presets, snapshot, staging protocol, seat generation, owner lock + generation record, `team_create/list/cancel` commands, GLaDOS filing message, pending state | Rex (Rust/TS) + Wheatley (templates) | P0 | each `E_*` path leaves only `.staging`; concurrent create → `E_LOCK_HELD`; owner write with wrong generation → `E_GENERATION_MISMATCH`, no write; crash between temp write and rename leaves the previous record readable; baseline token capture |
| P2 | Teams UI: library, wizard, grouped view, seat cards, Replace dialog, Archive checklist | Vance | P1 | §7 visual contract |
| P3 | Checkpoint writer service + tool + Stop hook; replacement protocol; hub revocation channel + close codes; remote-effect reconciliation list | Peppy (runtime) + Rex (hub) | P1 | busy / rate-limited / dead fixtures via an **injected state source** (test hub + fake harness error stream) and a **fake clock**; TERM/KILL timing; recorded-set vs unowned-match; recycled-pid negative; `none`/`stale` checkpoint recovery; revocation oracle (4001 within 1 s, 4003 on reconnect, survives hub restart); blocked-swap invariants |
| P4 | Archive + rollback script; M1 role extraction | Wheatley + Peppy | P2, P3 | archive refused with live pid / open bead / missing evidence; canonical state after archive; rollback restores byte-identical tree; same-name reuse refused |
| QA | one consolidated pass per phase + §7 journey | Izzy | each phase | transcript-usage receipts only |

Sizing (honest): P0 ½ day; P1 1½–2 days (lock/staging semantics are the bulk); P2 1½ days; P3 2 days; P4 ½–1 day; QA interleaved — about **one specialist-week**. A **foundation demo** (P0 + P1 + manual P3) is the smallest thing that proves seat ≠ session ≠ model and is *not* "V4 shipped".

---

## 7. Acceptance and testability (for Izzy)

**Fixtures (synthetic only):** `teams/presets/fullstack.json` (3 seats, lead = backend, fallback list `gpt-5.6-sol, gpt-5.6-terra`); a throwaway mission bead pre-approved for the window; two disposable worktrees; synthetic secret sentinels `SENTINEL_V4_SECRET_A/B` placed in a fixture skill body and a fixture env file; a test hub with an **injectable turn-state/error-stream source** and a **fake clock**; an unreadable-process fixture (a zombie with a stripped `/proc`-equivalent read permission on Linux CI, a `ps` denial shim on macOS); a recycled-pid fixture.

**Primary journey (launcher, 1280×800 then 1024×768):**
1. Teams → Presets shows `fullstack` → card lists 3 seat chips + lead marker. Click **Edit preset**, **Duplicate**, **New blank preset** each once → each opens and **Cancel/Escape** returns focus to the originating button.
2. **New team from preset**, name `t1`; type `T1.x` → inline error associated to the field (`aria-describedby`); `t1` accepted; seat names render `t1-backend`, `t1-frontend`, `t1-qa`; **+ add seat** twice (role backend) → `t1-backend-2`, `-3`; lead radio moves focus correctly.
3. **Create team** → `~/.aperture/teams/t1/state.json = pending`; the three generated seat dirs exist **only under `.staging/<uuid>/`** (nothing in the registry yet — `list_agents` shows no `t1-*` seats); preset file hash unchanged; GLaDOS receives the filing message; after the epic id arrives the journaled move runs → `active`, three `TEAM`-marked seat dirs with `.complete` markers now exist in the registry; the journal file is gone. Negative: cancel while pending → team dir and staging removed, `.staging/rejected/<uuid>` retained; kill the launcher between journal step 1 and 2 → on restart the team is either fully active or rolled back with `state = failed:<step>`, never a mixed registry.
4. Boot all three → windows, hub joins, cards show the **actual** model from the session; each seat's prompt carries exactly two `# Skill:` bodies; first-request tokens ≤ §4.8 thresholds.
5. `send_message(to:"t1-backend")` from GLaDOS and reply → delivered + acked; to `t1-nope` → `E_UNKNOWN_RECIPIENT`; to a stopped seat → stored and replayed after its next boot; `update_task(claim)` by `t1-backend` succeeds and `bd show` reports the assignee. With a second same-project team `t2` booted: `t1` lead ↔ `t2` lead delivered; `t1-backend` → `t2-frontend` denied `nonlead`; a third team on another project: lead → lead denied `cross_project`; demote `t2`'s lead while a message to it is stored → `withheld:nonlead` (§4.10).
6. Scoped task to `t1-backend`; inject `busy`; **Replace worker…** → `gpt-5.6-sol` (on the list) → checkpoint requested; advance fake clock 90 s; owned tree stopped (pids listed, start-time matched); socket closed 4001 within 1 s; remote effects `0 in-flight`; `g=2`, verified model, checkpoint on the bead; worktree byte-identical.
7. Repeat with injected `rate-limited` (proceeds with `stale`), `dead` (descendants still stopped), `unowned process with worktree cwd` → **blocked**, no new owner/token/thread/window, reason shown; `unreadable process table` → blocked; choose a model **off** the list → dialog waits for operator acknowledgement.
8. Lead completes the reconciliation record (§4.9: every bead completed-with-evidence, authorized-cancelled, or transferred-and-accepted), records review + success-metric evidence, **Archive** → checklist green → canonical archived state; `just setup` leaves it; rollback script restores byte-identical tree; creating `t1` again is refused.

**Visual contract (P2):** reference screenshots are **frozen before P2 starts** at `docs/superpowers/specs/assets/v4-ui-references/<screen>-<state>-<viewport>.png` (produced by Vance from the approved mockups, approved by the operator via GLaDOS, committed with their sha256 in `manifest.json` beside them — the same files P2 must match, so the comparison is not against P2's own output); capture conditions pinned in that manifest: Tauri macOS WKWebView, device pixel ratio 2, light theme, system font stack (`-apple-system` → SF Pro), viewports 1280×800 and 1024×768 logical px, animations disabled; screens: library, wizard, grouped view, Replace dialog (each state), Archive checklist. Assertion = invariants first (no clipping/overflow, all CTAs visible, text not truncated) plus a ≤ 1 % pixel-delta budget against the frozen reference under those exact conditions; keyboard: tab order matches visual order, dialogs trap focus and return it on close, Escape = Cancel; labels associated; WCAG AA contrast; targets ≥ 44×44 px; **every CTA clicked once** in the journey.

**Security acceptance:** name fixtures from §4.1 incl. `../x`, `a/b`, 31/32 chars, `<img onerror>`; symlink pointing outside `~/.aperture`/`~/.claude/aperture` refused; seat dir modes 0700, files 0600, owner = user; partial staging dir never visible in the registry; forged hello (token of seat A, `AGENT_NAME` B) closes 4002; revoked reconnect 4003 and after hub restart; generated `prompt.md` and `team.json` contain neither sentinel (scan scope: the team dir, the seat dirs, `/tmp/aperture-*`); source checkout oracle: `git -C ~/projects/aperture status --porcelain` and `git rev-parse HEAD^{tree}` unchanged before/after team-create and archive.

**Measurement acceptance:** §4.8 thresholds; fresh-thread proof by id; no `/context` inside windows.

---

## 8. Implementation gates (unresolved product choices) with recommendations

| # | Gate | Recommendation | Blocks |
|---|---|---|---|
| 1 | Operator-edited presets location | `~/.aperture/teams/presets/`; shipped defaults in repo | P1 |
| 2 | Personas per role | keep (already preferred) | P1 templates |
| 3 | Ownership storage | §4.3 lock + generation record; validate on APFS in P1 before relying on it | P1/P3 |
| 4 | Swap authority | **APPROVED by operator 2026-09-06:** lead/GLaDOS may replace within the operator-approved per-team fallback list; anything outside the list requires operator approval | P3 |
| 5 | Specialists' future | standing seats stay **temporarily**; retire/re-home in M3 at the operator's timing | M3 |
| 6 | Foundation demo before UI | only if the operator wants it Thursday | scheduling |
| 7 | Name ceiling | **APPROVED by operator 2026-09-06:** 31-character internal seat-ID bound — `^[a-z0-9][a-z0-9_-]{0,30}$`, per-part limits 16/10/3 | P0 |

Gates 4 and 7 are decided (operator approval 2026-09-06, approval status only — no architecture change); the others may be accepted as recommended or left non-gating at final spec approval.

## 9. Out of scope (V4.0)
Multi-machine ownership; tool-level write fencing or credential revocation; embeddings/transcript memory; agent-initiated team creation; cross-team memory policy; automatic destructive cleanup; renaming the coordination trio.

## 10. Files expected to change (Thursday plan)
`mcp-server/src/{index.ts,presence-snapshot.ts,presence-hint.ts,ws-hub.ts,codex-bridge.ts}`, `src-tauri/src/{agent_loader.rs,agents.rs,hub_auth.rs,lib.rs,launcher.rs,watchdog.rs}` + `teams.rs`, `owner.rs`, `src/components/{Teams*.ts,AgentCard.ts,roster.ts}`, `justfile`, `roles/`, `teams/presets/`, `scripts/`, `.claude/skills/team/SKILL.md`, `prompts/glados.md`, tests under `mcp-server/test/` and Rust `#[cfg(test)]`.

---

## 11. Resolution table (v1 → v2)

| Source | Finding | Resolution |
|---|---|---|
| GLaDOS | rename ≠ CAS/durability; single writer across GUI/CLI/restart | §4.3: `flock` + expected-generation check + fsync/rename; restart marks stale; labelled recommendation/gate 3 |
| GLaDOS | dead worker skipped descendant cleanup; snapshot ownership before kill; cwd/cmd match ≠ authority | §4.5 step 0 (snapshot first), step 2 applies to dead workers; unowned matches block, never killed |
| GLaDOS | local stop ≠ remote cancel/key revocation | §4.5 guarantee statement + step 4 reconciliation, `E_REMOTE_UNCERTAIN` |
| GLaDOS | revoke sockets immediately; token deletion ≠ proof | §4.5 step 3 durable record + close 4001 now + 4003 on reconnect |
| GLaDOS | Stop-hook checkpoint not guaranteed | §4.4 `none`/`stale` recovery; §4.5 step 1 never blocks on a missing checkpoint |
| GLaDOS | "reuse unchanged" overstated | §2 retitled "verified naming compatibility vs required changes" |
| GLaDOS | budgets need measured totals | §4.8 thresholds from measured baselines (60,260 / 34,338) + first-task growth |
| GLaDOS | specialists retention is temporary | §1, §5 M1/M3 |
| GLaDOS | gates with recommendations; name 31; swap policy | §8 (7 gates), §4.1, §4.6 — gates 4/7 labelled recommended defaults pending approval |
| GLaDOS (v2.1) | thresholds need same role/model/effort + input+cache definition; 34,338 provisional; 1.5× is a proposal | §4.8 rewritten accordingly |
| Izzy 1 | ambiguous regex; `<` fixture unreachable | §4.1 literal regex, boundary fixtures, below-loader injection seam |
| Izzy 2 | recipients vs per-boot tokens; BEADS round-trip | §4.6 registry-existence semantics with stored/replayed messages; P0 real `bd` test |
| Izzy 3 | P1 failure semantics | §4.2 staging protocol, `E_*` codes, pending state, rollback residue |
| Izzy 4 | subjective visuals | §7 visual contract (viewports, delta budget, keyboard, AA, 44 px, every CTA) |
| Izzy 5 | nondeterministic P3 fixtures | §4.5 injected state source, fake clock, TERM/KILL timing, fixtures; §7 step 7 |
| Izzy 6 | "tool authority revoked" | renamed "message/hub authority revoked" everywhere; honest guarantee in §4.5 |
| Izzy 7 | checkpoint race/idempotency | §4.4 ids, seq, schema version, dedupe, which record wins |
| Izzy 8 | revocation oracle | §4.5 step 3 close codes 4001/4002/4003, restart persistence, token sequencing |
| Izzy 9 | archive contradiction | §4.7 one canonical state (`enabled:false` + `_archived/` move + `archive/`), rollback, evidence schema, marker, same-name |
| Izzy 10 | no total-token ceiling | §4.8 |
| Izzy recheck 1 | Codex cache double-counted | §4.8 provider-specific formula; Codex baseline 34,208 → ≤ 37,629 |
| Izzy recheck 2 | P1 multi-rename crash / pending move timing | §4.2 journaled multi-rename, `.complete` markers, crash points, moves only at pending → active |
| Operator 2026-09-06 (addendum) | lead accountability explicit | §4.9 delegation, consolidated reporting, pre-archive reconciliation with three dispositions, archive blocking rules, fixtures; §4.7 preconditions and §7 step 8 aligned |
| Operator 2026-09-06 (cross-team) | lead-to-lead only, server-side, fail closed, no authority grant, replay re-validation; framework-awareness prerequisite | §4.10 rule + fixtures (i)–(viii), §7 step 5 extended; §4.11 contradiction review, harness tests, budgets, spec never injected |
| Izzy t17l8g (PR #66) | invented task ACL; E_UNKNOWN vs E_CROSS_TEAM precedence; exactly-once vs replay-until-ack; Stop hook vs Codex checkpoint | §4.10 BEADS access stays Constitution-governed, fixture (viii) = no mutation by send_message; precedence pinned (unknown/disabled first, cross-team only for valid principals, fixture ii); replay-until-ack with crash-before-ack redelivery, fixture (v); §4.4/§4.11 Claude Stop hook best-effort + milestone writes vs Codex explicit tool/protocol |
| GLaDOS (PR #66 review) + operator | lead-of-leads model; no unevidenced task-scope claim; no blanket QA path; no exactly-once promise | §4.12 responsibility table + escalation path; §4.10 (viii) marked implementation requirement `E_TASK_SCOPE`; QA path = explicit scoped grant in registry (`no_grant` reason); replay = at-least-once with idempotent stable-id ack, fixture (v) asserts no duplicated effect; §4.11 lead-specific guidance |
| GLaDOS + Izzy (PR #65 checks) | lead must not create beads; unilateral cancellation; transfer must re-parent; unfinished-vs-closed assignee; report timing contradiction | §4.9.1 lead proposes / GLaDOS files after operator ack (fixture f: created_by + seat assignee); cancellation under existing approval rules; transfer approved+accepted+re-parented with provenance, open-child/success-metric gates re-evaluated; archive blocks only UNFINISHED seat-assigned beads; reports = same-turn coalescing batch, delta per later blocker (fixture e) |
| Izzy final 1 | step fsync after rename under-reports; markers precede state flip | §4.2/§4.7 recovery reconciles physical from/to state (step = hint); visibility = marker AND team active AND no journal; fixtures at rename-before-fsync and markers-before-flip; zero seats listed until durable activation |
| Izzy recheck 3 | archive crash consistency | §4.7 same journal + pre-archive manifest oracle |
| Izzy recheck 4 | visual 1 % tautological | §7 frozen pre-P2 references with path, conditions, approver |
| Izzy 11 | negative FS/security fixtures | §7 security acceptance (paths, symlinks, modes, partial staging, forged hello, sentinels, source oracle) |
| Izzy 12 | decisions #4/#7 | §8 gates 4 and 7 with concrete recommended defaults (pending final approval) |
