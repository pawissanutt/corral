# Pi-native worker comparison: not yet measured

T-313 / T-252@P5, 2026-10-26. **Live comparison is in progress; no pair is complete.**
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

| Source | Private OmO / Pi cards (project) | Size | Randomized order | Expected pair tokens |
| --- | --- | --- | --- | ---: |
| T-248 | T-001 / T-007 (p01) | s | OmO, Pi | 26,450,204 |
| T-249 | T-008 / T-002 (p02) | s | Pi, OmO | 26,450,204 |
| T-244 | T-003 / T-009 (p03) | m | OmO, Pi | 60,872,940 |
| T-245 | T-004 / T-010 (p04) | m | OmO, Pi | 60,872,940 |
| T-246 | T-011 / T-005 (p05) | m | Pi, OmO | 60,872,940 |
| T-180 | T-006 / T-012 (p06) | l | OmO, Pi | 125,133,604 |

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

The first OmO candidate was independently approved after two review rounds.
Raw capture measures 18,136,378 worker-plus-review tokens, zero Needs me,
zero rescues and zero recorded silent endings; two of its three quick gates
passed. This is one arm, not an aggregate comparison result.

Two observed lifecycle details required correcting the offline validator:
approval selects the frozen full landing tier rather than the admission-time
quick review tier; and a discarded card cannot be reopened through `move`.
The tool now hashes both raw gate stages and rejects unrelated plan drift,
and binds distinct private task/run tuples to the original `source_task`.
The exact second-card aliases were attached before Pi inference. Source
specifications, cohort, base, seed/order, profiles, binary/config hashes and
budgets did not change. Both regression cases pass in the 26-test offline suite.

Pi's T-007/R1 admission at 2026-10-26T04:37:34.516146999Z exited before
binding a session or starting a model turn. Its canonical launch log reports
`credentials_unavailable`: the native launcher used the private server's
XDG data directory, unlike the preflight client's configured environment.
The credential guard rejected a direct auth-directory symlink, which was
removed without touching the original credentials. Process metadata then
confirmed that only the exited host's shell remained. Its launch environment
was corrected to use the original corral-owned data root; the plugin-free
server's environment was not changed. The frozen binary refused fenced recovery
with `op_unavailable: corral agent resume lands with T-252 child recovery`.
The failed private card was closed with its branch and raw failure retained;
no bootstrap was replayed and no extra dispatch was silently substituted.
Completing that pair now needs an owner-approved replacement allowance.
This setup failure, Needs me event and recovery must remain in the evidence;
they cannot be silently removed or attributed to worker implementation quality.

A new private-shell startup setting resolves the original corral-owned data
root while leaving herdr's server environment isolated. A disposable fresh pane
verified every doctor boolean, including credential presence, against its actual
environment; it was then closed. Dispatch 3/12, the Pi-first T-249 card
T-002/R1, now has a healthy generation-1 binding and a canonical first turn.
The other five pairs continue within the original budget while the first pair
remains explicitly incomplete.

The native T-249 arm was independently approved after two review rounds at
2026-10-26T05:39:18.182810382Z. Its captured worker-plus-review totals are
9,731,132 tokens and USD 1.8148016 API-equivalent cost, with zero recorded
rescues, Needs me events or silent endings. Two of three quick gates passed.
The canonical waiting-external turns resumed through gate wakes, and the
review correction arrived through the native inbox in the same generation.
Its candidate branch and complete raw captures are retained. The matched OmO
T-008/R1 arm started at 2026-10-26T05:46:11.474693667Z from the same unchanged
p02 main, binary/configuration, specification, model and medium effort.
This is dispatch 4/12; the single-arm measurements are not an aggregate verdict.

Remaining acceptance work is the six controlled pairs and their independent
reviews, raw show/event/identity captures, before/after plan measurements,
and the final computed measured report against the owner thresholds.

Pass bar, in aggregate: Pi has no more Needs me or silent endings than OmO,
review rounds are within +1, API-equivalent cost is within 1.25x, and gate pass
rate is no lower. No default switch follows automatically from any outcome.
