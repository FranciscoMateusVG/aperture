# Worktree Discipline — Precedents

Verbatim narratives moved out of `SKILL.md` on 2026-09-06 (aperture-psq0q skill diet). Each heading names the `SKILL.md` section the block came from. The rule each story established stays in `SKILL.md`; this file is the evidence, read on demand.

---

## §4 Which Agents — retired lanes (removed 2026-09-06)

The lane list previously carried three rows for agents retired 2026-07-19. Docs now go to the implementing agent, copy/SEO to Vance, review to Izzy.

- **Atlas** — README/docs in shared repos
- **Sage** — copy/content in shared repos
- **Sterling** — when reviewing requires checking out a branch locally

---

## §6.1 aperture-544mm — Peppy, 2026-08-10, squash-merge commit-message index

Rule kept in `SKILL.md` §6.1: diff content against the merged head, not commit messages against the PR description.

**Banked 2026-08-10 (Peppy, monorepo-incluir hygiene sweep, aperture-544mm).** During a worktree sweep, an unpushed local branch (`pr-742-final`, 2 commits, no remote) looked like real loss exposure: the bead it was named after had already shipped and deployed, but one of the two commits — `"fix(volunteers): align memory assignment windows"` — didn't appear anywhere in the merged PR's title or description. That absence read as "this never shipped, might be a real gap."

It wasn't. GitHub's squash-merge rewrites the merge commit's message from the PR's *first* commit (or the PR title) — it does not preserve every constituent commit's message verbatim in a way that's greppable from the outside. The second commit's content was fully present in the squashed merge commit; it just wasn't *named* there. A title-based search will never find it, no matter how carefully you grep.

This is the same root cause as the "clean + not merged by ancestry" caution elsewhere in this skill (a worktree whose branch was never actually merged via `git merge`/fast-forward looks "unmerged" to ancestry-based checks even when a squash-merge landed its content) — but this note is the actual resolution step: a content diff turns a scary-looking orphan into a confirmed-safe delete in about ninety seconds, instead of leaving you stuck at "flag it and move on."

---

## §8 Stacked PRs — original failure-mode narrative

Condensed in `SKILL.md` §8; original wording:

Sometimes your work genuinely depends on a parent PR that hasn't merged yet (e.g. Vance's frontend digest UI needs Rex's backend digest column to land first). When that happens, you might be tempted to open your PR with `--base <parent-branch>` instead of `--base main`. **Don't, unless you understand the auto-close failure mode.**

When the parent merges via the auto-merge workflow on `monorepo-incluir`, `gh pr merge --squash --delete-branch` deletes the parent's head branch. Three seconds later, GitHub auto-closes your dependent PR because its base no longer exists. Your code stays on the worktree's branch (nothing's lost from disk), but the PR ceremony evaporates and recovery requires opening a fresh PR — `gh pr edit --base main` and `gh pr reopen` both fail in that order.

**Before you stack at all, verify against the parent's actual code.** `aperture:stacked-pr-verification` is the pre-merge discipline: fetch the parent PR's head (`git fetch origin pull/<n>/head:ref`) and read the real handler bodies before rebasing, so the swap-over is a mechanical rebase with no contract surprises. The recovery procedures below are the operational gotchas that skill keeps from becoming panic-fixes.

### §8 Recovery procedure — PR #237→#245, PR #242→#244

**Recovery procedure (after auto-close):** see `aperture:incluir-deploy` Gotcha #9 for the full fresh-PR + cross-link-comment procedure. Banked precedents: PR #237→#245 (Vance, 2026-05-14), PR #242→#244 (Rex, 2026-05-14).

Worktree itself stays alive through the recovery — same branch, same files. You're only re-opening the GitHub PR surface, not the local work.

---

## §8.1 Vance PR #454 / 31b7ef3 — squash-merge-aftermath rebase trap (banked 2026-05-27)

Condensed in `SKILL.md` §8.1; original mechanism wording:

A second stacked-PR failure mode, distinct from auto-close. When you base a child branch on a parent PR's branch (e.g. `git checkout pr-453-rex-refactor && git checkout -b my-stacked-work`), your child branch carries the parent's pre-merge commits. After the parent squash-merges to main:

- Git's view of main now contains ONE squashed commit with the parent's full diff
- Your child branch's history still contains the parent's INDIVIDUAL pre-merge commits
- Plain `git rebase origin/main` will try to replay those pre-merge commits onto main → each one conflicts add/add with the squashed main version of the same files

**Symptom:** rebase conflicts in files that you personally never touched. The conflict markers appear in files the parent created (e.g. backend adapter files when you only edited frontend code).

Banked precedent:

**Banked precedent**: 2026-05-27, Vance on PR #454 (vwkg/k4qz-v2 stacked on Rex's #453). She'd based her branch on `pr-453-rex-refactor`; after #453 squash-merged, `git rebase origin/main` conflicted on Rex's backend adapter files Vance never touched. Recovered via `git rebase --onto origin/main 31b7ef3` where `31b7ef3` was the last commit from Rex's branch.

---

## §9 aperture-4f1vw — shared worktree collision (Rex/Vance, 2026-08-27)

Condensed in `SKILL.md` §9; original wording of the intro and the banked precedent:

Splitting one bead into parallel slices across agents (or subagents) that share a single worktree is a legitimate pattern — but it silently breaks if two slices touch the same files at the same time. There's no branch isolation inside a shared worktree; a live edit from one agent can get overwritten by another mid-read, with no error, no conflict marker, nothing. The only reason it surfaced was one agent's own trust-but-verify catching a claimed test-pass that had, in fact, never actually run green (the live-edit collision silently mutated the file between the claim and the check).

**Banked precedent (2026-08-27, aperture-4f1vw, monorepo-incluir):** Rex split a bead into parallel slices in a shared worktree, including a frontend hydration slice touching `ReportChatClient.tsx`/`reports-api.ts`. He separately handed the *same* file scope to Vance as a follow-up slice. Both ended up editing the same files concurrently in the same worktree — Vance's own type-error fix got silently overwritten, and a diff she was mid-reviewing changed size under her while she was reading it. She caught it only because she was independently re-verifying a claimed "6/6 tests PASS" and found the suite had never actually run green — the live collision was corrupting state between the claim and the check.
