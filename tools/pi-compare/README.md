# Pi worker comparison

Offline, read-only Bun tool for T-313 / T-252 OD9. It does not dispatch a run,
contact a provider, collect credentials, change a profile, or change a host default.

```sh
cd tools/pi-compare
bun install --frozen-lockfile
bun run typecheck
bun test
bun src/main.ts test/fixtures/manifest.json
bun src/main.ts test/fixtures/manifest.json --markdown
```

The committed fixture is **synthetic**, incomplete, and never evidence that Pi
passed the comparison. Those last two commands print a report and exit **2**.
Exit 0 means a complete, valid, measured comparison met every owner threshold;
exit 2 means no pass (incomplete, synthetic, invalid, budget stop, or threshold
failure); exit 1 means the input could not be parsed or measured.

## Capture contract

Before any live pair, preregister the seed, six task IDs, expected **pair** tokens,
and arm ordering in a manifest with the shape in `test/fixtures/manifest.json`.
Do not choose the seed after seeing results. `assignedOrder(seed, pairID)` in
`src/schema.ts` assigns order from the low bit of the first SHA-256 byte of
`seed + "\n" + pairID`. Record the selection-time Backlog readback as each pair's
`selection`. Required mix: two xs/s, three m, one l, and at least two task kinds.
`source_task` binds that original Backlog readback's ID. Private arm cards may
have different IDs and the same local run ordinal; each task/run tuple must be
distinct, and both copies must retain the source body, size and kind exactly.

The owner budget remains subscription-only, $0 paid API, twelve worker runs
maximum, and three review rounds per run maximum. Review sessions are accounting
children, not extra worker dispatches. Expected pair tokens must be recorded
**before** starting the pair; stop execution and report above twice that value.
This post-processing tool detects the excess in captured data; it is not a live
budget enforcer. The operator must monitor usage during execution.

Each arm names a raw admission `corral show TASK --json` capture (`start_show`),
an independently approved final capture (`show`), its selected run ID,
and the **complete** `corral log TASK --all --json` output. Plain `log --json`
filters noise, including question and inbox events, and is insufficient.
Set `event_surface` to `log_all` for those timeline rows. The parser also accepts
raw Event arrays or NDJSON (`event_surface: export_events`) for the declared
storage-v2 event-export surface. It never opens `.corral/events.jsonl` directly.
The installed binary in the initial T-313 preflight has no export subcommand;
actual post-cutover export compatibility is not claimed by the offline tests.

Paths are relative to the manifest. Inputs must be regular files of at most
32 MiB each. Keep raw captures as artifacts; the report records their SHA-256
digests and the manifest digest so inputs can be checked later.

Record `before` and `after` identity attestations for **each** arm:

- Hash the actual executable bytes of the absolute corral binary used.
- Hash all effective config inputs consistently, including project overrides.
  Retain the method and paths, not credential bytes, in the live evidence.
- Hash the `show.gate_contract` value with this tool's canonical JSON function.
  This comparison digest is not a replacement for corral's gate receipt hash.
- Record the same independent reviewer profile, OmO PM profile, and occupied
  slot count. PM usage is excluded; the PM is not evaluated.

Binary/config/reviewer/PM/slot attestations must match throughout. Gate selection
hashes must match across arms at the corresponding admission and approved stages,
and match those raw readbacks. An unchanged tiered plan may select quick review
at admission and full landing after approval; destination, scope, tests and other
fixed contract fields cannot change. Other selection transitions are rejected.
The tool checks the raw admission and final task/spec, base SHA,
model route (`chatgpt-subscription/gpt-6.1-sol` versus
`openai-codex/gpt-6.1-sol`), effort, gate contract, review child profile, seeded
order and Backlog selection. It cannot reconstruct historical binary/config
changes or PM identity from a later show capture. The attestations and equal-load
operational evidence must therefore be captured during the experiment, not
filled in retrospectively. Run the two arms from the same base, review both
independently, land only the selected candidate, and discard the other after
review. This tool does not authorize those board or Git actions.

## Metric definitions

- Review rounds are recorded verdict events, cross-checked against independent
  review children. Missing approval or reviewer usage refuses measurement.
  Wall time ends at the first approve verdict, not at `done` or landing.
- Worker and independent-review usage are summed, using only `estimated_usd`
  for API-equivalent cost. `reported_usd` stays separate. Missing pricing stays
  null and invalidates the pair; unknown tokens never become zero-dollar work.
  PM sessions and `total_usage` are deliberately not added.
- Request-backed event replays are deduplicated by task/kind/request ID.
  Question events use task/kind/key/revision/action timestamp; other byte-equivalent
  canonical events are deduplicated. Never count a duplicate raw log line twice.
- Needs me counts deduplicated events; duration is the union of open intervals,
  closing on the matching answered decisions or canonical resume/done/approval.
  Failed replies and outstanding decisions do not falsely close an interval.
- Rescues count effective `replied`, `message_retried`, manual
  `inbox_recovered`, manual terminal message marks, `unstuck`, audited reply
  retries/marks, manual observed receipts, and PM comments whose text is a JSON
  audit object containing `"comparison_rescue": "pane_input"`. Record that
  comment immediately after each PM pane intervention, bound to the task and
  run. Arbitrary comment prose is never guessed to mean a rescue.
- Pi silent endings come from canonical `agent_host.turns` classes `no_report`
  and `no_verdict`, including independent review children when present. OmO
  uses explicit `idle_no_report` warning events. These are operationally
  different detection surfaces, not claims of equal turn instrumentation;
  the report records `no_report_source`. Native evicted turn history refuses
  measurement instead of silently undercounting.
- Gates pair `gate_started` with `gate` by attempt, or the run's latest open
  legacy start. Missing tiers mean full. Duration, pass count and stale-result /
  invalid-command-receipt / command-failure classes are preserved; missing
  starts or unfinished gates refuse measurement.

Aggregate owner thresholds are: no more Needs me or silent endings than OmO;
review rounds within +1; API-equivalent cost at most 1.25x; gate pass rate no
lower. All six valid measured pairs are necessary. A pass is evidence for a
separate owner verdict, never permission to switch a default.
