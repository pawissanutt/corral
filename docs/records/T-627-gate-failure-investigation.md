# T-627: why did Pi-native workers fail the gate more often?

## Answer

The records support **missed check coverage and unrelated test failures**, but do not establish that the harness caused the reported 29% versus 19% difference. The failure mix contains many assertions whose ownership cannot be inferred from a log. Retries cluster heavily in a few tasks, and profiles differ in reasoning effort, task mix, timing, and gate tier. The original 42/157 statistics are not a matched experiment. The recommended cheap fixes are:

1. **Make validation match the changed surface:** T-579 ran a resolver-specific formatter yet its gate found formatting in that resolver; T-567 ran targeted capability tests but its all-targets Clippy found items after a test module in a changed file. Add a skill line: “After the final rebase, check formatting and all-targets Clippy for changed Rust packages, including nested manifests, where the run contract permits; record completed exit status.” Never run an extra gate contrary to the contract. These examples are not evidence that this is Pi-specific.
2. **Reduce unrelated failures and distinguish them in metrics:** T-491 hit the separately fixed T-616 capture-window failure and twice hit a main-branch pane-inventory regression; T-523 hit the parked-wake case addressed by T-526. Retain exact signature/fix-revision annotations in gate results, not blanket automatic flake labels; report first submission separately from retries. This is a demonstrated source of noise, not an estimate of the gap after all exclusions.
3. **Make submissions auditable:** record lifecycle purpose (worker submission, member, landing), worker profile and effort, failure signature, candidate and ordered completed check/exit-status events in the gate receipt. This avoids treating `ag-*` as one lifecycle or counting retries as independent runs, and makes interrupted builds observable. Compare matched size/period/tier and effort before changing a host default.

## Cohort and method

On October 30, enumerate YAML task frontmatter under `/home/pawis/miti-workspace/.corral/tasks`; select the worker run's `execution.resolved.profile` exactly `pi-sol` or `gpt-sol`, not reviewer profiles. Join task ID and run ID to **all** schema-v2 failed receipts, numerical and `ag-*`, under `/home/pawis/miti-workspace/.corral/state/gates`; read sibling `.log` for the decisive failure. The earliest observed pi-sol run is T-478 R1 at 2026-10-27 15:01:23 UTC, used as the start proxy; freeze at 2026-10-30 19:00 UTC. Include receipts started before the run ended (or the freeze when still open), and show member gates separately. This yields 42 pi-sol and 14 gpt-sol recorded worker runs, including open/non-submitted runs; these **are not** the historical 42/157 denominators. T-498 is another project and is retained because its resolved worker profile and gate match the selection.

A `policy:review_rounds` receipt is a review-round worker gate, `member-gate:*` a Plan member gate. `legacy` below means absent `identity.gate_tier.source`; the receipt alone does not prove worker submission versus a pre-done full/landing check. No receipt after recorded run end is counted. An `ag-*` prefix is not a lifecycle classification. Each row is one failing **attempt**, not one independent task; one receipt with multiple test failures remains one row. Do not divide these counts by all runs to claim a comparable rate. Review-round retries and legacy/member attempts are visible for audit rather than silently mixed into a first-submission rate.

## Results by evidence class

| Class | pi-sol | gpt-sol |
|---|---:|---:|
| Formatting / Clippy | 1 | 3 |
| Compile, non-Clippy | 0 | 1 |
| Real test failure on touched code | 1 | 0 |
| Confirmed unrelated test/regression | 2 | 0 |
| Gate infrastructure | 0 | 0 |
| Other gate check: size ceiling | 1 | 0 |
| Unknown assertion, docs or environment cause | 20 | 10 |
| Excluded specified fixed-flake signature | 3 | 0 |
| **All failed attempts** | **28** | **14** |

Unknown is deliberate: a test assertion or timeout does not alone prove touched-code regression, unrelatedness, or gate infrastructure. In particular no infrastructure classification is inferred from a test timeout. The T-526 test failure is on `crates/corral/tests/dispatch_queue/parked_wake.rs`, changed in its receipt's `git diff integration_base head`; it is a real failed test on touched code, not proof the candidate caused the underlying race. T-491's two unrelated pane failures are documented in its task PM comment as a main regression fixed by `89275b6b`. All other test assertions remain unknown unless separately proved.

### Receipt-level ledger

