---
name: worktree-discipline
description: Git worktree convention for any agent editing a shared repo. Use when claiming a task that involves code changes — monorepo-incluir, aperture itself, beads-galaxy, or any other repo where multiple agents may work concurrently. Triggers on task claims that involve editing a shared codebase.
---

# Worktree Discipline

Any agent editing a shared repo uses **per-task git worktrees** so agents never step on each other's branches or uncommitted state. Incident write-ups live in `references/precedents.md`.

---

## 1. The Convention

```
~/projects/<repo>-worktrees/<task-id>-<slug>        # directory
<task-id>-<slug>                                    # branch (same name)
```

e.g. `~/projects/monorepo-incluir-worktrees/aperture-fict-mariana-forum-fix`, `.../incluir-bl9p-secretaria-filter`. **Slug:** lowercase kebab-case, 2–5 words — long enough to identify, short enough to type. Same rule for aperture, beads-galaxy, mempalace — adapt the `<repo>`.

## 2. Setup

```bash
cd ~/projects/monorepo-incluir
git fetch
git worktree add -b <task-id>-<slug> ../monorepo-incluir-worktrees/<task-id>-<slug> origin/main
cd ../monorepo-incluir-worktrees/<task-id>-<slug>
```

Edit, commit, and push from this directory. Never edit the main checkout while another agent might be using it.

## 3. Cleanup On Close — mandatory

```bash
cd ~/projects/monorepo-incluir
git worktree remove ../monorepo-incluir-worktrees/<task-id>-<slug>
git branch -D <task-id>-<slug>     # only if merged or abandoned
git fetch --prune                  # drops the remote tracking branch after the PR closes
```

Close a BEADS task → its worktree is gone in the same session. Stale worktrees eat disk and pollute `git worktree list`.

## 4. Which Agents

**Every agent that edits a shared repo.** No "I'll just do this small one in main" — that's how state leaks. GLaDOS (direct edits), Wheatley (scoped implementations), Peppy (Dockerfiles, compose, CI), Rex (backend, migrations), Izzy (tests, repros, review checkouts), Cipher (security patches), Vance (frontend/CSS, copy/content), Scout (mobile). Docs go with the implementing agent. If your turn involves `git checkout` or any file edit in a shared repo, make a worktree first.

## 5. Anti-Patterns

| Don't | Why |
|---|---|
| Edit `~/projects/monorepo-incluir/` directly | Conflicts with whatever another agent is doing on `main` |
| Reuse a worktree across tasks | Branch state leaks between unrelated work |
| Skip the slug, use only the task ID | `aperture-2yho` tells nobody anything; `aperture-2yho-rate-limiter` is searchable |
| Leave dead worktrees lying around | `git worktree list` becomes noise; disk fills up |
| Push directly to `main` from a worktree | Worktrees are for branch work. PRs go through review. |

## 6. Hygiene Audit

GLaDOS spot-checks on a rolling basis: `ls ~/projects/<repo>-worktrees/` should match open BEADS tasks claimed by editing agents; closed tasks with surviving worktrees → flag the owner; worktrees with no bead → flag for cleanup. Light-touch, not a witch hunt.

**6.1 On a squash-merging repo, diff content, not commit messages.** Squash-merge rewrites the merge commit's message from the PR's first commit or title; a constituent commit whose message appears nowhere in the PR can still be fully present in the squashed content. Title search and ancestry checks ("not merged") are both blind to it. The only reliable test:

```bash
git diff <suspect-commit-or-branch-tip> <merged-head-sha>
```

Zero lines → content is already on `main`, safe to discard however orphaned it looks. Non-empty → real unshipped content; push or explicitly decide to discard, never silently delete. (Precedent: §6.1 aperture-544mm.)

## 7. Stacked PRs — when your branch depends on another open PR

Don't open your PR with `--base <parent-branch>` unless you understand the auto-close mode: when the parent squash-merges with `--delete-branch` (the `monorepo-incluir` auto-merge workflow), GitHub auto-closes your dependent PR because its base is gone. Your code survives on the branch; the PR ceremony doesn't — `gh pr edit --base main` and `gh pr reopen` both fail; recovery is a fresh PR + cross-link comment per `aperture:incluir-deploy` Gotcha #9. (Precedent: §8 PR #237→#245, #242→#244.)

- **Prefer `--base main`.** If your work doesn't typecheck against current main, you may not be ready to open the PR — wait for the parent, rebase, then open.
- **Before stacking, verify against the parent's actual code** (`aperture:stacked-pr-verification`): `git fetch origin pull/<n>/head:ref`, read the real handler bodies, so the swap-over is a mechanical rebase with no contract surprises.
- **If you must stack**, retarget BEFORE the parent merges, as soon as its CI is green:
  ```bash
  git fetch origin && git rebase origin/main
  git push --force-with-lease origin <your-branch>
  gh pr edit <your-pr-num> --base main
  ```

**7.1 The squash-merge-aftermath rebase trap.** A child branch based on a parent PR's branch carries the parent's individual pre-merge commits; after the parent squash-merges, plain `git rebase origin/main` replays those commits and each conflicts add/add with the squashed version. **Symptom: conflicts in files you never touched.** Fix — replay ONLY your commits:

```bash
git fetch origin
git rebase --onto origin/main <last-parent-commit-sha> <your-branch>
```

`<last-parent-commit-sha>` = the last commit from the parent's branch before your own commits (`git log --oneline`). **Prevention:** record that SHA in your bead notes at branch-creation time; better, base on `origin/main` and carry the parent's expected changes as local-only deltas you drop at rebase time. (Precedent: §8.1 Vance PR #454 / 31b7ef3.)

## 8. Shared worktree, multiple agents, same bead — one pen per file set

Splitting a bead into parallel slices that share one worktree is legitimate, but there is no branch isolation inside it: two slices touching the same file at the same time silently overwrite each other — no error, no conflict marker, and a diff under review can change under you. (Precedent: §9 aperture-4f1vw.)

**File sets must not overlap in time:**
- **Announce the file set you're claiming in the bead notes before editing** — "Taking ReportChatClient.tsx + reports-api.ts", not an implied scope.
- **One pen per file set at a time.** Same file needed by two slices → sequential, each gating green before the next starts.
- **On a scope handoff** ("I built X, but also asked Y to build the same surface"), the orchestrator resolves it immediately: one agent cedes, the other proceeds.
- **A claimed test-pass on a file someone else was concurrently editing is not trustworthy** — re-verify after any suspected collision; the claim may have been true when made and false by the time it's read.

This is the shared-worktree corollary of trust-but-verify (`specialist-delegation` §5).
