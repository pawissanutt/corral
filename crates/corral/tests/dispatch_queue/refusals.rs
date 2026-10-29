//! T-505: a persistent queue refusal is logged once per state and retried after a change.
//! T-560: a config change never strands owner-queued work.
use corral_core::Status;
use corral_ops::DispatchOpts;

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

#[test]
fn unavailable_queued_base_fails_once_and_dispatches_once_it_is_restored() {
    // Given work explicitly queued on the app repository's main branch.
    let f = crate::queue_fixture();
    let task = f.task("app", Status::Ready);
    let opts = DispatchOpts {
        base: Some("main".into()),
        ..DispatchOpts::default()
    };
    corral_ops::enqueue(&f.ctx, &task.id, &opts).unwrap();
    let before = churn(&f, &task.id).len();
    let repo = f.temp.path().join("app");

    // When main disappears and several sync passes run.
    crate::queue_support::git(&repo, &["branch", "-m", "main", "temporary"]);
    passes(&f);

    // Then exactly one failure is logged and nothing launches.
    let logged = churn(&f, &task.id)[before..].to_vec();
    assert_eq!(logged.len(), 1, "{logged:?}");
    assert!(logged[0].starts_with("dispatch_failed "), "{logged:?}");
    assert!(f.ctx.store.get(&task.id).unwrap().runs.is_empty());

    // And restoring the base, with no task or configuration edit, admits it on the next pass.
    crate::queue_support::git(&repo, &["branch", "-m", "temporary", "main"]);
    corral_ops::sync(&f.ctx).unwrap();
    assert_eq!(f.ctx.store.get(&task.id).unwrap().status, Status::Running);
}

/// Change a `limits.*` value the way an owner's config edit does, from a separate store.
fn edit_limits_running(f: &Fixture) {
    let mut fresh = corral_core::Store::open(f.ctx.store.root()).unwrap();
    fresh.config.limits.running += 1;
    fresh.save_config().unwrap();
}

/// Launch releases that carry an error; a successful launch also logs an error-free release.
fn failed_releases(f: &Fixture, id: &str) -> Vec<String> {
    f.ctx
        .store
        .events()
        .unwrap()
        .into_iter()
        .filter(|e| e.task.as_deref() == Some(id) && e.kind == "dispatch_released")
        .filter_map(|e| e.detail["error"].as_str().map(str::to_string))
        .filter(|error| !error.is_empty())
        .collect()
}

fn queue_authorization(f: &Fixture, id: &str) -> String {
    f.ctx
        .store
        .get(id)
        .unwrap()
        .dispatch_plan
        .unwrap()
        .authorization
        .unwrap()
}

/// T-560: a limits change is admission, not consent, so the owner's original queue approval
/// submits unchanged: no refusal, renewal or history entry.
fn assert_owner_consent_carried(f: &Fixture, id: &str, consent: &corral_core::ActionAuthorization) {
    let record = f.ctx.store.authorized_submission(&consent.id).unwrap();
    assert_eq!(
        record.disposition,
        corral_core::SubmissionDisposition::Submitted
    );
    assert_eq!(record.authorization, *consent);
    assert!(record.authorization_history.is_empty(), "{record:?}");
    let task = f.ctx.store.get(id).unwrap();
    assert_eq!(task.status, Status::Running);
    assert_eq!(task.runs.len(), 1);
    assert_eq!(f.state()["submits"], 1);
}

#[test]
fn owner_queued_task_dispatches_after_a_limits_change_without_a_launch_loop() {
    // Given an owner `queue add` waiting on a Backlog predecessor.
    let f = crate::queue_fixture();
    let task = f.task("app", Status::Ready);
    let predecessor = f.task("app", Status::Backlog);
    f.ctx
        .store
        .update(&task.id, |t| {
            t.depends_on = vec![predecessor.id.clone()];
            Ok(())
        })
        .unwrap();
    f.ok(&["queue", "add", &task.id]);
    f.ok(&["sync"]);
    assert!(f.ctx.store.get(&task.id).unwrap().runs.is_empty());
    let consent = f
        .ctx
        .store
        .authorized_submission(&queue_authorization(&f, &task.id))
        .unwrap()
        .authorization;
    assert!(matches!(
        consent.origin,
        corral_core::OperatingOrigin::ExplicitOwner { .. }
    ));

    // When the owner changes limits.running and the task then becomes eligible.
    edit_limits_running(&f);
    f.ctx
        .store
        .update(&predecessor.id, |t| {
            t.status = Status::Done;
            Ok(())
        })
        .unwrap();
    let before = churn(&f, &task.id).len();
    for _ in 0..3 {
        f.ok(&["sync"]);
    }

    // Then it dispatches once under the owner's queue consent, with no refusal or release loop.
    assert_owner_consent_carried(&f, &task.id, &consent);
    let released = failed_releases(&f, &task.id);
    assert!(released.is_empty(), "{released:?}");
    let logged = churn(&f, &task.id)[before..].to_vec();
    assert!(
        logged
            .iter()
            .all(|line| !line.contains("authorization refused")),
        "{logged:?}"
    );
}

#[test]
fn owner_queued_launch_retained_across_a_limits_change_resumes_once() {
    // Given an owner-queued launch that failed before submission, so its reservation is retained.
    let f = crate::queue_fixture();
    let task = f.task("app", Status::Ready);
    f.ok(&["queue", "add", &task.id]);
    let consent = f
        .ctx
        .store
        .authorized_submission(&queue_authorization(&f, &task.id))
        .unwrap()
        .authorization;
    f.fault("before workspace create");
    f.ok(&["sync"]);
    let failed = f.ctx.store.get(&task.id).unwrap();
    assert!(!failed.launch.as_ref().unwrap().reserved);
    assert_eq!(f.state()["submits"], 0);

    // When the owner changes limits.running before the retry.
    edit_limits_running(&f);
    for _ in 0..3 {
        f.ok(&["sync"]);
    }

    // Then the retained launch resumes under the owner's unchanged consent and submits once.
    assert_owner_consent_carried(&f, &task.id, &consent);
    assert!(f.ctx.store.get(&task.id).unwrap().launch.is_none());
    let released = failed_releases(&f, &task.id);
    assert_eq!(
        released.len(),
        1,
        "only the injected fault releases: {released:?}"
    );
}
