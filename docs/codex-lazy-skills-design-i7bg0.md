# Design: Codex-Safe Lazy Skill Loading (Phase 2 of Context Efficiency)

**Bead:** aperture-i7bg0 (parent: aperture-lquj5) · **Author:** Wheatley · **Date:** 2026-08-05
**Status:** PENDING GLADOS APPROVAL — no Rust lands before review sign-off.

## Title

Trim rex/izzy/cipher's force-injected skill context (~47–50k words each) by riding Codex CLI's **native** progressive-disclosure skills system instead of synthesizing a lazy loader.

## Core finding (changes the whole approach)

Codex CLI (v0.146.0, installed) natively implements exactly what this bead asked us to design: at session start it injects only a `## Skills` catalog (name + description + SKILL.md path, capped at 2% of context / 8k chars), and its embedded instructions require the model to *"read its SKILL.md completely before taking task actions"* when a task matches a skill description or the user invokes `$skill-name`. Discovery roots include `$CODEX_HOME/skills` — **the exact directory `populate_codex_skill_home` already populates on every launch**.

So today's Codex agents get every skill **twice**: full bodies concatenated into `prompt.md` by `inject_skills` (agents.rs:613) AND the native lazy catalog from `$CODEX_HOME/skills`. Phase 2 is therefore primarily *removing the redundant full-body injection*, not building new machinery.

Grep-receipt (recon, 2026-08-05): no catalog/lazy/summary primitive exists in the Aperture codebase (`catalog`/`lazy`/`on-demand`/`description` — zero code hits; `inject_skills` is the single choke point). The primitive we would have built exists natively in Codex instead. Both scouts' full reports are in the bead notes trail.

## Design

### 1. Two-set model per Codex agent: resident vs lazy

- **`skills.txt` stays the FULL set** (unchanged, all ~22–23 skills). It keeps driving both the runtime symlink tree and `$CODEX_HOME/skills` population — every skill remains on disk and in the native catalog. **No functional regression by construction.**
- **New file `agents/<name>/resident.txt`**: the small subset whose full bodies are still force-injected into `prompt.md` (behavioral norms that must be always-active, not fetched on demand: comms protocol, beads discipline, delegation discipline — mirroring Phase 1's resident-core philosophy). Proposed resident cores (subject to GLaDOS tuning):
  - rex/izzy/cipher common: `communicate`, `codex-comms`, `beads`, `team`, `worktree-discipline`, `specialist-delegation` (~14.4k words)
  - plus 1–2 role-central skills each (e.g. izzy: `verify-user-path`) if desired.
- **Why residency still matters on Codex:** its skills protocol is per-turn (*"Do not carry skills across turns unless re-mentioned"*). Continuous behavioral norms (how to message, how to close beads) can't rely on per-turn description matching — they must live in the standing prompt.

### 2. Rust change (small, one choke point)

In the Codex branch of `boot_agent_process` (agents.rs), filter the skill list passed to `inject_skills` to the `resident.txt` entries (fallback: if `resident.txt` absent, inject all — today's behavior, so rollout is opt-in per agent). `populate_codex_skill_home` is untouched. Claude-agent path untouched (Phase 1 already handled them; their lazy pool is Claude Code's own `.claude/skills` discovery).

Loader side: `load_agent_skills` gains a variant (or a filter param) reading `~/.claude/aperture/<agent>/resident.txt` (justfile copies/symlinks it into the runtime tree alongside `skills/`). No parsing changes to skills.txt.

### 3. Frontmatter audit (prerequisite, data-only)

Codex **drops** skills with invalid frontmatter and uses `description` as the sole implicit trigger, with a 1–1024 char constraint and budget-driven shortening. Before rollout:
- Audit every skill in rex/izzy/cipher's union (~27 skills): `name:` present and matching dir name (lowercase-hyphens), `description:` present and ≤1024 chars.
- Known offenders: several skills have very long trigger-list descriptions (e.g. specialist-delegation) and some have none visible. Trim/add descriptions — beneficial for Claude-side discovery too (shared files).
- Keep descriptions in "Use when… Triggers on…" form but tight, to stay under the 2% / 8k-char catalog budget for a ~23-skill set.

### 4. Verification plan (behavioral, per acceptance criteria)

Run against a **scratch/non-critical Codex agent first** (never rex/izzy/cipher mid-work):

1. **RISK #1 — must verify first:** does Aperture's `model_instructions_file = prompt.md` (config.toml) **suppress** Codex's built-in skills protocol / `## Skills` catalog injection? Recon extracted the protocol from the default instruction template; a custom instructions file might replace it. Check: boot scratch agent, run `/skills`, confirm catalog lists the lazy set. If suppressed → fallback design (§6).
2. **Catalog visibility:** `/skills` + `codex doctor`; confirm no "skipped N skill(s) due to invalid SKILL.md" and no 2%-budget truncation warning.
3. **Implicit invocation:** give the scratch agent a task squarely matching a lazy skill's description (e.g. a Playwright test question → `playwright-gotchas`); observe in the transcript that it reads the SKILL.md **before** acting. This is the acceptance-critical observation — behavior, not catalog text.
4. **Explicit invocation:** `$playwright-gotchas` in a prompt → same observation.
5. **Measurement:** prompt.md word count before/after per agent; expected ≈50k → ≈15–16k words (~68% cut), catalog overhead ≤8k chars.
6. **Regression pin:** community-reported skill-discovery regressions exist across codex releases — add a `codex doctor`/`/skills` check to the rollout checklist and re-verify after any codex upgrade.

### 5. Rollout order

1. Frontmatter audit + fixes (data-only, safe, benefits everyone).
2. Rust change + Tauri rebuild; scratch agent verification (§4).
3. Enable per agent by adding `resident.txt` — one agent at a time, while idle: cipher → izzy → rex.
4. Measure + report word counts; close bead per close-on-implementation-shipped.

### 6. Fallback (only if RISK #1 confirms suppression)

If the native catalog doesn't materialize under `model_instructions_file`, synthesize it: the same Rust filter injects, for non-resident skills, a compact catalog block into prompt.md — `name — description — read $CODEX_HOME/skills/<name>/SKILL.md before use` — mimicking Codex's own protocol wording. Paths MUST be `$CODEX_HOME`-based or absolute (**verified live: Codex app-servers run with cwd=/** — relative `.claude/...` paths are unreachable). Verification plan §4.3–4.5 unchanged.

## Acceptance criteria (restated from bead)

- Mechanism verified behaviorally: a Codex-backed test invocation pulls a non-resident skill when relevant (observed, not assumed).
- rex/izzy/cipher trimmed to resident cores via `resident.txt` (skills.txt intact = no availability regression).
- Word-count reduction measured and reported. Committed + pushed.

## Blast radius (named)

- `src-tauri/src/agents.rs` + `agent_loader.rs` (Codex injection filter), `justfile` (resident.txt into runtime tree), `agents/{rex,izzy,cipher}/resident.txt` (new), shared SKILL.md frontmatter edits (affects Claude agents' discovery too — descriptions only, bodies untouched), Tauri rebuild + agent restarts required.
- NOT touched: skills.txt contents, `populate_codex_skill_home`, Claude-agent injection path, MCP server.
