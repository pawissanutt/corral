//! Head forms of the launch's own-record and emitted reads and of the aggregate-attempt guard
//! (plan T-253 section 4: Worker-launch resume, `require_checkpoint`, the applied-lessons
//! snapshot and the stricter A->B->A guards).
use super::*;
use corral_core::work::{self, Counter, HotScope, WorkSnapshot};
use corral_core::{ObligationSource, Task, TaskRef as _, WaitHandle, WaitRequest};

fn measure<T>(call: impl FnOnce() -> T) -> (T, WorkSnapshot, u64) {
    let _hot = HotScope::enter();
    let before = work::snapshot();
    let git = corral_git::work::invocations();
    let value = call();
    let git = corral_git::work::invocations() - git;
    let delta = work::snapshot().since(&before);
    delta.check_identity().unwrap();
    (value, delta, git)
}

fn checkpoint(fx: &Fx, id: &str) {
    let packet = crate::progress_readback(&fx.ctx, id).unwrap();
    crate::save_checkpoint(
        &fx.ctx,
        id,
        &corral_core::CheckpointUpdate {
            task: packet.task,
            run: packet.run,
            spec_revision: packet.spec_revision,
            expected_revision: packet
                .checkpoints
                .last()
                .map_or(0, |entry| entry.checkpoint.revision),
            progress_revision: packet.progress.as_ref().map_or(0, |p| p.revision),
            head: packet.head,
            base: packet.base,
            tree: packet.tree,
            criterion_ids: vec![],
            evidence_refs: vec![],
            next_action: "Continue the change".into(),
            decisions: vec![],
            waits: vec![],
            resources: vec![],
        },
    )
    .unwrap();
}

/// A launched worker run R1 with one compatible checkpoint.
fn worker(fx: &Fx) -> String {
    let task = fx.task("resume fixture", Status::Ready);
    fx.start(&task.id);
    checkpoint(fx, &task.id);
    task.id
}

#[derive(Clone, Copy, Debug)]
enum Checkpoints {
    /// Every checkpoint, the compatible one included, is stale.
    AllStale,
    /// The compatible checkpoint stays; stale ones follow it, the newest stale too.
    OlderCompatible,
    /// The newest checkpoint is the compatible one.
    NewestCompatible,
}

/// `stale` checkpoints of R1 whose recorded head no longer matches, after the compatible one.
fn stale_checkpoints(fx: &Fx, id: &str, shape: Checkpoints, stale: usize) {
    fx.ctx
        .store
        .update(id, |t| {
            let run = t.current_run_mut().unwrap();
            let template = run.checkpoints.last().unwrap().clone();
            if matches!(shape, Checkpoints::AllStale) {
                for record in &mut run.checkpoints {
                    record.checkpoint.input.head = "f".repeat(40);
                }
            }
            if !matches!(shape, Checkpoints::NewestCompatible) {
                for i in 0..=stale {
                    let mut record = template.clone();
                    record.artifact = format!("{}-stale-{i:03}", template.artifact);
                    record.checkpoint.input.head = "f".repeat(40);
                    run.checkpoints.push(record);
                }
            }
            Ok(())
        })
        .unwrap();
}

fn packet(fx: &Fx, id: &str) -> (serde_json::Value, WorkSnapshot, u64) {
    let head = fx.ctx.store.head(id).unwrap();
    let shell = head.shell_task().unwrap();
    let run = head.current_run_record().unwrap().unwrap();
    let (packet, cells, git) = measure(|| {
        crate::resume::resume_packet_head(&fx.ctx, &head, &shell, head.current_run.as_deref(), &run)
            .unwrap()
    });
    (serde_json::to_value(packet).unwrap(), cells, git)
}

