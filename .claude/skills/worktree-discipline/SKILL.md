---
name: worktree-discipline
description: Git worktree convention for any agent editing a shared repo. Use when claiming a task that involves code changes — monorepo-incluir, aperture itself, beads-galaxy, or any other repo where multiple agents may work concurrently. Triggers on task claims that involve editing a shared codebase.
---

# Worktree Discipline

When any agent edits a shared repo, they use **per-task git worktrees** to avoid stepping on each other's branches and uncommitted state. Vance has been doing this from day one and it's our gold-standard pattern; the rule is now general.

---

## 1. The Convention

For any task that requires editing the shared monorepo:

```
~/projects/monorepo-incluir-worktrees/<task-id>-<slug>
```

Examples:
- `~/projects/monorepo-incluir-worktrees/aperture-fict-mariana-forum-fix`
- `~/projects/monorepo-incluir-worktrees/incluir-bl9p-secretaria-filter`
- `~/projects/monorepo-incluir-worktrees/aperture-3kx2-auth-rewrite`

**Slug rules:** lowercase, kebab-case, 2–5 words describing the task. Long enough to identify, short enough to type.

**Branch name** matches the directory: `<task-id>-<slug>`.

---

## 2. Setting Up a Worktree

When you claim a task that needs code changes:

```bash
cd ~/projects/monorepo-incluir
git fetch
git worktree add -b <task-id>-<slug> ../monorepo-incluir-worktrees/<task-id>-<slug> origin/main
cd ../monorepo-incluir-worktrees/<task-id>-<slug>
```

Now you have an isolated working tree on a fresh branch from `origin/main`. Edit, commit, and push from this directory. Never edit the main checkout while another agent might be using it.

---

## 3. Cleanup On Close

When the task is closed (merged, abandoned, or otherwise complete):

```bash
cd ~/projects/monorepo-incluir
git worktree remove ../monorepo-incluir-worktrees/<task-id>-<slug>
git branch -D <task-id>-<slug>  # only if branch was merged or abandoned
```

If the branch was merged via PR, the remote tracking branch is gone after the PR closes — `git fetch --prune` cleans that up.

**This step is mandatory.** Stale worktrees accumulate disk and pollute `git worktree list`. If you close a BEADS task, your worktree should be gone within the same session.

---

## 4. Which Agents

**Every agent that edits a shared repo follows this convention.** No exceptions, no "I'll just do this small one in main." That's how state leaks happen.

Typical edit-bearing agents and what they touch:

- **GLaDOS** — direct edits when not delegating
- **Wheatley** — small scoped implementations delegated by GLaDOS
- **Peppy** — Dockerfiles, compose, CI configs
- **Rex** — backend code, migrations
- **Izzy** — test files, bug repros
- **Cipher** — security patches
- **Vance** — frontend / CSS (gold standard, already doing this)
- **Scout** — mobile code
- **Atlas** — README/docs in shared repos
- **Sage** — copy/content in shared repos
- **Sterling** — when reviewing requires checking out a branch locally

If your turn involves `git checkout` or any file edit in a shared repo, you make a worktree first.

---

## 5. Anti-Patterns

| Don't | Why |
|---|---|
| Edit `~/projects/monorepo-incluir/` directly | Conflicts with whatever another agent is doing on `main` |
| Reuse a worktree across tasks | Branch state leaks between unrelated work |
| Skip the slug, use only the task ID | `aperture-2yho` tells nobody anything; `aperture-2yho-rate-limiter` is searchable |
| Leave dead worktrees lying around | `git worktree list` becomes noise; disk fills up |
| Push directly to `main` from a worktree | Worktrees are for branch work. PRs go through review. |

---

## 6. Hygiene Audit

GLaDOS spot-checks worktree hygiene on a rolling basis:

- `ls ~/projects/<repo>-worktrees/` should match open BEADS tasks claimed by editing agents
- Closed tasks with surviving worktrees → flag the owning agent
- Worktrees with no corresponding BEADS task → flag for cleanup

Light-touch enforcement, not a witch hunt. The goal is to keep shared repos clean and predictable.

### 6.1 On a squash-merging repo, commit-message matching is a broken index — diff content, not titles

**Banked 2026-08-10 (Peppy, monorepo-incluir hygiene sweep, aperture-544mm).** During a worktree sweep, an unpushed local branch (`pr-742-final`, 2 commits, no remote) looked like real loss exposure: the bead it was named after had already shipped and deployed, but one of the two commits — `"fix(volunteers): align memory assignment windows"` — didn't appear anywhere in the merged PR's title or description. That absence read as "this never shipped, might be a real gap."

It wasn't. GitHub's squash-merge rewrites the merge commit's message from the PR's *first* commit (or the PR title) — it does not preserve every constituent commit's message verbatim in a way that's greppable from the outside. The second commit's content was fully present in the squashed merge commit; it just wasn't *named* there. A title-based search will never find it, no matter how carefully you grep.

**The only reliable test: diff the branch's content against the merged head, not its commit messages against the PR description.**

```bash
git diff <suspect-commit-or-branch-tip> <merged-head-sha>
```

