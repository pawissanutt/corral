//! T-592: routine churn under a step is retried on the next sync, never handed to the owner.
use super::*;

fn conflicting(fx: &Fx) {
    fx.change("README.md", "agent version\n");
    fx.done();
    fx.commit_on_main("README.md", "main version\n", "main moved");
}

fn errors(fx: &Fx) -> Vec<String> {
    let events = fx.ctx.store.events().unwrap();
    events
        .iter()
        .filter(|e| e.kind == "autopilot_error" && e.task.as_deref() == Some(fx.id.as_str()))
        .filter_map(|e| e.detail["error"].as_str().map(str::to_owned))
        .collect()
}

fn escalations(fx: &Fx) -> usize {
    let events = fx.ctx.store.events().unwrap();
    let escalated = |e: &&corral_core::Event| {
        e.kind == "autopilot"
            && e.task.as_deref() == Some(fx.id.as_str())
            && e.detail["to"] == "escalated"
    };
    events.iter().filter(escalated).count()
}

fn assert_retried(fx: &Fx, task: &Task, refusal: &str) {
    assert_eq!(task.status, Status::Review, "{}", fx.reason());
    assert_ne!(fx.stage(), Some(AutoStage::Escalated));
    assert_eq!(task.rounds, 0);
    assert_eq!(escalations(fx), 0);
    let errors = errors(fx);
    assert!(errors.iter().any(|e| e.contains(refusal)), "{errors:?}");
}

#[test]
fn a_send_back_waiting_for_finishing_work_retries_instead_of_escalating() {
    // Given a conflicting candidate and another granted Review task whose gate has not started.
    let mut fx = Fx::new(|r| {
        r.gate = gate(&["true"]);
        r.accept = Some(Accept::Auto);
    });
    conflicting(&fx);
    let store = &fx.ctx.store;
    let other = store.create("Other", Some("app"), Status::Review, "");
    let other = other.unwrap().id;
    let run: Run = serde_json::from_value(serde_json::json!({
        "id": "R1", "agent": "fake", "worktree": fx.app.join(".worktrees/other"),
        "branch": "corral/other", "base": "main", "base_sha": git(&fx.app, &["rev-parse", "main"]),
        "started": "2026-10-29T00:00:00Z", "outcome": "done",
    }))
    .unwrap();
    store
        .update(&other, |t| {
            t.runs.push(run.clone());
            Ok(())
        })
        .unwrap();
    let id = fx.id.clone();
    fx.grant(&[id, other.clone()]);

    // When autopilot's send-back is refused so that finishing work goes first.
    let task = fx.sync();

    // Then the task waits in Review and the next sync delivers the send-back.
    assert_retried(
        &fx,
        &task,
        &format!("waiting for finishing work on {other}"),
    );
    let task = fx.sync();
    assert_eq!(task.status, Status::Running, "{}", fx.reason());
    assert_eq!(task.rounds, 1);
    assert_eq!(escalations(&fx), 0);
}

#[test]
fn a_gate_start_during_another_review_launch_retries_instead_of_escalating() {
    // Given a finished candidate that needs its gate.
    let fx = Fx::new(|r| {
        r.gate = gate(&["true"]);
        r.accept = Some(Accept::Auto);
    });
    fx.change("widget.txt", "w\n");
    fx.done();

    // When another review launch holds the launch lock during the sync.
    let held = fx.ctx.store.try_lock("review-budget").unwrap().unwrap();
    let _ = corral_ops::sync_at(&fx.ctx, now());

    // Then nothing escalates, and the next sync gates and lands the candidate.
    assert_retried(&fx, &fx.task(), "another review launch is in progress");
    drop(held);
    fx.sync();
    assert_eq!(fx.events("gate_started"), 1, "{}", fx.reason());
    assert_ne!(fx.task().status, Status::NeedsMe, "{}", fx.reason());
    assert_eq!(escalations(&fx), 0);
}
