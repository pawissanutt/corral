use super::*;
use crate::gate_tier::tests::{Fx, fixture};

fn added_events(fx: &Fx) -> usize {
    let events = fx.ctx.store.events().unwrap();
    events
        .iter()
        .filter(|e| e.kind == "plan_member_added")
        .count()
}

#[test]
fn refused_additions_write_nothing_and_an_interrupted_addition_completes_once() {
    // Given a Plan whose first member runs against the current membership and a new fix task.
    let fx = fixture(true);
    let (members, tracker) = fx.members(1);
    let running = members[0].id.clone();
    let fix = fx
        .ctx
        .store
        .create("Fix", Some("app"), Status::Backlog, "")
        .unwrap()
        .id;
    let add = |member: &str| plan_add_member(&fx.ctx, &tracker, member);

    // When a fix is added while that run is in flight, or the tracker or an existing member is named.
    let error = add(&fix).unwrap_err();
    assert!(format!("{error:#}").contains(&running), "{error:#}");
    assert!(add(&tracker).is_err());
    assert!(add(&running).is_err());

    // Then nothing is written.
    let children = fx.ctx.store.get(&tracker).unwrap().children;
    assert!(!children.contains(&fix));
    assert_eq!(fx.ctx.store.get(&fix).unwrap().parent, None);
    assert_eq!(added_events(&fx), 0);

    // Given the run finished and an earlier addition stopped after writing only the member.
    let finish = |task: &mut corral_core::Task| {
        task.status = Status::Done;
        Ok(())
    };
    fx.ctx.store.update(&running, finish).unwrap();
    let interrupted = |task: &mut corral_core::Task| {
        task.parent = Some(tracker.clone());
        Ok(())
    };
    fx.ctx.store.update(&fix, interrupted).unwrap();

    // When the addition is rerun, then it completes once and the Plan is consistent.
    let added = add(&fix).unwrap();
    assert_eq!(added.children.last(), Some(&fix));
    assert_eq!(added.children.len(), children.len() + 1);
    let plan = fx.ctx.store.get(&tracker).unwrap();
    assert_eq!(
        fx.ctx.store.execution_plan(&plan).unwrap().unwrap().id,
        tracker
    );
    assert!(add(&fix).is_err());
    assert_eq!(added_events(&fx), 1);
}

#[test]
fn a_started_unlanded_member_joins_and_its_next_run_starts_from_the_plan_base() {
    // Given an integration Plan and a queued task whose superseded run off main never landed.
    let fx = fixture(true);
    let (_, tracker) = fx.members(0);
    let main = corral_git::rev_sha(&fx.repo, "main").unwrap();
    let id = fx
        .ctx
        .store
        .create("Started fix", Some("app"), Status::Ready, "")
        .unwrap()
        .id;
    let mut queued = crate::queue::resolve_plan(
        &fx.ctx,
        &fx.ctx.store.get(&id).unwrap(),
        &crate::DispatchOpts::default(),
    )
    .unwrap();
    queued.base = Some("main".into());
    let retry = serde_json::to_value(&queued.choice).unwrap();
    let run: corral_core::Run = serde_json::from_value(serde_json::json!({
        "id": "R1", "agent": "fixture", "worktree": fx.repo.join(".worktrees/unused"),
        "branch": format!("corral/{id}"), "base": "main", "base_sha": main,
        "started": "2026-10-29T00:00:00Z", "ended": "2026-10-29T01:00:00Z",
        "outcome": "superseded", "retry": retry,
    }))
    .unwrap();
    fx.ctx
        .store
        .update(&id, |task| {
            task.runs.push(run.clone());
            task.queued = true;
            task.dispatch_plan = Some(queued.clone());
            Ok(())
        })
        .unwrap();

    // When it is added to the Plan.
    let added = plan_add_member(&fx.ctx, &tracker, &id).unwrap();

    // Then it is a member, and its next run resolves the Plan's delivery branch and passes the
    // Plan base check that its old base would fail.
    assert_eq!(added.children.last(), Some(&id));
    let task = fx.ctx.store.get(&id).unwrap();
    assert_eq!(task.parent.as_deref(), Some(tracker.as_str()));
    assert!(task.dispatch_plan.is_none());
    let next = crate::queue::resolve_plan(&fx.ctx, &task, &crate::DispatchOpts::default()).unwrap();
    assert_eq!(next.base.as_deref(), Some("feature/plan"));
    let head = {
        let _lock = corral_git::landing_lock(&fx.repo).unwrap();
        crate::plan_target::prepare_dispatch(&fx.ctx, &task)
            .unwrap()
            .unwrap()
            .0
            .head
    };
    crate::tracker::check_dispatch_base(&fx.ctx.store, &task, "feature/plan", &head).unwrap();
    assert!(crate::tracker::check_dispatch_base(&fx.ctx.store, &task, "main", &main).is_err());

    // And a member whose run landed still cannot join.
    let landed = fx
        .ctx
        .store
        .create("Landed", Some("app"), Status::Backlog, "")
        .unwrap()
        .id;
    fx.ctx
        .store
        .update(&landed, |task| {
            task.runs.push(run.clone());
            task.landed = Some(corral_core::LandingReceipt {
                run: "R1".into(),
                sha: main.clone(),
            });
            Ok(())
        })
        .unwrap();
    let error = plan_add_member(&fx.ctx, &tracker, &landed).unwrap_err();
    assert!(format!("{error:#}").contains("landed"), "{error:#}");
}