All log basenames are `<task>-<run>-<attempt>.log` in the gate directory; same prefix `.json` is the receipt. `P` = `policy:review_rounds`; `M` = `member-gate:T-339@1`; `L` = legacy/unresolved lifecycle. Short numeric attempts below include their complete first component; it uniquely identifies the full receipt under that task/run. All rows R1 except T-540 R3.

| Profile / task | Attempt(s), source | Classification and first decisive failure |
|---|---|---|
| pi T-241 | ag-f0a4217980013ab6 P | unknown: effects gate fence test |
| pi T-470 | ag-7f1fe5013186ae0c M | unknown: migration transcript retention |
| pi T-470 | ag-707088157a0dd211 M | unknown: owner-post race |
| pi T-470 | ag-fa590c7b93ff292e M | **excluded T-624**: operating-launch interrupted stage |
| pi T-470 | ag-80bb8eebcec3c1ff M | **excluded T-624**: approval-busy readiness |
| pi T-483 | 400478 L | unknown: pi-lane assertion |
| pi T-485 | ag-3da781e05320c222 P | unknown: pi-lane assertion |
| pi T-491 | ag-17de9a1120eb03e3 P | unknown: shipped-worker-skill consistency |
| pi T-491 | 457673 L | **excluded T-616**: `capture_outside_attempt` |
| pi T-491 | 2854894 L | unknown: build/docs check |
| pi T-491 | 161037, 1783922 L | unrelated: pane-inventory regression, two attempts |
| pi T-491 | ag-002ac17be9eff2ed L | unknown: filing/strace assertion |
| pi T-498 | ag-56e5c4e61fbd18ec L | unknown: e2e lookup and runner assertions |
| pi T-502 | 3171865 P | unknown: session ownership scan budget |
| pi T-502 | ag-ad71943898a2797c P | unknown: gate reservation headroom |
| pi T-502 | 4168478 L | unknown: sync-step inventory assertion |
| pi T-519 | 3935087 P | unknown: `approve_fast_forward`; worker reports unrelated timestamp flake, not independently proven |
| pi T-523 | 3132137 L | unknown: parked-wake receipt; worker reports unrelated and files T-526 |
| pi T-526 | ag-41a3bd8b8fab8d22 P | touched test: parked-wake; see diff and session below |
| pi T-571 | ag-d8b92e2710dd5b4b P | unknown: pi-lane assertion |
| pi T-579 | ag-8d99e8734a9e5c7e P | formatting: resolver manifest |
| pi T-583 | ag-31fc30186737d43a, ag-c3121af33186ea1d P | unknown: prompt install assertion, then pi-lane assertion |
| pi T-591 | ag-846b6e4e9bdd68a5 M | size ceiling |
| pi T-591 | ag-17c1bc991e54c8da, ag-2dcf3744dcad83ff, ag-9192f8357edefd8d M | unknown: importance, progress and action-budget assertions |
| gpt T-540 R3 | 1332842 P | Clippy: clone-to-slice lint |
| gpt T-540 R3 | 1891787 P | unknown: aggregate-live reviewer assertion |
| gpt T-540 R3 | 1048182 L | compile: macOS all-targets Linux-only fixture import and inference errors |
| gpt T-550 | 3321394 P | formatting |
| gpt T-550 | 3483560, 3814971, 1637173, 3434123 P | unknown: search, PM next, TUI parity, obligations/overview assertions |
| gpt T-550 | 3708642, 1848971 L | unknown: work-budget test, two attempts |
| gpt T-552 | 2595317 P | unknown: attention crash-replay fixture assertion |
| gpt T-562 | 1508551 P | unknown: pi-lane logout timeouts |
| gpt T-567 | 578114 P | Clippy: items after test module in changed `capabilities.rs` |
| gpt T-568 | 1020007 P | unknown: pi-lane fresh-process capability assertion |

The two member runs T-470 and T-591 contribute eight pi attempts, so excluding member attempts leaves 20 pi and 14 gpt attempts, including the legacy ambiguity. Excluding only the three exact fixed signatures from all sources leaves 25 pi and 14 gpt attempts. These are attempt totals, not adjusted run failure rates.

## Six requested fixed-flake checks

