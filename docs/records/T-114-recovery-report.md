# T-114 recovery evidence — T-141 R1

## Outcome and boundary

This is the isolated evidence child of approved **T-114@P2** (plan SHA-256
`938c43b9b581665dabe8f76888a3ab98a7ab86bb28b985702df3b02be7091a45`).
Q1 confirms implementation scope; Q2 permits only the two slow-test registration
files in addition to the declared scope. No production recovery code, policy,
timeout, budget, or quarantine entry changed. Canonical decisions are attached as
`T-141:R1:8a604c75-scope-decisions.json`.

**Ten isolated CLI/TUI journeys passed.** N's ordinary separate same-model
review, current digest, genuine callback, owner approval and zero recovery records
are proved. **N `acceptance_mode=policy` and `policy_satisfied=true` are NOT
VERIFIED:** those readback fields do not exist. Q3 assigns them to **T-971**;
neither tests nor reports synthesize them or claim full N acceptance.

No live T-102/other owner-store recovery, provider call, credential activation,
or owner verdict was performed. Automated fixture-owner interactions use the
unchanged real local CLI/TUI ceremony, not an actor flag or manufactured token.
Inert reviewers prove routing and exact callback identity, not review quality.

## Executed candidate and primary evidence

Latest surface run: code `7f9165c6afdc43bf583e3fb074a4056109805ed5`, rebased onto
main `4d6e4850671484956da7d925ffa109ce673336c8`. The executable's embedded
build identity matches that code commit; SHA-256 is
`1db5979add70cf675893a2ed48f196b5ef885ed698073d9b4158d66250ab3e7b`.
The following commit adds only this final report. Gate/review receipts identify
the final submitted tree separately.

| Proof | Attached artifact |
| --- | --- |
| Candidate identity and executable hash | `T-141:R1:84c9b97b-rebased-candidate-identity.json` |
| All ten journeys, exit 0 | `T-141:R1:8bc23e11-qa-rebased.log` |
| H/N/F/M JSON, policy sources, generated callback argv/session, events, receipts, hashes, input transcripts and captures | `T-141:R1:9187b70e-recovery-qa-rebased.tar.gz` |
| Inspected 80x24 keyboard confirmation | `T-141:R1:3906d8ae-F-keyboard-latest-80x24.png` |
| Inspected 120x40 mouse acceptance confirmation | `T-141:R1:e1f1e2a9-F-mouse-latest-120x40.png` |

The archive indexes `N-cli`, `F-cli`, `M-ordinary`, `M-recovery`, and keyboard/mouse
N/F/M cases separately. Each contains `before.json`, `route.json`, `final.json`,
`history.json`, `commands.jsonl`, events and workspace-cleanup proof. Real PTY
captures retain raw ANSI, decoded cells, ANSI-color SVGs, geometry and actual input
transcripts. The two PNGs render those captured cells through the repository SVG
renderer; they are not TestBackend snapshots. Inspected controls remain reachable,
Cancel is focused, exceptions retain their original results and the sheet says
"not ordinary policy success". Mouse navigation/activation uses clicks and wheel;
reason/notes are typed. This is not a full accessibility or pixel-fidelity audit.

The runner starts a fresh plugin-free server via `scripts/test-herdr.sh`, with
private HOME/XDG, inert harnesses, canonical task operations and no task-file edits.
It freezes each fixture configuration before observation. Decoded original plan
blob/source hashes, configuration hashes and an unrelated task are unchanged.
Each case closes only its recorded workspaces; final inventory is empty and the
private root is removed. No owner session is borrowed or stopped.

## H — retained history, not a current failure requirement

`T-141:R1:12318bc0-index.md` and `af0123ff-T-102-retained.json` distinguish
stored evidence from reported observations. D4/P1 records planner Sol and its
historical separation refusal. Original P1 SHA-256 is
`31844cdc68c8251a564fd3c54c090f93032400b871c9623c7b768c46a386b682`.
The PM's 2026-10-17T02:49:31.961760863Z comment reports refused owner acceptance;
it is not raw subprocess output. The explicit-Astra digest trace is retained
T-114 source analysis, not a newly executed historical reproduction.

The exact T-113 R2 standalone report is attached as
`T-141:R1:64c8b834-T-113-astra-review-report.md`, SHA-256
`64c8b8340b1c75c2247d32c6c7cad0b9c1b27ce3ae14684b7c7f38be32b93532`.
Its recommendation is expressly not a canonical T-102 verdict or acceptance.
Later ordinary T-102 approval history is preserved, not reclassified. No
pre-T-133 workflow executable was run or assigned an invented hash.

## Scenario ledger

