//! Worker-launch resume and require_checkpoint through their real entries (plan T-253 section
//! 4, "Worker-launch resume" and the retry/availability require_checkpoint scenario). The launch
//! is the action budget's real sync tick; retry and the availability handoff are their real
//! automatic entries. Each runs at 1x and 10x of the history it may read only as own records,
//! and must produce the reference outcome with own-record cells grown by exactly those rows'
//! stored bytes and every unattributed and emitted cell unchanged.
use corral_core::work::{self, HotScope, WorkSnapshot};
use corral_core::{ObligationSource, Task};

use super::action_budget::{Measured, emitted, measure, own, unattributed};
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

#[test]
fn worker_launch_resume_selects_on_metadata_through_the_tick() {
    for shape in [
        Shape::AllStale,
        Shape::OlderCompatible,
        Shape::NewestCompatible,
    ] {
        let mut sizes = Vec::new();
        for stale in [1, 10] {
            let bench = build(Kind::Worker, Variant::Base);
            shape_source(&bench, shape, stale);
            settle(&bench);
            let before: Task = bench.ctx.store.get(&bench.id).unwrap();
            let scanned = scan(&bench);
            let m = measure(&bench, Kind::Worker);
            assert!(
                m.failures.is_empty(),
                "{shape:?} x{stale}: {:?}",
                m.failures
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
        same_cells(&small, &large, &format!("{shape:?}"));
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
        assert_eq!(grown, want, "{shape:?}: own-record (rows, bytes) growth");
    }
}

/// A launched worker R1 with `n` waits and `n` decisions frozen into its newest checkpoint.
fn obligated(n: usize) -> (Fx, String) {
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
    // Retry acts on a run its worker finished: the task in Review.
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

#[test]
fn require_checkpoint_reads_the_run_own_obligations_through_retry_and_handoff() {
    for availability in [false, true] {
        let mut sizes = Vec::new();
        for n in [1, 10] {
            let (fx, id) = obligated(n);
            // The head form refuses and admits exactly as the full form does.
            let full: Task = fx.ctx.store.get(&id).unwrap();
            let head = fx.ctx.store.head(&id).unwrap();
            assert_eq!(
                crate::resume::require_checkpoint_head(&head, "R1").map_err(|e| e.to_string()),
                crate::resume::require_checkpoint(&full, full.current_run().unwrap())
                    .map_err(|e| e.to_string()),
                "x{n}"
            );
            let expected = obligation_rows(&fx, &id);
            let key =
                crate::integration::RepairKey::new(&full, full.current_run().unwrap(), "lost")
                    .unwrap();
            let delta = timed(|| {
                let outcome = if availability {
                    crate::dispatch::availability_retry(&fx.ctx, &id, "R1")
                } else {
                    crate::dispatch::auto_retry(
                        &fx.ctx,
                        &id,
                        1,
                        corral_core::AutopilotState {
                            stage: corral_core::AutoStage::Gating,
                            reason: None,
                            sha: None,
                            at: fixture::now(),
                        },
                        &key,
                    )
                };
                assert!(
                    matches!(outcome, Ok(crate::DispatchOutcome::Started { .. })),
                    "availability={availability} x{n}: {outcome:?}"
                );
            });
            delta.check_identity().unwrap();
            assert_eq!(delta.unattributed_hydrations(), 0, "x{n}");
            sizes.push((delta, expected));
        }
        let [(small, small_rows), (large, large_rows)] = sizes.try_into().unwrap();
        let grown = (
            large.own_record_rows() - small.own_record_rows(),
            large.own_record_bytes() - small.own_record_bytes(),
        );
        // retry_with checks the checkpoint before taking the dispatch lock and again under it.
        assert_eq!(
            grown,
            (
                2 * (large_rows.0 - small_rows.0),
                2 * (large_rows.1 - small_rows.1)
            ),
            "availability={availability}: own-record growth"
        );
        for counter in corral_core::work::Counter::DECODES {
            assert_eq!(small.get(counter), large.get(counter), "{counter:?}");
        }
        assert_eq!(
            small.unattributed_byte_remainder(),
            large.unattributed_byte_remainder(),
            "availability={availability}: unattributed bytes"
        );
    }
}
