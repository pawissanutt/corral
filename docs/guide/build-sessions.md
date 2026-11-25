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
If you don't choose a model or profile, Build uses the startup model saved in
Corral Pi's `/settings`. If that model is missing or unavailable, an interactive
launch asks before creating a session or worktree. Resuming keeps the recorded
model.

`--headless` runs without the terminal interface; non-interactive launches are
headless by default. A new headless Build needs a usable saved default, a profile
with a model, or `--model`. `--id <id>` sets the new session ID; it cannot be combined
with `--resume`. `--socket <path>` chooses the host socket and `--corral <path>`
chooses the Corral executable. `--data-dir <path>` selects the record store;
by default it is `$XDG_DATA_HOME/corral/corral-pi`, or
`~/.local/share/corral/corral-pi` when that variable is not an absolute path.

`--plan` starts the build in [plan mode](plan-mode.md): the agent plans with you and
changes nothing until you approve the plan. `/plan` does the same in a running session.

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

## Artifacts in Code sessions

A trusted, non-bare `corral-pi code` session has `corral_artifact`; a guarded Build
or bare Code session does not. In a Git checkout, explicit `--corral` can make Code
non-bare even without a discovered workspace. Tool availability does not itself provide
a default project or a workspace able to accept the attachment. Code defaults to its
configured project when available, or can specify a task or project with `to`. Without a
project or bound task, provide `to`. To revise a report, edit the working file and attach it again with the
previous reference in `replaces`; this publishes a new row without changing the old
one. Plan mode refuses `corral_artifact` calls while planning or awaiting plan review. See
[corral-pi](corral-pi.md#attaching-artifacts) for tool parameters and
[the artifact reference](cli.md#corral-artifact) for library and list behavior.

## Delegation

Build uses `standard` delegation unless configured otherwise. To encourage the parent to
start substantial independent slices or useful bounded read-only exploration earlier, add
this to `.corral-next/pi.toml` at the workspace root (Git top-level for a standalone build,
or the start directory outside Git):

```toml
[build.delegation]
mode = "proactive"
```

You do not need to configure any profiles or routes. The parent uses the `default`
child profile for unmatched work: a configured `default` if present, otherwise the
parent's launch model and effort. [Optional routes](configuration.md#build) describe
scoped assignments and select a child kind and profile in either mode. The parent
interprets the routes and calls `task`; the host does not automatically start children.
If a configured route is unavailable, the parent reports it rather than substituting a
profile, and only continues inline when allowed.

The parent keeps trivial or tightly coupled work inline, assigns each writer distinct
owned files, and integrates and checks child results. Delegation does not change approval
or spending limits, independent review, or who chooses the exit. Cancelling a child leaves
any edits it already made in the shared worktree. The per-session concurrency cap defaults
to 32; set a lower `[build] max_parallel` in the same file to limit simultaneous children.
To restore the original delegation policy, remove the `[build.delegation]` section and
its route tables. Policy changes take effect on the next launch or resume.

## Write and command approvals

For the parent build, edit and write inside the session worktree work normally, as does
bound workspace Markdown memory. An edit or write outside the worktree, including an ordinary
canonical symlink escape, requires host-issued owner approval. Protected `.git`,
`.corral`, `.corral-next` and `.worktrees` paths are hard blocks, except for the scoped
worktree and workspace memory exceptions. The outside-worktree approval prevents accidental
changes elsewhere; the `.worktrees` block prevents accidental edits in other checkouts. The
`.git` block prevents repository corruption; `.corral` and `.corral-next` protect engine state
from direct edits.

Build slice children cannot approve outside-worktree writes and retain these blocks. Trusted
Code slice children may edit outside the cwd and under `.worktrees`, but still cannot edit
`.git`, `.corral` or `.corral-next` (or unresolved destinations), preventing state corruption
and unsafe destination mistakes. Explore and review children remain read-only. Children cannot
run Git write commands, preventing conflicting Git changes in a shared worktree.

To enable `bash_rules` for `bash` and `job_start`, add this top-level table to
`.corral-next/pi.toml` in the workspace (or the repository root for a bare build). This
example allows known commands and asks about unmatched ones:

```toml
[bash_rules]
allow = ["git status*", "git diff*"]
ask = []
deny = []
default = "ask"
```

A matching `ask` also requests approval; deny takes precedence over ask, which takes
precedence over allow. Configured deny and fixed rules cannot be overridden.
The host-generated approval question appears in `/questions` or a connected app. The owner
can choose **Allow once** for one exact retry, **Allow for 30 minutes** starting from the owner choice,
**Allow for this session** for the live host only, or **Deny**. Longer grants cover only one
canonical file destination for edit/write, never a directory or sibling, or the exact shell
command, tool and arguments from the ask, within the same host. Grants clear on host
restart/reload or rewind; none is permanent. Denying an exact call blocks it for the live
session, even if a different change to the same file gets a grant. Approval does not execute
the tool automatically. Only the host prompt authorizes; ordinary chat and `ask_user` do not.

Never use shell, another tool or `ask_user` to bypass a refusal or approval. Rules do not
inspect command intent or side effects, grant OS elevation or provide a sandbox. Shell and
other tools can have effects outside edit/write enforcement. These checks are accident guards,
not a security boundary.

## Finish

Choose **Finish** in the app, or use `/finish` in the terminal. The agent can
offer the choices, but you decide:

- **Keep** records the outcome and keeps the worktree and branch.
- **Iterate** returns to the session to continue working.
- **Land** locally fast-forwards the base branch to the prepared commit. It does not
  rebase your work, run another gate, or ask the agent to change code after you click.

Preparation happens **before** the landing choice. For a workspace build, the host
captures the integration tip, rebases the committed candidate onto it when necessary,
and checks the prepared head. Finish runs any missing gate and, under review policy,
requests an independent reviewer when needed. The review compares that head to the
captured base, not to where the build originally started. A clean rebase is not a failure: the resulting
commit can still need a gate or review. If Git finds a conflict, the host reports the paths and aborts only when it can
identify the rebase as its own. Otherwise it leaves the operation in place; inspect
`git status` in the build worktree before continuing. A failing
gate reports its command and log. Repair the candidate before offering it for Land.

A reviewer who finishes on the captured base and head keeps that verdict in the
build record even if the integration branch moves during review. Finish can carry
approval through a clean replay only when the base advanced, the reviewed patch
is identical, and the source, target, review inputs and gate policy still match.
By default the old and new base trees must be identical.

An owner can opt specific prose files into this exception in the workspace's
`.corral-next/pi.toml`. Declare only files that are not requirements, instructions,
build inputs or otherwise relevant to reviewing changes in this project:

```toml
[build.review_neutral_docs]
app = ["docs/release-notes.md"] # app is the project name
```

Only modifications to existing regular Markdown files at those exact lowercase
paths under `docs/` are allowed; agent and instruction files cannot be listed.
Adds, deletes, renames, mode changes and edits to paths in the reviewed
patch still require fresh review. Carrying approval does not carry a passing gate
or an earlier Land choice.

The final choice covers a specific base and head. A previously passing gate on the
same head with the same commands and configured gate environment can be reused. A
receipt from before the host recorded those inputs does not count. Under review policy an owner
may supply a reason to override missing approval, never to bypass a failed gate. If either branch moves,
prepare the new candidate rather than reusing an old choice. A stale card cannot land
a different commit. Keep and Iterate do not require passing evidence.

All pinned submodules must be initialized recursively, clean and at their recorded commits
before verification. A gate run on a partial checkout cannot authorize a full checkout.

Land requires a clean candidate, no live children, a suitable base branch and a base
checkout that can safely fast-forward without overwriting staged, unstaged, untracked,
or ignored work. A checked-out target needs a supported full checkout; Corral will not
break an existing Git lock. If Git or the filesystem is interrupted after updating files,
inspect the named checkout before retrying. Corral does not reset user work to recover.

A standalone Git build prepares against its starting branch, with no Corral gate; outside Git only Keep and Iterate are available.

## Stop, resume and remove

Stopping a session stops its host; resume it in the app or with
`corral-pi build --resume <id>`. Resume uses the recorded checkout and refuses if
a separate build worktree no longer has its recorded branch or belongs to another
repository. It rereads the current delegation policy from the recorded workspace or
standalone root, not the caller's directory. A stopped build is read-only until resumed.

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
