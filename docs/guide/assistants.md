# Assistant sessions

Open [Ask in the Corral app](../../apps/corral-app/README.md#ask-and-attachments)
or start a terminal session with `corral-pi chat`. See the [Pi host guide](corral-pi.md)
for launch options and the [Pi workbench guide](pi-panels.md) for session controls.

## Write permissions

For a parent assistant, edit and write inside its starting cwd work normally, as does bound
workspace Markdown memory under `.corral-next/memory/`. An edit or write outside the starting
cwd, including a symlink that resolves outside it, requires host-issued owner approval.
Protected `.git`, `.corral`, `.corral-next` and `.worktrees` paths are hard blocks, except
for the scoped worktree and workspace memory exceptions. Outside-cwd approval prevents
accidental changes elsewhere; the `.worktrees` block prevents accidental edits in other
checkouts. The `.git` block prevents repository corruption; `.corral` and `.corral-next`
protect engine state from direct edits.

Trusted Code slice children instead may edit outside cwd and under `.worktrees`, but still
cannot edit `.git`, `.corral` or `.corral-next` or unresolved destinations, preventing state
corruption and unsafe destination mistakes. Explore and review children remain read-only;
children cannot run Git write commands, preventing conflicting Git changes in a shared worktree.

For `corral-pi chat`, enable rules for both `bash` and `job_start` by adding a top-level
`[bash_rules]` table to the workspace's `.corral-next/pi.toml`. For example:

```toml
[bash_rules]
allow = ["git status*", "git diff*"]
ask = []
deny = []
default = "ask"
```

This asks for unmatched commands; matching `ask` rules also ask. Deny takes precedence over
ask, which takes precedence over allow. Configured deny and fixed rules cannot be overridden.
The host-generated approval question appears in `/questions` or a connected app. The owner
can choose **Allow once** for one exact retry, **Allow for 30 minutes** starting from the owner choice,
**Allow for this session** for the live host only, or **Deny**. Longer grants cover only one
canonical file destination for edit/write, never a directory or sibling, or the exact shell
command, tool and arguments from the ask, within the same host. Grants clear on host
restart/reload or rewind; none is permanent. Denying an exact call blocks it for the live
session, even if a different change to the same file gets a grant. Approval does not execute
the tool automatically. Only the host prompt authorizes; ordinary chat and `ask_user` do not.

Never use shell, another tool or `ask_user` to bypass a refusal or approval. These checks do
not detect command intent or side effects, provide OS elevation or sandbox shell or other
tools. Shell and other tools can have effects outside edit/write enforcement; these are
accident guards, not a security boundary.
