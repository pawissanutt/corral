use super::*;

#[test]
fn holds_cancel_aggregate_and_checkpoint_gates_without_a_failure_and_release_requeues() {
    for checkpoint in [false, true] {
        let f = Fixture::new(Delivery::Integration, &["true"]);
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (resume_tx, resume_rx) = std::sync::mpsc::channel();
        std::thread::scope(|scope| {
            let ctx = &f.ctx;
            let id = &f.plan;
            let run = scope.spawn(move || {
                crate::gate::evidence::FAILPOINT.with(|hook| {
                    *hook.borrow_mut() = Some(Box::new(move |name| {
                        if name
                            == if checkpoint {
                                "plan_checkpoint_running"
                            } else {
                                "plan_verify_running"
                            }
                        {
                            ready_tx.send(()).unwrap();
                            resume_rx.recv().unwrap();
                        }
                    }));
                });
                let result = if checkpoint {
                    super::super::checkpoint::plan_checkpoint(ctx, id).map(|a| a.gate)
                } else {
                    plan_verify(ctx, id).map(|a| a.gate.unwrap())
                };
                crate::gate::evidence::FAILPOINT.with(|hook| *hook.borrow_mut() = None);
                result
            });
            ready_rx
                .recv_timeout(std::time::Duration::from_secs(10))
                .unwrap();
            crate::hold::hold(&f.ctx, &f.plan, Some("urgent gate first")).unwrap();
            resume_tx.send(()).unwrap();
            let gate = run.join().unwrap().unwrap();
            assert_eq!(gate.result, GateResult::Cancelled);
            assert!(gate.failed_cmd.is_none());
        });
        let scope = if checkpoint {
            "checkpoint"
        } else {
            "aggregate"
        };
        let attempt = if checkpoint { "C1" } else { "A1" };
        let row = f
            .ctx
            .store
            .gate_attempt(&f.plan, scope, attempt)
            .unwrap()
            .unwrap();
        assert_eq!(row.result.as_deref(), Some("cancelled"));
        assert!(row.ended.is_some());
        assert_eq!(f.ctx.store.head(&f.plan).unwrap().rounds, 0);
        crate::hold::release(&f.ctx, &f.plan).unwrap();
        assert_eq!(
            f.ctx.store.head(&f.plan).unwrap().review_wait,
            Some(corral_core::ReviewWork::Gate)
        );
        if checkpoint {
            let next = super::super::checkpoint::plan_checkpoint(&f.ctx, &f.plan).unwrap();
            assert_eq!(
                (next.id.as_str(), next.gate.result),
                ("C2", GateResult::Pass)
            );
            assert!(next.fix_command.is_none());
        } else {
            let next = plan_verify(&f.ctx, &f.plan).unwrap();
            assert_eq!(
                (next.id.as_str(), next.gate.unwrap().result),
                ("A2", GateResult::Pass)
            );
        }
    }
}
