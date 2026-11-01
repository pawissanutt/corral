# T-582 isolated QA policy repair

## Changes

Ten CLI-based QA journeys now rely on the fresh Assist default instead of the
refused `config set review.autopilot false` operation. Planner-review confirms
an exact single-task preview with dispatch authority before its automatic sync
regression check. Gallery likewise confirms only its disposable dashboard task.
Plan-gate cleanup now uses explicit owner approval (`--owner-ok`) rather than
assuming `--force` bypasses owner-idea drop protection. No production code,
review policy, gate tier, or landing behavior is changed.

## Candidate and isolation

Built with `cargo build -p corral` in this worktree. Candidate:
`target/debug/corral` (not the older sibling repository target). The initial
four probes accidentally used the sibling target; those are not acceptance
checks. All results below use this worktree's candidate.

All ten affected journeys were executed. Non-native journeys used fresh
`scripts/test-herdr.sh` servers, private HOME/XDG, and `plugin list` readbacks
reporting **No plugins installed**. Native journeys own the same helper through
their built-in wrappers. The wrappers' private servers and directories were
removed; no owner server or board was used. Raw output is in `.evidence/*.log`;
per-command transcripts and scenario receipts are in the journeys' evidence
folders. The policy-red probe independently reproduced the refused legacy write.

## Passing journeys

- planner-review: 11 checks, including confirmed finite dispatch grant and
  cancelled-plan identity preservation across automatic sync.
- planner-lifecycle: 15 checks.
- review-family: 20 checks.
- task-overview: passes at both 80 and 120 columns.

## Remaining blockers (assertions preserved)

- plan-gate: `missing` expects dispatch `--queue` to refuse a required-but-missing
  plan without side effects. Current dispatch deliberately selects the planning
  pipeline when no approved plan exists (`plan_dispatch::needed`), so this
  operation succeeds. The assertion cannot pass by merely replacing the policy
  setup. Later plan-gate scenarios remain unverified. Its cleanup receipt passes.
- gallery: the finite grant is confirmed, but dashboard configuration sets
  `dispatch_hold = true` on a never-run task. Automatic dispatch is therefore
  blocked even with the grant. The first-run assertion fails. No hold or policy
  validation has been bypassed to hide this.
- stages: policy's new attention banner contains `T-001`; the old mouse helper
  picks its first occurrence instead of the task card and opens Automation
  settings. Timeout at `03-review-preview`; existing assertions preserved.
- task-history: CLI timeline attribution and landing assertions pass. The UI
  wait still expects old `7 Timeline` text; current UI shows `[7] history` and
  opens Overview by default. Timeout at `09-spec`.
- t170-native-question: the installed hardcoded runtime is gone. Invoked with
  `T170_RUNTIME=/home/pawis/.omo/agent/runtime/b745ed833fc53929-f176205d0bda/dist`.
  Native initialization passes; reply refuses an acknowledgement identity
  mismatch. Private server and HOME teardown pass.
- t196-ordinary-reply: same available runtime via `T196_RUNTIME`. Native
  initialization passes; deadline waiting for real terminal user message.
  Private server, HOME and native-process teardown pass.

Owner answer to Q1 authorizes the supported fixture changes, requires every
substantive assertion to remain unchanged, and explicitly directs leaving the
known-drift journeys failing rather than chasing UI timeouts or changing
assertions to match new behavior. The PM will file follow-ups from this list.
No skips have been added. In particular, gallery still fails after its exact
finite dispatch grant because dashboard setup puts the task on dispatch hold;
without that grant, Assist independently refuses automatic dispatch.

Static TOML-only latency/route fixtures also contain legacy
`autopilot = false`; they do not call the refused setter or grant authority and
have not been changed or run as part of these ten journey repairs. These
compatibility values do not override the fresh Assist operating policy.
