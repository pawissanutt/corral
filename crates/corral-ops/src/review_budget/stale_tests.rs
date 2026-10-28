use super::*;

fn rebase_candidate(fx: &Fixture) {
    let app = fx.tmp.path().join("app");
    git(&app, &["checkout", "main"]);
    std::fs::write(app.join("upstream"), "new base\n").unwrap();
    git(&app, &["add", "."]);
    git(&app, &["commit", "-qm", "upstream"]);
    git(&app, &["checkout", "candidate"]);
    git(&app, &["rebase", "main"]);
}

fn candidate_branch(fx: &Fixture, id: &str) {
    let app = fx.tmp.path().join("app");
    git(&app, &["checkout", "-qb", "candidate"]);
    std::fs::write(app.join("feature"), "candidate\n").unwrap();
    git(&app, &["add", "."]);
    git(&app, &["commit", "-qm", "candidate"]);
    fx.ctx
        .store
        .update(id, |task| {
            task.current_run_mut().unwrap().branch = "candidate".into();
            Ok(())
        })
        .unwrap();
}

#[test]
fn rebase_supersedes_preparation_and_next_automatic_gate_launches() {
    let mut fx = Fixture::new(1);
    let id = fx.task();
    grant(&mut fx, std::slice::from_ref(&id));
    candidate_branch(&fx, &id);
    drop(prepared(&fx, &id, Kind::Gate));
    let old = retained(&fx, &id, Kind::Gate);
    rebase_candidate(&fx);

    // Agent requests must not adopt the old attempt before admission retires it.
    let adopted = crate::review_budget::retained_attempt(&fx.ctx, &id, "R1").unwrap();
    crate::gate::gate_start_automatic(&fx.ctx, &id).unwrap();
    assert!(adopted.is_none());
    let task = fx.ctx.store.get(&id).unwrap();
    let gate = task.current_run().unwrap().gate.as_ref().unwrap();
    let identity = gate.identity.as_ref().unwrap();
    assert_ne!(
        Some(identity),
        old.authorization
            .as_ref()
            .unwrap()
            .action
            .candidate
            .as_ref()
    );
    assert_eq!(
        identity.head,
        git(&fx.tmp.path().join("app"), &["rev-parse", "HEAD"])
    );
    assert_ne!(
        identity.attempt,
        old.authorization.as_ref().unwrap().action.attempt
    );
    assert!(gate.holds_slot());
    assert_eq!(fx.created(), 1);
    assert!(!submission::retained_path(&fx.ctx, &id, "R1", Kind::Gate).exists());
    let artifact = task
        .artifacts
        .iter()
        .find(|a| a.title.contains("Superseded"))
        .unwrap();
    let archived: Pending = serde_json::from_slice(
        &std::fs::read(crate::artifact_path(&fx.ctx, artifact).unwrap()).unwrap(),
    )
    .unwrap();
    assert_eq!(archived.authorization, old.authorization);
    assert_eq!(archived.prepared_gate, old.prepared_gate);
    assert!(task.review_reservation.is_none());
}

#[test]
fn stale_origin_can_be_replaced_but_matching_origin_cannot_be_changed() {
    let mut fx = Fixture::new(1);
    let id = fx.task();
    grant(&mut fx, std::slice::from_ref(&id));
    candidate_branch(&fx, &id);
    drop(prepared(&fx, &id, Kind::Gate));
    assert!(ReviewLaunch::reserve(&fx.ctx, &id, Kind::Gate).is_err());
    rebase_candidate(&fx);
    let launch = ReviewLaunch::reserve(&fx.ctx, &id, Kind::Gate).unwrap();
    assert!(!launch.restored());
    launch.commit().unwrap();
    assert!(fx.ctx.store.get(&id).unwrap().review_reservation.is_none());
}

#[test]
fn rebase_supersedes_reviewer_preparation_without_reusing_child() {
    let mut fx = Fixture::new(1);
    let id = fx.task();
    grant(&mut fx, std::slice::from_ref(&id));
    candidate_branch(&fx, &id);
    drop(prepared(&fx, &id, Kind::Reviewer));
    rebase_candidate(&fx);
    let launch =
        ReviewLaunch::reserve_origin(&fx.ctx, &id, Kind::Reviewer, false, Origin::Automatic)
            .unwrap();
    assert!(!launch.restored());
    launch.commit().unwrap();
    let task = fx.ctx.store.get(&id).unwrap();
    let run = task.current_run().unwrap();
    assert!(run.review.is_none());
    assert!(run.review_runs.last().unwrap().ended.is_some());
    assert!(
        task.artifacts
            .iter()
            .any(|a| a.title.contains("Superseded"))
    );
}

#[test]
fn uncertain_preparations_are_not_disposed_on_candidate_change() {
    for submitted in [None, Some(true)] {
        let mut fx = Fixture::new(1);
        let id = fx.task();
        grant(&mut fx, std::slice::from_ref(&id));
        candidate_branch(&fx, &id);
        drop(prepared(&fx, &id, Kind::Gate));
        let path = submission::retained_path(&fx.ctx, &id, "R1", Kind::Gate);
        let mut pending = retained(&fx, &id, Kind::Gate);
        pending.submitted = submitted;
        std::fs::write(&path, serde_json::to_vec(&pending).unwrap()).unwrap();
        rebase_candidate(&fx);
        assert!(
            ReviewLaunch::reserve_origin(&fx.ctx, &id, Kind::Gate, false, Origin::Automatic)
                .is_err()
        );
        assert!(path.exists());
        assert!(fx.ctx.store.get(&id).unwrap().artifacts.is_empty());
    }
}

#[test]
fn retained_claim_cannot_monopolize_other_tasks_finishing_priority() {
    let mut fx = Fixture::new(1);
    let id = fx.task();
    let other = fx.task();
    grant(&mut fx, &[id.clone(), other.clone()]);
    fx.ctx
        .store
        .update(&id, |task| {
            task.current_run_mut().unwrap().outcome = Some(corral_core::RunOutcome::Done);
            task.review_wait = Some(Kind::Gate);
            Ok(())
        })
        .unwrap();
    // Eligible finishing work normally has priority.
    assert!(
        crate::slots::finishing_first(&fx.ctx.store, &other)
            .unwrap()
            .is_some()
    );
    let target = fx.ctx.store.get(&other).unwrap();
    assert!(
        crate::queue::blocked_reason(&fx.ctx, &fx.ctx.store, &target, false)
            .unwrap()
            .is_some()
    );
    drop(prepared(&fx, &id, Kind::Gate));
    // Even before a re-admission attempt (and even if recovery cannot replay it),
    // retained evidence cannot indefinitely refuse another task's changes/dispatch.
    assert!(
        crate::slots::finishing_first(&fx.ctx.store, &other)
            .unwrap()
            .is_none()
    );
    // This is the shared admission check used by request_changes.
    assert!(
        crate::queue::blocked_reason(&fx.ctx, &fx.ctx.store, &target, false)
            .unwrap()
            .is_none()
    );
    ReviewLaunch::reserve(&fx.ctx, &other, Kind::Gate)
        .unwrap()
        .commit()
        .unwrap();
}
