use corral_core::{Status, Verdict};
use corral_ops::{PlanVerdict, plan_policy_set, plan_submit, plan_verdict};

use crate::queue_support::Fixture;

const PLAN: &str = "---\ngoal: Ship fixture\nnon_goals: []\ndecisions: []\naffected_paths: [fixture.rs]\ncontracts: []\ndependencies: []\nacceptance:\n  - scenario: run fixture\n    expected: success\nverification: [cargo test]\nowner_decisions: []\n---\nImplementation.\n";

#[test]
fn approved_plan_stays_bound_to_claim_and_refusal_releases_capacity() {
    let f = Fixture::new();
    let task = f.task("app", Status::Ready);
    plan_policy_set(&f.ctx, &task.id, Some(true)).unwrap();
    for verb in [
        vec!["dispatch", task.id.as_str()],
        vec!["queue", "add", task.id.as_str()],
    ] {
        assert!(!f.cli(&verb).status.success());
    }
    let rejected = f.ctx.store.get(&task.id).unwrap();
    assert!(rejected.runs.is_empty());
    assert!(!rejected.queued);

    let path = f.temp.path().join("plan.md");
    std::fs::write(&path, PLAN).unwrap();
    let plan = plan_submit(&f.ctx, &task.id, &path).unwrap();
    plan_verdict(
        &f.ctx,
        &task.id,
        &PlanVerdict {
            plan: &plan.id,
            attempt: None,
            verdict: Verdict::Approve,
            notes: "Approved fixture",
        },
    )
    .unwrap();
    f.fault("claimed workspace list");
    assert!(!f.cli(&["dispatch", &task.id]).status.success());
    let claimed = f.ctx.store.get(&task.id).unwrap();
    let reservation = claimed.launch.as_ref().unwrap();
    assert_eq!(reservation.approved_plan.as_ref().unwrap().id, plan.id);
    assert!(!reservation.reserved);

    // A persisted claim cannot turn a now-stale approval into permission to launch.
    std::fs::write(&path, format!("{PLAN}Changed implementation.\n")).unwrap();
    let out = f.cli(&["dispatch", &task.id]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("stale plan"));
    let refused = f.ctx.store.get(&task.id).unwrap();
    assert!(!refused.launch.as_ref().unwrap().reserved);
    assert!(!refused.current_run().unwrap().worktree.exists());
    assert_eq!(f.state()["creates"], 0);
    assert_eq!(corral_ops::slot_usage(&f.ctx).unwrap().total(), 0);

    // Restoring the approved bytes recovers the same run and records the bound receipt.
    std::fs::write(&path, PLAN).unwrap();
    f.ok(&["dispatch", &task.id]);
    let started = f.ctx.store.get(&task.id).unwrap();
    assert_eq!(started.runs.len(), 1);
    assert!(started.current_run().unwrap().checkpoint_required);
    let event = f
        .ctx
        .store
        .events()
        .unwrap()
        .into_iter()
        .find(|e| e.kind == "dispatched")
        .unwrap();
    assert_eq!(event.detail["approved_plan"]["plan"], plan.id);
    assert_eq!(
        event.detail["approved_plan"]["sha256"],
        plan.artifact.sha256
    );
}
