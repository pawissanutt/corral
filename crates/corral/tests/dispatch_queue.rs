#![cfg(unix)] // These scenarios execute the shebang-based queue engine fixture.

#[path = "dispatch_queue/approval.rs"]
mod approval;
#[path = "dispatch_queue/capacity.rs"]
mod capacity;
#[path = "dispatch_queue/contention.rs"]
mod contention;
#[path = "dispatch_queue/filing.rs"]
mod filing;
#[path = "dispatch_queue/isolation.rs"]
mod isolation;
#[path = "dispatch_queue/live.rs"]
mod live;
#[path = "dispatch_queue/pending_plans.rs"]
mod pending_plans;
mod queue_support;
#[path = "dispatch_queue/ready.rs"]
mod ready;
#[path = "dispatch_queue/recovery.rs"]
mod recovery;
#[path = "dispatch_queue/review_family.rs"]
mod review_family;
#[path = "dispatch_queue/routing.rs"]
mod routing;

use std::sync::{Arc, Barrier};

use corral_core::{Profile, RouteSource, Status};
use corral_ops::{DispatchOpts, DispatchOutcome};
use queue_support::Fixture;

fn queue_fixture() -> Fixture {
    let mut f = Fixture::new();
    // Keep one automatic coding slot while reserving one finishing slot.
    f.ctx.store.config.limits.running = 2;
    f.ctx.store.config.limits.review = 1;
    f.ctx.store.config.dispatch.auto_backlog = false;
    f.ctx.store.save_config().unwrap();
    f
}

#[test]
fn admission_is_project_scoped_ready_only_and_dequeue_survives_sync() {
    let f = queue_fixture();
    let ready = f.task("app", Status::Ready);
    let excluded = f.task("app", Status::Ready);
    let backlog = f.task("app", Status::Backlog);
    let idea = f.task("app", Status::Idea);
    let other = f.task("other", Status::Ready);
    f.ok(&["queue", "remove", &excluded.id]);
    f.ok(&["queue", "admit", "app"]);
    f.ok(&["sync"]);
    assert_eq!(f.ctx.store.get(&ready.id).unwrap().status, Status::Running);
    let excluded = f.ctx.store.get(&excluded.id).unwrap();
    assert_eq!(excluded.status, Status::Backlog);
    assert!(excluded.dispatch_hold);
    assert!(excluded.runs.is_empty());
    assert!(!excluded.queued);
    for task in [backlog, idea, other] {
        let actual = f.ctx.store.get(&task.id).unwrap();
        assert_eq!(actual.status, task.status);
        assert!(actual.runs.is_empty());
        assert!(!actual.queued);
    }
    assert_eq!(f.state()["creates"], 1);
}

#[test]
fn default_sync_admits_ready_without_explicit_enqueue() {
    let f = queue_fixture();
    let task = f.task("app", Status::Ready);
    f.ok(&["sync"]);
    assert_eq!(f.ctx.store.get(&task.id).unwrap().status, Status::Running);
    assert_eq!(f.state()["creates"], 1);
}

#[test]
fn explicitly_queued_idea_is_promoted_and_authorized() {
    let f = queue_fixture();
    let task = f.task("app", Status::Idea);
    assert!(!f.cli(&["dispatch", &task.id]).status.success());
    f.ok(&["queue", "add", &task.id]);
    let queued = f.ctx.store.get(&task.id).unwrap();
    assert_eq!(queued.status, Status::Ready);
    assert!(queued.queued);
    assert!(queued.dispatch_plan.is_some());
    assert!(!f.temp.path().join("protocol.json").exists());
    f.ok(&["sync"]);
    assert_eq!(f.ctx.store.get(&task.id).unwrap().status, Status::Running);
    assert_eq!(f.state()["creates"], 1);
}

#[test]
fn enqueue_is_side_effect_free_and_pause_is_independent_of_review() {
    let f = queue_fixture();
    let task = f.task("app", Status::Ready);
    f.ok(&["queue", "add", &task.id, "--profile", "generic"]);
    assert!(!f.temp.path().join("protocol.json").exists());
    assert!(!f.temp.path().join("app/.worktrees").exists());
    f.ok(&["queue", "pause"]);
    f.ok(&["sync"]);
    let out = f.ok(&["queue", "list", "--json"]);
    let entries: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(entries[0]["reason"], "dispatch paused");
    assert_eq!(f.state()["creates"], 0);
    f.ok(&["queue", "resume"]);
    f.ok(&["sync"]);
    assert_eq!(f.state()["creates"], 1);
    assert!(!f.ctx.store.config.autopilot_enabled());
}

