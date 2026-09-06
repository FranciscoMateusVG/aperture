# Aperture V4 — Project Teams: seats, incarnations, presets, safe replacement

**Bead:** aperture-nap3u. **Author:** Wheatley. **Status:** v1 for GLaDOS review + Izzy testability pass. **Implementation:** not before the operator resumes Thursday (token reset); nothing in this document authorizes code, installs or a demo before then.

Baseline this spec is written against: master `4fb8cad` (v3.2.0 released and canonical 2026-09-06: context diet, Claude busy/idle hooks, Constitution pilot on Peppy, PR #62 Rex rollout pending Izzy QA). Everything marked **verified** below was read in that tree by a read-only recon pass on 2026-09-06; everything marked **hypothesis** has not been exercised end-to-end.

---

## 1. Vision and vocabulary

Aperture keeps a **permanent coordination layer** — GLaDOS (orchestration), Wheatley (planning/specs), Peppy (infra/runtime) — and adds **temporary, mission-scoped project teams**. A team is created from an **editable preset**, runs until its acceptance is met and reviewed, and is archived. Inside a team the three ideas that today's roster fuses into one word are separated:

| Term | Meaning | Persistence |
|---|---|---|
| **Role** | A reusable persona + skill set (backend, frontend, QA, security, mobile, planner…). Lives in the repo as a template. | Permanent, versioned |
| **Seat** | A named position on one team, bound to one role: `<team>-<role>[-n]`. Has the BEADS assignee, the hub token, the registry folder, the tmux window. | Lives as long as the team |
| **Incarnation** (worker) | One concrete session filling a seat: harness (Claude/Codex) + model + reasoning + a thread id. Numbered `i = 1, 2, …` | Replaceable at any time |
| **Team** | Mission, seats, lead seat, acceptance, budget policy. Backed by an epic bead + `team:<name>` label. | Created → active → archived |
| **Preset** | A template team (mission placeholder, seats with role/model/reasoning, lead, fallback policy, acceptance skeleton). Editable in the UI. | Permanent; instantiation takes a **snapshot** |
| **Checkpoint** | A structured, reconcilable record of a worker's state written by the worker and validated by the lead. | Per seat, append-only history |
| **Lead** | The one seat the operator and GLaDOS contact for the team. Validates checkpoints, authorizes swaps within policy. | Per team |

Operator preferences already recorded (2026-09-06 PM, via GLaDOS): editable presets are the default flow; instantiation snapshots the preset so later team edits never mutate the template or sibling teams; start-blank stays available; personas are kept; the full-stack example (Rex + Vance + Izzy) is illustrative, not a mandatory roster.

---

## 2. What we can reuse today — verified reuse vs hypotheses

| Area | Finding (master `4fb8cad`) | Status |
|---|---|---|
| Agent definition is folder-driven | `just setup` loops `agents/*/` (justfile:55) and reads only `manifest.json`, `prompts/<name>.md`, `skills.txt`, `resident.txt`; `agent_loader.rs:71-171` scans `~/.claude/aperture/*` with no allowlist; `config.rs:3-5` states agents are no longer hardcoded | **verified** |
| Hub identity + auth accept any name | Tokens are provisioned **per boot, per folder name** (`agents.rs:197`, `hub_auth.rs:77`); `ws-hub.ts:234-275` checks only that `~/.aperture/run/hub-tokens/<agent>.token` exists with mode 0600 — no roster | **verified** |
| Name charset | `hub_auth.rs:15-25` and `ws-hub.ts:68`: `^[a-z0-9][a-z0-9_-]{0,63}$`; `presence-hint.ts:52` is tighter: `{1,32}`; Unix `sun_path` caps Codex socket names at ~62 chars | **verified** (the 32/64 mismatch is a latent bug) |
| tmux, watchdog, codex-bridge, BEADS assignee | Window = folder name, all targeting by `@id` (`tmux.rs:91-112`); watchdog roster = loaded agents (`watchdog.rs:615-630`); codex-bridge discovers by manifest model prefix (`codex-bridge.ts:96-121`); `assignee` is a free string everywhere in `mcp-server/src/beads.ts` | **verified** |
| **Hard blocker**: messaging allowlist | `PERMANENT_RECIPIENTS` at `mcp-server/src/index.ts:57` — a new seat can neither send nor receive via `send_message`; tool/param descriptions at `:72-73` enumerate the roster | **verified blocker** |
| Frontend | `roster.ts:8-17` only sorts known names first; `AgentCard.ts:10-25` falls back to a default theme; `AgentCard.ts:26-32` renders `agent.name` unescaped on the stale assumption that names come from a fixed list | **verified** (cosmetic + one escaping fix) |
| Documentation coupling | `team` skill and `prompts/glados.md` name the roster in prose; every prompt hard-codes its own inbox-monitor line | **verified** |
| Resident/lazy skills + Constitution on both harnesses | Peppy pilot PASS (aperture-leszs); Rex on Codex in flight (PR #62) | **verified on Claude**, Codex pending QA |
| `bd` accepts arbitrary assignee/actor strings | Not constrained anywhere in this repo; the `bd` binary itself was not exercised with a new name | **hypothesis** |
| A new-name seat round-trips a message through the hub end-to-end | Name *acceptance* is verified; delivery is blocked by the allowlist above and was not tested | **hypothesis** (T1 test) |
| Resume-newest bug | `codex-bridge.ts:359-360` binds the newest thread by heuristic; the bridge's own comment (`:256`) warns not to leave ownership to it | **verified risk** → every incarnation must be a fresh thread bound by id |

The whole document rests on one decision the recon makes cheap: **seat names reuse the existing agent machinery unchanged**; only the allowlist, the prose roster, and the UI need to learn about teams.

---

## 3. UX — what the operator actually sees

The launcher (Tauri, `src/`) gains a **Teams** area alongside today's agent list. Today's fixed roster remains visible as the "Coordination" group.

### 3.1 Presets library
A list of preset cards (`name`, one-line mission placeholder, seat chips like `backend · gpt-6-astra`, `frontend · opus`, `qa · sonnet`). Actions: **New team from preset**, **Edit preset**, **Duplicate**, **New blank preset**. Editing a preset never touches existing teams (snapshot rule).

### 3.2 New Team wizard (one screen, editable before Create)
```
┌ New team ─────────────────────────────────────────────────────────┐
│ Preset: fullstack ▾                       Team name: [fitt-relaunch]│
│ Mission: [Rebuild raul-fitt landing + booking …]                  │
│ Acceptance: [Izzy walks journey; Lighthouse ≥ 90; PR merged]      │
│                                                                    │
│ Seats                        role      harness  model      reasoning│
│ ● fitt-relaunch-backend  ★   backend   codex    gpt-6-astra  high  │
│ ● fitt-relaunch-frontend     frontend  claude   opus         —     │
│ ● fitt-relaunch-qa           qa        claude   sonnet       —     │
│ [+ add seat]   ★ = lead (radio)                                    │
│                                                                    │
│ Fallback policy: [ask operator ▾]  (ask · auto within preset list) │
│ Budget note:     [optional]                                        │
│                          [Cancel]  [Create team]                   │
└────────────────────────────────────────────────────────────────────┘
```
Create = snapshot to `~/.aperture/teams/<team>/team.json`, generate seat registry entries, file the epic bead (GLaDOS-only creation gate: the launcher asks GLaDOS via BEADS message; see §5.6), and show the team group. Seat names are derived and validated live against the charset rule; collisions with existing registry folders are refused.

### 3.3 Grouped sessions view
Each team is a collapsible group of seat cards. A seat card shows: role, **lead badge**, harness + **actual model** (read from the running session, not the card's configured value — see §5.4), turn state (busy/idle/offline from the hub), context % (from transcript usage, never from `/context`), incarnation number, **checkpoint age**, and actions: `Open`, `Checkpoint now`, `Replace worker…`, `Stop`. A **Team** header row shows mission, lead, epic bead, open PRs, and `Archive…`.

### 3.4 Replace worker dialog
```
┌ Replace worker: fitt-relaunch-backend (incarnation 2) ────────────┐
│ Current: codex · gpt-6-astra · high · busy (rate-limited 3 min)    │
│ New:     harness [codex ▾] model [gpt-5.6-sol ▾] reasoning [high ▾]│
│ Checkpoint: 00:04 ago · bead aperture-xxxx · worktree clean ✔      │
│ Stop verification:  ☐ checkpoint requested  ☐ process tree gone    │
│                     ☐ tool authority revoked ☐ worktree inventoried│
│ Policy: preset allows gpt-5.6-* fallback → no operator ack needed  │
│                              [Cancel]  [Stop, verify, then start]  │
└────────────────────────────────────────────────────────────────────┘
```
The button is disabled until the verification list is green; an unverifiable stop **blocks** the swap and shows why. Model changes outside the preset's fallback list require operator acknowledgement (dialog says so and waits).

### 3.5 Archive flow
Checklist gate: epic success metric observed ✔, all seat beads closed ✔, reviews recorded ✔, zero live processes for every seat ✔, worktrees clean or explicitly protected ✔. Archive disables the seats (`enabled:false`), moves `team.json` under `~/.aperture/teams/archive/`, and **never** deletes worktrees or branches (they remain for hygiene sweeps with today's rules).

---

## 4. Architecture

### 4.1 Where things live
| Thing | Location | Why |
|---|---|---|
| Role templates | repo `roles/<role>/{prompt.md.tmpl, skills.txt, resident.txt}` | versioned, reviewed like skills |
| Presets | repo `teams/presets/<name>.json` (shipped) + `~/.aperture/teams/presets/` (operator-edited) | editable without a PR |
| Team snapshot | `~/.aperture/teams/<team>/team.json` | runtime state, not source |
| Generated seat definitions | `~/.claude/aperture/<seat>/{manifest.json,prompt.md,skills/,resident.txt}` — written **directly by the launcher**, atomically (temp dir → rename) | avoids mutating the source checkout at runtime (GLaDOS constraint); `agent_loader` already reads this tree |
| Checkpoints | structured note on the seat's bead + mirror file `~/.aperture/teams/<team>/checkpoints/<seat>/<i>-<ts>.json` | BEADS is the record; the file makes recovery independent of Dolt availability |
| Leases | `~/.aperture/run/leases/<seat>.json` | see §4.3 |

`just setup` keeps rebuilding the fixed roster from `agents/*/`; it must **not** wipe team seat folders (today it wipes all non-`shared` dirs — justfile:27-34 — so it needs a "managed by teams" marker to skip).

### 4.2 Seat lifecycle
```
defined ──boot(i=1)──▶ live(i) ──checkpoint──▶ live(i)
   ▲                     │ stop requested
   │                     ▼
archived ◀── team archive ── stopped+verified(i) ──boot(i+1, explicit model)──▶ live(i+1)
                                   │ verification fails
                                   ▼
                              blocked (swap refused; operator/lead decides)
```
`live` sub-states come from the hub: `busy`, `idle`, `offline`, plus `rate-limited` (detected from the session's API error stream) and `dead` (window gone or process missing).

### 4.3 Durable ownership and concurrency (open requirement A)
A seat has at most one **writable** incarnation. Recommendation: a **lease file** per seat, `~/.aperture/run/leases/<seat>.json` `{incarnation, pid, start_time, thread_id, since}` created with `O_EXCL` and replaced only via write-to-temp + `rename` (atomic on APFS), guarded by the launcher, which is the single writer on this one-machine deployment. The hub caches lease state for the UI but is **not** authority. BEADS notes and JSON rows edited read-then-write are explicitly **not** a claim (GLaDOS). Restart semantics: on launcher start, every lease whose `pid`/`start_time` no longer match a live process is marked `stale` and surfaced, never silently reused. This is the smallest thing that is actually atomic here; a Dolt-backed lease is deferred until we need more than one machine.

### 4.4 Checkpoints
Written by the worker (a `checkpoint` MCP tool + the Stop hook, both writing the same schema) and validated by the lead (schema + reconciliation against `git`/`gh`):
```json
{"seat":"fitt-relaunch-backend","incarnation":2,"task":"aperture-xxxx","worktree":"…/aperture-xxxx-slug",
 "branch":"aperture-xxxx-slug","head_sha":"…","dirty_files":["src/a.ts"],"open_pr":"#71",
 "running_procs":[{"pid":123,"start":"…","cmdline":"pnpm test"}],
 "decisions":["kept Kysely","rate-limit reused adapter X"],"verified":["tests green at head_sha"],
 "next_step":"wire route B","written_at":"…","by":"worker|stop-hook|lead-forced"}
```
Recovery **never depends on the dead worker**: the replacement reads the last checkpoint, inventories the worktree as-is (`git status`, secret scan on dirty files), compares `head_sha`/`open_pr` with reality, and reports divergence before doing anything. It never auto-commits, pushes, or force-acks messages (open requirement C: replayed messages are idempotent acknowledgements, not repeated side effects — reconcile artifacts first).

### 4.5 Safe stop-before-replace (open requirement B — honest scope)
Aperture has **no tool-level write fencing**: a live worker's shell/git/file/deploy authority ends only when its processes end. The swap protocol therefore guarantees *stop before start*, and blocks when it cannot prove the stop:

1. **Request checkpoint.** `idle` → the Stop hook already wrote one; `busy` → send a checkpoint request and wait a bounded time (default 90 s); `rate-limited` → same, then proceed with the last valid checkpoint flagged `stale`; `dead` → skip to 3 using the last checkpoint.
2. **Stop.** Kill the seat's tmux window; then walk the **whole process tree** (process group + `ppid` chain) recorded for that incarnation and any child whose `cmdline` names the seat's worktree; verify each pid is gone **by pid + start-time match** (a recycled pid is not a match). Descendants not in the checkpoint's `running_procs` are still killed — the list is a hint, not the boundary.
3. **Revoke authority.** Delete the seat's hub token file and add the old `thread_id`/incarnation to a hub revocation list so a thawed predecessor's socket is closed on its next frame (hub change; today only the token file check exists). Confirm the seat's MCP server process is gone (it is a child of the CLI, so step 2 covers it — verify anyway).
4. **Only then boot i+1** with an explicitly selected harness/model/reasoning, bound to a **new thread id** (never resume-newest), and record the lease.
5. **Verify the actual model** from the new session (Codex: `thread/start` response; Claude: first assistant `message.model` in the transcript) and display that, not the card.

If any of 2–3 cannot be verified (e.g. an orphan with the worktree cwd that refuses to die, or the process list cannot be read) the dialog shows `blocked` with the offending pids and the swap does not start. This is the guarantee we can test; anything stronger is out of scope.

### 4.6 Permissions and messaging
- **Recipients:** `send_message` validates `to` against the **enabled seat registry** (the same folders `agent_loader` loads, filtered by `enabled:true`, plus `operator`) and requires that a hub token was provisioned for that name — fail closed on unknown or disabled seats. No wildcard, no impersonation widening: a seat still only ever sends as its own `AGENT_NAME`.
- **Who may do what:** create/edit presets — operator (UI); create team — operator via UI, epic bead filed by GLaDOS after ack (creation gate unchanged); replace worker within the preset's fallback list — lead or GLaDOS; outside the list or with a budget change — operator ack; archive — GLaDOS after the checklist; delete anything — nobody (archive only).
- **Lead as contact:** GLaDOS dispatches to the lead; the lead decomposes inside the team. Seats keep the Constitution: no queue/presence sweeps, await scoped dispatch, process-then-ack inbox.

### 4.7 Constitution and context budget per seat
Each generated seat gets `resident.txt` = `constitution` + the role's single core skill (as with Peppy → `deploy-workflow`, Rex → `walk-the-route`); everything else lazy. Boot budget target per seat: prompt ≤ 14 KB, resident ≤ 24 KB, Claude SessionStart per-command ≤ 9,800 B (existing gate), no `/context` in measured windows; measured via transcript usage. Team creation must run `skills-matrix`/`context-budget` for the generated seats and refuse to boot a seat over budget.

---

## 5. Migration from the fixed roster

| Phase | Change | Rollback |
|---|---|---|
| M0 | Fixed roster unchanged; `PERMANENT_RECIPIENTS` becomes registry-derived (fail closed) — roster still passes | revert one file + `just build-mcp` |
| M1 | Roles extracted from the five specialists' prompts/skills into `roles/`; the specialists' folders keep working as **standing seats** of the coordination layer (compat) | roles dir unused |
| M2 | Teams backend + UI ship; first team created from a preset while the roster keeps running | archive the team; registry entries removed by the archive routine |
| M3 | Specialist seats (izzy, vance, rex, scout, cipher) optionally re-homed as a standing "coordination QA/security" team or left as-is — **operator choice**, not required | none needed |

GLaDOS/Wheatley/Peppy stay permanent, fixed-name seats throughout. No history rewrite, no worktree deletion, no registry wipe in any phase.

---

## 6. Phased implementation and test dependencies (Thursday onward)

| Phase | Work | Owner (proposed) | Depends on | Tests that gate it |
|---|---|---|---|---|
| P0 | Registry-derived recipients (fail closed); align the three name regexes to `{0,30}` for now; escape `agent.name`; `just status` from `agents/*/`; `just setup` skips team-managed folders | Rex | — | unit: unknown/disabled seat rejected; new-name seat round-trips a message via the real hub; charset fuzz |
| P1 | Team model + files: presets, snapshot, seat generation from role templates (atomic), lease file, Tauri commands `team_create/list/archive`, GLaDOS bead-filing message | Wheatley (spec) + Rex (Rust/TS) | P0 | `team_create` on a fixture preset yields loadable seats; `just setup` leaves them intact; lease O_EXCL/rename semantics |
| P2 | UI: Presets library, New Team wizard, grouped view, seat cards with actual-model + checkpoint age | Vance | P1 | visual acceptance §7; keyboard/escape behaviour; no unescaped names |
| P3 | Checkpoint tool + Stop-hook writer + lead validation; Replace-worker protocol incl. process-tree stop, token revocation, explicit-model boot, actual-model verification | Peppy (runtime) + Rex (hub revocation) | P1 | busy / rate-limited / dead worker fixtures; recycled-pid negative test; blocked-swap path; new thread id ≠ old |
| P4 | Archive flow + checklist gate; migration M1 role extraction | Wheatley + Peppy | P2, P3 | archive refused with live pid / open bead; nothing deleted |
| QA | One consolidated pass per phase + the end-to-end journey (§7) | Izzy | each phase | transcript-usage receipts, no `/context` in windows |

Honest sizing: P0 ½ day; P1 1–1½ days; P2 1–1½ days; P3 1½–2 days; P4 ½–1 day; QA interleaved. Roughly one working week of specialist time, not one day. A **foundation demo** (P0 + P1 + manual P3 via the launcher) is the smallest thing that proves seat ≠ session ≠ model, and it must be labelled as such — it is not "V4 shipped" because it has no team UI.

---

## 7. Acceptance and testability (for Izzy)

**Seed data / fixtures:** `teams/presets/fullstack.json` (3 seats, lead = backend, fallback list `gpt-5.6-sol, gpt-5.6-terra`); a throwaway mission bead pre-approved by the operator for the test window; two disposable worktrees.

**Primary user journey (browser-less, launcher):**
1. Open Teams → Presets shows `fullstack` → **Expected:** card lists 3 seat chips and a lead marker.
2. Click *New team from preset*, name `t1`, edit mission, change QA seat model to `sonnet` → **Expected:** seat names render as `t1-backend`, `t1-frontend`, `t1-qa`; invalid name `T1.x` is refused inline.
3. Click *Create team* → **Expected:** `~/.aperture/teams/t1/team.json` exists; three registry folders exist; GLaDOS receives the bead-filing request; the group appears with three offline seats; the preset file is unchanged (hash equal).
4. Boot all three → **Expected:** three tmux windows; hub shows three joins; each seat card shows the **actual** model read from the session; each seat's prompt carries exactly two `# Skill:` bodies (constitution + role core).
5. From GLaDOS, `send_message(to:"t1-backend")` and from `t1-backend` reply → **Expected:** delivered and acked; `send_message(to:"t1-nope")` → `isError` "unknown seat"; a disabled seat is refused the same way.
6. Give `t1-backend` a scoped task; while it is **busy**, click *Replace worker…*, choose `gpt-5.6-sol` → **Expected:** checkpoint requested; after the bounded wait the dialog shows the checkpoint age; process tree verified gone (list of pids shown as ✔); token revoked; new window boots; card shows incarnation 2 and the **verified** new model; the checkpoint note is on the bead; the worktree is untouched (no auto-commit).
7. Repeat 6 against a **dead** worker (kill its pane first) and a **rate-limited** fixture → **Expected:** dead → proceeds from last checkpoint flagged `stale`; rate-limited → bounded wait then proceed; a fixture orphan holding the worktree cwd → **blocked**, swap refused, pid displayed.
8. Close the mission bead, record the review, click *Archive* → **Expected:** checklist all green; seats disabled; `team.json` moved to `archive/`; worktrees and branches still present; `just setup` afterwards does not resurrect the seats.

**Security acceptance:** no wildcard recipients; a seat cannot send as another name (hub rejects mismatched hello/agent); revoked incarnation's socket is closed within one frame; generated `prompt.md` contains no operator secrets; team creation writes only under `~/.aperture/teams` and `~/.claude/aperture/<seat>` (source checkout hash unchanged).

**Visual acceptance:** grouped view legible at 1280 px and 1024 px; lead badge, model chip and turn-state chip visible without hover; disabled *Stop, verify, then start* button explains why; no unescaped names (fixture folder with `<` in a manifest `name` renders as text).

**Measurement acceptance:** per-seat boot ≤ budget (§4.7) measured from transcript usage; no `/context` invocations inside the measured window.

---

## 8. Open operator decisions (with recommendations)

| # | Decision | Recommendation | Why |
|---|---|---|---|
| 1 | Where operator-edited presets live | `~/.aperture/teams/presets/` (runtime), shipped defaults in repo | edit without PR; repo keeps the vetted baseline |
| 2 | Seat personas | Keep personas per role (already preferred) | continuity with today's specialists |
| 3 | Lease storage | Launcher-owned `O_EXCL` + rename lease file (§4.3) | the only truly atomic primitive available today; Dolt lease deferred |
| 4 | Who authorizes swaps | Lead/GLaDOS within the preset's fallback list; operator beyond it or for budget changes | speed inside the sandbox the operator drew, ack outside |
| 5 | Specialists' future (M3) | Leave izzy/vance/rex/scout/cipher as standing seats for now | zero migration risk; teams prove themselves first |
| 6 | Foundation demo before UI? | Only if the operator wants it Thursday; otherwise go straight to P2 | demo proves the model, UI is what was asked for |
| 7 | Name length ceiling | `{0,30}` fleet-wide now (matches the tightest gate) | avoids the silent presence-hint no-op |

---

## 9. Out of scope (V4.0)
Multi-machine leases; MCP/tool-level write fencing; embeddings or transcript memory; autonomous team creation by agents (creation gate stays GLaDOS + operator ack); cross-team memory policy; automatic destructive cleanup of worktrees/branches; changing the coordination layer's fixed names.

## 10. Files this spec expects to touch (for the Thursday plan)
`mcp-server/src/{index.ts,presence-snapshot.ts,presence-hint.ts,ws-hub.ts}`, `src-tauri/src/{agent_loader.rs,agents.rs,hub_auth.rs,lib.rs,launcher.rs,watchdog.rs}` + new `teams.rs`, `src/components/{Teams*.ts,AgentCard.ts,roster.ts}`, `justfile`, `roles/`, `teams/presets/`, `.claude/skills/team/SKILL.md`, `prompts/glados.md`, tests under `mcp-server/test/` and `src-tauri/src/*/tests`.