Checked decisive failure names/messages in the ledger against the six task records, not merely task IDs. T-607 is fake-bun `ETXTBSY` (none of the above decisive signatures); T-610 is `decision_answer_replays_its_journaled_receipt_after_a_crash` (none exact; its task reports underlying cause **unverified**, so similar crash tests are not excluded); T-613 is browser navigate/screenshot startup timeout (none exact); T-616 is `plan_aggregate_cli` capture window (`capture_outside_attempt`: T-491 457673); T-621 is partial fake-corral `calls.jsonl` (none exact); T-624 specifically names `operating_launch` interrupted stages and approval-busy fixture (T-470 two receipts above). T-624 is a wider sweep, not a license to exclude every timeout. T-562 logout timeouts and T-552 crash-replay are uncertain, **retained**. Matching is limited to decisive log messages; a hidden secondary failure cannot be ruled out. See task records T-607, T-610, T-613, T-616, T-621 and T-624 in `.corral/tasks` and the receipt logs.

## Pre-submit behavior

Read the worker `session.transcript` JSONL from each run's metadata, ordering assistant tool calls by timestamp before its first failed receipt; line numbers below are in that transcript. OmO can delegate to child sessions under `~/.omo/agent/sessions`, so a parent-only command inventory is a lower bound. Calls are not proof of successful completion without matching tool results. Later retries may have additional checks; see the same transcript and ledger timestamps.

| Run | Recorded before first failure (line numbers in run transcript) |
|---|---|
| pi T-241 | focused `review_budget` cargo tests and fmt (49, 53, 63, 87). |
| pi T-470 | transcript, storage, recovery and CLI tests plus fmt (174–214). |
| pi T-483 | stress and task-cache tests and fmt check (26, 32). |
| pi T-485 | repeated `bun test ... recovery-tools.cross-lane.test.ts` (28–64). |
| pi T-491 | sandbox fixture work before first gate; no cargo test call established from parent transcript before first failure (38–62). |
| pi T-498 | `cargo test --locked --test context`, Clippy and quick/context tests (99–153); its local quick test is a test target, not the Corral quick gate tier. |
| pi T-502 | targeted `autopilot_ops` tests and fmt (131–180). |
| pi T-519 | focused nextest fixture (22, 26). |
| pi T-523 | `plan_acceptance::tests::membership` and fmt (59–83). |
| pi T-526 | sandbox attempt, then targeted `dispatch_queue` parked-wake tests and fmt (29, 33, 43). The failing receipt is the contracted review-round gate, not proof of a separate local quick gate. |
| pi T-571 | Rust auxiliary/capabilities and Pi auxiliary tests, fmt (315–380). |
| pi T-579 | resolver build, Bun capability test, resolver fmt and check (60, 92, 166, 174); gate still caught fmt. |
| pi T-583 | Pi guidance and Rust skill/config tests and fmt (88–118). |
| pi T-591 | targeted done/checkpoint tests and fmt (306–364). |
| gpt T-540 R3 | metered and Bedrock Rust tests, CLI build; authoritative quick gate (187, 281, 340, 429). |
| gpt T-550 | CLI/attention tests; authoritative quick gate (192, 353, 479, 607). |
| gpt T-552 | targeted nextest and timing checks; authoritative quick gate (394, 504, 626). |
| gpt T-562 | todo Rust and Pi checks, isolated Pi lane (60, 84, 158, 269). |
| gpt T-567 | MCP Bun tests, capability/recovery Rust tests; authoritative quick gate (296, 465, 516). No all-targets Clippy shown in those parent calls. |
| gpt T-568 | web Bun tests and authoritative quick gate (90–216). |

These calls demonstrate targeted prechecks on both harnesses. They do **not** establish how many separate local quick-tier gates were run or whether every long build completed: receipt gates are not an additional local precheck, and output/child-session completion has not been exhaustively linked per attempt. T-526's line 29 sandbox command was followed by process inspection and forced container cleanup (31–33); the record alone does not establish a general Pi build-cutoff pattern. T-567's recorded targeted tests did not prevent a Clippy-only failure. Session paths and tool results can be resolved through each worker run's frontmatter; do not infer an unrecorded command did not occur in a delegated session.

**Limitations:** source-less legacy lifecycle is unresolved; multiple attempts share candidates and tasks; class counts include member attempts and retain uncertain matches; no complete child-session/build-result audit or matched causal comparison is claimed. These limits prevent explaining the size of the original harness gap, but the ledger and concrete check failures justify the inexpensive instrumentation and validation improvements above. No product code was changed and no load was generated for this investigation.
