# Agent skills

corral installs one managed skill per agent role into each agent harness found under `$HOME`
(`~/.omo/agent/skills`, `~/.omp/agent/skills`, `~/.claude/skills`, `~/.agents/skills`). Each skill is
`<skills dir>/<name>/SKILL.md`:

| Skill | Role | Loaded by |
|---|---|---|
| `corral` | Workspace PM: board coordination, setup, model selection, automation | PM panes and agents asked about the board |
| `corral-worker` | Dispatched coding run: worktree, gate, evidence, `corral done` | The run prompt names it |
| `corral-planner` | Planning session: write and submit a plan without editing source | The plan prompt names it |
| `corral-submit` | Filing agent: capture ideas, file and split tasks, import plans | Agents asked to file work |

There is no reviewer skill: reviewers follow the versioned review contract in their prompt.
The role skills are short procedures that point at `corral <verb> --help`; the run and plan
prompts stay authoritative for their sessions.

## Versions and refresh

Each managed skill carries the managed marker followed by a stamp line:

    <!-- corral-skill: corral-worker version=0.3.0 sha256=<digest of the unstamped body> -->

A copy is current when it is byte-identical to what the running binary would write. When a run or
planning session is dispatched, corral installs any missing role skill for the launched harness and
refreshes any stale managed copy, logging `skill_installed` or `skill_refreshed` on the task.
A `SKILL.md` without the managed marker belongs to you: corral never overwrites it at dispatch,
reports a warning and logs `skill_unmanaged`.

## Installing manually

`corral skill install` installs every role skill for every harness found, or for the ones named with
`--harness`, or into one skills directory with `--dir`. It prints one line per harness and role:
`wrote`, `refreshed` or `up to date`. A file corral did not write is skipped with a warning and the
command exits non-zero; `--force` replaces it. `corral skill` prints the PM skill, and
`corral doctor --fix` runs the same install.
