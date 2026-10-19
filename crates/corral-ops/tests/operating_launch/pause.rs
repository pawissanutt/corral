use super::*;

fn enqueue(fx: &Fx, planning: bool) -> corral_core::ActionAuthorization {
    let task = fx.task("queued owner intent", Status::Ready);
    fx.ctx
        .store
        .update(&task.id, |task| {
            task.design = planning;
            Ok(())
        })
        .unwrap();
    let queued = corral_ops::enqueue(&fx.fresh(), &task.id, &Default::default()).unwrap();
    let id = queued.dispatch_plan.unwrap().authorization.unwrap();
    let record = fx.ctx.store.authorized_submission(&id).unwrap();
    assert_eq!(record.disposition, SubmissionDisposition::Prepared);
    assert!(matches!(
        record.authorization.origin,
        OperatingOrigin::ExplicitOwner { .. }
    ));
    record.authorization
}

#[test]
fn enqueue_while_paused_retains_owner_consent_after_resume() {
    for planning in [false, true] {
        // Given an explicit coding or planning intent recorded while dispatch is paused.
        let fx = Fx::new();
        corral_ops::dispatch_pause(&fx.ctx, true).unwrap();
        let approval = enqueue(&fx, planning);
        corral_ops::sync_at(&fx.fresh(), now()).unwrap();
        assert!(fx.records("submissions.jsonl").is_empty());
        assert!(fx.records("panes.json").is_empty());
        // When only the dispatch pause is lifted and a fresh process drains the queue.
        corral_ops::dispatch_pause(&fx.fresh(), false).unwrap();
        corral_ops::sync_at(&fx.fresh(), now()).unwrap();
        // Then the original consent submits once without being refreshed or widened.
        assert_eq!(fx.records("submissions.jsonl").len(), 1, "{planning}");
        let record = fx.ctx.store.authorized_submission(&approval.id).unwrap();
        assert_eq!(record.authorization, approval);
        assert_eq!(record.disposition, SubmissionDisposition::Submitted);
        assert!(record.authorization_history.is_empty());
        let task = fx.ctx.store.get(&approval.action.task).unwrap();
        assert_eq!(task.status, Status::Running);
        assert_eq!(task.runs.len() + task.plan_runs.len(), 1);
    }
}

#[test]
fn pause_during_prepared_owner_reservation_blocks_callback_and_retains_consent() {
    for planning in [false, true] {
        // Given queued owner consent and a subscribed barrier before final submission.
        let fx = Fx::new();
        let approval = enqueue(&fx, planning);
        let barrier = Barrier::subscribe(&fx);
        std::thread::scope(|scope| {
            let worker = scope.spawn(|| corral_ops::sync_at(&fx.fresh(), now()).unwrap());
            let mut release = barrier.arrival();
            let prepared = fx.ctx.store.get(&approval.action.task).unwrap();
            assert_eq!(prepared.runs.len() + prepared.plan_runs.len(), 1);
            // When another context pauses dispatch before releasing the prepared executor.
            corral_ops::dispatch_pause(&fx.fresh(), true).unwrap();
            release.write_all(b"R").unwrap();
            worker.join().unwrap();
        });
        drop(barrier);
        // Then no launch callback ran, and the exact consent remains definitely unsubmitted.
        assert!(fx.records("submissions.jsonl").is_empty());
        let record = fx.ctx.store.authorized_submission(&approval.id).unwrap();
        assert_eq!(record.authorization, approval);
        assert!(matches!(
            record.disposition,
            SubmissionDisposition::AuthorizationRefused { .. }
        ));
        assert!(record.authorization_history.is_empty());
        let prepared = fx.ctx.store.get(&approval.action.task).unwrap();
        corral_ops::sync_at(&fx.fresh(), now()).unwrap();
        assert!(fx.records("submissions.jsonl").is_empty());
        // Resuming reuses that reservation and its original run, pane and owner consent.
        corral_ops::dispatch_pause(&fx.fresh(), false).unwrap();
        corral_ops::sync_at(&fx.fresh(), now()).unwrap();
        assert_eq!(fx.records("submissions.jsonl").len(), 1, "{planning}");
        let resumed = fx.ctx.store.get(&approval.action.task).unwrap();
        assert_eq!(resumed.runs.len() + resumed.plan_runs.len(), 1);
        assert_eq!(
            resumed.current_run().map(|run| &run.herdr),
            prepared.current_run().map(|run| &run.herdr)
        );
        assert_eq!(
            resumed.plan_runs.last().map(|run| &run.herdr),
            prepared.plan_runs.last().map(|run| &run.herdr)
        );
        let record = fx.ctx.store.authorized_submission(&approval.id).unwrap();
        assert_eq!(record.authorization, approval);
        assert_eq!(record.disposition, SubmissionDisposition::Submitted);
        assert!(record.authorization_history.is_empty());
    }
}

#[test]
fn resume_does_not_refresh_owner_consent_after_meaningful_changes() {
    for planning in [false, true] {
        for change in ["dispatch", "review", "authority", "contract"] {
            // Given owner intent queued during a pause, before a meaningful change.
            let fx = Fx::new();
            corral_ops::dispatch_pause(&fx.ctx, true).unwrap();
            let approval = enqueue(&fx, planning);
            let store = Store::open(fx.temp.path()).unwrap();
            match change {
                "dispatch" => {
                    store
                        .edit_config(|doc| doc.set_str("dispatch.auto_backlog", "false"))
                        .unwrap();
                }
                "review" => {
                    store
                        .edit_config(|doc| doc.set_str("review.max_rounds", "7"))
                        .unwrap();
                }
                "authority" => {
                    store
                        .edit_config(|doc| doc.set_str("automation.authority.dispatch", "true"))
                        .unwrap();
                }
                "contract" => {
                    store
                        .update(&approval.action.task, |task| {
                            task.body.push_str("\nChanged scope");
                            Ok(())
                        })
                        .unwrap();
                }
                _ => unreachable!(),
            };
            // When dispatch resumes without another explicit owner action.
            corral_ops::dispatch_pause(&fx.fresh(), false).unwrap();
            corral_ops::sync_at(&fx.fresh(), now()).unwrap();
            // Then none of these changes is treated as a pause-only consent renewal.
            assert!(
                fx.records("submissions.jsonl").is_empty(),
                "{planning} {change}"
            );
            let record = fx.ctx.store.authorized_submission(&approval.id).unwrap();
            assert_eq!(record.authorization, approval);
            assert!(record.authorization_history.is_empty());
            assert_ne!(record.disposition, SubmissionDisposition::Submitted);
        }
    }
}
