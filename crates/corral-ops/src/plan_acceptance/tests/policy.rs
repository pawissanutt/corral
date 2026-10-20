use super::*;
use corral_core::Accept;

const GATE: [&str; 2] = ["test -f first.txt", "test -f second.txt"];

fn authority(accept: bool, plan_accept: bool, aggregate_accept: bool) -> OperatingAuthority {
    OperatingAuthority {
        dispatch: true,
        accept,
        plan_accept,
        aggregate_accept,
    }
}

#[test]
fn automatic_acceptance_without_a_grant_is_refused() {
    // Given a fully verified and independently approved attempt but no grant.
    let f = Fixture::new(Delivery::Integration, &GATE);
    let main = f.main();
    let attempt = f.passing();
    f.review(&attempt.id, Verdict::Approve).unwrap();
    // Then automatic acceptance is refused; owner acceptance remains available.
    let error = message(plan_accept(&f.ctx, &f.plan, &attempt.id, true));
    assert!(error.contains("not authorized"), "{error}");
    f.assert_not_completed(&main);
}

#[test]
fn task_and_design_acceptance_grants_do_not_delegate_plan_acceptance() {
    // Given a grant with task and design-plan acceptance, but not aggregate acceptance.
    let f = Fixture::new(Delivery::Integration, &GATE);
    f.grant(authority(true, true, false));
    let main = f.main();
    let attempt = f.passing();
    f.review(&attempt.id, Verdict::Approve).unwrap();
    // Then the resolver explains the missing delegation.
    let error = message(plan_accept(&f.ctx, &f.plan, &attempt.id, true));
    assert!(error.contains("action was not delegated"), "{error}");
    f.assert_not_completed(&main);
}

#[test]
fn granted_automatic_acceptance_still_requires_independent_aggregate_review() {
    // Given an explicit aggregate grant.
    let f = Fixture::new(Delivery::Integration, &GATE);
    f.grant(authority(false, false, true));
    let main = f.main();
    let attempt = f.passing();
    // When no review exists, or the reviewer requested changes.
    let error = message(plan_accept(&f.ctx, &f.plan, &attempt.id, true));
    assert!(
        error.contains("independent approving aggregate review"),
        "{error}"
    );
    f.review(&attempt.id, Verdict::Changes).unwrap();
    assert!(plan_accept(&f.ctx, &f.plan, &attempt.id, true).is_err());
    // And neither the owner nor the PM can act as the independent reviewer.
    for reviewer in ["owner", "pm"] {
        assert!(
            plan_aggregate_review(&f.ctx, &f.plan, &attempt.id, reviewer, Verdict::Approve, "")
                .is_err()
        );
    }
    // When evidence changes after an approving review, the approval is stale.
    f.review(&attempt.id, Verdict::Approve).unwrap();
    f.observe(&attempt.id, "C1", ObservationStatus::NotRun)
        .unwrap();
    assert!(plan_accept(&f.ctx, &f.plan, &attempt.id, true).is_err());
    f.assert_not_completed(&main);
}

#[test]
fn explicit_owner_acceptance_restriction_blocks_an_aggregate_grant() {
    // Given an aggregate grant and an explicit owner-only acceptance setting.
    let f = Fixture::new(Delivery::Integration, &GATE);
    let mut store = f.ctx.store.reopen().unwrap();
    store.config.review.accept = Some(Accept::Owner);
    store.save_config().unwrap();
    f.grant(authority(false, false, true));
    let main = f.main();
    let attempt = f.passing();
    f.review(&attempt.id, Verdict::Approve).unwrap();
    // Then automatic acceptance is refused.
    let error = message(plan_accept(&f.ctx, &f.plan, &attempt.id, true));
    assert!(error.contains("explicit owner-only acceptance"), "{error}");
    f.assert_not_completed(&main);
}

#[test]
fn explicit_grant_and_independent_review_permit_automatic_acceptance() {
    // Given an explicit aggregate grant and an independent approving review.
    let f = Fixture::new(Delivery::Integration, &GATE);
    f.grant(authority(false, false, true));
    let policy = f.ctx.store.effective_operating_policy().unwrap();
    let tracker = &policy.tasks[&f.plan];
    assert!(tracker.allows(corral_core::OperatingActionKind::AcceptAggregate));
    assert!(!tracker.allows(corral_core::OperatingActionKind::Dispatch));
    let attempt = f.passing();
    f.review(&attempt.id, Verdict::Approve).unwrap();
    // When automatic acceptance runs under that grant.
    let acceptance = plan_accept(&f.ctx, &f.plan, &attempt.id, true).unwrap();
    // Then it lands with durable automatic provenance.
    assert!(acceptance.automatic);
    assert_eq!(acceptance.by, "operating-policy");
    let record = f
        .ctx
        .store
        .authorized_submission(&acceptance.authorization)
        .unwrap();
    assert!(matches!(
        record.authorization.origin,
        corral_core::OperatingOrigin::Automatic { .. }
    ));
    assert_eq!(f.main(), attempt.identity.head);
    assert_eq!(f.plan().status, Status::Done);
}
