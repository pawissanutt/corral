---
schema_version: 2
task:
  id: T-072
  title: Bind T-058 progress/checkpoints to T-052 acceptance criteria and evidence
  status: done
  project: corral
  priority: P2
  depends_on:
  - T-052
  - T-058
  created: 2026-10-15T11:59:27.256517882Z
  updated: 2026-10-16T02:39:33.343006673Z
  runs:
  - completion:
      audit:
        version: 1
        audited: false
        passed: true
        identity:
          version: 1
          task: T-072
          run: R1
          attempt: no-gate
          head: dc9f3db44c3c9197eea30add693d14e9286916fd
          tree: 51ab52defa532f3a48116d7577470947251662c8
          integration_base: 943a7d7be69cd732700659b7b02b3fe937c096b8
          dispatch_base: 943a7d7be69cd732700659b7b02b3fe937c096b8
          spec_digest: 4b93042330ef990349f02c6b5469af50ea5cac16a41e20d8b87079b8fd2b377a
          policy_digest: e2cd6d6dd157d4502ddd3636bab2ea077c5ae9b8c39afe49965cbbb7ba18ba15
          inputs_digest: ea475910136e5c1b80372290993080f14e032ac11a1857c86c23e177310e2323
        manifest_digest: null
        required_checks: []
        evidence_keys: []
        failures: []
        checked: 2026-10-15T14:03:22.134626592Z
    id: R1
    checkpoints:
    - checkpoint:
        version: 1
        revision: 1
        input:
          task: T-072
          run: R1
          spec_revision: 4b93042330ef990349f02c6b5469af50ea5cac16a41e20d8b87079b8fd2b377a
          expected_revision: 0
          progress_revision: 0
          head: dc9f3db44c3c9197eea30add693d14e9286916fd
          base: 943a7d7be69cd732700659b7b02b3fe937c096b8
          tree: 05a9aa25b58c430ae5aca57066cc3fe3abf17a409ea4f069ab76d616f07d2449
          criterion_ids: []
          evidence_refs: []
          next_action: Review and accept T-072 candidate dc9f3db using the attached acceptance report, rebased full gate log, and isolated CLI resume evidence.
          decisions:
          - Criterion IDs and evidence keys resolve only through canonical T-052 records.
          - A criterion is execution-complete only when every linked progress step is done.
          - Historical source-run receipts remain visible on resume but do not transfer acceptance to the replacement run.
          waits: []
          resources: []
        progress: null
        completed_steps: []
        current_step: null
        captured: 2026-10-15T14:03:01.07404094Z
      artifact: artifacts/T-072/R1/ccfd5997c3ccbc45f149231539b07a3c704209cc64295af9312ed8e7ad55fb2f-checkpoint-1.json
    checkpoint_required: true
    agent: gpt-sol
    harness: omo
    model: chatgpt-subscription/gpt-6.1-sol
    effort: high
    route:
      source: override
    vendor: openai
    switches:
    - from: gpt-sol
      to: sonnet
      reason: cross-vendor review
    mode: interactive
    worktree: /home/pawis/miti-workspace/corral/.worktrees/T-072
    branch: corral/T-072
    base: main
    base_sha: 943a7d7be69cd732700659b7b02b3fe937c096b8
    herdr:
      workspace_id: w2P
      pane_id: w2P:p1
    started: 2026-10-15T13:51:01.34480759Z
    ended: 2026-10-15T14:03:22.187923769Z
    outcome: done
    summary: |-
      Bound T-058 progress/checkpoint references to T-052 registered criterion IDs and current-run evidence observation keys through validate_references. Resume packets, progress readback and corral show now join canonical criteria, step states and receipt records; source-run evidence remains historical rather than transferring acceptance. Done preflight and progress readback share canonical evidence checks and flag progress_without_evidence for completed criteria lacking valid proof. Added mapping, resumed-run and mismatch coverage, documented the contract, and extracted unchanged autopilot formatting helpers to fix the pre-existing 818-line ceiling violation. Committed as 15cb206 and dc9f3db; rebased onto current main 943a7d7. Attached acceptance report, rebased gate log and isolated CLI evidence; saved compatible handoff checkpoint.
      Verified: cargo fmt --all -- --check PASS; requested git ls-files/grep Rust SLOC loop PASS (231 files, maximum 800); cargo clippy --workspace --all-targets -- -D warnings PASS; CORRAL_HERDR_SESSION=corral-test cargo test --workspace PASS (722 passed, 0 failed, 0 ignored); cargo build -p corral && scripts/check-docs.sh target/debug/corral PASS (962 spans/blocks, 24 TOML examples, 13 files). Full gate passed before and after git rebase main. cargo test -p corral-ops --test progress_ops --test completion_ops PASS (50 tests); CORRAL_HERDR_SESSION=corral-test cargo test -p corral-ops --test autopilot_ops PASS (46 tests); CORRAL_HERDR_SESSION=corral-test cargo test -p corral --test progress_cli -- --nocapture PASS (3 tests), including actual criteria register, evidence record, human/JSON show, checkpoint and retry into fresh harness B retaining criterion-linked steps and source receipt identity. Five new criteria regressions cover unknown IDs, receipt mapping, resumed progress, manifest staleness, and missing/failed/stale/non-live versus valid evidence under audited and unaudited optional criteria. Changed-file LSP diagnostics and git diff --check clean; working tree clean.
      Not verified: none
    agent_state: working
    seen_working: true
    gate:
      identity:
        version: 1
        task: T-072
        run: R1
        attempt: 1911000-1792073007840316690
        head: dc9f3db44c3c9197eea30add693d14e9286916fd
        tree: 51ab52defa532f3a48116d7577470947251662c8
        integration_base: 943a7d7be69cd732700659b7b02b3fe937c096b8
        dispatch_base: 943a7d7be69cd732700659b7b02b3fe937c096b8
        spec_digest: 4b93042330ef990349f02c6b5469af50ea5cac16a41e20d8b87079b8fd2b377a
        policy_digest: e2cd6d6dd157d4502ddd3636bab2ea077c5ae9b8c39afe49965cbbb7ba18ba15
        inputs_digest: 96ed11d038d6f396933fc821d3e0a5745fca744813fa5320ef96f7cd45e6071f
      result: running
      process_lost: true
      at: 2026-10-15T14:03:27.860746884Z
      sha: dc9f3db44c3c9197eea30add693d14e9286916fd
      pane: w2P:p2
    review:
      identity:
        version: 1
        task: T-072
        run: R1
        attempt: 1911000-1792073007840316690
        head: dc9f3db44c3c9197eea30add693d14e9286916fd
        tree: 51ab52defa532f3a48116d7577470947251662c8
        integration_base: 3e45540ae3c8041414bc21b88bcf4075c8f4a5fb
        dispatch_base: 943a7d7be69cd732700659b7b02b3fe937c096b8
        spec_digest: 4b93042330ef990349f02c6b5469af50ea5cac16a41e20d8b87079b8fd2b377a
        policy_digest: e2cd6d6dd157d4502ddd3636bab2ea077c5ae9b8c39afe49965cbbb7ba18ba15
        inputs_digest: 96ed11d038d6f396933fc821d3e0a5745fca744813fa5320ef96f7cd45e6071f
      evidence_digest: 4f53cda18c2baa0c0354bb5f9a3ecbe5ed12ab4d8e11ba873c2f11161202b945
      verdict: approve
      reviewer: sonnet
      notes: |
        T-072 R1 review: approve.

        AC1: validate_references is the only seam; it now resolves criterion ids against task.completion.criteria (unknown_criterion) and evidence refs against current-run observation keys, with an evidence/criterion mismatch check. ProgressStep.criteria reuses T-052 ids; no second criteria model (CriterionProgress is a read-only join).
        AC2: resume packet, progress readback and corral show (human and JSON) list each criterion with step states and receipt records. Source-run evidence stays historical and is not promoted.
        AC3: evidence_failures is shared by done preflight and readback; a criterion complete in progress without valid evidence emits progress_without_evidence:<id>, for audited and unaudited manifests.
        AC4: new tests in corral-ops/tests/progress/criteria.rs (unknown ids, receipt mapping, resumed progress, manifest staleness, missing/failed/stale evidence) plus progress_cli resume into a fresh harness.
        AC5: gate log ends 'corral gate: pass' at dc9f3db (722 tests, docs check); SLOC extraction in autopilot/output.rs is a pure move.

        Non-blocking observations:
        - progress_spec_revision now hashes task.completion, so existing checkpoints become stale once after upgrade (intended for manifest staleness).
        - resume_packet computes evidence_failures against the new run's progress, not the checkpoint progress, so a mismatch inherited from a checkpoint is surfaced mainly after the new run resumes. Acceptable.
        - Previously valid artifact-path evidence refs in checkpoints are now rejected (intentional contract change; documented).
      sha: dc9f3db44c3c9197eea30add693d14e9286916fd
      at: 2026-10-15T14:05:00.750938725Z
      pane: w2P:p3
    review_runs:
    - completion: {}
      id: R1-review1
      agent: sonnet
      harness: omo
      model: anthropic-subscription/claude-sonnet-5-5
      effort: medium
      vendor: anthropic
      mode: interactive
      worktree: /home/pawis/miti-workspace/corral/.worktrees/T-072
      branch: corral/T-072
      base: main
      base_sha: 943a7d7be69cd732700659b7b02b3fe937c096b8
      started: 2026-10-15T14:04:34.057880232Z
      ended: 2026-10-15T14:05:00.750938725Z
      seen_working: false
      usage:
        input: 8
        output: 2564
        cache_read: 167900
        cache_write: 36578
        cost_usd: 0.133891
        reported_usd: 0.133891
        estimated_usd: null
        unpriced_tokens: 207050
        cost_basis: reported
        entries:
        - model: claude-sonnet-5-5
          input: 2
          output: 224
          cache_read: 20337
          cache_write: 27460
          reported_usd: 0.0729277
          estimated_usd: null
          unpriced_tokens: 48023
        - model: claude-sonnet-5-5
          input: 2
          output: 426
          cache_read: 47797
          cache_write: 1287
          reported_usd: 0.012261200000000002
          estimated_usd: null
          unpriced_tokens: 49512
        - model: claude-sonnet-5-5
          input: 2
          output: 1675
          cache_read: 50682
          cache_write: 6233
          reported_usd: 0.0374047
          estimated_usd: null
          unpriced_tokens: 58592
        - model: claude-sonnet-5-5
          input: 2
          output: 239
          cache_read: 49084
          cache_write: 1598
          reported_usd: 0.0112974
          estimated_usd: null
          unpriced_tokens: 50923
    usage:
      input: 215528
      output: 21340
      cache_read: 7593088
      cache_write: 0
      cost_usd: 1.4037648000000003
      reported_usd: 1.4037648000000003
      estimated_usd: 1.4037647999999998
      unpriced_tokens: 0
      cost_basis: reported
      entries:
      - model: gpt-6.1-sol
        input: 2219
        output: 642
        cache_read: 126336
        cache_write: 0
        reported_usd: 0.0234916
        estimated_usd: 0.023491599999999998
        unpriced_tokens: 0
      - model: gpt-6.1-sol
        input: 3255
        output: 181
        cache_read: 81536
        cache_write: 0
        reported_usd: 0.016473599999999998
        estimated_usd: 0.016473599999999998
        unpriced_tokens: 0
      - model: gpt-6.1-sol
        input: 1124
        output: 197
        cache_read: 114560
        cache_write: 0
        reported_usd: 0.015674
        estimated_usd: 0.015674
        unpriced_tokens: 0
      - model: gpt-6.1-sol
        input: 1027
        output: 852
        cache_read: 119808
        cache_write: 0
        reported_usd: 0.022554800000000003
        estimated_usd: 0.022554800000000003
        unpriced_tokens: 0
      - model: gpt-6.1-sol
        input: 616
        output: 94
        cache_read: 129152
        cache_write: 0
        reported_usd: 0.015087200000000002
        estimated_usd: 0.0150872
        unpriced_tokens: 0
      - model: gpt-6.1-sol
        input: 2295
        output: 881
        cache_read: 121600
        cache_write: 0
        reported_usd: 0.02556
        estimated_usd: 0.02556
        unpriced_tokens: 0
      - model: gpt-6.1-sol
        input: 1986
        output: 429
        cache_read: 129536
        cache_write: 0
        reported_usd: 0.0212156
        estimated_usd: 0.021215599999999998
        unpriced_tokens: 0
      - model: gpt-6.1-sol
        input: 1153
        output: 157
        cache_read: 123776
        cache_write: 0
        reported_usd: 0.0162536
        estimated_usd: 0.0162536
        unpriced_tokens: 0
      - model: gpt-6.1-sol
        input: 605
        output: 91
        cache_read: 185728
        cache_write: 0
        reported_usd: 0.0206928
        estimated_usd: 0.0206928
        unpriced_tokens: 0
      - model: gpt-6.1-sol
        input: 951
        output: 123
        cache_read: 176384
        cache_write: 0
        reported_usd: 0.0207704
        estimated_usd: 0.0207704
        unpriced_tokens: 0
      - model: gpt-6.1-sol
        input: 1161
        output: 879
        cache_read: 113536
        cache_write: 0
        reported_usd: 0.0224656
        estimated_usd: 0.0224656
        unpriced_tokens: 0
      - model: gpt-6.1-sol
        input: 1652
        output: 250
        cache_read: 156416
        cache_write: 0
        reported_usd: 0.0214456
        estimated_usd: 0.0214456
        unpriced_tokens: 0
      - model: gpt-6.1-sol
        input: 2531
        output: 494
        cache_read: 111232
        cache_write: 0
        reported_usd: 0.021125200000000004
        estimated_usd: 0.0211252
        unpriced_tokens: 0
      - model: gpt-6.1-sol
        input: 1509
        output: 92
        cache_read: 33664
        cache_write: 0
        reported_usd: 0.0073044
        estimated_usd: 0.0073044
        unpriced_tokens: 0
      - model: gpt-6.1-sol
        input: 6350
        output: 228
        cache_read: 177152
        cache_write: 0
        reported_usd: 0.0326952
        estimated_usd: 0.0326952
        unpriced_tokens: 0
      - model: gpt-6.1-sol
        input: 369
        output: 523
        cache_read: 186368
        cache_write: 0
        reported_usd: 0.024604800000000003
        estimated_usd: 0.0246048
        unpriced_tokens: 0
      - model: gpt-6.1-sol
        input: 4079
        output: 295
        cache_read: 144640
        cache_write: 0
        reported_usd: 0.025572
        estimated_usd: 0.025572
        unpriced_tokens: 0
      - model: gpt-6.1-sol
        input: 838
        output: 128
        cache_read: 183936
        cache_write: 0
        reported_usd: 0.021349600000000003
        estimated_usd: 0.021349600000000003
        unpriced_tokens: 0
      - model: gpt-6.1-sol
        input: 1540
        output: 298
        cache_read: 205824
        cache_write: 0
        reported_usd: 0.0266424
        estimated_usd: 0.0266424
        unpriced_tokens: 0
      - model: gpt-6.1-sol
        input: 17679
        output: 235
        cache_read: 64000
        cache_write: 0
        reported_usd: 0.044108
        estimated_usd: 0.044108
        unpriced_tokens: 0
      - model: gpt-6.1-sol
        input: 970
        output: 283
        cache_read: 32896
        cache_write: 0
        reported_usd: 0.0080596
        estimated_usd: 0.0080596
        unpriced_tokens: 0
      - model: gpt-6.1-sol
        input: 309
        output: 109
        cache_read: 208384
        cache_write: 0
        reported_usd: 0.022546400000000005
        estimated_usd: 0.0225464
        unpriced_tokens: 0
      - model: gpt-6.1-sol
        input: 1755
        output: 1811
        cache_read: 124800
        cache_write: 0
        reported_usd: 0.034100000000000005
        estimated_usd: 0.0341
        unpriced_tokens: 0
      - model: gpt-6.1-sol
        input: 1228
        output: 169
        cache_read: 118784
        cache_write: 0
        reported_usd: 0.0160244
        estimated_usd: 0.0160244
        unpriced_tokens: 0
      - model: gpt-6.1-sol
        input: 886
        output: 359
        cache_read: 128384
        cache_write: 0
        reported_usd: 0.0182004
        estimated_usd: 0.018200400000000002
        unpriced_tokens: 0
      - model: gpt-6.1-sol
        input: 3541
        output: 830
        cache_read: 115456
        cache_write: 0
        reported_usd: 0.026927600000000003
        estimated_usd: 0.0269276
        unpriced_tokens: 0
      - model: gpt-6.1-sol
        input: 8235
        output: 575
        cache_read: 103168
        cache_write: 0
        reported_usd: 0.032536800000000005
        estimated_usd: 0.032536800000000005
        unpriced_tokens: 0
      - model: gpt-6.1-sol
        input: 289
        output: 119
        cache_read: 208256
        cache_write: 0
        reported_usd: 0.0225936
        estimated_usd: 0.022593600000000002
        unpriced_tokens: 0
      - model: gpt-6.1-sol
        input: 826
        output: 367
        cache_read: 183296
        cache_write: 0
        reported_usd: 0.0236516
        estimated_usd: 0.023651600000000002
        unpriced_tokens: 0
      - model: gpt-6.1-sol
        input: 6031
        output: 428
        cache_read: 170496
        cache_write: 0
        reported_usd: 0.0333916
        estimated_usd: 0.03339160000000001
        unpriced_tokens: 0
      - model: gpt-6.1-sol
        input: 673
        output: 103
        cache_read: 207744
        cache_write: 0
        reported_usd: 0.0231504
        estimated_usd: 0.0231504
        unpriced_tokens: 0
      - model: gpt-6.1-sol
        input: 318
        output: 98
        cache_read: 208512
        cache_write: 0
        reported_usd: 0.0224672
        estimated_usd: 0.0224672
        unpriced_tokens: 0
      - model: gpt-6.1-sol
        input: 573
        output: 372
        cache_read: 157824
        cache_write: 0
        reported_usd: 0.020648400000000004
        estimated_usd: 0.0206484
        unpriced_tokens: 0
      - model: gpt-6.1-sol
        input: 10241
        output: 259
        cache_read: 84608
        cache_write: 0
        reported_usd: 0.0315328
        estimated_usd: 0.0315328
        unpriced_tokens: 0
      - model: gpt-6.1-sol
        input: 29152
        output: 212
        cache_read: 3968
        cache_write: 0
        reported_usd: 0.060820799999999994
        estimated_usd: 0.0608208
        unpriced_tokens: 0
      - model: gpt-6.1-sol
        input: 5660
        output: 148
        cache_read: 48640
        cache_write: 0
        reported_usd: 0.017664
        estimated_usd: 0.017664
        unpriced_tokens: 0
      - model: gpt-6.1-sol
        input: 3046
        output: 743
        cache_read: 131328
        cache_write: 0
        reported_usd: 0.0266548
        estimated_usd: 0.026654800000000003
        unpriced_tokens: 0
      - model: gpt-6.1-sol
        input: 10060
        output: 195
        cache_read: 54144
        cache_write: 0
        reported_usd: 0.0274844
        estimated_usd: 0.027484400000000003
        unpriced_tokens: 0
      - model: gpt-6.1-sol
        input: 702
        output: 441
        cache_read: 207232
        cache_write: 0
        reported_usd: 0.0265372
        estimated_usd: 0.0265372
        unpriced_tokens: 0
      - model: gpt-6.1-sol
        input: 1074
        output: 213
        cache_read: 135040
        cache_write: 0
        reported_usd: 0.017782
        estimated_usd: 0.017782
        unpriced_tokens: 0
      - model: gpt-6.1-sol
        input: 1150
        output: 93
        cache_read: 184576
        cache_write: 0
        reported_usd: 0.0216876
        estimated_usd: 0.0216876
        unpriced_tokens: 0
      - model: gpt-6.1-sol
        input: 840
        output: 478
        cache_read: 155776
        cache_write: 0
        reported_usd: 0.022037600000000004
        estimated_usd: 0.022037599999999997
        unpriced_tokens: 0
      - model: gpt-6.1-sol
        input: 6007
        output: 1380
        cache_read: 199936
        cache_write: 0
        reported_usd: 0.0458076
        estimated_usd: 0.045807600000000004
        unpriced_tokens: 0
      - model: gpt-6.1-sol
        input: 12520
        output: 476
        cache_read: 158208
        cache_write: 0
        reported_usd: 0.0456208
        estimated_usd: 0.0456208
        unpriced_tokens: 0
      - model: gpt-6.1-sol
        input: 311
        output: 196
        cache_read: 185600
        cache_write: 0
        reported_usd: 0.021142
        estimated_usd: 0.021142
        unpriced_tokens: 0
      - model: gpt-6.1-sol
        input: 409
        output: 105
        cache_read: 186112
        cache_write: 0
        reported_usd: 0.020479200000000003
        estimated_usd: 0.0204792
        unpriced_tokens: 0
      - model: gpt-6.1-sol
        input: 4938
        output: 269
        cache_read: 135936
        cache_write: 0
        reported_usd: 0.0261596
        estimated_usd: 0.026159599999999998
        unpriced_tokens: 0
      - model: gpt-6.1-sol
        input: 13920
        output: 193
        cache_read: 34944
        cache_write: 0
        reported_usd: 0.0332644
        estimated_usd: 0.0332644
        unpriced_tokens: 0
      - model: gpt-6.1-sol
        input: 7461
        output: 557
        cache_read: 148480
        cache_write: 0
        reported_usd: 0.03534
        estimated_usd: 0.03534
        unpriced_tokens: 0
      - model: gpt-6.1-sol
        input: 13557
        output: 564
        cache_read: 186496
        cache_write: 0
        reported_usd: 0.0514036
        estimated_usd: 0.05140360000000001
        unpriced_tokens: 0
      - model: gpt-6.1-sol
        input: 8688
        output: 219
        cache_read: 94720
        cache_write: 0
        reported_usd: 0.029038
        estimated_usd: 0.029038
        unpriced_tokens: 0
      - model: gpt-6.1-sol
        input: 1058
        output: 235
        cache_read: 120704
        cache_write: 0
        reported_usd: 0.0165364
        estimated_usd: 0.016536400000000003
        unpriced_tokens: 0
      - model: gpt-6.1-sol
        input: 1052
        output: 527
        cache_read: 134144
        cache_write: 0
        reported_usd: 0.0207884
        estimated_usd: 0.020788400000000002
        unpriced_tokens: 0
      - model: gpt-6.1-sol
        input: 4192
        output: 345
        cache_read: 140672
        cache_write: 0
        reported_usd: 0.0259012
        estimated_usd: 0.0259012
        unpriced_tokens: 0
      - model: gpt-6.1-sol
        input: 917
        output: 800
        cache_read: 208640
        cache_write: 0
        reported_usd: 0.030698000000000003
        estimated_usd: 0.030698
        unpriced_tokens: 0
  artifacts:
  - run: R1
    kind: report
    title: T-072 acceptance criteria and verification report
    path: artifacts/T-072/R1/21efe742-report.md
    source: /home/pawis/miti-workspace/corral/.worktrees/T-072/.omo/evidence/T-072/report.md
    sha256: 21efe742f282b373183caf6b7cb3615fd611dd8eee278d64af8fd76cb17a8853
    bytes: 4332
    added: 2026-10-15T14:02:44.102427489Z
  - run: R1
    kind: log
    title: 'T-072 rebased full gate: 722 passing tests and SLOC ceiling'
    path: artifacts/T-072/R1/13306e71-gate-rebased.log
    source: /home/pawis/miti-workspace/corral/.worktrees/T-072/.omo/evidence/T-072/gate-rebased.log
    sha256: 13306e712078bf43440f67005756689eb22995bce23614df8a124f45e0083906
    bytes: 62246
    added: 2026-10-15T14:02:49.909639636Z
  - run: R1
    kind: evidence
    title: T-072 isolated CLI receipt mapping and fresh harness resume
    path: artifacts/T-072/R1/20cbe24a-live-cli.log
    source: /home/pawis/miti-workspace/corral/.worktrees/T-072/.omo/evidence/T-072/live-cli.log
    sha256: 20cbe24adf922cec466aac27d20a12917207f099dd5793a5776eb49c0aeb3463
    bytes: 16695
    added: 2026-10-15T14:02:55.345334363Z
  - run: R1
    kind: evidence
    title: Resume checkpoint 1
    path: artifacts/T-072/R1/ccfd5997c3ccbc45f149231539b07a3c704209cc64295af9312ed8e7ad55fb2f-checkpoint-1.json
    source: /home/pawis/miti-workspace/.corral/artifacts/T-072/R1/ccfd5997c3ccbc45f149231539b07a3c704209cc64295af9312ed8e7ad55fb2f-checkpoint-1.json
    sha256: ccfd5997c3ccbc45f149231539b07a3c704209cc64295af9312ed8e7ad55fb2f
    bytes: 1111
    added: 2026-10-15T14:03:01.07404094Z
  - run: R1
    kind: diff
    title: Final diff +493 −54
    path: artifacts/T-072/R1/6e8cd895-final.diff
    source: /home/pawis/miti-workspace/.corral/state/tmp/T-072-R1/final.diff
    sha256: 6e8cd8954062deefbe3955f1484935ce19fe5d819a6d7d4a0e31fa0efa2b52ed
    bytes: 35061
    added: 2026-10-15T14:03:22.154084041Z
  - run: R1
    kind: summary
    title: Done summary
    path: artifacts/T-072/R1/e19b1519-summary.md
    source: /home/pawis/miti-workspace/.corral/state/tmp/T-072-R1/summary.md
    sha256: e19b151902a65c31469a554b2973cfee6512d0c2cfd011a4d5280263b3dff817
    bytes: 2048
    added: 2026-10-15T14:03:22.154646251Z
  - run: R1
    kind: report
    title: Review (sonnet)
    path: artifacts/T-072/R1/c991535a-review-notes.md
    source: /home/pawis/miti-workspace/.corral/state/tmp/T-072-R1-review/review-notes.md
    sha256: c991535acddbf84acd243d2a0d05d5f4457044509ca2056934827f6e068dca7f
    bytes: 1572
    added: 2026-10-15T14:05:00.75192887Z
  - run: R1
    kind: log
    title: Reviewer transcript (R1-review1)
    path: artifacts/T-072/R1/e68233d1-R1-review1.transcript.log
    source: /home/pawis/miti-workspace/.corral/state/review-watch/T-072/R1-review1.transcript.log
    sha256: e68233d1c6025afc94fa1c65e42ff66582f02b025ac3e53c72ec93a854b70e2e
    bytes: 66285
    added: 2026-10-15T23:51:57.50548393Z
---
## Goal
T-058 (phased progress and resume checkpoints) shipped progress-only on purpose: it did not wait for T-052's canonical acceptance-criteria/evidence API. Once both have landed, bind them so progress items, checkpoints and resume briefs refer to T-052's criterion ids and evidence records.

## Acceptance criteria
1. Progress/checkpoint items that correspond to acceptance criteria carry T-052's criterion id; the seam T-058 left (see its summary's Not verified) is the only integration point; no duplicate criteria model remains.
2. Resume briefs and `corral show` list each criterion with its progress state and linked evidence (from T-052 receipts), not free text.
3. Done preflight (T-052) and progress (T-058) agree: a criterion marked complete in progress without evidence is flagged.
4. Tests cover the mapping, a resumed run seeing criterion-linked progress, and a mismatch being flagged.
5. Full gate passes; no Rust file over 800 SLOC.

Small task; profile gpt-sol.
