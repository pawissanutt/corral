use corral_core::{PlanRunState, Status};
use serde_json::Value;

use crate::{queue_fixture, queue_support::Fixture};

const PLAN: &str = "---\ngoal: Ship widget\nnon_goals: []\ndecisions: []\naffected_paths: [widget.rs]\ncontracts: []\ndependencies: []\nacceptance: [{scenario: spin, expected: clockwise}]\nverification: [check]\nowner_decisions: []\n---\n";

fn submit(f: &Fixture, id: &str) {
    let path = f.temp.path().join("plan.md");
    std::fs::write(&path, PLAN).unwrap();
    f.ok(&["plan", "submit", id, path.to_str().unwrap()]);
}

#[test]
fn pending_ready_plan_keeps_both_pm_json_readers_usable_without_dispatch_suggestion() {
    // Given a Ready task with a current plan awaiting review.
    let f = queue_fixture();
    let task = f.task("app", Status::Ready);
    submit(&f, &task.id);
    let before = f.ctx.store.list().unwrap();
    let events = f.ctx.store.events().unwrap();
    // When either workspace PM reader computes its JSON response.
    for command in ["brief", "next"] {
        let output = f.ok(&["pm", command, "--json"]);
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        let action = if command == "brief" {
            &value["next"]
        } else {
            &value
        };
        // Then the reader succeeds and cannot suggest implicit dispatch of this task.
        assert_ne!(action["kind"], "dispatch");
    }
    assert_eq!(f.ctx.store.list().unwrap(), before);
    assert_eq!(f.ctx.store.events().unwrap(), events);
}

#[test]
fn explicit_queued_replacement_starts_after_pause_is_released() {
    // Given a pending plan and an explicit replacement queued while paused.
    let f = queue_fixture();
    let task = f.task("app", Status::Ready);
    submit(&f, &task.id);
    f.ok(&["queue", "pause"]);
    f.ok(&["dispatch", &task.id, "--plan", "--queue"]);
    let queued = f.ctx.store.get(&task.id).unwrap();
    assert!(queued.queued && queued.dispatch_plan.as_ref().unwrap().plan);
    assert!(queued.dispatch_plan.as_ref().unwrap().explicit_plan);
    // When queue draining resumes in a separate CLI process.
    f.ok(&["queue", "resume"]);
    f.ok(&["sync"]);
    // Then the persisted replacement permission produces an actual planner.
    let started = f.ctx.store.get(&task.id).unwrap();
    assert_eq!(started.status, Status::Running);
    assert_eq!(started.plan_runs.len(), 1);
    assert_eq!(started.plan_runs[0].state, PlanRunState::Planning);
    assert!(started.runs.is_empty());
    assert!(!started.queued);
    assert_eq!(f.state()["creates"], 1);
}

#[test]
fn implicit_queued_planning_stays_blocked_when_a_plan_is_submitted() {
    // Given implicit plan-required work queued before a plan existed.
    let f = queue_fixture();
    let task = f.task("app", Status::Ready);
    f.ctx
        .store
        .update(&task.id, |t| {
            t.plan_required = Some(true);
            Ok(())
        })
        .unwrap();
    f.ok(&["queue", "pause"]);
    f.ok(&["dispatch", &task.id, "--queue"]);
    submit(&f, &task.id);
    let before = f.ctx.store.get(&task.id).unwrap();
    assert!(!before.dispatch_plan.as_ref().unwrap().explicit_plan);
    // When a current pending plan appears before the implicit queue drains.
    f.ok(&["queue", "resume"]);
    f.ok(&["sync"]);
    // Then mode alone is not replacement permission.
    let blocked = f.ctx.store.get(&task.id).unwrap();
    assert_eq!(blocked.plan_runs, before.plan_runs);
    assert!(blocked.queued);
    assert_eq!(blocked.status, Status::Ready);
    assert_eq!(f.state()["creates"], 0);
}
