//! Worker-launch resume and require_checkpoint through their real entries (plan T-253 section
//! 4, "Worker-launch resume" and the retry/availability require_checkpoint scenario). The launch
//! is the action budget's real sync tick; retry and the availability handoff are their real
//! automatic entries. Each runs at 1x and 10x of the history it may read only as own records,
//! and must produce the reference outcome with own-record cells grown by exactly those rows'
//! stored bytes and every unattributed and emitted cell unchanged.
use corral_core::work::{self, HotScope, WorkSnapshot};
use corral_core::{ObligationSource, Task};

use super::action_budget::{Measured, emitted, measure, measure_cold, own, unattributed};
use super::action_budget_fixture::{Bench, Kind, Variant, build, settle};
use super::fixture::{self, Fx};
use super::{Path, action_heads};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shape {
    /// No compatible checkpoint: every meta is stale.
    AllStale,
    /// The older compatible checkpoint stays; stale metas follow it, the newest stale too.
    OlderCompatible,
    /// The newest checkpoint is the compatible one; stale metas precede it.
    NewestCompatible,
}

/// `stale` metas on the source run R1 whose recorded head no longer matches its worktree.
fn shape_source(bench: &Bench, shape: Shape, stale: usize) {
    bench
        .ctx
        .store
        .update(&bench.id, |t| {
            let run = t.runs.iter_mut().find(|r| r.id == "R1").unwrap();
            let template = run.checkpoints.last().unwrap().clone();
            let stale_record = |i: usize| {
                let mut record = template.clone();
                record.artifact = format!("{}-stale-{i:03}", template.artifact);
                record.checkpoint.input.head = "f".repeat(40);
                record
            };
            match shape {
                Shape::AllStale => {
                    for record in &mut run.checkpoints {
                        record.checkpoint.input.head = "f".repeat(40);
                    }
                    run.checkpoints.extend((0..stale).map(stale_record));
                }
                Shape::OlderCompatible => run.checkpoints.extend((0..=stale).map(stale_record)),
                Shape::NewestCompatible => {
                    let at = run.checkpoints.len() - 1;
                    run.checkpoints
                        .splice(at..at, (0..stale).map(stale_record).collect::<Vec<_>>());
                }
            }
            Ok(())
        })
        .unwrap();
}

/// What the selection reads at most: every meta of R1 newest first, each with its
/// registration lookup, as own records of R1.
fn scan(bench: &Bench) -> (u64, u64) {
    let head = bench.ctx.store.head(&bench.id).unwrap();
    let _hot = HotScope::enter();
    let before = work::snapshot();
    {
        let _own = work::own_records("budget", &bench.id, "R1");
        for meta in head.checkpoint_metas_newest_first("R1").unwrap() {
            let meta = meta.unwrap();
            head.checkpoint_by_artifact("R1", &meta.artifact).unwrap();
        }
    }
    let delta = work::snapshot().since(&before);
    (delta.own_record_rows(), delta.own_record_bytes())
}

/// The `<corral-resume>` packet the launched run's prompt carries.
fn prompt_packet(bench: &Bench, run: &str) -> serde_json::Value {
    let path = crate::prompt::run_prompt_file(&bench.ctx, &bench.id, run).unwrap();
    let text = std::fs::read_to_string(path).unwrap();
    let json = text
        .split_once("<corral-resume>\n")
        .unwrap()
        .1
        .split_once("\n</corral-resume>")
        .unwrap()
        .0;
    serde_json::from_str(json).unwrap()
}

fn same_cells(a: &Measured, b: &Measured, what: &str) {
    let differ: Vec<_> = unattributed(&a.cells)
        .filter(|(k, v)| b.cells.get(*k) != Some(*v))
        .map(|(k, v)| format!("{k}: {v} -> {:?}", b.cells.get(k)))
        .collect();
    assert!(differ.is_empty(), "{what}: unattributed cells {differ:?}");
    assert_eq!(
        emitted(&a.cells),
        emitted(&b.cells),
        "{what}: emitted cells"
    );
}

