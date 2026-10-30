# T-627: why did Pi-native workers fail the gate more often?

## Answer

The records support **missed check coverage and unrelated test failures**, but do not establish that the harness caused the reported 29% versus 19% difference. The failure mix contains many assertions whose ownership cannot be inferred from a log. Retries cluster heavily in a few tasks, and profiles differ in reasoning effort, task mix, timing, and gate tier. The original 42/157 statistics are not a matched experiment. The recommended cheap fixes are:

1. **Make validation match the changed surface:** T-567 ran targeted capability tests but its all-targets Clippy found items after a test module in a changed file. Add a skill line: “After the final rebase, check formatting and all-targets Clippy for changed Rust packages, including nested manifests, where the run contract permits; record completed exit status.” Never run an extra gate contrary to the contract. This example is not evidence that the gap is Pi-specific. T-579 instead exposed a gate selector bug: it ran a nested-manifest formatter, but the gate addressed that nested package as a workspace member.
2. **Reduce unrelated failures and distinguish them in metrics:** T-491 hit the separately fixed T-616 capture-window failure and twice hit a main-branch pane-inventory regression; T-523 hit the parked-wake case addressed by T-526. Retain exact signature/fix-revision annotations in gate results, not blanket automatic flake labels; report first submission separately from retries. This is a demonstrated source of noise, not an estimate of the gap after all exclusions.
3. **Make submissions auditable:** record lifecycle purpose (worker submission, member, landing), worker profile and effort, failure signature, candidate and ordered completed check/exit-status events in the gate receipt. This avoids treating `ag-*` as one lifecycle or counting retries as independent runs, and makes interrupted builds observable. Compare matched size/period/tier and effort before changing a host default.

## Cohort and method

On October 30, enumerate YAML task frontmatter under `/home/pawis/miti-workspace/.corral/tasks`; select the worker run's `execution.resolved.profile` exactly `pi-sol` or `gpt-sol`, not reviewer profiles. Join task ID and run ID to **all** schema-v2 failed receipts, numerical and `ag-*`, under `/home/pawis/miti-workspace/.corral/state/gates`; read sibling `.log` for the decisive failure. The earliest observed pi-sol run is T-478 R1 at 2026-10-27 15:01:23 UTC, used as the start proxy; freeze at 2026-10-30 19:00 UTC. Include receipts started before the run ended (or the freeze when still open), and show member gates separately. This yields 42 pi-sol and 14 gpt-sol recorded worker runs, including open/non-submitted runs; these **are not** the historical 42/157 denominators. T-498 is another project and is retained because its resolved worker profile and gate match the selection.

A `policy:review_rounds` receipt is a review-round worker gate, `member-gate:*` a Plan member gate. `legacy` below means absent `identity.gate_tier.source`; the receipt alone does not prove worker submission versus a pre-done full/landing check. No receipt after recorded run end is counted. An `ag-*` prefix is not a lifecycle classification. Each row is one failing **attempt**, not one independent task; one receipt with multiple test failures remains one row. Do not divide these counts by all runs to claim a comparable rate. Review-round retries and legacy/member attempts are visible for audit rather than silently mixed into a first-submission rate.

## Results by evidence class

| Class | pi-sol | gpt-sol |
|---|---:|---:|
| Formatting / Clippy | 0 | 3 |
| Compile, non-Clippy | 0 | 1 |
| Real test failure on touched code | 1 | 0 |
| Confirmed unrelated test/regression | 2 | 0 |
| Gate infrastructure | 1 | 0 |
| Other gate check: size ceiling | 1 | 0 |
| Unknown assertion, docs or environment cause | 21 | 10 |
| Excluded specified fixed-flake signature | 2 | 0 |
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
| pi T-470 | ag-80bb8eebcec3c1ff M | unknown: approval-busy peer `verdict` subprocess timeout, not established as fixed by T-624 |
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
| pi T-579 | ag-8d99e8734a9e5c7e P | gate infrastructure: formatter selects a nested resolver as a workspace member |
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

The two member runs T-470 and T-591 contribute eight pi attempts, so excluding member attempts leaves 20 pi and 14 gpt attempts, including the legacy ambiguity. Excluding only the two supported fixed signatures from all sources leaves 26 pi and 14 gpt attempts. These are attempt totals, not adjusted run failure rates.

## Six requested fixed-flake checks

