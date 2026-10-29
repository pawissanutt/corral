---
name: corral-submit
description: Procedure for filing work on the corral board - search for duplicates, write a well-formed spec, classify kind and size, choose idea/backlog/ready, split into dependent tasks or import a Markdown plan. Use when asked to file, capture or split corral tasks, or when a run finds follow-up work.
---
{{marker}}

# corral submit

You file work for the owner or PM. **Filing is not dispatch authorization.** A task filed to Backlog or
Ready can be dispatched automatically under a matching operating grant and will then consume a run
slot. File only what you would be content to see run, and never dispatch what you filed. Read
`corral <verb> --help` for exact flags.

## 1. Search before filing

1. `corral search "<keywords>" --in all` covers specs, reviews and artifacts; add `--json` to script it.
2. `corral list --json --all` lists every task, including Done and Dropped ones; `corral list --query "<keywords>"`
   searches all statuses in relevance order.
3. Check open tasks (Idea, Backlog, Ready, Running, Blocked, Review) for overlap in goal **and** in files:
   a conflicting open task is a reason to `corral comment <id> "<text>"` on it, or to file with `--depends`,
   rather than to file a rival. A Done task that already shipped the behavior means there is nothing to file.
4. Inspect candidates with `corral show <id> --json`. Importing the same plan file again is refused
   while its tracker is open; look at the existing tracker instead.

## 2. Spec template

Pass the spec with `--body` or `--body-file`; the title is the positional argument of `corral new`.

```markdown
## Goal
One or two sentences: the outcome and why it matters. No implementation recipe unless it is a constraint.

## Acceptance
- Observable, checkable criteria (a command and its expected result, a behavior, a file that exists).

## Relevant files
- Paths, symbols, docs or earlier tasks (T-123) the implementer should read first.

## Classification
<kind>, <size>: why this kind, why this size. Name related tasks and what they own.
```

Vague goals, acceptance criteria that cannot fail, and missing paths are what make a task bounce between
agent and owner. If you cannot write testable acceptance, file an idea instead (see status below).

## 3. Kind and size

Pass `--kind` and `--size`; they feed model routing, so a wrong value picks the wrong profile.

| Kind | Use for | Example |
|---|---|---|
| `ui` | Rendered surfaces, layout, interaction | Add a Plan tab to the board card |
| `backend` | Services, storage, process and CLI plumbing | Persist the dispatch cursor atomically |
| `logic` | Algorithms and state machines where correctness is the risk | Resolve dependency cycles during import |
| `docs` | Guides, skills, prose with no behavior change | Document the filing procedure |
| `test` | Adding or repairing tests only | Cover the empty-inventory guard |
| `refactor` | Behavior-preserving restructuring | Split a file over the SLOC ceiling |
| `research` | Investigation whose deliverable is a report | Compare two search backends |

Pick the kind that carries the main risk; a bug fix is `logic` or `backend`, not `refactor`.

| Size | Shape |
|---|---|
| `xs` | One-line or one-file tweak, a typo, a constant |
| `s` | One module or one document, a single focused change |
| `m` | Several files in one crate, new behavior plus tests |
| `l` | Multiple crates or a new subsystem; consider splitting |
| `xl` | Cross-cutting change; split it |

Say why in the Classification section. When unsure between two sizes, pick the larger and say so.

## 4. Split, import or design

- **Split** when the work has independent deliverables, or a later part cannot start until an earlier one
  lands. File each as its own task and link them with `--depends <id>` (repeat or comma-separate for several).
  Do not file a dependent task that merely says "after T-x" in prose.
- **Import** a multi-task plan with `corral import plan.md --dry-run`, then `corral import plan.md --origin <you>`.
  The file is Markdown with YAML frontmatter: `title`, optional `project`, `ready`, and `tasks` whose entries have a
  plan-local `key`, `title`, and optional `kind`, `size`, `priority`, `profile`, `labels`, `body`, `depends`.
  It creates one tracker plus dependent children, in Backlog unless `ready: true`.
- **Design first** with `--design` when the approach is a real decision (several viable architectures, a
  contract change, unclear acceptance). The task is routed through a reviewed plan run before implementation.
  Do not use it for routine, well-specified work.
- One task, one deliverable: do not bundle an unrelated cleanup into a spec.

## 5. Status, origin and profile

| Status | Flag | Meaning |
|---|---|---|
| Idea | `--idea` | A thought without settled scope. Needs an owner or PM decision before it can run |
| Backlog | (default) | Specified work waiting to be scheduled |
| Ready | `--ready` | Specified and approved to run next; the most likely to be picked up automatically |

- Default to `--idea` when scope or acceptance is open, Backlog when the spec is complete, and `--ready`
  only when the owner or PM told you to. Never promote with `corral move` to jump the queue.
- Pass your own `--origin owner|pm|agent` on `corral new` and `corral import`; the default is `owner`, which you
  must not claim unless the owner is the one asking. A PM files with `--origin pm`; a worker with `--origin agent`.
  Where a command takes `--author` (`corral comment`, `corral move`, `corral idea`), pass your own role too.
- Do not pin `--profile` (alias `--agent`) without a written reason in the body, such as a harness-specific
  capability the task needs. Otherwise leave routing to kind and size, then check the selection:
  `corral route <id>` explains the implementer and reviewer mapping without starting a run. If it looks wrong,
  fix the kind or size rather than overriding the profile.
- Other useful flags: `-p/--project`, `--priority`, `--label`. Add the label `owner-only` only when told to keep an Idea from automatic promotion.

## 6. Workers filing follow-ups

When a dispatched run discovers out-of-scope work, the run contract stays authoritative and the parent task
keeps its scope:

1. Search first as above; do not file what the board already holds.
2. File with `--origin agent`, usually `--idea` or Backlog, with the parent in the body ("Found while working on T-123")
   and, if it must wait for the parent, `--depends <parent>`.
3. Do not dispatch, move to Ready, or wait on the follow-up. Mention its id in your `corral done` summary.
4. If you need a decision before you can continue, ask it in your session and stop. The owner's reply
   reaches you through the board, whatever harness you run in.

## 7. Verify what you filed

`corral show <id> --json` confirms status, kind, size, origin and dependencies. Add missing context later with
`corral comment <id> "<text>"`; change status only when the owner asks, with `corral move <id> <status>`.
