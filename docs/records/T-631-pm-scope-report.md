# T-631: workspace Pi PM

## Outcome

New PM launches bind to the workspace (empty project identity). Approved projects are
resolved from the current authorized operating scope, not frozen into a session.
Task reads, comments/replies/capture, question operations, receipt lookups and question
wakes use that project set. Legacy project-bound records remain readable and migrate
on resume without discarding the saved Pi session file.

Workspace-root guidance is passed to the host, with workspace text trust recorded in
the capability snapshot/digest. Project checkouts are not preloaded. Scope refusals
include the owner preview and confirmation commands.

## Acceptance proof

- `cargo test -p corral-ops --lib agent::hooks::pm:: -- --nocapture`:
  5 passed, 2 deliberately ignored subprocess entry points. The three-project fixture
  runs the same profile/preflight entry point as `corral pi --pm`, authenticates a PM,
  reads all three projects and commits a comment to each. An unapproved fourth project
  is rejected. The changed-scope regression resumes both workspace and legacy bindings,
  drops the old project, adds a new one, and asserts the same saved session path,
  workspace-root guidance and recorded text trust. The empty-scope regression asserts
  the exact preview command and confirmation flag.
- `cargo test -p corral-ops --lib agent::pm_wake:: -- --nocapture`: passed.
- In `packages/corral-pi`, `bun run typecheck`: passed;
  `bun test src/host/config.test.ts src/roles/pm/pm.test.ts`: 53 passed.
- `cargo test -p corral-ops --lib agent::pi::tests:: -- --nocapture`:
  11 passed, 1 subprocess entry point ignored. The first quick gate exposed a fixture
  that reused the legacy project identity after migration and an obsolete scope-change
  refusal expectation. The fixture now reads the new bootstrap identity; the pane-safety
  refusal test uses a missing role asset instead, preserving its adoption assertions.
- `cargo test -p corral-ops --test pm_policy`: 19 passed. The second quick gate
  reached all 344 selected tests and found three more legacy identity/ambiguity
  assertions in PM-policy integration tests. These now assert workspace binding,
  authenticate with its empty project identity, and prove an actual PM launch with
  three approved projects succeeds; missing assets and missing approval still refuse.
- `git diff --check`: passed.

The red log was obtained by temporarily restoring the old single-project refusal,
project-bound recovery guard and unapproved-scope message in the changed implementation.
All three acceptance regressions failed for their intended reasons. Those guards were
then removed again; this was a targeted replay of old behavior, not a checkout-wide
baseline test. Raw red/green logs are attached separately.

All Rust PM checks use fake engine/credential fixtures and disposable processes, not
the owner's live herdr session or a paid model. A real owner PM restart/install remains
a post-landing PM obligation and was not performed by this worker. No load test ran.
The receipted quick gate log is attached by Corral separately.