#[test]
fn review_dependency_waits_but_approval_refills_the_slot() {
    let f = queue_fixture();
    let dep = f.task("app", Status::Ready);
    f.ok(&["dispatch", &dep.id]);
    f.ok(&[
        "done",
        &dep.id,
        "--summary",
        "Verified: fixture\nNot verified: none",
    ]);
    let task = f.task("app", Status::Ready);
    f.ctx
        .store
        .update(&task.id, |t| {
            t.depends_on.push(dep.id.clone());
            Ok(())
        })
        .unwrap();
    f.ok(&["queue", "add", &task.id]);
    f.ok(&["sync"]);
    assert_eq!(f.state()["creates"], 1);
    assert!(
        corral_ops::dispatch_queue(&f.ctx).unwrap()[0]
            .reason
            .contains("Review")
    );
    f.ok(&["approve", &dep.id, "--merge", "keep", "--keep-worktree"]);
    f.ok(&["sync"]);
    assert_eq!(f.state()["creates"], 2);
    assert_eq!(f.ctx.store.get(&task.id).unwrap().status, Status::Running);
}

#[test]
fn needs_me_and_reservations_count_but_project_saturation_does_not_starve_others() {
    let mut f = queue_fixture();
    f.ctx.store.config.limits.running = 3;
    f.ctx
        .store
        .config
        .limits
        .per_project
        .insert("app".into(), 1);
    f.ctx.store.save_config().unwrap();
    let busy = f.task("app", Status::NeedsMe);
    let app = f.task("app", Status::Ready);
    let other = f.task("other", Status::Ready);
    for task in [&app, &other] {
        f.ok(&["queue", "add", &task.id]);
    }
    f.ok(&["sync"]);
    assert_eq!(f.ctx.store.get(&busy.id).unwrap().status, Status::NeedsMe);
    assert!(f.ctx.store.get(&app.id).unwrap().runs.is_empty());
    assert_eq!(f.ctx.store.get(&other.id).unwrap().status, Status::Running);
    assert_eq!(corral_ops::slots(&f.ctx).unwrap(), (2, 3));
}

#[test]
fn priority_then_creation_then_id_order_is_deterministic() {
    let f = queue_fixture();
    let low = f.task("app", Status::Ready);
    let later = f.task("app", Status::Ready);
    let first_tie = f.task("app", Status::Ready);
    let second_tie = f.task("app", Status::Ready);
    for (task, priority, seconds) in [
        (&low, "P3", 0),
        (&later, "P0", 2),
        (&first_tie, "P0", 1),
        (&second_tie, "P0", 1),
    ] {
        f.ctx
            .store
            .update(&task.id, |t| {
                t.priority = priority.into();
                t.created = jiff::Timestamp::from_second(seconds).unwrap();
                Ok(())
            })
            .unwrap();
        f.ok(&["queue", "add", &task.id]);
    }
    let ids: Vec<_> = corral_ops::dispatch_queue(&f.ctx)
        .unwrap()
        .into_iter()
        .map(|e| e.id)
        .collect();
    assert_eq!(ids, [first_tie.id.clone(), second_tie.id, later.id, low.id]);
    f.ok(&["sync"]);
    assert_eq!(
        f.ctx.store.get(&first_tie.id).unwrap().status,
        Status::Running
    );
}

#[test]
fn malformed_dependencies_are_visible_and_do_not_stop_other_projects() {
    let f = queue_fixture();
    let missing = f.task("app", Status::Ready);
    let cycle = f.task("app", Status::Ready);
    let next = f.task("app", Status::Ready);
    for (task, dep) in [
        (&missing, "T-999"),
        (&cycle, next.id.as_str()),
        (&next, cycle.id.as_str()),
    ] {
        f.ctx
            .store
            .update(&task.id, |t| {
                t.depends_on = vec![dep.into()];
                Ok(())
            })
            .unwrap();
    }
    let other = f.task("other", Status::Ready);
    f.ok(&["queue", "add", &other.id]);
    f.ok(&["sync"]);
    let entries = corral_ops::dispatch_queue(&f.ctx).unwrap();
    assert!(
        entries
            .iter()
            .any(|e| e.id == missing.id && e.reason.contains("missing dependency"))
    );
    assert!(
        entries
            .iter()
            .any(|e| e.id == cycle.id && e.reason.contains("dependency cycle"))
    );
    assert_eq!(f.ctx.store.get(&other.id).unwrap().status, Status::Running);
    assert!(!f.cli(&["queue", "add", &cycle.id]).status.success());
}

