# Pi-native worker comparison: not yet measured

T-313 / T-252@P5, 2026-10-26. **No live comparison pair has started.**
There is no measured pass/fail verdict, and no host default has changed.
The PM is not evaluated; both comparison arms must retain the OmO PM.

## Owner controls

OD9 permits subscription runs only, $0 paid API, at most twelve worker runs
(six pairs), and at most three review rounds per run. Stop and report when a
pair exceeds twice its preregistered expected token count. Required task mix:
two xs/s, three m, one l, from real Backlog work with mixed kinds.

Each pair must freeze the source task/spec, base SHA, gpt-6.1-sol model and
effort, gate contract/tier, independent reviewer profile, OmO PM, executable
digest, effective config digest and slot load. Randomize arm order before
execution. A changed executable or config invalidates a pair. Review both
arms independently, land one candidate, and discard the other after review.

The owner amendment of 2026-10-26 permits T-313 to start before T-297 closes;
it does not waive any of these controls or change T-297.

## Offline deliverable

`tools/pi-compare/` parses canonical show readbacks and complete timeline or
raw event captures. It computes per-arm and aggregate review rounds, rescues,
Needs me counts/duration, silent endings, PM versus owner question answers,
gate counts/results/durations, API-equivalent cost, tokens and wall time to
approval. It preserves raw-input hashes and refuses incomplete measurement
instead of inventing zero values.

The committed raw fixtures are explicitly synthetic. Their results are not
live comparison results and do not satisfy the six-pair acceptance criterion.
The tool does not start or authorize live runs. Capture procedure, manifest
shape, exit statuses and metric limitations are in `tools/pi-compare/README.md`.

Offline checks passed: frozen Bun installation, strict TypeScript checking,
and 24 deterministic tests across two files, including the actual CLI. The
committed fixture prints `data_kind: synthetic`, `outcome: not_evaluated`
and exits 2 as designed. The retained check log and synthetic CLI output are
separate artifacts, not a corral gate receipt or live acceptance evidence.
Editor diagnostics for the source directory were clean; individual test-file
diagnostic requests timed out, while the complete compiler check passed.

## Live status and remaining evidence

The first preflight reported no installed Pi assets or corral-owned
credentials. A later preflight in this session reported a managed GO
installation, all three roles ready and credential metadata present. No
credential contents were read, copied or attached. Credential presence alone
does not prove provider subscription coverage or authorize a live pair.

The PM requested that live pairs remain held while offline work proceeds.
Remaining acceptance work is the six controlled pairs and their independent
reviews, raw show/event/identity captures, before/after plan measurements,
and the final computed measured report against the owner thresholds.

Pass bar, in aggregate: Pi has no more Needs me or silent endings than OmO,
review rounds are within +1, API-equivalent cost is within 1.25x, and gate pass
rate is no lower. No default switch follows automatically from any outcome.