Checked decisive failure names/messages in the ledger against the six task records, not merely task IDs. T-607 is fake-bun `ETXTBSY` (none of the above decisive signatures); T-610 is `decision_answer_replays_its_journaled_receipt_after_a_crash` (none exact; its task reports underlying cause **unverified**, so similar crash tests are not excluded); T-613 is browser navigate/screenshot startup timeout (none exact); T-616 is `plan_aggregate_cli` capture window (`capture_outside_attempt`: T-491 457673); T-621 is partial fake-corral `calls.jsonl` (none exact); T-624 specifically changes `operating_launch` interrupted-stage barrier observation (T-470 ag-fa590c7b93ff292e); its log reports waiting on that barrier, matching the fixed class. Its sweep explicitly retains approval-busy's bounded subprocess coordination: T-470 ag-80bb8eebcec3c1ff timed out waiting for peer `verdict`, so it is **not** excluded. T-616's `capture_outside_attempt` matches the cross-process capture-window fix; its task notes pre-fix reproduction was not obtained, so this is a supported signature match, not proof of a reproduced root cause. T-624 is a wider sweep, not a license to exclude every timeout. T-562 logout timeouts and T-552 crash-replay are uncertain, **retained**. Matching is limited to decisive log messages; a hidden secondary failure cannot be ruled out. See task records T-607, T-610, T-613, T-616, T-621 and T-624 in `.corral/tasks` and the receipt logs.

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
| pi T-526 | sandbox attempt, then targeted `dispatch_queue` parked-wake tests and fmt (29, 33, 43). The failing receipt is a review-round gate; earlier quick receipt ag-8fec5e51cc287dbd passed its commands, although its wake was labeled fail (see per-attempt notes). |
| pi T-571 | Rust auxiliary/capabilities and Pi auxiliary tests, fmt (315–380). |
| pi T-579 | resolver build, Bun capability test, resolver fmt and check (60, 92, 166, 174); gate could not locate the nested package, rather than finding a formatting difference. |
| pi T-583 | Pi guidance and Rust skill/config tests and fmt (88–118). |
| pi T-591 | targeted done/checkpoint tests and fmt (306–364). |
| gpt T-540 R3 | metered and Bedrock Rust tests, CLI build; authoritative quick gate (187, 281, 340, 429). |
| gpt T-550 | CLI/attention tests; authoritative quick gate (192, 353, 479, 607). |
| gpt T-552 | targeted nextest and timing checks; authoritative quick gate (394, 504, 626). |
| gpt T-562 | todo Rust and Pi checks, isolated Pi lane (60, 84, 158, 269). |
| gpt T-567 | MCP Bun tests, capability/recovery Rust tests; authoritative quick gate (296, 465, 516). No all-targets Clippy shown in those parent calls. |
| gpt T-568 | web Bun tests and authoritative quick gate (90–216). |

These calls demonstrate targeted prechecks on both harnesses. They do **not** establish cohort-wide rates of separate local quick-tier gates or interrupted builds: receipt gates are not necessarily an additional local precheck. The per-attempt and child-session audit below resolves the cited real-failure cases, not all unknown assertions. T-526's line 29 sandbox command was followed by process inspection and forced container cleanup (31–33); the record alone does not establish a general Pi build-cutoff pattern. T-567's recorded targeted tests did not prevent a Clippy-only failure. Session paths and tool results can be resolved through each worker run's frontmatter; do not infer an unrecorded command did not occur in a delegated session.

### Per-attempt follow-up: candidate checks and later retries

Line references are to the worker transcript named by its run's `session.transcript` in task frontmatter; `G` denotes that attempt's `.log` under `.corral/state/gates`. Observations of a completed test do not imply it covered a later changed tree. Child agent outputs are correlated to the separate child JSONL histories as described below. The child logs are evidence for those agents' commands, not for unrecorded checks by other processes.

