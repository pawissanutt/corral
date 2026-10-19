# Profile selection

Proposers and the PM choose a configured profile from task meaning. Corral supplies
current guidance and validates the structured Auto receipt; these operations make
no model calls, contact no terminal engine, and do not dispatch.

The authoritative **Profile selector procedure (v1)** lives in
`crates/corral-ops/assets/pm-runbook.md`. Both generated PM prompts and the installed
task-filing skill embed that same asset. Use that procedure rather than maintaining
a second selector checklist. Existing PM panes need the updated instructions supplied
explicitly; installing a new binary does not rewrite their conversation.

## Commands and output

```sh
corral selection catalog T-001 --role implementation --json
corral selection catalog T-001 --role planning --json
corral selection save --file selection.json --json
corral route T-001 --json
```

Selection commands emit JSON by default; `--json` is accepted explicitly. Failures
emit a JSON object with `command` and `error` on stderr and a nonzero exit status.
The save input must be an ordinary regular JSON file, not a symlink, device or pipe,
bounded by the workspace's `artifacts.max_bytes`. Stdin is never read.

The catalog includes the full task, task-spec/catalog/config revision identities,
the requested role, complete eligible profile metadata (including custom guidance),
and a separate `excluded` map of reasons. Eligibility checks role, project, enabled
and Auto status. Supported harness/model/effort configuration and candidate/fallback
metadata constraints are checked in code. Legacy profiles have `metadata: null` and
`profile_revision: "legacy"`; no guidance or evidence is invented for them.
Existing local authentication/settings and usable discovery-cache observations exclude
known unavailable models without probing providers. Missing observations do not prove access.
`access_status: "not_checked"` explicitly means no account/availability/execution
verification was performed. This read is not authorization to spend or launch.

## Receipt shape

Populate identities from the current catalog, not the illustrative values below:

```json
{
  "version": 1,
  "id": "T-001-implementation-1",
  "task": "T-001",
  "actor": "proposer",
  "selected_at": "2026-10-19T00:00:00Z",
  "role": "implementation",
  "profile": "database",
  "profile_revision": "database-r1",
  "spec_revision": "<catalog spec_revision>",
  "catalog_revision": "<catalog catalog_revision>",
  "reason": "Transaction rollback correctness is the acceptance target.",
  "closest_alternative": "general",
  "distinction": "Persistence invariants dominate over ordinary pattern-following.",
  "review": null
}
```

`actor` is `proposer` or `pm`; saving never rewrites proposer provenance. An
alternative must be another eligible profile and requires a nonempty distinction.
Use null for both alternative fields when no relevant overlap exists. Review-role
and PM-role saves are unsupported; reviewer integration needs trusted attempt,
candidate and policy context supplied separately by T-154.

Save returns the validated receipt. Identical current redelivery is idempotent;
conflicting IDs, stale revisions, unsupported configuration and ineligible choices
are rejected. Historical receipts remain in the store but cannot be resubmitted as
current choices after relevant changes. Saving does not edit task profile pins,
operating authority or configuration. Use `route` to inspect precedence rather than
assuming the saved Auto choice overrides an owner rule or explicit choice.