/// The git work of one `resume_packet` call after the launch, and of its three identity
/// computations (head, base and tree) done once on their own: they must be equal.
fn identity_processes(bench: &Bench) -> (u64, u64) {
    let head = bench.ctx.store.head(&bench.id).unwrap();
    let shell = head.to_shell_task().unwrap();
    let source = head.run_head("R1").unwrap();
    let run = shell.current_run().unwrap();
    let before = corral_git::work::invocations();
    corral_git::rev_sha(&run.worktree, "HEAD").unwrap();
    corral_git::rev_sha(&run.worktree, &run.base).unwrap();
    crate::progress::tree_revision(run).unwrap();
    let identity = corral_git::work::invocations() - before;
    let before = corral_git::work::invocations();
    crate::resume::resume_packet_head(&bench.ctx, &head, &shell, source.as_ref(), run).unwrap();
    (corral_git::work::invocations() - before, identity)
}

#[test]
fn worker_launch_resume_selects_on_metadata_through_the_tick() {
    for (cold, shape) in [false, true].into_iter().flat_map(|cold| {
        [
            Shape::AllStale,
            Shape::OlderCompatible,
            Shape::NewestCompatible,
        ]
        .map(|s| (cold, s))
    }) {
        let mut sizes = Vec::new();
        for stale in [1, 10] {
            let bench = build(Kind::Worker, Variant::Base);
            shape_source(&bench, shape, stale);
            settle(&bench);
            let before: Task = bench.ctx.store.get(&bench.id).unwrap();
            let scanned = scan(&bench);
            let m = if cold {
                measure_cold(&bench, Kind::Worker)
            } else {
                measure(&bench, Kind::Worker)
            };
            assert!(
                m.failures.is_empty(),
                "cold={cold} {shape:?} x{stale}: {:?}",
                m.failures
            );
            let (call, identity) = identity_processes(&bench);
            assert_eq!(
                call, identity,
                "{shape:?} x{stale}: a packet's git work is its three identity computations, once"
            );
            // The packet equals the full form's on the same inputs.
            let after: Task = bench.ctx.store.get(&bench.id).unwrap();
            let run = after.current_run().unwrap();
            let reference =
                serde_json::to_value(crate::resume_packet(&bench.ctx, &before, run).unwrap())
                    .unwrap();
            assert_eq!(
                prompt_packet(&bench, &run.id),
                reference,
                "{shape:?} x{stale}: the launch's packet differs from the reference"
            );
            sizes.push((m, scanned));
        }
        let [(small, small_scan), (large, large_scan)] = sizes.try_into().unwrap();
        same_cells(&small, &large, &format!("cold={cold} {shape:?}"));
        // The selection reads every added meta and its lookup, and no rejected payload; a
        // compatible newest checkpoint ends the scan at once.
        let grown = (
            own(&large.cells).0 - own(&small.cells).0,
            own(&large.cells).1 - own(&small.cells).1,
        );
        let want = match shape {
            Shape::NewestCompatible => (0, 0),
            // A launch builds the packet twice: progress restoration, then the prompt.
            _ => (
                2 * (large_scan.0 - small_scan.0),
                2 * (large_scan.1 - small_scan.1),
            ),
        };
        assert_eq!(
            grown, want,
            "cold={cold} {shape:?}: own-record (rows, bytes) growth"
        );
    }
}

#[test]
fn worker_launch_packet_with_emitted_obligations_equals_the_reference() {
    // Given a source run with waits, decisions and a larger frozen payload.
    let bench = build(Kind::Worker, Variant::Emitted);
    settle(&bench);
    let before: Task = bench.ctx.store.get(&bench.id).unwrap();
    // When the tick launches the next run.
    let m = measure(&bench, Kind::Worker);
    assert!(m.failures.is_empty(), "{:?}", m.failures);
    // Then the prompt's packet is the full form's on the same inputs, obligations included.
    let after: Task = bench.ctx.store.get(&bench.id).unwrap();
    let run = after.current_run().unwrap();
    let reference = crate::resume_packet(&bench.ctx, &before, run).unwrap();
    assert!(
        !reference.prior_run_obligations.waits.is_empty()
            && !reference.prior_run_obligations.decisions.is_empty(),
        "the reference carries the source's obligations"
    );
    assert_eq!(
        prompt_packet(&bench, &run.id),
        serde_json::to_value(reference).unwrap()
    );
}