Zero lines of output = the content is already on `main`, byte-for-byte — safe to discard regardless of how orphaned the commit message looked. Non-empty output = real, distinct content that didn't ship — needs a push or an explicit discard decision, not a silent delete.

This is the same root cause as the "clean + not merged by ancestry" caution elsewhere in this skill (a worktree whose branch was never actually merged via `git merge`/fast-forward looks "unmerged" to ancestry-based checks even when a squash-merge landed its content) — but this note is the actual resolution step: a content diff turns a scary-looking orphan into a confirmed-safe delete in about ninety seconds, instead of leaving you stuck at "flag it and move on."

---

## 7. Other Repos

This skill is written for `monorepo-incluir`. The same pattern applies if you're working concurrently in any shared repo — adapt the path:

```
~/projects/<repo>-worktrees/<task-id>-<slug>
```

Aperture itself, beads-galaxy, mempalace — same rule, same hygiene.

---

## 8. Stacked PRs — when your worktree's branch depends on another open PR

Sometimes your work genuinely depends on a parent PR that hasn't merged yet (e.g. Vance's frontend digest UI needs Rex's backend digest column to land first). When that happens, you might be tempted to open your PR with `--base <parent-branch>` instead of `--base main`. **Don't, unless you understand the auto-close failure mode.**

When the parent merges via the auto-merge workflow on `monorepo-incluir`, `gh pr merge --squash --delete-branch` deletes the parent's head branch. Three seconds later, GitHub auto-closes your dependent PR because its base no longer exists. Your code stays on the worktree's branch (nothing's lost from disk), but the PR ceremony evaporates and recovery requires opening a fresh PR — `gh pr edit --base main` and `gh pr reopen` both fail in that order.

**Prevention:** prefer `--base main` for your PR. If your work doesn't typecheck or test against current main, reconsider whether you should be opening the PR yet — wait until the parent merges, then rebase onto fresh main and open.

**Before you stack at all, verify against the parent's actual code.** `aperture:stacked-pr-verification` is the pre-merge discipline: fetch the parent PR's head (`git fetch origin pull/<n>/head:ref`) and read the real handler bodies before rebasing, so the swap-over is a mechanical rebase with no contract surprises. The recovery procedures below are the operational gotchas that skill keeps from becoming panic-fixes.

**If you genuinely must stack:** retarget to main as soon as the parent's CI is green and merge is imminent, BEFORE the parent merges:

```bash
git fetch origin && git rebase origin/main
git push --force-with-lease origin <your-branch>
gh pr edit <your-pr-num> --base main
```

**Recovery procedure (after auto-close):** see `aperture:incluir-deploy` Gotcha #9 for the full fresh-PR + cross-link-comment procedure. Banked precedents: PR #237→#245 (Vance, 2026-05-14), PR #242→#244 (Rex, 2026-05-14).

Worktree itself stays alive through the recovery — same branch, same files. You're only re-opening the GitHub PR surface, not the local work.

### 8.1 The squash-merge-aftermath rebase trap (banked 2026-05-27)

A second stacked-PR failure mode, distinct from auto-close. When you base a child branch on a parent PR's branch (e.g. `git checkout pr-453-rex-refactor && git checkout -b my-stacked-work`), your child branch carries the parent's pre-merge commits. After the parent squash-merges to main:

- Git's view of main now contains ONE squashed commit with the parent's full diff
- Your child branch's history still contains the parent's INDIVIDUAL pre-merge commits
- Plain `git rebase origin/main` will try to replay those pre-merge commits onto main → each one conflicts add/add with the squashed main version of the same files

**Symptom:** rebase conflicts in files that you personally never touched. The conflict markers appear in files the parent created (e.g. backend adapter files when you only edited frontend code).

**The fix:** use `git rebase --onto` with the cutoff at the last parent commit so git replays ONLY your commits:

```bash
# branch:  parent-commit-1 → parent-commit-2 → parent-commit-3 (LAST PARENT) → your-commit
# After parent merges, you want to replay ONLY your-commit onto origin/main.

git fetch origin
git rebase --onto origin/main <last-parent-commit-sha> <your-branch>

# Or equivalently if you're already on your branch:
git rebase --onto origin/main <last-parent-commit-sha>
```

Find `<last-parent-commit-sha>` via `git log --oneline` on your branch — it's the last commit that came from the parent PR's branch, before your own commits started.

**Banked precedent**: 2026-05-27, Vance on PR #454 (vwkg/k4qz-v2 stacked on Rex's #453). She'd based her branch on `pr-453-rex-refactor`; after #453 squash-merged, `git rebase origin/main` conflicted on Rex's backend adapter files Vance never touched. Recovered via `git rebase --onto origin/main 31b7ef3` where `31b7ef3` was the last commit from Rex's branch.

**Prevention pattern**: when you must stack on an open PR, record the last-parent-commit SHA at branch-creation time in your BEADS notes. Future-you (or another agent recovering the worktree) doesn't have to dig through git log to find it.

**Even better prevention**: don't base on the parent's branch at all. Base on `origin/main` and just include the parent's expected changes as local-only deltas that you'll drop at rebase time. More work upfront, no rebase trap.
