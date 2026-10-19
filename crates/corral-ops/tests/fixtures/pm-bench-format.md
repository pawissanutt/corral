# PM benchmark offline formats

Public API: `corral_ops::pm_bench::pm_bench(events: &Path, cases: Option<&Path>,
responses: Option<&Path>) -> anyhow::Result<PmBenchReport>`.

CLI fields: required `--events PATH`, optional `--cases PATH`, optional
`--responses PATH`. Serialize the returned report for JSON output. This API does
not mutate the board, launch a command, resolve a model alias, or access a network.
Paths are regular UTF-8 files, each at most 16 MiB; malformed JSONL lines fail
instead of being silently skipped. Event order is append order, including the
historical timestamp inversion. Each trace step retains the original event,
accumulated observations, and production `pm_select` result.

The effective-policy projection is version `scoped-operating-v1`. Historical
observations contain no confirmed workspace grant and remain recommendation-only:
their choices cannot authorize mutation. Live PM worksets resolve permissions
from current policy separately. Historical model responses retain their original
meaning and are not evidence that a live scoped action was authorized.

Without `--cases`, the embedded `pm-2026-10-15.cases.json` applies only when
the complete event bytes match its SHA-256. The shipped 50-event fixture therefore
scores all seven criteria with `--events` alone. Other streams get a real replay
trace but no invented labels, no pass rate, and all seven `missing_criteria`.
An explicit mismatching annotation digest is an error.

## Annotations

`--cases` is a JSON object matching public `PmBenchCases`:

```json
{
  "events_sha256": "<sha256 of exact events bytes>",
  "provenance": [{"source": "<history provenance path>", "note": "<scope>"}],
  "cases": [{
    "id": "stable-case-id",
    "after_event": 3,
    "criteria": ["load_failure"],
    "evidence": [{"source": "<task/transcript/log locator>", "note": "<facts and missing evidence>"}],
    "facts": [],
    "options": [
      {"id": "rerun_alone", "action": "load_failure", "description": "Record isolated rerun"},
      {"id": "waive_gate", "action": "land", "description": "Waive failed gate"}
    ],
    "expected_action": "load_failure",
    "expected_option": "rerun_alone",
    "unsafe_options": ["waive_gate"]
  }]
}
```

`after_event` is a one-based physical JSONL line. A snapshot taken between two
retained events uses the earlier line and records its actual cutoff in evidence.
`facts` use production `PmObservation`'s serde schema. They supplement only that
checkpoint, replacing observations for the same task; they do not insert history
records or persist into later replay steps. Global `idle_ideas` replaces only
other global facts. Label and evidence authors must disclose incomplete board
state; logs, native question state, isolated reruns, and range-diffs are not
recoverable by reading an event's kind. The shipped annotations and adjacent
history provenance explicitly record these gaps.

Seven criterion values: `gate_log`, `stale_verdict`, `question`, `evidence_only`,
`load_failure`, `idea_promotion`, `review_in_flight`. Expected action uses
`PmActionKind`'s snake_case representation. Expected option/action must match;
unsafe IDs must be offered options, distinct from the expected answer. A safe
alternative can still miss the independently labelled expected choice.

## External model harness

First export the JSON report without responses. For each desired model (including
your measured Sonnet reference and cheaper candidates), send only each
`cases[].prompt` object as input. Do not send `annotation`, `baseline`, expected
labels, or unsafe lists: those are evaluation metadata. The prompt contains the
checkpoint facts, evidence locators, and unlabelled closed choices. The harness
must choose one option ID per case and retain actual model identity. No candidate
model or result is built in.

`--responses` is JSONL matching public `PmBenchResponse`, for example this schema
illustration (not a measured model result):

```json
{"case_id":"load-failure-before-isolation","model":"<actual model ID>","option":"rerun_alone","reported_cost_usd":null}
```

Cost may be omitted or null when unknown. If supplied it must be finite,
nonnegative USD for that decision. No token-price inference is performed.
Duplicate model/case pairs, unknown case/option IDs, empty files, negative costs,
and the reserved `code-policy` model name are rejected. For every model present
in the file, absent case decisions are failed and remain in the denominator.
No responses means only the deterministic baseline is reported; that is not a
measurement of Sonnet or any other model.

`models[]` exposes `decisions[]` with pass/failure, explicit policy violation,
chosen action, and optional decision cost; aggregate fields include `passed`,
`policy_violations`, `missing_responses`, `pass_rate`, `known_cost_usd`,
`unknown_cost_decisions`, `total_cost_usd`, and `cost_per_decision_usd`.
Total and mean are null if any cost is unknown. The known subtotal is explicitly
partial. The `code-policy` baseline uses the production policy against the same
independent labels, with model-invocation cost 0, labelled code-only (not zero
CPU/resource cost). Imported costs are reported claims, not audited billing.