/// A launched worker R1 with `n` waits and `n` decisions frozen into its newest checkpoint.
fn obligated(n: usize, finished: bool) -> (Fx, String) {
    corral_core::clock::pin_thread(Some(fixture::now()));
    let (mut fx, id) = Path::Worker.setup();
    fx.start(&id);
    action_heads::waits(&fx, &id, n);
    for i in 0..n {
        crate::register_decision(
            &fx.ctx,
            &id,
            corral_core::DecisionRequest {
                version: 1,
                id: format!("choice-{i:03}"),
                run: "R1".into(),
                question: "Which release branch?".into(),
                requested_action: "Choose a branch".into(),
                owner: "owner".into(),
                source: ObligationSource::Worker,
                deadline: None,
            },
            fixture::now(),
        )
        .unwrap();
        crate::resume_run(
            &fx.ctx,
            &id,
            crate::ResumeTarget::Decision(&format!("choice-{i:03}")),
            corral_core::ResumeReceipt {
                version: 1,
                id: format!("answer-{i:03}"),
                run: "R1".into(),
                outcome: corral_core::ReceiptOutcome::Answered,
                delivery: corral_core::ResumeDelivery::Acknowledged,
                observed_at: fixture::now(),
                answer: Some("main".into()),
                data: serde_json::json!({}),
            },
            fixture::now(),
        )
        .unwrap();
    }
    action_heads::checkpoint(&fx, &id);
    // Retry and the handoff act on a run its worker finished, the task in Review; approve
    // (`corral done`) on the running one.
    if finished {
        fx.ctx
            .store
            .update(&id, |t| {
                let run = t.current_run_mut().unwrap();
                run.outcome = Some(corral_core::RunOutcome::Done);
                run.ended = Some(fixture::now());
                t.status = corral_core::Status::Review;
                Ok(())
            })
            .unwrap();
    }
    fx.grant(&[&id]);
    (fx, id)
}

/// The rows require_checkpoint reads as R1's own records: its waits and decisions, its newest
/// meta and that meta's payload, measured by the store's own readers.
fn obligation_rows(fx: &Fx, id: &str) -> (u64, u64) {
    let head = fx.ctx.store.head(id).unwrap();
    let _hot = HotScope::enter();
    let before = work::snapshot();
    head.emitted_obligations("R1").unwrap();
    let meta = {
        let _own = work::own_records("budget", id, "R1");
        head.checkpoint_metas_newest_first("R1")
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
    };
    head.emitted_checkpoint(&meta).unwrap();
    let delta = work::snapshot().since(&before);
    (
        delta.emitted_rows() + delta.own_record_rows(),
        delta.emitted_doc_bytes() + delta.own_record_bytes(),
    )
}

fn timed(call: impl FnOnce()) -> WorkSnapshot {
    let _hot = HotScope::enter();
    let before = work::snapshot();
    call();
    work::snapshot().since(&before)
}

/// The require_checkpoint entries the plan names: automatic retry, the availability handoff
/// and approve (`corral done`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Entry {
    Retry,
    Handoff,
    Approve,
}

/// Moves R1's worktree past its checkpoint: a commit the checkpoint does not record.
fn stale(fx: &Fx, id: &str) {
    let task: Task = fx.ctx.store.get(id).unwrap();
    let worktree = task.current_run().unwrap().worktree.clone();
    std::fs::write(worktree.join("after-checkpoint.txt"), "later work\n").unwrap();
    for args in [
        vec!["add", "after-checkpoint.txt"],
        vec![
            "-c",
            "user.name=fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "-q",
            "-m",
            "work after the checkpoint",
        ],
    ] {
        let out = std::process::Command::new("git")
            .args(&args)
            .current_dir(&worktree)
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
    }
}