| Attempt | Checks and completion **before this attempt**; quick-tier and long-build evidence |
|---|---|
| T-526 ag-41a3bd8b8fab8d22 | Parent's red command returned 101 (34), parked-wake targeted test returned 0 (44), and sandbox test returned 0 (48). First submitted quick ag-8fec5e51cc287dbd had a contradictory fail wake but command receipt exit 0 and 554 passes (51–63); a rebase preceded this new quick attempt (75–78). Initial sandbox command needed forced container removal (29–34); outcome of that initial build was not recorded. No independent local quick gate beyond the earlier receipted quick is established. |
| T-579 ag-8d99e8734a9e5c7e | Resolver build (60–61) and 46 Bun tests/validation command (174–175) completed; the latter included the explicit resolver-manifest fmt check. The failed quick gate tried `cargo fmt -p pi-profile-resolver` from the root and reported the package was not a workspace member (G:7); no separate local quick tier established. Build was not shown interrupted. |
| T-540 R3 1332842 | Offline metered test build finished in 2m45s (242); the CLI build completed in 3m10s with exit 0 (352). Initial Bedrock suite failed (322), then corrected tests passed (364). The authoritative quick gate was invoked at 429 and failed Clippy (G); no earlier independent quick tier established. |
| T-540 R3 1891787 | After 1332842, the worker corrected the Clippy lint, committed and rebased (532–568); the next quick gate failed aggregate-live. No additional independent test/build completion between the receipts is established. |
| T-540 R3 1048182 | After 1891787, a PM-authorized quick retry completed exit 0 (923); changes-round planner regression tests compiled and ran (1564, 1606, 1627), followed by a later quick gate pass (1879). The legacy macOS cross-check failed at G, after those checks. Its exact submission/landing purpose remains unresolved; the two implementation children had already stopped without running tests or gates, as independently checked below. Do not treat the earlier quick pass as an all-targets macOS check. |
| T-550 3321394 | Parent CLI checks completed (192, 219, 353, 401, 417), attention metrics passed exit 0 (479, 501); a delegated child reported `cargo check -p corral-tui` passed with affected tests pending (261), later child completion (341). The authoritative quick was initially refused (607, 635) and its retry failed fmt (693–695). The child TUI test loop timed out after a 40.49s build and was resumed successfully before this attempt; see child audit below. |
| T-550 3483560 | After fmt failure, card regression passed 9 tests (703, 721); next authorized quick failed search assertion (759, 826). No separate local quick or new interrupted build established. |
| T-550 3814971 | Search-row repair checks completed (839, 857); retry quick failed PM-next assertion (921). No separate quick or new interrupted build established. |
| T-550 1637173 | Reproduced PM failure (943, 967), then PM fixtures passed 20 tests with exit 0 (1010, 1022); retry quick failed TUI parity (1073). No separate quick or new interrupted build established. |
| T-550 3434123 | Broader app runs first failed (1113–1115, 1149–1151, 1177–1179); corrected run passed 124 tests (1205) and board checks passed (1219), before retry failed two TUI assertions (1279). No separate quick or new interrupted build established. |
| T-550 3708642 | After those failures, attention regressions passed 99 tests (1534–1538), and changes-round quick passed on `fec2e18` (1623). This later legacy full receipt failed work-budget; lifecycle is unresolved. The changes-round child red and green tests completed before this receipt; see child audit below. |
| T-550 1848971 | Previous full work-budget failure was followed by another failure of the same test; the parent reproduced doubled attention visits only afterwards (1718–1720), and the repaired work-budget pass at 1808–1810 is **after** both failures. The previously passed changes-round quick is not this full receipt. No intervening completed build or independent quick established. |
| T-567 578114 | MCP fixture/launch checks exited 0 (332–334), rebased MCP/guidance checks exited 0 (495–503), Rust capability and recovery checks completed after builds of 1m08s and 2m37s (512, 544). Private Pi lane exited 1 (435); a separate summary watcher was killed (425). Authoritative quick was admitted at 579 and failed Clippy at 588; no earlier all-targets Clippy or separate local quick is established. |

### Delegated-session join and outcomes

The parent wake IDs are `st_...` task IDs, not the UUIDs of `~/.omo/agent/sessions` files. A recursive filename and first-12-lines scan of all 1,518 `.jsonl` files in that directory, plus a full-content search for the parent UUIDs and short IDs, found no matching child history there. Searching `~/.omo` by child ID found the actual records under `~/.omo/agent/projects/<project>/senpi-task/children/<child>/sessions/<child>/*.jsonl`. Parent wake IDs, task records under `senpi-task/tasks`, timestamps and child session paths were joined; the child JSONL and its associated `*-artifacts` directories are readable. The earlier claim that these histories were unavailable was incorrect.

