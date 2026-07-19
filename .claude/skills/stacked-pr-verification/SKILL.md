---
name: stacked-pr-verification
description: Pre-merge contract-pinning discipline for stacked PRs — how to verify a dependent PR against its parent's ACTUAL code before the parent merges, not against the spec doc and not after the parent lands. Use when you are building a frontend/consumer PR that depends on a backend/producer PR still open in review, when running parallel tracks (specialist-delegation §9), or any time you are about to rebase a stacked branch onto a just-merged parent. Triggers on stacked PR, parallel-track prep, "wait for X then rebase Y", pre-rebase verification, contract drift between paired PRs, silent-404-before-it-ships.
---

# Stacked-PR Verification

The companion `aperture:audit-route-contract` skill catches contract drift **after** it ships — the silent 404 a real user finds in production. This skill catches the same drift class **before** the dependent PR ever merges. It is the verification step that makes parallel tracks (`aperture:specialist-delegation` §9) safe to run.

The core move is one git command most people don't know they can run: **read the parent PR's actual handler code while the parent is still open in review**, by fetching its PR head into a local ref. That single capability is what lets you verify against reality (the code) instead of against a model of reality (the spec doc) — `cipher-verify-reality` applied to the seam between two in-flight PRs.

---

## 1. The War Story

On 2026-05-24 the reports epic was running parallel tracks. Rex's B1 backend (`y18h`, PR #377) added the `/confirm` and `/abandon` reports endpoints plus an SSE preview emit. Vance's frontend (`kj0v`) — `PreviewPanel` + helpers — was being built **at the same time**, against Wheatley's scoping artifact (`aperture-70ql`), so it would be ready to rebase the moment #377 merged.

The naive shape is: build the frontend against the spec doc, wait for #377 to merge, rebase onto main, open the PR. That shape ships a latent bug. The spec said `/abandon` transitions a `draft` report to abandoned. The **actual handler** Rex wrote accepted *both* `draft` AND `draft_pending_confirm` as editable states that `/abandon` would act on — a runtime branch decision that no zod schema encodes and no spec line happened to enumerate.

Vance caught it pre-rebase by fetching #377's head into a local ref and reading the real handler body:

```bash
git fetch origin pull/377/head:rex-pr-377
git show rex-pr-377:apps/hono-app/src/http/routes/reports.ts
```

The frontend's optimistic-update logic was keying off `status === 'draft'` only; reading the real handler revealed it had to handle the `draft_pending_confirm` case too. Fixed before the rebase, shipped clean. Had it gone the naive route, the bug would have surfaced as a frontend state-desync **in production**, against a backend already merged — the worst place to debug it.

Rex independently reached the same discipline from the opposite direction — "how do you stop the silent-404 bug class when the frontend is about to merge before its backend dependency is even reviewable on main?" Two specialists, two angles, one conclusion. That two-instance independent derivation is the bar for crystallizing a pattern into a skill (banked: `bd memories contract-pinning-4-layer-defense`).

---

## 2. The 4-Layer Defense

Contract drift between paired PRs is caught — or missed — at four layers. Each pins a different thing and catches a different bug class. They are cumulative, not alternatives.

| Layer | What it pins | Owner / mechanism | Catches divergence at |
|---|---|---|---|
| **1. Scoping** | The contract itself | Wheatley scoping artifact in BEADS | Spec-doc review |
| **2. Implementation** | Code built to the spec | Backend specialist writes against the artifact | Code review of the producer PR |
| **3. Shared zod schemas** (`@repo/domains`) | **Wire shapes** | TypeScript compiler | Compile time |
| **4. Pre-rebase source-read** | **Runtime semantics** | `git show` on the parent's open-PR branch | Before you rebase — i.e. before you ship |

**Layers 3 and 4 are non-overlapping.** This is the load-bearing insight, the part that justifies the skill existing at all:

- **Layer 3 (zod) catches structural drift** — a response field renamed, a request type changed, an enum value missing from the shared schema. If the shape is wrong, the consumer's typecheck goes red. This is a *compile-layer* gate.
- **Layer 4 (source-read) catches semantic drift** — the handler accepts a status enum value the spec didn't enumerate, the error payload has an unexpected shape, retry/idempotency semantics differ from the doc, a sentinel string triggers a special branch. **No zod schema can encode "the handler also accepts `draft_pending_confirm`."** That behavior lives only in the handler body. This is a *behavior-layer* gate.

A team that has layer 3 (shared schemas) often *believes* it has full coverage and skips layer 4. It does not. Shapes matching is necessary but not sufficient; the `/abandon` war story is a shape-clean, behavior-divergent example that only layer 4 catches.

### Success metric (Rex's framing)

> **Zero code change at swap-over, OR every change attributable to one of the four layers detecting drift.**

Negative-space framing — it captures both success modes (clean swap; or a caught-and-fixed divergence) and the failure mode (zero change at swap-over *but* runtime later shows a mismatch = a divergence the four layers missed, which becomes a fresh lab-notebook entry).

---

## 3. The Procedural Recipe

The whole technique hinges on step 2 — fetching the parent's PR head into a local ref. GitHub exposes every open PR's head commit at the magic ref `pull/<num>/head`. You do **not** need the parent merged, and you do **not** need the author to push to a shared branch. If the PR is open, you can read its code.

