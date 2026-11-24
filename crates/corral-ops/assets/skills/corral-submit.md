---
name: corral-submit
description: Procedure for filing work on the corral board - search for duplicates, write a well-formed spec, classify kind, size and difficulty, choose idea/backlog/ready, split into dependent tasks or import a Markdown plan. Use when asked to file, capture or split corral tasks, or when a run finds follow-up work.
---
{{marker}}

# corral submit

Use this procedure only when your role or run contract authorizes filing. **Filing is not dispatch
authorization.** A task filed to Backlog or Ready can be dispatched automatically under a matching
operating grant and consume a run slot. This skill does not authorize promotion, dispatch or review
acceptance. Read `corral <verb> --help` for exact legacy CLI flags.

## 1. Search before filing

1. `corral search "<keywords>" --in all` covers specs, reviews and artifacts; add `--json` to script it.
2. `corral list --json --all` lists every task, including Done and Dropped ones; `corral list --query "<keywords>"`
   searches all statuses in relevance order.
3. Inspect candidates with `corral show <id> --json`. Compare the affected behavior, scope and files
   on both open and completed tasks. Reuse the matching task, adding evidence with an authorized
   comment if useful. If a Done task shipped the behavior but there is a regression or a distinct gap,
   link the Done task and explain what is different. Do not file a duplicate or conflicting replacement.
4. Do not rely on import's duplicate-source check: it only refuses the same plan file while its tracker
   is open. Check equivalent trackers and members on the board first.

## 2. Spec template

Every authorized new filing, including an Idea or follow-up, gets its durable spec **at creation**.
Pass the Markdown with `corral new "<title>" --body-file spec.md` (or use `--body` for inline Markdown).
The title, a chat message, an artifact link or a later comment is not a substitute for the task body.

```markdown
## Problem / Goal
What needs to change, for whom, and why it matters.

## Observed vs. expected
What is observed today (with evidence or attribution) and what should happen instead.

## Repro (bugs)
Smallest known steps, command or input, relevant environment/version and actual result.

## Acceptance criteria
- Observable, checkable outcomes, with expected results or retained behavior.

## Relevant files / starting points
- Verified paths, symbols, docs, logs or related task IDs and what to look at there.

## Open questions / unknowns
Unconfirmed facts, labeled assumptions, and decisions or evidence still needed. Say "None known" if none.

## Classification
Explain the kind, estimated work volume and separate reasoning difficulty, including any omissions.
```

Ground the spec in the owner's report, board and repository evidence. Mark unverified reports as
such; say what is unknown (including an unconfirmed repro), rather than making up a command, cause
or path. Omit Repro for non-bugs. For an authorized Idea with unsettled scope, record the observable
outcome if known and mark provisional criteria or decisions still needed instead of inventing certainty.

## 3. Kind, size and difficulty

Set `--kind`, `--size` and `--difficulty` at creation, including with `--idea`, when the evidence
supports them. Preserve explicit owner values (including project, status, priority, labels, classification
and profile); surface any conflict or invalid value rather than silently substituting. Use Agent: Auto
unless the owner explicitly pins a profile.

| Kind | Use for | Example |
|---|---|---|
| `ui` | Rendered surfaces, layout, interaction | Add a Plan tab to the board card |
| `backend` | Services, storage, process and CLI plumbing | Persist the dispatch cursor atomically |
| `logic` | Algorithms and state machines where correctness is the risk | Resolve dependency cycles during import |
| `docs` | Guides, skills, prose with no behavior change | Document the filing procedure |
| `test` | Adding or repairing tests only | Cover the empty-inventory guard |
| `refactor` | Behavior-preserving restructuring | Split a file over the SLOC ceiling |
| `research` | Investigation whose deliverable is a report | Compare two search backends |

Pick the kind that carries the main risk. A bug is not a built-in kind: for example, use `ui` for an
interaction defect or `logic` for a state-machine defect, not `refactor`.

| Size | Shape |
|---|---|
| `xs` | One-line or one-file tweak, a typo, a constant |
| `s` | One module or one document, a single focused change |
| `m` | Several files in one crate, new behavior plus tests |
| `l` | Multiple crates or a new subsystem; consider splitting |
| `xl` | Cross-cutting change; split it |

Use the best supported estimate and explain it in the Classification section; mark any assumptions.

Size measures work volume, not reasoning difficulty. Assess difficulty separately:

| Difficulty | Shape |
|---|---|
| `routine` | A known pattern or repetition with a settled approach |
| `standard` | Bounded reasoning within understood contracts |
| `hard` | Uncertainty, algorithmic optimization, or correctness argued from invariants |

A small task can be hard; large repetitive work can be routine. Read the goal, acceptance and
repository evidence, never classify by title keywords, kind or size alone. Omit a classification
field only when you cannot assess it, and explain which field and why in the body. Unset difficulty
keeps legacy routing; it is not routine. Persist assessed difficulty before loading
`corral selection catalog` or generating an Auto receipt. Revise it with `corral edit <id>`, then
reload the catalog and save a fresh receipt: difficulty affects the selection revision independently
of plan identity.

Use relevant built-in labels or exact existing board labels, as seen on related tasks. For legacy
`corral new`, use `--label <label>` (repeat or comma-separate for several); do not invent labels or
use them in place of the classification fields. If no relevant label can be identified or the owner
asks you to omit labels, say why in the body. Do not guess scheduling labels: `owner-only` keeps an
Idea from automatic promotion and should be used only when instructed.

## 4. Split, import or design

- **Split** unrelated problems or independent deliverables into their own specs. If a later part
  cannot start until an earlier one lands, set `--depends <id>` (repeat or comma-separate for several)
  instead of merely saying "after T-x" in prose.
- **Import** an authorized multi-task plan with `corral import plan.md --dry-run`, then `corral import plan.md --origin <you>`.
  Supply the tracker's spec in the Markdown after YAML frontmatter and each child's spec in its `body` entry.
  Frontmatter has `title`, optional `project`, `ready`, and `tasks` whose entries have a plan-local `key`,
  `title`, and `kind`, `size`, `difficulty`, `priority`, `profile`, `labels`, `body`, `depends` as applicable.
  The tracker has no separate size/difficulty input fields; record those assessments in its body.
  The import creates one tracker plus dependent children, in Backlog unless `ready: true` is authorized.
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
- Preserve explicit owner pins (`--profile`, alias `--agent`) and record the reason if known. For Auto,
  follow "Profile selector procedure (v1)" in the `corral` skill after filing with assessed metadata:
  `corral route <id>` explains the implementer and reviewer mapping without starting a run. If it looks wrong,
  reconsider the classification and refresh the Auto receipt rather than overriding the profile.

## 6. Workers filing follow-ups

When a dispatched run discovers out-of-scope work, the run contract stays authoritative and the parent task
keeps its scope. If filing is not authorized, report the finding instead.

1. Search first as above; do not file what the board already holds.
2. File with `--origin agent`, usually `--idea` or Backlog, using the same spec and classification procedure.
   Name the parent in the body ("Found while working on T-123") and use `--depends <parent>` only if
   the follow-up must wait for the parent to land.
3. Do not dispatch, move to Ready, or wait on the follow-up. Mention its ID in your `corral done` summary.
4. If you need a decision before you can continue, ask it in your session and stop. The owner's reply
   reaches you through the board, whatever harness you run in.

## 7. Verify what you filed

Use `corral show <id> --json` to confirm the stored body and metadata, including labels, status and
classification. Use authorized specification edits (`corral edit <id>`) to correct the spec;
comments are for additional evidence or discussion, not a replacement for the spec. If creation's
outcome is uncertain, check the board before retrying.
