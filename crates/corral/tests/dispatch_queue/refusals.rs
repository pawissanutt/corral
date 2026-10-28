//! T-505: a persistent queue refusal is logged once per state and retried after a change.
use corral_core::Status;

use crate::queue_support::Fixture;

fn churn(f: &Fixture, id: &str) -> Vec<String> {
    f.ctx
        .store
        .events()
        .unwrap()
        .into_iter()
        .filter(|e| e.task.as_deref() == Some(id))
        .filter(|e| matches!(e.kind.as_str(), "queued" | "dispatch_failed"))
        .map(|e| {
            format!(
                "{} {}",
                e.kind,
                e.detail["error"].as_str().unwrap_or_default()
            )
        })
        .collect()
}

fn passes(f: &Fixture) {
    for _ in 0..3 {
        corral_ops::sync(&f.ctx).unwrap();
    }
}

#[test]
fn unroutable_ready_admission_fails_once_until_its_route_changes() {
    // Given an authorized Ready task whose pinned profile does not exist.
    let mut f = crate::queue_fixture();
    let task = f.task("app", Status::Ready);
    f.ctx
        .store
        .update(&task.id, |t| {
            t.profile = Some("missing-profile".into());
            Ok(())
        })
        .unwrap();
    f.grant(&[&task.id]);

    // When several sync passes meet the same refusal.
    passes(&f);

    // Then exactly one visible failure is logged and nothing is enqueued or launched.
    let logged = churn(&f, &task.id);
    assert_eq!(logged.len(), 1, "{logged:?}");
    assert!(logged[0].starts_with("dispatch_failed "), "{logged:?}");
    assert!(logged[0].contains("missing-profile"), "{logged:?}");
    assert!(f.ctx.store.get(&task.id).unwrap().runs.is_empty());

    // And repairing the route is a state change that admits the task on the next pass.
    f.ctx
        .store
        .update(&task.id, |t| {
            t.profile = None;
            Ok(())
        })
        .unwrap();
    corral_ops::sync(&f.ctx).unwrap();
    assert_eq!(f.ctx.store.get(&task.id).unwrap().status, Status::Running);
}

#[test]
fn queued_task_whose_graph_becomes_a_cycle_fails_once_until_it_changes() {
    // Given an authorized task queued behind a Backlog predecessor.
    let mut f = crate::queue_fixture();
    let task = f.task("app", Status::Ready);
    let predecessor = f.task("app", Status::Backlog);
    f.ctx
        .store
        .update(&task.id, |t| {
            t.depends_on = vec![predecessor.id.clone()];
            Ok(())
        })
        .unwrap();
    f.grant(&[&task.id]);
    corral_ops::sync(&f.ctx).unwrap();
    assert!(f.ctx.store.get(&task.id).unwrap().queued);
    let before = churn(&f, &task.id).len();

    // When the predecessor is edited to depend on the queued task, and several passes run.
    f.ctx
        .store
        .update(&predecessor.id, |t| {
            t.depends_on = vec![task.id.clone()];
            Ok(())
        })
        .unwrap();
    passes(&f);

    // Then exactly one failure names the cycle, without a launch.
    let logged = churn(&f, &task.id)[before..].to_vec();
    assert_eq!(logged.len(), 1, "{logged:?}");
    assert!(logged[0].contains("dependency cycle"), "{logged:?}");
    assert!(f.ctx.store.get(&task.id).unwrap().runs.is_empty());

    // And repairing the graph and finishing the predecessor admits the task on the next pass.
    f.ctx
        .store
        .update(&predecessor.id, |t| {
            t.depends_on.clear();
            t.status = Status::Done;
            Ok(())
        })
        .unwrap();
    corral_ops::sync(&f.ctx).unwrap();
    assert_eq!(f.ctx.store.get(&task.id).unwrap().status, Status::Running);
}