```bash
# ── PARENT'S CI IS RUNNING; YOUR DEPENDENT BRANCH IS BUILT AGAINST THE SPEC ──

# 1. Confirm the parent is real and see its check state.
gh pr view <parent-pr-num> --json statusCheckRollup,state

# 2. THE UNLOCK — pull the parent PR's head into a local ref.
#    Without this you'd have to wait for merge to read the parent's code,
#    which is too late: a divergence found post-merge is a prod-facing scramble.
git fetch origin pull/<parent-pr-num>/head:<local-name>

# 3. Read the ACTUAL handler bodies — the code, not the spec doc.
#    This is layer 4. Read every seam your consumer touches:
#    route handlers, schema/tool definitions, the emit sites of any
#    events/SSE/webhooks your consumer subscribes to.
git show <local-name>:<path-to-route-handler>
git show <local-name>:<path-to-schema-or-tool-defs>

#    Look specifically for what the spec CANNOT carry:
#      - status/enum values the handler branches on but the spec didn't list
#      - error-payload shapes (the catch path, not just the happy path)
#      - sentinel strings ('anonymous', 'all', '*') with special meaning
#      - idempotency / retry / partial-success semantics
#      - default-when-field-absent behavior

# 4. Optional paranoia — see if a rebase would conflict, without doing it.
git merge-tree $(git merge-base HEAD <local-name>) HEAD <local-name> | head -40
#    (or: git rebase --dry-run is not a thing; use merge-tree for a preview)

# ── PARENT MERGES (CI green + auto-merge fires) ──

# 5. Confirm the merge actually happened before you rebase.
gh pr view <parent-pr-num> --json state,mergedAt

# 6. Rebase onto FRESH main — never onto the now-deleted parent branch.
git fetch origin main
git rebase origin/main
#    If the parent SQUASH-merged and your branch carries the parent's
#    pre-merge commits, plain rebase conflicts on files you never touched.
#    Use the --onto form — see worktree-discipline §8.1:
#       git rebase --onto origin/main <last-parent-commit-sha>

# 7. Force-push (rebase rewrote history) + open your PR against main.
git push --force-with-lease origin <your-branch>
gh pr create --base main --title "..." --body "..."

# 8. Clean up the local parent ref.
git branch -D <local-name>
```

**Why `--base main`, never `--base <parent-branch>`:** stacking your PR's base on the parent's branch triggers the auto-close failure mode when the parent merges with `--delete-branch` (GitHub kills your PR 1–3s later, and `gh pr reopen` is deadlocked). See `aperture:worktree-discipline` §8 for the full failure mode and §8.1 for the squash-merge rebase trap. This skill is the discipline that keeps that gotcha from ever becoming a panic-fix — you verified pre-merge, so the post-merge step is a mechanical rebase with no surprises.

---

## 4. Anti-Patterns

| Don't | Why it bites |
|---|---|
| **"Read the spec doc, skip layer 4"** | The spec doc lags the code and rarely enumerates runtime branches. You're trusting a model of reality, not reality. The `/abandon` bug was spec-clean and behavior-divergent. |
| **"Wait for the parent to merge, then read main"** | Too late. A divergence found post-merge means patching your consumer against *production* — the worst debug surface. The whole point of `pull/<num>/head` is to read pre-merge. |
| **"Skip layer 3 (no shared schemas)"** | Wire-shape drift moves from compile time to runtime. Works until a renamed field 500s in prod. Shared zod in `@repo/domains` is the cheap structural gate; don't trade it away. |
| **"Skip layer 4 (zod's enough)"** | Semantic drift ships silently. zod proves the *shape* matches; it cannot prove the *behavior* matches. The handler accepting an unlisted status is invisible to every schema. |
| **Stack the PR with `--base <parent-branch>`** | Auto-close deadlock when the parent merges with `--delete-branch`. Always `--base main` + rebase. (worktree-discipline §8) |
| **Read only the one handler you "know" you call** | Adjacent seams drift too — the SSE emit site, the error path, the sibling endpoint. Read every seam your consumer touches, not just the obvious one. (verify-reality at seam-scope, mirroring the file-scope discipline in `audit-route-contract`) |

---

## 5. When This Applies

- You're building a **consumer PR** (frontend, SDK client, downstream service) that depends on a **producer PR** (backend route, schema, tool def) still open in review.
- You're running **parallel tracks** per `aperture:specialist-delegation` §9 — prepping the dependent work while the dependency is in flight, to collapse the wait-state.
- You're about to **rebase a stacked branch** onto a just-merged parent and want zero surprises at swap-over.

It does **not** apply when the dependency is already merged to main and deployed — at that point read main directly, and if you find drift, you're in `audit-route-contract` (post-merge detection) territory, not this skill.

**Prerequisite:** per-task worktrees (`aperture:worktree-discipline`). The local parent ref (`<local-name>`) lives in your worktree's git dir; keep it isolated from other tasks' branch state.

---

## 6. Cross-Links

- **`aperture:audit-route-contract`** — the POST-merge detection skill for the same contract-drift bug class. This skill is its PRE-merge prevention companion: catch the drift before it ships rather than after a user finds the 404.
- **`aperture:worktree-discipline`** §8 / §8.1 — the stacked-PR auto-close failure mode and the squash-merge `--onto` rebase trap. The operational gotchas this skill keeps from becoming panic-fixes.
- **`aperture:specialist-delegation`** §9 — parallel tracks. This skill is the verification step that makes "build Y while X is in flight" safe.
- **`aperture:beads`** — bead-status conventions for parallel-track prep work (claim the dependent bead, note the dependency, close on PR-open).
- **`cipher-verify-reality`** (bd memory) — the parent principle: verify against the artifact, not the description of the artifact. Layer 4 is that principle applied to the seam between two in-flight PRs.
- **`contract-pinning-4-layer-defense`** (bd memory) — the banked two-instance derivation this skill crystallizes.

---

## 7. The One-Sentence Version

**If your PR depends on an open PR, fetch the parent's head (`git fetch origin pull/<n>/head:ref`) and read the actual handler bodies before you rebase — because zod proves the shapes match but only the source proves the behavior matches.**
