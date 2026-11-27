---
name: git-master
description: Answer Git state or history questions, or prepare and perform explicitly authorized commits, rebases and pushes without disturbing unrelated work.
allowed-tools: read grep bash
---

# Git master

Start with the user's Git question and run only reads that answer it. For example, `git log --follow -- path` can trace a file's rename history without a worktree inventory. Stop once you have the relevant evidence; cite the commit hash, path and diff or lines, and distinguish evidence from inferred motivation. If a command isn't available, report the missing evidence rather than guessing.

Use `git log -S 'text' -- path` to find changes in an exact string's occurrence count, `git log -G 'regex' -- path` for matching changed lines, `git blame -L start,end -- path` to trace line changes, or `git show hash -- path` to inspect the evidence. Use reflog to investigate local history without making destructive changes. Bisect changes checkout state: do it only with authorization, known good and bad bounds, and a deterministic reproduction in a disposable clean checkout; restore that checkout afterward.

## Stage and commit

Get explicit authorization for Git writes. Before staging or committing, confirm the checkout and branch, inspect status and both staged and unstaged diffs, and check for operations already in progress. If one is active and ownership or intent is unclear, inspect and report; do not abort or continue it. Stage only authorized paths or hunks and inspect the staged diff for scope and secrets. If unrelated hunks are already staged, leave them intact and stop rather than committing them or unstaging them without agreement on how to proceed.

Keep a fix with its direct regression tests and use the local commit-message style. Don't bypass hooks or claim checks you didn't run. Report the resulting hash and remaining changes.

## Rewrite or publish

Before rewriting, confirm explicit authorization, the exact branch and base, a clean or fully accounted-for worktree, and any in-progress operations. Inspect linked worktrees before switching or moving refs so you don't disrupt another checkout. Check publication status; no upstream does not establish that commits are private. If ownership, publication or the intended base is uncertain, stop and ask. Don't rewrite protected or shared branches. Record the starting commit as a recovery point; avoid broad reset or clean commands.

For an authorized rebase, use a deliberate sequence editor if needed. Resolve conflicts by preserving both sides' intended behavior, not by blanket selection of one side. If intent is unclear, stop and report the conflict. Rerun relevant checks after the final rebase. Push only with explicit authorization and confirm the destination ref. If an authorized history update requires a force-push, use a lease against the expected remote tip; if the lease fails, stop and inspect rather than overriding it.