#[test]
fn same_task_barrier_race_cannot_create_two_runs_or_panes() {
    let f = Fixture::new();
    let task = f.task("app", Status::Ready);
    let barrier = Arc::new(Barrier::new(3));
    let results = std::thread::scope(|s| {
        let handles: Vec<_> = (0..2)
            .map(|_| {
                let b = barrier.clone();
                let ctx = &f.ctx;
                let id = &task.id;
                s.spawn(move || {
                    b.wait();
                    corral_ops::dispatch(ctx, id, &DispatchOpts::default())
                })
            })
            .collect();
        barrier.wait();
        handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert_eq!(
        results
            .iter()
            .filter(|r| matches!(r, Ok(DispatchOutcome::Started { .. })))
            .count(),
        1
    );
    assert_eq!(f.ctx.store.get(&task.id).unwrap().runs.len(), 1);
    assert_eq!(f.state()["creates"], 1);
    assert_eq!(f.state()["submits"], 1);
}

#[test]
fn explicit_route_and_base_survive_reload_without_silent_model_replacement() {
    let mut f = queue_fixture();
    f.ctx
        .store
        .config
        .harnesses
        .get_mut("generic")
        .unwrap()
        .command = "echo {model_args}".into();
    let h = f.ctx.store.config.harnesses.get_mut("generic").unwrap();
    h.model_flag = Some("--model {model}".into());
    h.effort_flag = Some("--effort {effort}".into());
    h.efforts = vec!["high".into(), "low".into()];
    f.ctx.store.config.profiles.insert(
        "pinned".into(),
        Profile {
            harness: "generic".into(),
            model: Some("default-model".into()),
            effort: Some("low".into()),
            args: vec![],
            vendor: None,
            escalate_to: None,
        },
    );
    f.ctx.store.save_config().unwrap();
    queue_support::git(&f.temp.path().join("app"), &["branch", "chosen"]);
    let task = f.task("app", Status::Ready);
    f.ok(&[
        "queue",
        "add",
        &task.id,
        "--profile",
        "pinned",
        "--model",
        "exact-model",
        "--effort",
        "high",
        "--base",
        "chosen",
        "--headless",
    ]);
    f.ctx.store.config.defaults.profile = Some("omo".into());
    f.ctx.store.save_config().unwrap();
    f.ok(&["sync"]);
    let task = f.ctx.store.get(&task.id).unwrap();
    let run = task.current_run().unwrap();
    assert_eq!(
        (
            &run.agent,
            run.model.as_deref(),
            run.effort.as_deref(),
            run.base.as_str()
        ),
        (
            &"pinned".to_string(),
            Some("exact-model"),
            Some("high"),
            "chosen"
        )
    );
    assert_eq!(run.route, Some(RouteSource::Override));
    assert_eq!(run.mode, corral_core::RunMode::Headless);
}

#[test]
fn known_missing_explicit_model_is_rejected_before_queue_or_git() {
    let f = Fixture::new();
    let task = f.task("app", Status::Ready);
    let out = f.cli(&[
        "queue",
        "add",
        &task.id,
        "--profile",
        "claude",
        "--model",
        "missing-model",
    ]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("not an exact discovered model"));
    assert!(!f.ctx.store.get(&task.id).unwrap().queued);
    assert!(!f.temp.path().join("app/.worktrees").exists());
}

#[test]
fn request_changes_uses_the_same_capacity_as_queued_dispatch() {
    let f = Fixture::new();
    let reviewed = f.task("app", Status::Ready);
    f.ok(&["dispatch", &reviewed.id]);
    f.ok(&["done", &reviewed.id, "--summary", "fixture"]);
    let busy = f.task("other", Status::Ready);
    f.ok(&["dispatch", &busy.id]);
    let out = f.cli(&["changes", &reviewed.id, "fix"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("waiting for a slot"));
    assert_eq!(
        f.ctx.store.get(&reviewed.id).unwrap().status,
        Status::Review
    );
    assert_eq!(f.state()["submits"], 2);
}
