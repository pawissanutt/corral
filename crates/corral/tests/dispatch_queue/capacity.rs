use std::sync::{Arc, Barrier};

use corral_core::{GateResult, ReviewWork, Status};
use corral_ops::{DispatchOpts, DispatchOutcome};

use crate::queue_support::Fixture;

fn review_fixture() -> Fixture {
    let mut f = Fixture::new();
    f.ctx.store.config.review.gate = Some(vec!["true".into()]);
    f.ctx.store.config.review.reviewer = Some("generic".into());
    f.ctx.store.save_config().unwrap();
    f
}

fn finished(f: &Fixture, project: &str) -> String {
    let task = f.task(project, Status::Ready);
    f.ok(&["dispatch", &task.id]);
    f.ok(&[
        "done",
        &task.id,
        "--summary",
        "Verified: fixture\nNot verified: none",
    ]);
    task.id
}

#[test]
fn full_global_capacity_blocks_manual_gate_and_reviewer_before_pane_creation() {
    let f = review_fixture();
    let reviewed = finished(&f, "app");
    let busy = f.task("other", Status::Ready);
    f.ok(&["dispatch", &busy.id]);
    for args in [vec!["gate", &reviewed], vec!["review-run", &reviewed]] {
        let out = f.cli(&args);
        assert!(!out.status.success());
        assert!(String::from_utf8_lossy(&out.stderr).contains("waiting for a slot"));
    }
    assert_eq!(f.state()["creates"], 2);
    assert!(
        f.ctx
            .store
            .get(&reviewed)
            .unwrap()
            .review_reservation
            .is_none()
    );
}

#[test]
fn finished_reviewer_frees_slot_for_waiting_gate_before_ready_dispatch() {
    let mut f = review_fixture();
    let first = finished(&f, "app");
    let second = finished(&f, "app");
    let ready = f.task("other", Status::Ready);
    f.ok(&["queue", "add", &ready.id]);
    f.ctx.store.config.review.autopilot = Some(true);
    f.ctx.store.save_config().unwrap();
    // Execute the generated script before the first sync, keeping its policy identity stable.
    f.ok(&["gate", &first]);
    let paths = corral_ops::gate_paths(&f.ctx, &first, "R1");
    let output = std::process::Command::new("bash")
        .arg(&paths.script)
        .env("CORRAL_ROOT", f.temp.path())
        .env_remove("CORRAL_GATE_ATTEMPT")
        .env_remove("CORRAL_CANDIDATE_HEAD")
        .env_remove("CORRAL_INTEGRATION_BASE")
        .env_remove("HERDR_PANE_ID")
        .env_remove("HERDR_WORKSPACE_ID")
        .env_remove("HERDR_TAB_ID")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    f.ok(&["sync"]);
    let waiting = f.ctx.store.get(&second).unwrap();
    assert_eq!(waiting.status, Status::Review);
    assert_eq!(waiting.review_wait, Some(ReviewWork::Gate));
    assert!(
        waiting
            .current_run()
            .unwrap()
            .autopilot
            .as_ref()
            .unwrap()
            .reason
            .as_ref()
            .unwrap()
            .contains("waiting for a slot")
    );
    let before = f.state()["creates"].as_u64().unwrap();

    f.ok(&["verdict", &first, "approve", "--notes", "fixture approval"]);
    f.ok(&["sync"]);

    let task = f.ctx.store.get(&second).unwrap();
    assert!(
        task.current_run().unwrap().gate.is_some(),
        "waiting task: {task:#?}\nfirst task: {:#?}\nevents: {:#?}",
        f.ctx.store.get(&first).unwrap(),
        f.ctx.store.events().unwrap()
    );
    assert_eq!(
        task.current_run().unwrap().gate.as_ref().unwrap().result,
        GateResult::Running
    );
    assert!(task.review_wait.is_none());
    assert!(f.ctx.store.get(&ready.id).unwrap().runs.is_empty());
    assert_eq!(f.state()["creates"], before + 1);
    let usage = corral_ops::slot_usage(&f.ctx).unwrap();
    assert_eq!(
        (usage.tasks, usage.reviews, usage.gates, usage.total()),
        (0, 0, 1, 1)
    );
}

