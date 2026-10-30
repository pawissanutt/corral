//! T-506: the owner moves unstarted, held members between tracker Plans.
use super::*;
use crate::{plan_add_member, plan_move_member};

/// A second tracker Plan in the same project with one required member.
fn later_plan(f: &Fixture) -> (String, String) {
    let source = f._dir.path().join("later.md");
    std::fs::write(
        &source,
        "---\ntitle: Later\nproject: app\napproved_scope:\n  goal: Held work\n  acceptance:\n    - {criterion: L1, scenario: held work works, expected: pass}\ntasks:\n  - {key: later, title: Later}\n---\n",
    )
    .unwrap();
    let imported = crate::import_plan(&f.ctx, &source, false, Origin::Agent).unwrap();
    (imported.tracker.id, imported.tasks[0].id.clone())
}

/// An unstarted Backlog task with a dispatch hold and a dependency, added to the fixture Plan.
fn held_member(f: &Fixture) -> String {
    let id = f
        .ctx
        .store
        .create("Held", Some("app"), Status::Backlog, "")
        .unwrap()
        .id;
    let first = f.members[0].clone();
    f.ctx
        .store
        .update(&id, |task| {
            task.dispatch_hold = true;
            task.depends_on = vec![first.clone()];
            Ok(())
        })
        .unwrap();
    plan_add_member(&f.ctx, &f.plan, &id).unwrap();
    id
}

fn history(f: &Fixture, id: &str) {
    f.ctx
        .store
        .update(id, |task| {
            task.runs = serde_json::from_value(json!([
                {"id":"R1", "agent":"fixture", "worktree":"/unused", "branch":"old",
                 "base":"main", "base_sha":"base", "started":"2026-10-19T00:00:00Z",
                 "ended":"2026-10-19T01:00:00Z", "outcome":"abandoned"},
                {"id":"R2", "agent":"fixture", "worktree":"/unused", "branch":"parked",
                 "base":"main", "base_sha":"base", "started":"2026-10-19T02:00:00Z"}
            ]))
            .unwrap();
            Ok(())
        })
        .unwrap();
}

#[test]
fn held_abandoned_and_parked_runs_move_unchanged() {
    let f = Fixture::new(Delivery::Main, &["true"]);
    let held = held_member(&f);
    history(&f, &held);
    let (later, _) = later_plan(&f);
    let before = f.ctx.store.get(&held).unwrap();
    plan_move_member(&f.ctx, &f.plan, &held, &later, None).unwrap();
    let after = f.ctx.store.get(&held).unwrap();
    assert_eq!(after.runs, before.runs);
    assert_eq!(after.parent.as_deref(), Some(later.as_str()));
    assert!(after.dispatch_hold);
    assert_eq!(after.status, Status::Backlog);
    assert_eq!(f.plan().membership_revision, 1);
    assert_eq!(f.ctx.store.get(&later).unwrap().membership_revision, 1);
}

#[test]
fn live_or_landed_history_refuses_without_writes() {
    for case in [
        "running",
        "needs_me",
        "review",
        "gate",
        "open_review",
        "landed",
        "integrated",
        "queued",
    ] {
        let f = Fixture::new(Delivery::Main, &["true"]);
        let held = held_member(&f);
        history(&f, &held);
        let (later, _) = later_plan(&f);
        f.ctx
            .store
            .update(&held, |task| {
                match case {
                    "running" => task.status = Status::Running,
                    "needs_me" => task.status = Status::NeedsMe,
                    "review" => task.status = Status::Review,
                    "queued" => task.queued = true,
                    "landed" => {
                        task.landed = Some(corral_core::LandingReceipt {
                            run: "R1".into(),
                            sha: "sha".into(),
                        })
                    }
                    "gate" => {
                        task.runs[0].gate = Some(
                            serde_json::from_value(json!({
                                "result":"running", "at":"2026-10-19T00:00:00Z", "sha":"sha"
                            }))
                            .unwrap(),
                        )
                    }
                    "open_review" => {
                        task.runs[0].review = Some(
                            serde_json::from_value(json!({
                                "reviewer":"owner", "at":"2026-10-19T00:00:00Z", "sha":"sha"
                            }))
                            .unwrap(),
                        )
                    }
                    _ => (),
                }
                Ok(())
            })
            .unwrap();
        if case == "integrated" {
            f.ctx
                .store
                .log_event(
                    Some(&held),
                    "approved",
                    json!({"run":"R1", "strategy":"fast_forward", "sha":"sha"}),
                )
                .unwrap();
        }
        let before = (
            f.plan(),
            f.ctx.store.get(&later).unwrap(),
            f.ctx.store.get(&held).unwrap(),
        );
        let error = message(plan_move_member(&f.ctx, &f.plan, &held, &later, None));
        if case != "queued" {
            let run = if matches!(case, "running" | "needs_me" | "review") {
                "R2"
            } else {
                "R1"
            };
            assert!(error.contains(run), "{case}: {error}");
        }
        assert_eq!(
            (
                f.plan(),
                f.ctx.store.get(&later).unwrap(),
                f.ctx.store.get(&held).unwrap()
            ),
            before
        );
        assert_eq!(moved_events(&f), 0);
    }
}

