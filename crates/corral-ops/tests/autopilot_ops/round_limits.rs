use super::*;

#[test]
fn events_explain_effective_round_limit_for_main_and_plan_members() {
    for plan_member in [false, true] {
        for turbo in [false, true] {
            let mut fx = if plan_member {
                let (mut fx, _, ids) = plan_targets::plan();
                plan_targets::dispatch(&mut fx, &ids[0]);
                fx.change("widget.txt", "defective\n");
                fx.done();
                fx.sync();
                fx
            } else {
                let fx = Fx::new(|r| {
                    r.gate = gate(&["true"]);
                    r.reviewer = Some("rev".into());
                    r.remove_worktree = Some(false);
                });
                fx.change("widget.txt", "defective\n");
                fx.done();
                fx.sync();
                fx
            };
            assert_eq!(fx.stage(), Some(AutoStage::Reviewing));
            fx.ctx.store.config.review.max_rounds = Some(2);
            fx.ctx.store.config.automation.turbo = turbo;
            fx.ctx.store.save_config().unwrap();
            fx.ctx
                .store
                .update(&fx.id, |t| {
                    t.rounds = 4;
                    Ok(())
                })
                .unwrap();
            fx.verdict(Verdict::Changes, "Fix the widget.");
            let task = fx.sync();
            assert_eq!(task.rounds, if turbo { 5 } else { 4 });
            assert_eq!(
                task.status,
                if turbo {
                    Status::Running
                } else {
                    Status::NeedsMe
                }
            );
            let events = plan_targets::events(&fx, &fx.id, "autopilot");
            let event = events.last().unwrap();
            assert_eq!(event["max_rounds"], 2);
            assert!(event.get("effective_max_rounds").is_some(), "{event}");
            assert_eq!(
                event["effective_max_rounds"],
                if turbo {
                    serde_json::Value::Null
                } else {
                    serde_json::json!(2)
                }
            );
            assert_eq!(
                event["round_limit_source"],
                if turbo {
                    "automation.turbo"
                } else {
                    "task_review_policy.max_rounds"
                }
            );
        }
    }
}