#[test]
fn resume_packet_head_equals_the_full_packet_and_reads_only_scanned_metas() {
    for shape in [
        Checkpoints::AllStale,
        Checkpoints::OlderCompatible,
        Checkpoints::NewestCompatible,
    ] {
        let mut sizes = Vec::new();
        for stale in [1, 10] {
            let fx = Fx::new();
            let id = worker(&fx);
            stale_checkpoints(&fx, &id, shape, stale);
            let task: Task = fx.ctx.store.get(&id).unwrap();
            let full = serde_json::to_value(
                crate::resume_packet(&fx.ctx, &task, task.current_run().unwrap()).unwrap(),
            )
            .unwrap();
            let (head, cells, git) = packet(&fx, &id);
            assert_eq!(head, full, "{shape:?} x{stale}: the head packet differs");
            sizes.push((stale, cells, git));
        }
        let [(small, a, git_a), (large, b, git_b)] = sizes.try_into().unwrap();
        let added = u64::try_from(large - small).unwrap();
        // A scanned meta and its registration lookup are each one own record.
        let scanned = match shape {
            Checkpoints::NewestCompatible => 0,
            _ => 2 * added,
        };
        assert_eq!(
            b.own_record_rows() - a.own_record_rows(),
            scanned,
            "{shape:?}: own-record rows"
        );
        assert_eq!(git_a, git_b, "{shape:?}: identity work follows the metas");
        assert_eq!(
            a.emitted_rows(),
            b.emitted_rows(),
            "{shape:?}: a rejected candidate's payload was decoded"
        );
        for counter in Counter::DECODES {
            assert_eq!(a.get(counter), b.get(counter), "{shape:?} {counter:?}");
        }
        assert_eq!(
            (a.unattributed_hydrations(), b.unattributed_hydrations()),
            (0, 0)
        );
    }
}

fn waits(fx: &Fx, id: &str, n: usize) {
    for i in 0..n {
        crate::register_wait(
            &fx.ctx,
            id,
            WaitRequest {
                version: 1,
                id: format!("build-{i:03}"),
                run: "R1".into(),
                condition: "The external build completes".into(),
                owner: "fixture".into(),
                source: ObligationSource::Worker,
                deadline: None,
                handle: WaitHandle::Channel {
                    channel_id: format!("build-{i:03}"),
                    capability: None,
                },
            },
            fixture::now(),
        )
        .unwrap();
    }
}

#[test]
fn require_checkpoint_head_reads_the_run_own_obligations_as_own_records() {
    let mut measured = Vec::new();
    for n in [1, 10] {
        let fx = Fx::new();
        let task = fx.task("checkpoint fixture", Status::Ready);
        fx.start(&task.id);
        waits(&fx, &task.id, n);
        checkpoint(&fx, &task.id);
        let full: Task = fx.ctx.store.get(&task.id).unwrap();
        let expected = crate::resume::require_checkpoint(&full, full.current_run().unwrap())
            .map_err(|e| e.to_string());
        let head = fx.ctx.store.head(&task.id).unwrap();
        let (actual, cells, _) = measure(|| {
            crate::resume::require_checkpoint_head(&head, "R1").map_err(|e| e.to_string())
        });
        assert_eq!(actual, expected, "x{n}");
        // An obligation changed after the checkpoint is refused by both forms alike.
        waits(&fx, &task.id, n + 1);
        let full: Task = fx.ctx.store.get(&task.id).unwrap();
        let stale = crate::resume::require_checkpoint(&full, full.current_run().unwrap())
            .unwrap_err()
            .to_string();
        let head = fx.ctx.store.head(&task.id).unwrap();
        assert_eq!(
            crate::resume::require_checkpoint_head(&head, "R1")
                .unwrap_err()
                .to_string(),
            stale
        );
        measured.push(cells);
    }
    let [a, b] = measured.try_into().unwrap();
    for counter in Counter::DECODES {
        assert_eq!(a.get(counter), b.get(counter), "{counter:?}");
    }
    assert_eq!(
        a.unattributed_byte_remainder(),
        b.unattributed_byte_remainder()
    );
    // The added waits are own records, and the frozen payload grew with them.
    assert_eq!(b.own_record_rows() - a.own_record_rows(), 9);
    assert!(b.own_record_bytes() > a.own_record_bytes());
}

