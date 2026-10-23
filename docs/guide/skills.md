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

OmO loads `~/.agents/skills` as well as `~/.omo/agent/skills`, and reports a collision when both hold a skill
of the same name. So when `~/.agents` exists, corral installs the role skills for `omo` there only, and
removes any corral-owned copy (one carrying the managed marker) from `~/.omo/agent/skills`; a file there
that corral did not write is never touched. Without `~/.agents`, OmO keeps its own directory. If the
`~/.agents` copy is a file corral did not write, the `~/.omo` copy stays until `--force` replaces it.
`corral doctor` reports a leftover duplicate as `skill_duplicates`, and `--fix` removes it.

There is no reviewer skill: reviewers follow the versioned review contract in their prompt.
The role skills are procedures that point at `corral <verb> --help`; the run and plan
prompts stay authoritative for their sessions. `scripts/check-docs.sh` (part of the gate) checks
every `corral` command and flag shown in a role skill against the binary's `--help`.

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
