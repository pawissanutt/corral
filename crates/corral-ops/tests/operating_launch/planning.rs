use super::*;
use corral_core::{PlanRunState, Verdict};

const PLAN: &str = "---\ngoal: Choose bounded implementation\nnon_goals: []\ndecisions: []\naffected_paths: [candidate.txt]\ncontracts: []\ndependencies: []\nacceptance: [{scenario: bounded, expected: verified}]\nverification: [check]\nowner_decisions: []\n---\n";

#[derive(Clone, Copy, Debug)]
enum Path {
    Initial,
    Revision,
    Reviewer,
}

#[test]
fn concrete_planning_paths_refuse_every_revocation_and_collect_running_receipts() {
    for path in [Path::Initial, Path::Revision, Path::Reviewer] {
        for change in [
            Revoke::Pause,
            Revoke::Assist,
            Revoke::Member,
            Revoke::Dispatch,
        ] {
            // Given real disposable Git, a finite manifest, and an independent running gate.
            let mut fx = Fx::new();
            let task = fx.task("planning obligation", Status::Ready);
            fx.ctx
                .store
                .update(&task.id, |task| {
                    task.design = true;
                    Ok(())
                })
                .unwrap();
            let running = fx.task("independent worker", Status::Ready);
            fx.start(&running.id);
            std::fs::write(fx.bin.join("hold-gates"), "").unwrap();
            let running_gate = corral_ops::gate_start(&fx.ctx, &running.id).unwrap();
            let running_paths = corral_ops::gate_paths(&fx.ctx, &running.id, "R1");
            if !matches!(path, Path::Initial) {
                // The initial owner start supplies no authority for automatic revision/review.
                fx.start(&task.id);
                let source = fx.ctx.store.state_dir().join("prepared-plan.md");
                std::fs::write(&source, PLAN).unwrap();
                let plan = corral_ops::plan_submit(&fx.ctx, &task.id, &source).unwrap();
                if matches!(path, Path::Revision) {
                    let review = corral_ops::plan_review(&fx.ctx, &task.id, Some("rev")).unwrap();
                    corral_ops::plan_verdict(
                        &fx.ctx,
                        &task.id,
                        &corral_ops::PlanVerdict {
                            plan: &plan.id,
                            attempt: Some(&review.id),
                            verdict: Verdict::Changes,
                            notes: "Preserve exact revision notes",
                        },
                    )
                    .unwrap();
                }
            }
            fx.grant(&[&task.id, &running.id]);
            let kind = if matches!(path, Path::Reviewer) {
                OperatingActionKind::PlanReview
            } else {
                OperatingActionKind::Plan
            };
            let before = fx.records("submissions.jsonl").len();
            let barrier = Barrier::subscribe(&fx);
            let closed_before = std::thread::scope(|scope| {
                // Both arrival and release subscriptions exist before the real sync executor runs.
                let worker = scope.spawn(|| corral_ops::sync_at(&fx.fresh(), now()).unwrap());
                let mut release = barrier.arrival();
                let closed_before = fx.records("closed.jsonl");
                let original = authorization(&fx, &task.id, kind);
                assert_eq!(
                    original.disposition,
                    SubmissionDisposition::Prepared,
                    "{path:?} {change:?}"
                );
                let prepared = fx.ctx.store.get(&task.id).unwrap();
                let design = prepared.plan_runs.last().unwrap();
                assert_eq!(design.id, "D1");
                assert!(design.herdr.is_some());
                assert!(design.output_dir.join("prompt.md").is_file());
                // When another context commits revocation before releasing final submission.
                revoke(&fx, &task.id, &running.id, change);
                release.write_all(b"R").unwrap();
                worker.join().unwrap();
                // Then no new command or uncertain marker replaces the prepared identity.
                assert_eq!(
                    fx.records("submissions.jsonl").len(),
                    before,
                    "{path:?} {change:?}"
                );
                let record = authorization(&fx, &task.id, kind);
                assert_eq!(record.authorization, original.authorization);
                assert!(matches!(
                    record.disposition,
                    SubmissionDisposition::AuthorizationRefused { .. }
                ));
                let retained = fx.ctx.store.get(&task.id).unwrap();
                assert_eq!(retained.plan_runs.len(), 1);
                assert_eq!(retained.plan_runs[0].rounds, 0);
                assert_eq!(retained.runs.len(), 0);
                assert!(!retained.plan_runs[0].holds_slot());
                match path {
                    Path::Initial => {
                        assert_eq!(retained.plan_runs[0].state, PlanRunState::Planning);
                        assert_eq!(retained.plan_runs[0].herdr, design.herdr);
                    }
                    Path::Revision => {
                        assert_eq!(
                            retained.plan_runs[0].pending_reply.as_deref(),
                            Some("Preserve exact revision notes")
                        );
                        assert_eq!(retained.plan_runs[0].herdr, design.herdr);
                    }
                    Path::Reviewer => {
                        assert!(retained.plans[0].reviews.is_empty());
                        let journal = fx
                            .ctx
                            .store
                            .state_dir()
                            .join(format!("plan-review-{}-P1.json", task.id));
                        let pending: serde_json::Value =
                            serde_json::from_slice(&std::fs::read(journal).unwrap()).unwrap();
                        assert_eq!(pending["review"]["id"], "PR1");
                        assert!(pending["review"]["pending_reply"].is_string());
                        assert!(pending["review"]["herdr"].is_object());
                    }
                }
                closed_before
            });
            drop(barrier);
            // A genuinely submitted process completes during revocation; sync still collects it.
            let receipt = std::process::Command::new("bash")
                .arg(&running_paths.script)
                .env("CORRAL_ROOT", fx.temp.path())
                .stdin(std::process::Stdio::null())
                .output()
                .unwrap();
            assert!(receipt.status.success());
            corral_ops::sync_at(&fx.fresh(), now()).unwrap();
            assert_eq!(fx.records("submissions.jsonl").len(), before);
            let collected = fx.ctx.store.get(&running.id).unwrap();
            assert_eq!(
                collected
                    .current_run()
                    .unwrap()
                    .gate
                    .as_ref()
                    .unwrap()
                    .result,
                corral_core::GateResult::Pass
            );
            assert_eq!(
                collected.current_run().unwrap().gate.as_ref().unwrap().pane,
                running_gate.pane
            );
            assert_eq!(fx.records("closed.jsonl"), closed_before);
            // Restore the same finite identities and reuse exactly one prepared action.
            fx.grant(&[&task.id, &running.id]);
            corral_ops::sync_at(&fx.fresh(), now()).unwrap();
            assert_eq!(
                fx.records("submissions.jsonl").len(),
                before + 1,
                "{path:?} {change:?}"
            );
            let restored = fx.ctx.store.get(&task.id).unwrap();
            assert_eq!(restored.plan_runs.len(), 1);
            assert_eq!(
                restored.plan_runs[0].rounds,
                u32::from(matches!(path, Path::Revision))
            );
            if matches!(path, Path::Reviewer) {
                assert_eq!(restored.plans[0].reviews.len(), 1);
                assert_eq!(restored.plans[0].reviews[0].id, "PR1");
            }
        }
    }
}
