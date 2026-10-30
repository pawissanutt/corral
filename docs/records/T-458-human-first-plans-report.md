# T-458: human-first plan review

## Same scope, before and after

Both examples propose one explicit clockwise widget rotation through the API and
CLI. Existing scheduled rotations keep their schedule. Scheduling changes and
notifications are excluded. Completion means `widget --spin` rotates once and a
scheduled rotation still runs on its existing schedule. The examples use the
same task boundaries, file scopes, dependency edge and verification command.

Before, the review opens with child-task metadata and revision history, followed
by a snapshot whose affected paths, contracts and task checklist precede
acceptance. The owner must infer included behavior from `widget-api`,
`widget-cli`, `Widget.spin`, and the checklist.

After, the review opens with these canonical plan values:

| Review question | Example answer |
| --- | --- |
| Why is work needed? | Operators must wait for the scheduler to rotate a widget. |
| Goal | Let operators rotate a widget once on demand. |
| What changes? | Add an explicit spin operation through the API and CLI. |
| What stays? | Automatic rotations keep their current schedule. |
| Where does work stop? | No scheduling changes; no notifications. |
| Important choice | Keep the existing widget contract; the CLI calls the same operation. |
| How is completion demonstrated? | Request a rotation: one clockwise turn. Wait for a scheduled rotation: it runs on its existing schedule. |

The task checklist is available through `i` / **Implementation details** in that
same Plan tab; `o` / **Review overview** returns. The plan remains a single
Markdown artifact with YAML frontmatter, not a separately maintained brief.
The optional `human_review` field adds only the problem, included changes and
retained behavior. Existing goal, exclusions, choices and acceptance fields
remain canonical.

## Revision examples

The notifications example is deliberately different scope: it adds a completion
notification, removes the notification exclusion, and adds a notification
acceptance scenario. These changes appear before the full overview, as added
and removed values in the relevant sections.

The technical example keeps exactly the original behavior while moving the
source paths and narrowing the test command. It reports that the human-review
fields are unchanged, rather than claiming new product scope.

For a historical plan acquiring its first structured overview, the UI explicitly
says that new review context alone is not evidence of a scope addition.
Historical plans without the opt-in remain valid and keep their existing
snapshot view. Structural validation does not grade the quality of a proposal,
grant approval, or change dispatch or routing policy.

## Evidence

The reproducible live driver is `scripts/qa/human-first-plans.mjs`. It uses the
repository's isolated, plugin-free herdr server, disposable stores and a copied
candidate executable. It subscribes to terminal output before each action;
there are no fixed sleeps or timing-based pass conditions.

Its output includes the full `before.md` and `after.md` plans for direct
same-scope comparison, notification and technical revisions, command results,
and terminal captures at 120 and 80 columns. Overview captures, acceptance at
the end of the overview, and both keyboard and mouse transitions demonstrate
the review path. Captures preserve the real terminal's ANSI output.

Targeted tests cover structural completeness, explicit no-exclusions statements,
historical round trips, overview-first rendering, compact navigation, scroll
reset, actual application mouse routing, and structured revision differences.

The complete run evidence is under `.omo/evidence/T-458/proof/`:

| Evidence | What it demonstrates |
| --- | --- |
| `before.md`, `after.md` | Same scope and execution fields; only human context is added. |
| `after-120/01-overview.png`, `after-80/01-overview.png` | Problem and goal lead the review, not task metadata. |
| `after-120/01a-included-and-retained.png`, `after-80/01b-boundaries.png` | Included work, retained behavior and explicit exclusions are readable without the checklist. |
| `after-80/02-acceptance.png` | Concrete done scenarios are readable in the overview. |
| `after-80/03-details-keyboard.png`, `after-80/05-details-mouse.png`, `after-80/06-overview-mouse.png` | Both input paths open implementation details and return to the same overview. |
| `notifications-80/01-overview.png` | Added notifications are named as included-scope changes. |
| `technical-80/01-overview.png` | File and command edits do not appear as changed product scope. |
| `commands.json`, `result.json` | Submission rejects empty exclusions before any plan or artifact is created, accepts an explicit no-exclusions statement, and accepts historical plans. |
| `screenshots.json`, `capture-checks.json` | All 87 rendered captures have the expected dimensions and no line overflow; the same-scope fields compare equal, and every canonical revision overview value appears in the captured overview. |
| `notifications-{80,120}/01-scroll-*.png`, `technical-{80,120}/01-scroll-*.png` | Overlapping intermediate captures cover the entire revised overview, including removed exclusions between the first and last screens. |

Checks run: core plan tests (18 passed), targeted overview/revision/navigation
tests (8 passed), the application mouse-navigation test (passed), candidate CLI
build (passed), and isolated live acceptance (14 checks across eight terminal
scenarios passed). Rust 2024 formatting and changed-file diagnostics were clean.
The post-rebase gate is recorded separately by Corral's gate receipt.

The unchanged attention header and blocked-automation warning toasts are visible
in both baseline and after captures. They do not cover the new review content;
they are not redesigned by this task. The TUI retains its existing dark theme.
Historical plans have no retroactive overview requirement, and the parser makes
no semantic judgment about proposal quality.

Independent visual checks confirmed the requested content and navigation, but
did not give the existing TUI a blanket accessibility pass. They identified
inherited muted-text contrast, one-row shared button targets, and the shared
button primitive's lack of hover/pressed styling. The new controls use that
existing primitive; changing the global palette and input-feedback system is
outside this narrow plan-review change. The reviewers also requested complete
intermediate revision scroll evidence; the live driver now captures overlapping
scroll steps so a removed exclusion cannot fall between the first and last
screens. The owner-only T-107 broad review audit remains separate and undispatched.