#[test]
fn applied_lessons_read_one_event_of_their_run() {
    let mut measured = Vec::new();
    for n in [1, 10] {
        let fx = Fx::new();
        let task = fx.task("lesson fixture", Status::Ready);
        fx.start(&task.id);
        let store = &fx.ctx.store;
        for i in 0..n {
            for kind in ["lessons_applied", "fixture_history"] {
                store
                    .log_event(
                        Some(&task.id),
                        kind,
                        serde_json::json!({"run": format!("H{i:03}"), "lessons": []}),
                    )
                    .unwrap();
            }
        }
        let full: Task = store.get(&task.id).unwrap();
        let run = full.current_run().unwrap();
        let (lessons, cells, _) =
            measure(|| crate::lessons::applied_lessons(&fx.ctx, &full, run).unwrap());
        assert_eq!(lessons, Some(Vec::new()), "x{n}: the launch's own snapshot");
        measured.push(cells);
    }
    let [a, b] = measured.try_into().unwrap();
    assert_eq!(a.get(Counter::EventDecodes), b.get(Counter::EventDecodes));
    assert_eq!(a.get(Counter::EventDecodes), 1);
}

/// A completed aggregate review edited and edited back (A->B->A) between the attempt read and
/// the acceptance guard refuses the acceptance as a genuine edit does, and the next attempt
/// accepts in both cases.
#[test]
fn aggregate_acceptance_refuses_an_edited_back_review_as_a_genuine_change() {
    for code in [false, true] {
        for back in [true, false] {
            let (mut fx, id, attempt, _) = aggregate::scenario(code, true);
            let repo = fx.ctx.store.project_path("app").unwrap();
            let main = fixture::git(&repo, &["rev-parse", "main"]);
            let status = fx.ctx.store.head(&id).unwrap().status;
            let review = fx
                .ctx
                .store
                .head(&id)
                .unwrap()
                .last_aggregate_attempt
                .unwrap()
                .current_review
                .unwrap();
            let plain = fx.ctx.store.clone();
            let edited = Arc::new(Mutex::new(false));
            let once = edited.clone();
            let (task, attempt_id) = (id.clone(), attempt.clone());
            fx.ctx.store = fx.ctx.store.with_submission_barrier(Arc::new(move |point| {
                if point != "submission_entered"
                    || std::mem::replace(&mut *once.lock().unwrap(), true)
                {
                    return;
                }
                let notes = [Some("edited".to_owned()), review.notes.clone()];
                for notes in notes.into_iter().take(if back { 2 } else { 1 }) {
                    plain
                        .update_head(&task, |t| {
                            t.edit_aggregate_review(&attempt_id, &review.id, |r| {
                                r.notes = notes;
                                Ok(())
                            })
                        })
                        .unwrap();
                }
            }));
            let refused = crate::plan_accept(&fx.ctx, &id, &attempt, true).unwrap_err();
            assert!(crate::operating::refused(&refused), "{refused:#}");
            assert!(
                refused
                    .to_string()
                    .contains("aggregate attempt changed before acceptance"),
                "{refused:#}"
            );
            assert_eq!(fixture::git(&repo, &["rev-parse", "main"]), main);
            let record = authorizations(&fx.ctx, &id, OperatingActionKind::AcceptAggregate)
                .pop()
                .unwrap();
            assert!(matches!(
                record.disposition,
                SubmissionDisposition::AuthorizationRefused { .. }
            ));
            assert_eq!(fx.ctx.store.head(&id).unwrap().status, status);
            let accepted = crate::plan_accept(&fx.ctx, &id, &attempt, true).unwrap();
            assert_eq!(accepted.state, corral_core::AggregateLandingState::Landed);
        }
    }
}