fn moved_events(f: &Fixture) -> usize {
    let events = f.ctx.store.events().unwrap();
    events
        .iter()
        .filter(|event| event.kind == "plan_member_moved")
        .count()
}

#[test]
fn moving_a_held_member_updates_both_plans_and_stales_the_earlier_attempt() {
    // Given a verified Plan, then a held member added to it, and a second tracker Plan.
    let f = Fixture::new(Delivery::Main, &["true"]);
    let attempt = f.passing();
    let held = held_member(&f);
    let error = message(plan_verify(&f.ctx, &f.plan));
    assert!(error.contains(&held), "{error}");
    let (later, later_member) = later_plan(&f);

    // When the owner moves the held member to the second Plan.
    let moved = plan_move_member(&f.ctx, &f.plan, &held, &later, None).unwrap();

    // Then both Plans record the new membership and the task keeps its ID, hold and dependency.
    assert_eq!(moved.from_children, f.members);
    assert_eq!(moved.to_children, [later_member, held.clone()]);
    assert_eq!(f.plan().children, f.members);
    assert_eq!(f.ctx.store.get(&later).unwrap().children, moved.to_children);
    let task = f.ctx.store.get(&held).unwrap();
    assert_eq!(task.parent.as_deref(), Some(later.as_str()));
    assert!(task.dispatch_hold);
    assert_eq!(task.depends_on, [f.members[0].clone()]);
    assert_eq!(task.status, Status::Backlog);
    let plan = f.ctx.store.execution_plan(&task).unwrap().unwrap();
    assert_eq!(plan.id, later);
    assert_eq!(moved_events(&f), 1);

    // And the attempt verified before the member joined stays stale, though the list is restored.
    let error = message(plan_accept(&f.ctx, &f.plan, &attempt.id, false));
    assert!(error.contains("stale Plan proof"), "{error}");

    // And progress and verify follow the new membership on both Plans.
    let progress = crate::plan_progress(&f.ctx, &f.plan).unwrap();
    let work: Vec<_> = progress.work.iter().map(|work| work.task.clone()).collect();
    assert_eq!(work, f.members);
    assert!(progress.aggregate.unwrap().ready);
    let progress = crate::plan_progress(&f.ctx, &later).unwrap();
    assert!(progress.work.iter().any(|work| work.task == held));
    let verified = plan_verify(&f.ctx, &f.plan).unwrap();
    assert_ne!(verified.id, attempt.id);
    let members: Vec<_> = verified.members.iter().map(|m| m.task.clone()).collect();
    assert_eq!(members, f.members);
}

#[test]
fn refused_moves_write_nothing_and_an_interrupted_move_completes_once() {
    // Given a Plan with a held member, a member started with a run, and a second Plan.
    let f = Fixture::new(Delivery::Main, &["true"]);
    let held = held_member(&f);
    let started = held_member(&f);
    f.accept_receipt(&started, &f.main(), &f.main(), "fast_forward");
    f.ctx
        .store
        .update(&started, |task| {
            task.status = Status::Backlog;
            Ok(())
        })
        .unwrap();
    let (later, _) = later_plan(&f);
    let before = (f.plan(), f.ctx.store.get(&later).unwrap());
    let to_later = |member: &str| plan_move_member(&f.ctx, &f.plan, member, &later, None);

    // When the started member, a non-member or the same Plan is named.
    let error = message(to_later(&started));
    assert!(error.contains("R1") && error.contains("landed"), "{error}");
    let error = message(plan_move_member(&f.ctx, &later, &held, &f.plan, None));
    assert!(error.contains("is not a member of"), "{error}");
    assert!(plan_move_member(&f.ctx, &f.plan, &held, &f.plan, None).is_err());
    let later_member = before.1.children[0].clone();
    let error = message(plan_move_member(
        &f.ctx,
        &later,
        &later_member,
        &f.plan,
        None,
    ));
    assert!(error.contains("last member"), "{error}");

    // Then nothing is written.
    assert_eq!((f.plan(), f.ctx.store.get(&later).unwrap()), before);
    let parent = f.ctx.store.get(&held).unwrap().parent;
    assert_eq!(parent.as_deref(), Some(f.plan.as_str()));
    assert_eq!(moved_events(&f), 0);

    // Given an earlier move stopped after writing only the member.
    f.ctx
        .store
        .update(&held, |task| {
            task.parent = Some(later.clone());
            Ok(())
        })
        .unwrap();
    // When the owner reruns it, then it completes once.
    let moved = to_later(&held).unwrap();
    assert!(!moved.from_children.contains(&held));
    assert_eq!(moved.to_children.last(), Some(&held));
    assert!(to_later(&held).is_err());
    assert_eq!(moved_events(&f), 1);
    let plans = (f.plan(), f.ctx.store.get(&later).unwrap());
    assert_eq!(
        (plans.0.membership_revision, plans.1.membership_revision),
        (1, 1)
    );
}
