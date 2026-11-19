# Build sessions

A build session is a Pi conversation for working directly in a checkout without a
Corral task or dispatch slot. The app and terminal use the same kind of session:
you can watch and drive a terminal-started build from the app, or attach a terminal
to a stopped app-started build by resuming it.

## Start

In the app, choose **Agents › New › New build session…**. Choose a project,
name, model and whether to use a new worktree or the checkout.

From a terminal in the checkout, for example:

```sh
corral-pi build --name example
corral-pi build --here
corral-pi build --resume B-<id>
```

By default, a Git build creates `.worktrees/build-<name>` on `build/<name>` at
the current commit. `--here` works in the existing checkout and branch instead.
The name is generated if omitted. `--project <name>` chooses a configured project;
otherwise Pi detects the project from the checkout when possible. `--profile <name>`
chooses a build profile. `--model [<provider>/]<id>`, `--provider <id>`,
`--effort <level>`, `--aws-profile <name>`, `--aws-region <region>` and repeated
`--fallback <provider>/<model>` select the model route; see [corral-pi](corral-pi.md).
The default model is `openai-codex/gpt-6.1-sol` unless a profile supplies one.

`--headless` runs without the terminal interface; non-interactive launches are
headless by default. `--id <id>` sets the new session ID; it cannot be combined
with `--resume`. `--socket <path>` chooses the host socket and `--corral <path>`
chooses the Corral executable. `--data-dir <path>` selects the record store;
by default it is `$XDG_DATA_HOME/corral/corral-pi`, or
`~/.local/share/corral/corral-pi` when that variable is not an absolute path.

## Work together

Send a message to start a turn. While the agent works, send a steer to change its
direction or queue a follow-up for later; stop the turn without ending the session
when you need to interrupt it. Answer questions and dialogs in the app or the
terminal, including dialogs opened in an attached terminal. The first answer wins.

**Agents** shows child agents and their activity. Open a child to inspect its
conversation and, where available, answer, steer, stop or change its model.

**Changes** lets you inspect changed files and diffs, switch comparisons and
scopes, and see attribution and changes outside assigned ownership.

**Board** shows workspace tasks and blocked or failed work. Refresh it and review
proposed plans; you can file a proposal, file and start its tasks, or dismiss it.

## Finish

Choose **Finish** in the app, or use `/finish` in the terminal. The agent can
offer the choices, but you decide:

- **Keep** records the outcome and keeps the worktree and branch.
- **Iterate** returns to the session to continue working.
- **Land** rebases onto the base branch, runs the project's gate on the rebased
  commit, and fast-forwards the base branch locally. A standalone Git build
  instead lands on its starting branch without a Corral gate.

Land is unavailable without a suitable Git branch and base, commits beyond the
session base, a clean worktree and no live child agents. It is also unavailable
when the project or its landing base is missing, or project policy disables it.
In a workspace with review policy, provide a reason if the current commit lacks
an approving review with every finding resolved. The primary checkout must be
ready for the fast-forward. If the rebase conflicts or the base moves, Land fails
and the session stays open. A failed gate can be handed to the agent for repair,
with up to three repair rounds.

Land shows its steps as they run: the rebase, the gate and the fast-forward, each
with its time. Under the gate step you see the command that runs, the last lines
of its log and the log's path; a failed gate keeps them. Land does not wait for a
result it cannot use:

- The gate is not run again on a commit it already passed on with the same
  commands. When the rebase changes nothing, Land goes straight to the
  fast-forward and names the earlier attempt.
- If the base branch moves while the gate runs, the gate is stopped at once and
  Land fails. Choose Land again to rebase onto the new tip.
- If the checkout of the base branch has uncommitted changes to files the build
  changes, Land fails before the gate, because the fast-forward would refuse.

## Stop, resume and remove

Stopping a session stops its host; resume it in the app or with
`corral-pi build --resume <id>`. Resume uses the recorded checkout and refuses if
a separate build worktree no longer has its recorded branch or belongs to another
repository. A stopped build is read-only until resumed.

To remove a finished build, choose **Remove** from its Agents row, or use:

```sh
corral-pi build --prune --session <id> --dry-run
corral-pi build --prune --session <id> --json
```

The preview changes nothing; omit `--dry-run` to remove. Single-session removal
deletes the eligible worktree, branch and record, including the conversation when
it is inside the removed directories. A transcript stored elsewhere is retained.
It refuses a running session, a checkout used with `--here`, uncommitted changes,
or commits not on the landing base; it never forces Git removal. Removing a
worktree also removes ignored build output inside it.

`corral-pi build --prune --dry-run` previews global cleanup;
`corral-pi build --prune` applies it. Global cleanup keeps conversations and
their records, though it can remove an unused session's record. It skips running
sessions and in-place checkouts, and retains work that is not safe to remove.
