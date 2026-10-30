use super::*;

#[test]
fn planning_recipients_use_native_delivery_for_owner_kick_and_sync() {
    for (review, sync) in [(false, false), (false, true), (true, false), (true, true)] {
        // Given a planning-only container bound through the real admission and hello path.
        let fx = ts::fx();
        let id = ts::worker_task(&fx, "p1");
        let socket = fx.ctx.herdr.socket_path().unwrap();
        fx.ctx
            .store
            .update(&id, |task| {
                task.runs.clear();
                let revision = crate::plan_spec_revision(task)?;
                task.plan_runs.push(serde_json::from_value(json!({
                    "id":"D1", "profile":"pi", "harness":"pi-native", "delivery":"inbox",
                    "spec_revision":revision, "started":"2026-10-29T00:00:00Z",
                    "output_dir":fx.ctx.store.state_dir().join("plans"),
                    "rounds":0, "state":if review { "submitted" } else { "planning" },
                    "herdr":{"workspace_id":"w1","pane_id":"p1","socket":socket}
                }))?);
                if review {
                    task.plans.push(serde_json::from_value(json!({
                        "id":"P1", "run":"D1", "spec_revision":revision,
                        "artifact":{
                            "run":"D1", "kind":"plan", "title":"Plan", "path":"plans/P1.md",
                            "source":"/tmp/plan.md", "sha256":"abc", "bytes":1,
                            "added":"2026-10-29T00:00:00Z"
                        },
                        "document":{
                            "goal":"Plan", "non_goals":[], "decisions":[], "affected_paths":[],
                            "contracts":[], "dependencies":[], "acceptance":[],
                            "verification":[], "owner_decisions":[]
                        },
                        "reviews":[{
                            "id":"PR1", "reviewer":"pi", "policy_digest":"fixture",
                            "delivery":"inbox", "at":"2026-10-29T00:00:00Z",
                            "herdr":{"workspace_id":"w1","pane_id":"p1","socket":socket}
                        }]
                    }))?);
                }
                Ok(())
            })
            .unwrap();
        let (recipient, target) = if review {
            (
                Recipient::PlanReview {
                    task: id.clone(),
                    plan: "P1".into(),
                    review: "PR1".into(),
                },
                BindTarget::PlanReviewer {
                    task: id,
                    plan: "P1".into(),
                    review: "PR1".into(),
                },
            )
        } else {
            (
                Recipient::PlanRun {
                    task: id.clone(),
                    plan_run: "D1".into(),
                },
                BindTarget::Planner {
                    task: id,
                    run: "D1".into(),
                },
            )
        };
        let _runtime = runtime(&fx, &[&recipient]);
        let host = bound(&fx, &target, "p1", "s-planning");
        let fake = FakeHost::start(&fx, &recipient, &host);
        let seq = post(&fx, &recipient, "owner_reply", "Owner instruction.");
        // When either immediate delivery or sync processes this native recipient.
        if sync {
            owner::reconcile_all(&fx.ctx).unwrap();
        } else {
            owner::kick(&fx.ctx, &recipient).unwrap();
        }
        // Then a typed transcript observation settles the message through the native adapter.
        assert!(
            observed(&fx, &recipient, seq),
            "review={review}, sync={sync}"
        );
        assert!(!fake.submits().is_empty());
        let inbox = Inbox::new(&fx.ctx.store, recipient).unwrap();
        assert!(
            inbox
                .envelopes()
                .unwrap()
                .iter()
                .all(|e| e.adapter == ADAPTER)
        );
    }
}
