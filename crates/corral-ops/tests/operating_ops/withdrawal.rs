use super::*;

#[test]
fn withdrawn_owner_start_requires_new_enqueue_after_ready_move() {
    for planning in [false, true] {
        for destination in [
            None,
            Some(Status::Backlog),
            Some(Status::Idea),
            Some(Status::Dropped),
        ] {
            // Given Assist and one explicit owner start that is subsequently withdrawn.
            let fx = Fx::new();
            let task = fx.task("withdrawn owner start", Status::Ready);
            fx.ctx
                .store
                .update(&task.id, |task| {
                    task.design = planning;
                    Ok(())
                })
                .unwrap();
            corral_ops::enqueue(&fx.ctx, &task.id, &Default::default()).unwrap();
            match destination {
                None => {
                    corral_ops::dequeue(&fx.ctx, &task.id).unwrap();
                }
                Some(destination) => {
                    corral_ops::move_task(&fx.ctx, &task.id, destination, false).unwrap();
                }
            }

            // When Ready is restored without another dispatch or enqueue action.
            corral_ops::move_task(
                &fx.ctx,
                &task.id,
                Status::Ready,
                destination == Some(Status::Dropped),
            )
            .unwrap();
            corral_ops::sync_at(&fx.fresh(), now()).unwrap();

            // Then Ready cannot resurrect either a coding or planner start.
            let retained = fx.ctx.store.get(&task.id).unwrap();
            assert!(retained.runs.is_empty(), "{planning} {destination:?}");
            assert!(retained.plan_runs.is_empty(), "{planning} {destination:?}");
            assert!(
                fx.records("submissions.jsonl").is_empty(),
                "{planning} {destination:?}"
            );
            // A new explicit enqueue still authorizes exactly one fresh start.
            corral_ops::enqueue(&fx.fresh(), &task.id, &Default::default()).unwrap();
            corral_ops::sync_at(&fx.fresh(), now()).unwrap();
            assert_eq!(fx.records("submissions.jsonl").len(), 1);
        }
    }
}

#[test]
fn withdrawn_owner_start_does_not_block_a_new_scoped_automatic_start() {
    for planning in [false, true] {
        let mut fx = Fx::new();
        let task = fx.task("new scoped start", Status::Ready);
        fx.ctx
            .store
            .update(&task.id, |task| {
                task.design = planning;
                Ok(())
            })
            .unwrap();
        corral_ops::enqueue(&fx.ctx, &task.id, &Default::default()).unwrap();
        corral_ops::dequeue(&fx.ctx, &task.id).unwrap();
        corral_ops::move_task(&fx.ctx, &task.id, Status::Ready, false).unwrap();
        fx.grant(&[&task.id]);

        // A fresh grant supplies automatic authority, not the withdrawn owner intent.
        corral_ops::sync_at(&fx.fresh(), now()).unwrap();
        let started = fx.ctx.store.get(&task.id).unwrap();
        assert_eq!(started.status, Status::Running);
        assert_eq!(started.runs.len() + started.plan_runs.len(), 1);
        assert_eq!(fx.records("submissions.jsonl").len(), 1);
    }
}