#[test]
fn gate_and_dispatch_barrier_contenders_share_one_atomic_reservation() {
    let f = review_fixture();
    let reviewed = finished(&f, "app");
    let ready = f.task("other", Status::Ready);
    let barrier = Arc::new(Barrier::new(3));
    let (gate, dispatch) = std::thread::scope(|s| {
        let ctx = &f.ctx;
        let review_id = &reviewed;
        let id = &ready.id;
        let b = barrier.clone();
        let gate = s.spawn(move || {
            b.wait();
            corral_ops::gate_start(ctx, review_id)
        });
        let b = barrier.clone();
        let dispatch = s.spawn(move || {
            b.wait();
            corral_ops::dispatch(
                ctx,
                id,
                &DispatchOpts {
                    queue_if_blocked: true,
                    ..DispatchOpts::default()
                },
            )
        });
        barrier.wait();
        (gate.join().unwrap(), dispatch.join().unwrap())
    });
    let started = usize::from(gate.is_ok())
        + usize::from(matches!(dispatch, Ok(DispatchOutcome::Started { .. })));
    assert_eq!(started, 1);
    assert_eq!(corral_ops::slot_usage(&f.ctx).unwrap().total(), 1);
    assert_eq!(f.state()["creates"], 2); // Initial worker plus the one winning process.
}

#[test]
fn counting_switches_are_opt_out_and_review_budget_remains_bounded() {
    let mut f = review_fixture();
    let a = finished(&f, "app");
    let b = finished(&f, "app");
    let busy = f.task("other", Status::Ready);
    f.ok(&["dispatch", &busy.id]);
    f.ctx.store.config.limits.count_reviews = false;
    f.ctx.store.config.limits.count_gates = false;
    f.ctx.store.config.limits.review = 1;
    f.ctx.store.save_config().unwrap();
    f.ok(&["gate", &a]);
    let out = f.cli(&["review-run", &b]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("review process limit"));
    let usage = corral_ops::slot_usage(&f.ctx).unwrap();
    assert_eq!((usage.tasks, usage.gates, usage.total()), (1, 1, 1));
}

#[test]
fn project_cap_includes_gate_and_does_not_block_other_project() {
    let mut f = review_fixture();
    f.ctx.store.config.limits.running = 2;
    f.ctx.store.config.limits.review = 1;
    f.ctx
        .store
        .config
        .limits
        .per_project
        .insert("app".into(), 1);
    f.ctx.store.save_config().unwrap();
    let a = finished(&f, "app");
    f.ok(&["gate", &a]);
    let app = f.task("app", Status::Ready);
    let other = f.task("other", Status::Ready);
    f.ok(&["queue", "add", &app.id]);
    f.ok(&["queue", "add", &other.id]);
    f.ok(&["sync"]);
    assert!(f.ctx.store.get(&app.id).unwrap().runs.is_empty());
    assert_eq!(f.ctx.store.get(&other.id).unwrap().status, Status::Running);
    assert_eq!(corral_ops::slot_usage(&f.ctx).unwrap().total(), 2);
}

#[test]
fn reviewer_pane_disappearance_releases_shared_capacity() {
    let mut f = review_fixture();
    f.ctx.store.config.limits.running = 2;
    f.ctx.store.config.limits.review = 1;
    f.ctx.store.save_config().unwrap();
    let a = finished(&f, "app");
    f.ok(&["review-run", &a]);
    let task = f.ctx.store.get(&a).unwrap();
    let review = task.current_run().unwrap().review.as_ref().unwrap();
    let pane = review.pane.as_ref().unwrap();
    // Debounce ticks use an explicit clock, independent of CLI scheduling under load.
    let seen_at = review.at + jiff::SignedDuration::from_secs(600);
    corral_ops::sync_at(&f.ctx, seen_at).unwrap();
    f.ctx.herdr.pane_close(pane).unwrap();
    let next = f.task("other", Status::Ready);
    f.ok(&["queue", "add", &next.id]);
    corral_ops::sync_at(&f.ctx, seen_at + jiff::SignedDuration::from_secs(1)).unwrap();
    assert_eq!(f.ctx.store.get(&a).unwrap().status, Status::Review);
    // The live reviewer occupies the reserved finishing slot, leaving one coding slot.
    assert_eq!(corral_ops::slot_usage(&f.ctx).unwrap().reviews, 1);
    assert_eq!(f.ctx.store.get(&next.id).unwrap().status, Status::Running);
    corral_ops::sync_at(&f.ctx, seen_at + jiff::SignedDuration::from_secs(2)).unwrap();
    // T-075 escalates the missing reviewer. Its process slot is released, but
    // Needs me still holds the task slot until the owner resolves the handoff.
    assert_eq!(f.ctx.store.get(&a).unwrap().status, Status::NeedsMe);
    assert_eq!(f.ctx.store.get(&next.id).unwrap().status, Status::Running);
    let usage = corral_ops::slot_usage(&f.ctx).unwrap();
    assert_eq!((usage.tasks, usage.reviews, usage.total()), (2, 0, 2));
    f.ok(&[
        "done",
        &a,
        "--summary",
        "Verified: missing reviewer inspected\nNot verified: none",
    ]);
    f.ok(&["sync"]);
    assert_eq!(f.ctx.store.get(&next.id).unwrap().status, Status::Running);
    assert_eq!(corral_ops::slot_usage(&f.ctx).unwrap().reviews, 0);
}