* **T-550 initial**: parent `st_01a18679` maps to `T-550-ff6f9c5068de/.../st_01a18679/2026-10-29T07-04-13.693Z_31597e19-72f0-4d97-83b2-bedecae13208.jsonl`. Before attempt 3321394 at 07:43:58 UTC, its initial focused test failed (91–94); affected-module loop failed (121–124); a second loop's compilation finished in 40.49s but the loop was **killed at deadline** at 07:23:24 (270–271). The child then restarted plan/snapshot tests, which passed 8 and 6 respectively with watcher exit 0 (302, 311, 328). The crate check finished in 1m21s exit 0 (200, 236). Parent's 341 completion is consistent with these results. The timed-out loop was a test command, not proof that its build was killed. The child ran no separate quick-tier gate in the recorded history.
* **T-550 attention child** `st_01a18678`, same project child tree, session `2026-10-29T07-03-39.679Z_b0e8dc9d-fe67-4011-9d4a-b8aa9fe06800.jsonl`: initial red compilation failed (60, 122), core attention tests passed (273, 302), broader metrics failed (349), and independent final attention integration completed exit 0 (353–417), all before 3321394. No quick gate appears in that child's history. Parent subsequently ran its own repairs and retries; these earlier checks are not proof of later candidate coverage.
* **T-550 changes-round child** `st_01a186f0`, session `2026-10-29T09-14-25.211Z_60751cd5-f570-4d75-b6ba-8893135a4221.jsonl`: red dependency test compilation completed and tests exited 101 (36, 57, 60); green rerun completed exit 0 after a 17.98s build (75, 103, 106), before both legacy work-budget attempts 3708642 at 10:06:01 and 1848971 at 10:10:38. The child did not run a quick gate; parent changes-round quick passed separately at parent line 1623.
* **T-540 R3**: `st_01a1870e` and `st_01a1870f` map to `T-540-r3-34bb2a50fc1d/.../children/<id>/sessions/<id>/` sessions `2026-10-29T09-47-01.224Z_473ea9d0-0393-4e0b-a009-2706eff10ce2.jsonl` and `2026-10-29T09-47-18.950Z_905fe48e-078f-4d36-ba6d-649897bed26c.jsonl`. Both implementation children completed before the first quick failure and their parent wakes (159, 258) explicitly say no tests, gates or commits were run; inspecting both child histories confirms no cargo test/build or gate invocation, although diagnostics were requested. The later consultation child `st_01a18742` is a read-only architectural consultation, not a build; its session `2026-10-29T10-44-23.827Z_54c5afdb-8041-41c9-a3e6-71b28c9ae943.jsonl` and parent wake 1096 report no gate. Therefore no delegated long build or separate quick gate is evidenced before 1048182; parent builds and quick gates are described above.
* **T-567**: `st_01a188dd` maps to `T-567-a0a8033dfc70/.../children/st_01a188dd/sessions/st_01a188dd/2026-10-29T18-13-01.932Z_1b054b08-bf77-453c-b784-5cbf2727cd15.jsonl`. Child core and ops tests completed with `T567_RUST_TESTS_EXIT core=0 ops=0` at 18:22:39 (51, 92, 126, 133); a final core run completed 20 tests with exit 0 at 18:23:37 (103, 152, 162). Both predate Clippy receipt 578114. The child did not run a gate; its parent later ran the rebased tests and receipted quick described above. The child's build completed rather than being cut off.
* T-526 and T-579 are native parent transcripts; no delegated OmO child is identified in their recorded worker histories. An unrecorded or inaccessible process cannot be ruled out. No separate local quick-tier precheck is established for either; T-526 did have the earlier receipted quick described above.

The child sessions make one specific cutoff visible (T-550's timed-out test loop) but do not support a Pi-versus-OmO cutoff rate: OmO child coverage and Pi task mix are not matched. For remaining assertion receipts the earlier table is a first-attempt parent-session index, **not** an individual audit of all later retries. They remain unknown rather than being presented as proven real touched-code regressions.

**Limitations:** source-less legacy lifecycle is unresolved; multiple attempts share candidates and tasks; class counts include member attempts and retain uncertain matches; no matched causal comparison or complete audit of unclassified assertion retries is claimed. These limits prevent explaining the size of the original harness gap, but the ledger and concrete check failures justify the inexpensive instrumentation and validation improvements above. No product code was changed and no load was generated for this investigation.
