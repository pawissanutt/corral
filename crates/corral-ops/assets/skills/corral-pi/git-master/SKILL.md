---
name: git-master
description: Safely inspect Git state, create atomic commits, rebase task branches and search history with log, blame, pickaxe and bisect. Use for commits, rebases and history questions.
allowed-tools: read grep bash
---
{{marker}}

# Git master

Choose STATUS, COMMIT, REBASE or HISTORY mode from the user's request/run contract. Investigation
does not authorize commits, rewrites, pushes, resets or deletion. Use `bash` for Git argv commands,
with terminal input disabled and paging off; use `read` and `grep` for source/conflict inspection.
Never change repository boundaries or the task's branch identity.

## Ground truth

Inspect `git status --short`, `git branch --show-current`, `git diff`, `git diff --staged` and recent
`git log --oneline` before writing history. Determine the base and upstream if present; a missing
upstream is unknown publication status, not proof that commits are private. Preserve unrelated work
and never use a broad reset or clean to make the tree convenient.

## Commit

Follow local message style. Group changes by behavior and revertability; keep a fix and its direct
regression tests together. Stage explicit paths/hunks, inspect the staged diff for unrelated changes
and secrets, then commit only the authorized scope. Report commit hashes and remaining dirty files.
Do not bypass hooks or claim checks that did not run.

## Rebase

Rebase only the authorized task branch onto its named base, with a clean/accounted-for worktree.
Record the starting commit as a recovery point. For non-interactive fixups use a deliberate sequence
editor; do not let an editor hang waiting for terminal input. Resolve conflicts by preserving both
sides' intended invariants, never blanket ours/theirs. If intent is unclear, stop and ask; aborting a
rebase is safer than discarding work. Rerun relevant checks after the final rebase, and any gate the
run contract requires. Never rewrite protected/shared branches or force-push without explicit
permission; if permitted, use force-with-lease rather than force.

## History

Pick the tool for the question: `git log -S 'text' -- path` for a change in exact-string count,
`git log -G 'regex' -- path` for matching changed lines, `git blame -L start,end -- path` for line
ownership, `git log --follow -- path` for renames, and `git show hash -- path` for evidence.
Use bisect only with known good/bad bounds and a deterministic repro in a disposable clean checkout;
restore its state afterwards. Use reflog to explain/recover local history without destructive guesses.
Cite hash, subject, path and relevant diff/lines; distinguish evidence from inferred motivation.