/// `entry` on R1, its outcome as the status it reached or its error.
fn enter(fx: &Fx, id: &str, entry: Entry, key: &crate::integration::RepairKey) -> String {
    let outcome = match entry {
        Entry::Retry => crate::dispatch::auto_retry(
            &fx.ctx,
            id,
            1,
            corral_core::AutopilotState {
                stage: corral_core::AutoStage::Gating,
                reason: None,
                sha: None,
                at: fixture::now(),
            },
            key,
        )
        .map(|outcome| {
            format!("{outcome:?}")
                .split_whitespace()
                .next()
                .unwrap()
                .to_owned()
        }),
        Entry::Handoff => crate::dispatch::availability_retry(&fx.ctx, id, "R1").map(|outcome| {
            format!("{outcome:?}")
                .split_whitespace()
                .next()
                .unwrap()
                .to_owned()
        }),
        Entry::Approve => crate::done(&fx.ctx, id, "finished the change")
            .map(|task| task.status.label().to_owned()),
    };
    outcome.unwrap_or_else(|error| format!("error: {error:#}"))
}

#[test]
fn require_checkpoint_reads_the_run_own_obligations_through_its_real_entries() {
    for entry in [Entry::Retry, Entry::Handoff, Entry::Approve] {
        for is_stale in [false, true] {
            let what = format!("{entry:?} stale={is_stale}");
            let mut sizes = Vec::new();
            for n in [1, 10] {
                // Given R1 with n waits and n answered decisions frozen into its newest
                // checkpoint, which a later commit may have made stale.
                let (fx, id) = obligated(n, entry != Entry::Approve);
                if is_stale {
                    stale(&fx, &id);
                }
                // The repair key names the run as it now stands: only the checkpoint refuses.
                let full: Task = fx.ctx.store.get(&id).unwrap();
                let key =
                    crate::integration::RepairKey::new(&full, full.current_run().unwrap(), "lost")
                        .unwrap();
                // The head form refuses and admits exactly as the full form does.
                let head = fx.ctx.store.head(&id).unwrap();
                let reference =
                    crate::resume::require_checkpoint(&full, full.current_run().unwrap())
                        .map_err(|e| format!("{e:#}"));
                assert_eq!(
                    crate::resume::require_checkpoint_head(&head, "R1")
                        .map_err(|e| format!("{e:#}")),
                    reference,
                    "{what} x{n}"
                );
                assert_eq!(reference.is_err(), is_stale, "{what} x{n}: {reference:?}");
                let expected = obligation_rows(&fx, &id);
                // When the real entry runs.
                let mut outcome = String::new();
                let delta = timed(|| outcome = enter(&fx, &id, entry, &key));
                // Then it proceeds, or refuses with the reference's error.
                match &reference {
                    Ok(()) => assert!(!outcome.starts_with("error"), "{what} x{n}: {outcome}"),
                    Err(error) => {
                        assert!(outcome.contains(error.as_str()), "{what} x{n}: {outcome}")
                    }
                }
                delta.check_identity().unwrap();
                if entry != Entry::Approve {
                    assert_eq!(delta.unattributed_hydrations(), 0, "{what} x{n}");
                }
                sizes.push((delta, expected, outcome));
            }
            let [
                (small, small_rows, small_out),
                (large, large_rows, large_out),
            ] = sizes.try_into().unwrap();
            assert_eq!(small_out, large_out, "{what}: outcome");
            if entry == Entry::Approve {
                // `corral done` still reads the full task: its cells are T-528's (PM decision
                // 2026-10-28). Here it is held to the reference's outcome and refusal.
                continue;
            }
            // A proceeding entry checks the checkpoint before its lock and again under it; a
            // refused one stops at the first check.
            let times = if is_stale { 1 } else { 2 };
            let grown = (
                large.own_record_rows() - small.own_record_rows(),
                large.own_record_bytes() - small.own_record_bytes(),
            );
            assert_eq!(
                grown,
                (
                    times * (large_rows.0 - small_rows.0),
                    times * (large_rows.1 - small_rows.1)
                ),
                "{what}: own-record growth"
            );
            for counter in corral_core::work::Counter::DECODES {
                assert_eq!(small.get(counter), large.get(counter), "{what} {counter:?}");
            }
            assert_eq!(
                small.unattributed_byte_remainder(),
                large.unattributed_byte_remainder(),
                "{what}: unattributed bytes"
            );
        }
    }
}
