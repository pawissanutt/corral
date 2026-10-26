# Pi-native worker comparison: not yet measured

T-313 / T-252@P5, 2026-10-26. **The first pair is running; no pair is complete.**
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

The owner login decision is resolved by the accepted R1 receipt
`owner-T313-pi-login-20261026T023138`. Its answer explicitly resumes the
existing comparison contract and retains every OD9 restriction. The worker
read the complete receipt and independently reran doctor; all Pi checks
were ready. Login confirmation is no longer a blocker.

The owner selected **Private clones (Recommended)**. Accepted receipt
`owner-T313-private-clones-20261026` confirms that independently reviewed
clone-only candidate landings satisfy this comparison. The private store has
its own plugin-free server and six repositories pinned to
`d4f1cd444cc9697b9bc9d239dcc1f6f07790f1e4`. Original source cards, main,
configuration and session binding remain untouched.

Preregistration was captured at 2026-10-26T03:43:31.892Z and attached before
worker inference. The cohort uses unchanged Backlog bodies:

| Source | Private task/project | Size | Randomized order | Expected pair tokens |
| --- | --- | --- | --- | ---: |
| T-248 | T-001 / p01 | s | OmO, Pi | 26,450,204 |
| T-249 | T-002 / p02 | s | Pi, OmO | 26,450,204 |
| T-244 | T-003 / p03 | m | OmO, Pi | 60,872,940 |
| T-245 | T-004 / p04 | m | OmO, Pi | 60,872,940 |
| T-246 | T-005 / p05 | m | Pi, OmO | 60,872,940 |
| T-180 | T-006 / p06 | l | OmO, Pi | 125,133,604 |

The pair estimate is twice the canonical historical median worker-plus-review
tokens per run at each size (49 s samples, 82 m, 49 l), measured before live
calls. The stop threshold remains twice the registered pair estimate. The
random seed is `3cef08e6f8e0c714d4d2dcd53f1eb598`; the preregistration records
source-body/readback hashes, binary/config hashes and the fixed medium effort,
gpt-6.1-sol routes, independent OmO reviewer and common OmO PM. No Pi PM is
launched or evaluated.

The private-context doctor passed all Pi checks. Both profile routes resolve
to gpt-6.1-sol, medium effort and `comparison-review`; Pi model discovery is
explicitly unavailable, so actual provider execution is still live evidence
to collect, not something inferred from doctor. A directory notification
observer reads canonical public task data and enforces token/run/review caps;
it does not parse pane or transcript text for control.

Worker 1 of 12 was admitted at 2026-10-26T03:53:50.744095596Z: the OmO arm
of source T-248, private T-001/R1 in p01. Its actual readback matches the
comparison parser and records the frozen base, subscription model, medium
effort and quick review GateContract. Canonical usage collection is active
and priced, not inferred from a screen. The private baseline worker and common
OmO PM use their normal harness credential store; Pi uses only its own
corral-owned credentials. No credential file was copied.

Remaining acceptance work is the six controlled pairs and their independent
reviews, raw show/event/identity captures, before/after plan measurements,
and the final computed measured report against the owner thresholds.

Pass bar, in aggregate: Pi has no more Needs me or silent endings than OmO,
review rounds are within +1, API-equivalent cost is within 1.25x, and gate pass
rate is no lower. No default switch follows automatically from any outcome.
