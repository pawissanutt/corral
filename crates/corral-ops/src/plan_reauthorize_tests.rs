//! A policy change under a prepared plan review must not wedge its owner or automatic retry.
use super::*;
use corral_core::{ActionAuthorization, OperatingOrigin, SubmissionDisposition};

/// A consent-relevant policy input; `limits.*` capacity no longer changes the revision (T-560).
fn change_policy(store: &Store) {
    let mut fresh = Store::open(store.root()).unwrap();
    fresh.config.review.max_rounds = Some(fresh.config.review.max_rounds.unwrap_or(3) + 1);
    fresh.save_config().unwrap();
}

/// Refuse the first launch at its final submission boundary by changing review policy there,
/// then restore the plain engine so the retry runs unobstructed.
fn refuse_by_policy_change(
    tmp: &std::path::Path,
    ctx: &Ctx,
    id: &str,
    launch: impl FnOnce(&Ctx, &str) -> anyhow::Result<corral_core::PlanReview> + Send + 'static,
) -> ActionAuthorization {
    let barrier = metadata_barrier(tmp);
    let launch_ctx = ctx.clone();
    let launch_id = id.to_string();
    let launch = std::thread::spawn(move || launch(&launch_ctx, &launch_id));
    let mut bytes = [0; 32];
    let (_, peer) = barrier.recv_from(&mut bytes).unwrap();
    change_policy(&ctx.store);
    barrier
        .send_to(b"release", peer.as_pathname().unwrap())
        .unwrap();
    let error = launch.join().unwrap().unwrap_err();
    assert!(crate::operating::refused(&error), "{error:#}");
    assert!(
        format!("{error:#}").contains("action policy revision changed"),
        "{error:#}"
    );
    let engine = tmp.join("engine");
    std::fs::copy(engine.join("base"), engine.join("herdr")).unwrap();
    assert!(!engine.join("launch-command").exists());
    let prepared: serde_json::Value = serde_json::from_slice(
        &std::fs::read(
            ctx.store
                .state_dir()
                .join(format!("plan-review-{id}-P1.json")),
        )
        .unwrap(),
    )
    .unwrap();
    let refused: ActionAuthorization =
        serde_json::from_value(prepared["authorization"].clone()).unwrap();
    assert!(matches!(
        ctx.store
            .authorized_submission(&refused.id)
            .unwrap()
            .disposition,
        SubmissionDisposition::AuthorizationRefused { .. }
    ));
    refused
}

fn assert_superseded(ctx: &Ctx, refused: &ActionAuthorization) -> ActionAuthorization {
    let record = ctx.store.authorized_submission(&refused.id).unwrap();
    assert_eq!(record.disposition, SubmissionDisposition::Submitted);
    assert_eq!(record.authorization_history.len(), 1);
    assert_eq!(&record.authorization_history[0].authorization, refused);
    assert!(matches!(
        record.authorization_history[0].disposition,
        SubmissionDisposition::AuthorizationRefused { .. }
    ));
    assert_ne!(
        record.authorization.policy_revision,
        refused.policy_revision
    );
    record.authorization
}

#[test]
fn owner_plan_review_after_a_policy_change_launches_with_fresh_consent() {
    // Given an owner plan review refused because review policy changed under it.
    let (tmp, ctx, id, _) = fixture();
    let refused = refuse_by_policy_change(tmp.path(), &ctx, &id, |ctx, id| {
        crate::plan_review(ctx, id, None)
    });
    assert!(matches!(
        refused.origin,
        OperatingOrigin::ExplicitOwner { .. }
    ));
    // When an automatic caller reaches the owner's preparation, it still cannot use it.
    assert!(crate::plan_review::review_auto(&ctx, &id).is_err());
    assert!(!tmp.path().join("engine/launch-command").exists());
    // When the owner asks again under the changed policy.
    let review = crate::plan_review(&ctx, &id, None).unwrap();
    // Then the same attempt launches under fresh owner consent; the refusal stays as audit.
    assert_eq!(review.id, "PR1");
    assert!(review.herdr.is_some());
    assert!(tmp.path().join("engine/launch-command").exists());
    let fresh = assert_superseded(&ctx, &refused);
    assert!(matches!(
        fresh.origin,
        OperatingOrigin::ExplicitOwner { .. }
    ));
    assert_ne!(fresh.origin, refused.origin);
}

#[test]
fn automatic_plan_review_after_a_policy_change_re_prepares_and_launches() {
    // Given an automatic plan review refused because review policy changed under it.
    let (tmp, ctx, id, _) = fixture();
    let refused = refuse_by_policy_change(tmp.path(), &ctx, &id, |ctx, id| {
        crate::plan_review::review_auto(ctx, id)
    });
    assert!(crate::operating::is_automatic(&refused));
    // When autopilot advances the plan again under the changed policy.
    advance(&ctx, &ctx.store.get(&id).unwrap(), Timestamp::now()).unwrap();
    // Then it re-prepares the same attempt under current policy and launches it.
    let task = ctx.store.get(&id).unwrap();
    let review = task.plans[0].reviews.last().unwrap();
    assert_eq!(review.id, "PR1");
    assert!(review.verdict.is_none() && review.herdr.is_some());
    assert!(tmp.path().join("engine/launch-command").exists());
    assert!(crate::operating::is_automatic(&assert_superseded(
        &ctx, &refused
    )));
}
