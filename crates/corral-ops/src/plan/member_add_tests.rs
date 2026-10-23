use super::*;
use crate::gate_tier::tests::{Fx, fixture};

fn owner(fx: &Fx) -> OwnerEntry {
    OwnerEntry {
        agent: false,
        pm: false,
        cwd: fx.repo.parent().unwrap().to_path_buf(),
    }
}

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
    let add =
        |member: &str, entry: &OwnerEntry| plan_add_member_from(&fx.ctx, &tracker, member, entry);

    // When the owner adds while that run is in flight, or an agent, the tracker itself or an
    // existing member is named.
    let error = add(&fix, &owner(&fx)).unwrap_err();
    assert!(format!("{error:#}").contains(&running), "{error:#}");
    let agent = OwnerEntry {
        agent: true,
        ..owner(&fx)
    };
    assert!(add(&fix, &agent).is_err());
    assert!(add(&tracker, &owner(&fx)).is_err());
    assert!(add(&running, &owner(&fx)).is_err());

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

    // When the owner reruns the addition, then it completes once and the Plan is consistent.
    let added = add(&fix, &owner(&fx)).unwrap();
    assert_eq!(added.children.last(), Some(&fix));
    assert_eq!(added.children.len(), children.len() + 1);
    let plan = fx.ctx.store.get(&tracker).unwrap();
    assert_eq!(
        fx.ctx.store.execution_plan(&plan).unwrap().unwrap().id,
        tracker
    );
    assert!(add(&fix, &owner(&fx)).is_err());
    assert_eq!(added_events(&fx), 1);
}
