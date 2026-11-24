# Task metadata in corral-next

Size, difficulty and tags are optional. They do not set task kind, status or priority.

| Field | How to choose |
| --- | --- |
| Size | Estimate the amount of work: `xs` for a tiny contained change, `s` for a small change, `m` for a typical bounded task, `l` for substantial work across several pieces, `xl` for a large effort that may benefit from splitting. |
| Difficulty | Estimate technical complexity or uncertainty: `routine` when the approach is known, `standard` when it takes some investigation or judgment, `hard` when there are difficult constraints or substantial unknowns. A tiny hard fix and a large routine migration are both possible. |
| Tags | Add labels that help find related work. Built-in suggestions are `accessibility`, `bug`, `dependencies`, `documentation`, `performance` and `testing`. No registry or configuration is needed to add a custom label. |

Tags are exact and case-sensitive. Both kinds can appear on the same task. Internal spaces
and punctuation are allowed; empty labels, surrounding whitespace and control characters are
rejected. Exact duplicates are kept once, in first-appearance order. `Payments` and `payments`
are different tags. Tags don't affect dispatch or routing.

In Corral App, use **New task** or **More → Edit**. Pick size and difficulty independently,
or leave either unset. Pick a suggested tag, or type a custom label and press **Add** or save
the form. Remove labels you no longer want. The task list and detail show the saved values.
In the Tasks filter, use
`tag:performance` or `tag:"Payments, EU"` to match an exact label. Multiple terms must all
match. Use `size:unset` or `difficulty:unset` to find tasks missing an estimate.

```sh
corral-next new 'Investigate a slow EU checkout' --body-file checkout.md \
  --size s --difficulty hard --tag performance --tag 'Payments, EU'
corral-next edit T-042 --size m --difficulty standard --tag 'Payments, EU'
corral-next edit T-042 --size= --difficulty= --clear-tags
corral-next list
corral-next show T-042 --json
```

On edit, one or more `--tag` flags replace the tag list. Include labels you want to keep;
use `--clear-tags` to remove them all. Omitting a field leaves it unchanged. `--size=` and
`--difficulty=` clear their respective estimates. Existing tasks with no estimates stay unset;
legacy exports retain size and difficulty when recorded.

## Routing

`[[routing]]` can match `difficulty = ["routine", "standard", "hard"]` (use any subset)
independently of `size = ["xs", "s", "m", "l", "xl"]`. Values within each list are alternatives;
when both lists are nonempty, the task must match both. An unset task field doesn't match a
nonempty list for that field. Omitted and empty lists don't constrain that field, so existing
rules match as before. First matching rule wins; the explicit dispatch or task profile still
takes precedence.

## Agent filing

Before filing, check `corral-next list --all --json` for likely duplicates (including Done and
Dropped tasks) and inspect matches with `corral-next show <id> --json`. Reuse existing work
where appropriate and file unrelated problems separately.

Use this Markdown outline for every new task, including Ideas and follow-ups:

```markdown
## Problem / Goal
What needs to change and why.

## Observed vs Expected
What you observed and what should happen instead.

## Repro steps (bugs only)
The smallest known sequence, or what has yet to be reproduced.

## Acceptance criteria
- An observable check that would show the work is complete.

## Relevant files / starting points
Paths, symbols, logs or related tasks to inspect first.

## Open questions / unknowns
What still needs confirmation. Label guesses and explain any omitted fields.
```

Pass the body with `new --body-file <spec.md>` so it is present from creation, not a later
comment. Include a best-effort size from actual scope, a separate difficulty, and relevant tags.
Use applicable built-ins or exact existing custom labels. Omit a field only if the owner asks
or it genuinely cannot be assessed; say why in the body. Verify the result with `show <id> --json`.
Previewed plan fields carry into confirmed filing; filing follow-on work does not authorize
rewriting metadata on existing tasks.
