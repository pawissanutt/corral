# Build sessions

A build session is a Pi conversation for working in Git without a Corral task or
dispatch slot. Start it from the checkout you want to work on:

```sh
corral-pi build --name example
corral-pi build --here
corral-pi build --resume B-<id>
```

By default, Corral creates `.worktrees/build-<name>` on `build/<name>` at the
current commit. `--here` uses your existing checkout. Resume uses the recorded
worktree and refuses if a separate build worktree no longer has its recorded
branch or belongs to another repository.

The local record lives in `build/<id>/` under `--data-dir` (default
`$XDG_DATA_HOME/corral/corral-pi`, or `~/.local/share/corral/corral-pi`).
Use `--profile <name>` to select a workspace Pi profile. API spending uses the
same host cap as other Pi sessions; see [corral-pi](corral-pi.md#api-spending-and-paid-continuation).

## Verify and land

Commit the candidate and have the agent run `build_verify`. It runs the project's
`gate` commands from `.corral-next/corral.toml` in the build worktree, with the
project's configured environment. The build journal keeps the result and log.

Open the exit choices with `/finish`, or ask the agent to offer them with
`build_finish`. The agent cannot choose an exit for you.

| Choice | What happens |
|---|---|
| **Iterate** | Stay in the session and keep working. |
| **Keep** | Record the result locally and leave the branch and worktree in place. |
| **Land** | Rebase onto the base branch, verify the rebased commit, then fast-forward the base locally. |

Land needs a clean worktree with committed changes and no live child agents.
With the review policy, it also needs an approving review of the change or an
override reason you supply. If the rebase conflicts, verification fails or the
base moves during verification, the session stays open. A stand-alone Git build
can land onto its starting branch in a clean primary checkout without a Corral gate.