| Invariant | Executed proof and result |
| --- | --- |
| N ordinary same-model review | Built-in `require_cross_family=false`; project-configured Sol; separate Sol session, generated callback and owner acceptance succeed on CLI, keyboard and mouse. No recovery history. Missing two machine fields remain **NOT VERIFIED (T-971)**. |
| F current family block, not historical digest failure | Project `require_cross_family=true`; ordinary Sol and explicit Astra both refuse OpenAI model-family equality; no review attempt saved. Raw ordinary digest succeeds. Preview resolution failure is null and identifies family plus configured-binding predicates separately. |
| F exact exception continuity | Astra recovery, generated callback and separately confirmed acceptance succeed on all three surfaces. Both exceptions remain overridden, the real approve is retained, acceptance records `owner_recovery`/false, persistent reviewer/configuration stay unchanged and dispatch remains held. |
| M callback versus selection | Default false: explicit Astra launches, its actual callback escalates as unauthorized. T-111 task-local Astra selection before a fresh attempt succeeds ordinarily and preserves the escalation. Separate CLI/keyboard/mouse recovery overrides binding only; family remains satisfied. |
| Recovery-only model floor | Existing operation test rejects planner profile and normalized model alias; N proves the stronger recovery floor does not leak into ordinary review. |
| Waiver and completed code outcomes | Existing waiver/code/landing/external contracts pass: missing independent review stays absent, failed gates/reviews stay failed, missing/pending/stale receipts remain hard blockers and recovery provenance is linked rather than forged. These are fixture operations, not live landings. |
| Freshness and authority | Existing ops, three CLI tests and twelve TUI tests pass for changed effects/policy/bytes/candidates, redirected or managed authorization, stale confirmation and preserved input. Actual F-keyboard and M-mouse stale captures in the latest archive prove no operation was persisted until a new explicit confirmation. |
| Concurrent apply and process loss | One explicit slow test passes: accepted fake submission is held at a socket barrier; competing apply/reconcile refuse without mutation. Owned group loss leaves Applying/Unknown; identical retries submit nothing. The canonical PR1 callback resolves Applied with the original immutable intent and exact journal links. Launch count stays exactly one. |
| Resources and publication | Existing fixed-probe resource/planning tests preserve capacity/dependency/stage boundaries. Waiver/publication/reopen contracts and real F/M receipts retain parent/child dispatch holds; no implementation starts. No host load is generated. |
| Compatibility and immutable evidence | Eighteen core tests pass, including legacy/schema-2/schema-3 envelope preservation, future-store open/write refusal, recovery summary accuracy, WAL-sync/EIO failure propagation and a genuine old-reader refusal of the current schema-4 database. |

Current F error text refers to **model family**, including the routing-vendor
qualification; the raw error is retained. The old plan's vendor-only spelling is
not pinned as a current CLI wording requirement.

## Targeted checks and test cost

On code `fd0515cb6acf64c9e65efd21a1f758fdc869f0e3`/base `9b6b9e86`, before the
later app/project-protocol-only rebase (recovery/core/CLI/TUI input code unchanged):

| Command | Result / attached log |
| --- | --- |
| `cargo test -p corral-core --test recovery --test recovery_schema --test recovery_durability -- --nocapture` | **18 passed**; `4937dc97-core-tests-final.log` |
| `cargo nextest run --profile quick -p corral-ops --lib -E 'test(recovery::tests::)'` | **32 passed**, new slow case excluded; `8afec123-recovery-quick-final.log` |
| `cargo nextest run --profile slow -p corral-ops --lib -E 'test(=recovery::tests::concurrency::concurrent_apply_and_process_loss_never_replay_one_reviewer)'` | **1 passed, 1.740s**; `f438850e-recovery-slow-final.log`, JUnit `ff33a887-recovery-slow-final.xml` |
| `cargo test -p corral --test recovery_cli -- --nocapture` | **3 passed**; `5975be76-cli-tests-final.log` |
| `cargo test -p corral-tui --lib recovery::tests -- --nocapture` | **12 passed**; `f062087d-tui-tests-final.log` |
| `python3 scripts/test_gate_policy.py` | **10 passed**; `2dd6a880-gate-policy-final.log` |

Changed-file Rust diagnostics are clean after retrying an initial unlinked-file
result/indexing timeout. Rustfmt, Python/shell syntax and diff-whitespace checks
pass. Builds use repository Cargo configuration and the configured mbx pilot.

`CORRAL_SCHEMA2_WRITER` supplied a real executable linked to 68 hash-verified,
unchanged old-core files at `911486b4347027f522e155c69812405f10cb3759`;
the optional regression did not skip. Executable SHA-256 is
`a0c5ef30f936716cc21cebaabe376c012cfab789fa09234bbcecba5353bf1628`.
Source/build/provenance are attached as `6dc208d2-schema2_probe.rs`,
`621c47b9-schema2-build.log`, `0b080c3e-schema2-provenance.json`.
Fresh WAL traces are `451b3553-recovery-durability-final.tar.gz`.

**Test-cost justification:** latest ten opt-in real-terminal journeys took
**56.515s**, including setup and cleanup. Actual transport, repaint, local owner
ceremony and generated callbacks cannot be proved by TestBackend alone. Variants
remain in existing cheap unit/operation tests; this adds only **one 1.740s slow
registration**, not another routine end-to-end matrix. Q2 registration changes
no existing timeout, budget, catalog entry or gate tier.

## Remaining limits and completion receipts

H is retained only; real-model review quality and live recovery are not tested.
The single process-loss regression is not an exhaustive kill-at-every-boundary
matrix. A genuine already-open old-writer variant was not rerun; the fresh old
reader and current future-schema write fence were executed. N's missing typed
acceptance fields remain **NOT VERIFIED (T-971)**, with no full N acceptance claim.

The one authoritative quick gate, any final prebuilt rechecks and advisory review
receipts are attached separately on the final committed candidate. They are not
inferred from earlier logs. Corral owns the subsequent full landing gate; this
worker does not run a second tier or grant a board verdict.
