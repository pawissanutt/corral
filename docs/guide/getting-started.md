# Your first reviewed task

Install corral, open the PM, and take one small change through checks, a separate review and your
approval. You do not need to read configuration docs or write TOML first.

## 1. Install

Have Git and an authenticated agent CLI on PATH: OmO (`omo`), `omp`, or Claude Code (`claude`).
Authenticate through that agent's own login flow before starting; corral uses your existing access.

On Linux or macOS:

```sh
curl -fsSL https://raw.githubusercontent.com/pawissanutt/corral/main/install.sh | sh -s -- --yes
export PATH="$HOME/.local/bin:$PATH"
corral --version
```

This downloads a prebuilt binary to `~/.local/bin`; no Rust is needed. `--yes` accepts installation
or upgrade of herdr, the terminal engine (minimum 0.9.3). Save the PATH line in your shell's startup
file to keep it available in new terminals.

On native Windows x86_64, run in PowerShell:

```powershell
Invoke-RestMethod https://raw.githubusercontent.com/pawissanutt/corral/main/install.ps1 | Invoke-Expression
$env:PATH = "$env:LOCALAPPDATA\Programs\corral\bin;$env:PATH"
corral --version
```

The Windows installer prints engine setup guidance rather than installing herdr. Complete the
[Windows herdr and Git Bash setup](install.md#native-windows), then use Git Bash for the steps below.
The [installation guide](install.md) also covers persistent PATH, updates and version pins.

## 2. Open your workspace

Start in a git repository, or a directory containing your repositories:

```sh
cd ~/code
corral up
```

Accept the initialization offer after checking the detected repositories. A **workspace** is the
directory holding `.corral/`; each registered repository is a **project**. Startup applies safe setup
fixes, installs the agent skill and opens the board inside herdr. You do not need to start herdr
separately. If the board shows a tour, follow it or press `Esc` to skip.

## 3. Open the PM with an agent you use

Press `P` on the board. The default PM uses OmO.

If you use Claude Code or `omp` instead, open another terminal in the same workspace and preview
the connection you already use:

```bash
corral setup bootstrap --harness claude
```

Use `--harness omp` for `omp`. After checking the preview, repeat the same command with
`--confirm` to launch the setup PM. This chooses the connection for that PM, not every future worker.
Tell it which installed agent and account you want to use; it can propose worker and reviewer profiles
and show the exact settings for your approval. One model is enough for implementation and a separate
review session unless you explicitly require different model families. Do not paste credentials
into the PM conversation.

## 4. Choose checks and review, then dispatch

Start with a small change in a project that already has a test command. For example:

> Set up app for my first task. Use the agent access I already have. Inspect the repository's checks,
> propose a test gate and a separate reviewer, and keep final acceptance with me. Show me the exact
> settings before applying them. Then file a task to add a dry-run option: print what would happen,
> change nothing, and add a test. Show me its scope and ask before dispatching.

Replace `app` and the feature with your repository's name and a useful, bounded change.
The PM will ask about the decisions it cannot infer:

| Decision | What it means | For this first task |
|---|---|---|
| Gate | Commands that must pass on the candidate, such as the repository's test script. | Use real project checks. An empty gate is not a pass. |
| Reviewer | A fresh agent session that reads the diff and evidence and records a verdict. | Choose a profile you can run; it can use the same model in a separate session. |
| Acceptance | Who may land the finished change into the local base branch. | Keep it with you. |
| Automatic actions | Permission to start gates, reviews, repairs or landings without another explicit command. | Stay in Assist and request each step explicitly. |

New workspaces default to **Assist**, no gate, no reviewer and owner acceptance. The PM's proposed
settings add checks; they do not authorize task execution. Approve the exact setup after inspecting it.
If the repository has no checks yet, make adding a meaningful check the first task instead of calling
an empty gate verified.

Ask the PM to show the task, selected worker and reviewer before launching. When the scope is right,
say **"Dispatch this task."** You do not need a configuration guide or a CLI task ID to do this.
The card moves to **Running**. corral creates a worktree at `<project>/.worktrees/<task>` on
branch `corral/<task>` and opens the coding agent there, separate from your working tree.

Select the card and press `a` to visit the worker; `Ctrl-b`, then `k` returns to the board with default
keys. If it needs an answer, the card moves to **Blocked** with a reason. Press `r` to reply.

## 5. Check, review and land

The worker calls `corral done` when it has committed and verified its work. That submits the run to
**Review** with a diff, summary and evidence; it does not mean the change has landed.

In the PM pane, ask:

> Run this task's configured gate, then start its independent review. Show me the results and the
> diff before I accept. Do not land it for me.

In Assist, these are explicit requests, not automatic advancement. The PM can run the same commands
you can run yourself (replace the example ID with the card's ID):

```bash
corral gate T-001 --wait
corral review-run T-001
corral review T-001 --diff
```

Wait for a passing gate and an approving reviewer verdict on the current candidate. If checks fail
or the reviewer requests changes, ask the PM to send the concrete findings back to the worker, then
repeat checks and review on the repaired candidate. A summary saying "done" is not a passing gate.

Open the Review card with `Enter`. Inspect the diff, gate log and reviewer notes. Approve with `a`
when satisfied; `c` requests changes instead. Approval lands into the project's **local** base branch,
using rebase by default, and the card becomes **Done**. Nothing is pushed to a remote. If the base
changed, corral rechecks the integrated candidate; a failure returns for repair rather than landing.

You have now taken a task from intent to a checked, independently reviewed local change.

## Coming back and next steps

- `q` closes the board without stopping agents; `corral up` brings you back.
- `?` shows board help and your configured navigation keys.
- `corral doctor` diagnoses setup; `corral doctor --fix` applies safe fixes without touching agents.
- To do the filing yourself, press `n` to create a task, then select a Ready card and press `d`
  to choose a profile and confirm dispatch.
- When ready to delegate more, ask the PM for a scoped Autopilot preview. Only your explicit
  confirmation grants automatic actions; configuration alone does not.

See [automation](automation.md) for delegation and review policy, [the terminal UI](tui.md) for
board controls, and [the CLI reference](cli.md) for direct commands. [Configuration](configuration.md)
is a reference for later, not a prerequisite for your first task.
