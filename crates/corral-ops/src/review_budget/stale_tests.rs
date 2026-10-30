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

fn move_main_without_replaying(fx: &Fixture) {
    let app = fx.tmp.path().join("app");
    git(&app, &["checkout", "main"]);
    std::fs::write(app.join("upstream"), "task B landed\n").unwrap();
    git(&app, &["add", "."]);
    git(&app, &["commit", "-qm", "Land task B"]);
    git(&app, &["checkout", "candidate"]);
}

#[test]
fn automatic_preparation_rebinds_only_the_moved_integration_base() {
    for kind in [Kind::Gate, Kind::Reviewer] {
        let mut fx = Fixture::new(1);
        let id = fx.task();
        grant(&mut fx, std::slice::from_ref(&id));
        candidate_branch(&fx, &id);
        let mut launch = prepared(&fx, &id, kind);
        let old = launch.pending.authorization.clone().unwrap();

        // Task B lands after A prepared, but before it submitted anything.
        move_main_without_replaying(&fx);
        let task = fx.ctx.store.get(&id).unwrap();
        let candidate = crate::verification::identity(
            &fx.ctx,
            &task,
            task.current_run().unwrap(),
            old.action.candidate.as_ref().unwrap(),
        )
        .unwrap();
        assert_ne!(
            candidate.integration_base,
            old.action.candidate.as_ref().unwrap().integration_base
        );
        launch.bind(&candidate, &old.action.attempt, None).unwrap();
        let new = launch.pending.authorization.as_ref().unwrap();
        assert_eq!(
            new.id, old.id,
            "refresh must keep the single submission slot"
        );
        assert_ne!(new, &old, "stale authorization must not be reused");
        assert_eq!(new.action.candidate.as_ref(), Some(&candidate));
        assert_eq!(fx.ctx.store.get(&id).unwrap().rounds, 0);
        let journal = fx.ctx.store.authorized_submission(&old.id).unwrap();
        assert_eq!(journal.authorization, *new);
        assert_eq!(
            journal.authorization_history.last().unwrap().authorization,
            old
        );
        let sent = std::cell::Cell::new(false);
        assert!(
            crate::operating::submit(
                &fx.ctx,
                &old,
                |_, _| Ok(()),
                || {
                    sent.set(true);
                    Ok(())
                }
            )
            .is_err()
        );
        assert!(!sent.get(), "the superseded preparation must never submit");
        launch.commit().unwrap();
    }
}

#[test]
fn any_changed_unsubmitted_automatic_preparation_is_replaced_not_refused() {
    for field in ["head", "tree", "policy", "inputs", "environment"] {
        let mut fx = Fixture::new(1);
        let id = fx.task();
        grant(&mut fx, std::slice::from_ref(&id));
        candidate_branch(&fx, &id);
        let mut launch = prepared(&fx, &id, Kind::Gate);
        let old = launch.pending.authorization.clone().unwrap();
        move_main_without_replaying(&fx);
        let mut candidate = old.action.candidate.clone().unwrap();
        candidate.integration_base = git(&fx.tmp.path().join("app"), &["rev-parse", "main"]);
        match field {
            "head" => candidate.head.push_str("changed"),
            "tree" => candidate.tree.push_str("changed"),
            "policy" => candidate.policy_digest.push_str("changed"),
            "inputs" => candidate.inputs_digest.push_str("changed"),
            _ => candidate.environment.push(("OTHER".into(), None)),
        }
        if let Err(error) = launch.bind(&candidate, &old.action.attempt, None) {
            assert!(
                !format!("{error:#}").contains("prepared action identity changed"),
                "{field}: {error:#}"
            );
        } else {
            let journal = fx.ctx.store.authorized_submission(&old.id).unwrap();
            assert_eq!(
                journal.authorization.action.candidate.as_ref(),
                Some(&candidate),
                "{field}"
            );
            let superseded = &journal.authorization_history.last().unwrap().authorization;
            assert_eq!(superseded, &old, "{field}");
        }
        launch.commit().unwrap();
    }
}

#[test]
fn moved_base_cannot_renew_submitted_or_policy_refused_preparation() {
    for submitted in [false, true] {
        let mut fx = Fixture::new(1);
        let id = fx.task();
        grant(&mut fx, std::slice::from_ref(&id));
        candidate_branch(&fx, &id);
        let mut launch = prepared(&fx, &id, Kind::Gate);
        let old = launch.pending.authorization.clone().unwrap();
        if submitted {
            crate::operating::submit(&fx.ctx, &old, |_, _| Ok(()), || Ok(())).unwrap();
        } else {
            let mut config = Store::open(fx.ctx.store.root()).unwrap();
            config.config.automation = Default::default();
            config.save_config().unwrap();
        }
        move_main_without_replaying(&fx);
        let mut candidate = old.action.candidate.clone().unwrap();
        candidate.integration_base = git(&fx.tmp.path().join("app"), &["rev-parse", "main"]);
        assert!(launch.bind(&candidate, &old.action.attempt, None).is_err());
        assert_eq!(
            fx.ctx
                .store
                .authorized_submission(&old.id)
                .unwrap()
                .authorization,
            old
        );
        launch.commit().unwrap();
    }
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
    let adopted = crate::review_budget::adoptable_attempt(&fx.ctx, &id, "R1").unwrap();
    assert_eq!(
        crate::review_budget::retained_attempt(&fx.ctx, &id, "R1")
            .unwrap()
            .as_deref(),
        Some(old.authorization.as_ref().unwrap().action.attempt.as_str())
    );
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
