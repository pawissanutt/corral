//! T-506: the owner moves unstarted, held members between tracker Plans.
use super::*;
use crate::{OwnerEntry, plan_add_member_from, plan_move_member_from};

fn owner(f: &Fixture) -> OwnerEntry {
    OwnerEntry {
        agent: false,
        pm: false,
        cwd: f._dir.path().to_path_buf(),
    }
}

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
    plan_add_member_from(&f.ctx, &f.plan, &id, &owner(f)).unwrap();
    id
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
    let moved = plan_move_member_from(&f.ctx, &f.plan, &held, &later, &owner(&f)).unwrap();

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
    let to_later = |member: &str, entry: &OwnerEntry| {
        plan_move_member_from(&f.ctx, &f.plan, member, &later, entry)
    };

    // When the started member, an agent or PM author, a non-member or the same Plan is named.
    let error = message(to_later(&started, &owner(&f)));
    assert!(error.contains("must be unstarted"), "{error}");
    for (agent, pm) in [(true, false), (false, true)] {
        let entry = OwnerEntry {
            agent,
            pm,
            ..owner(&f)
        };
        let error = message(to_later(&held, &entry));
        assert!(error.contains("owner-only"), "{error}");
    }
    let error = message(plan_move_member_from(
        &f.ctx,
        &later,
        &held,
        &f.plan,
        &owner(&f),
    ));
    assert!(error.contains("is not a member of"), "{error}");
    assert!(plan_move_member_from(&f.ctx, &f.plan, &held, &f.plan, &owner(&f)).is_err());

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
    let moved = to_later(&held, &owner(&f)).unwrap();
    assert!(!moved.from_children.contains(&held));
    assert_eq!(moved.to_children.last(), Some(&held));
    assert!(to_later(&held, &owner(&f)).is_err());
    assert_eq!(moved_events(&f), 1);
    let plans = (f.plan(), f.ctx.store.get(&later).unwrap());
    assert_eq!(
        (plans.0.membership_revision, plans.1.membership_revision),
        (1, 1)
    );
}
