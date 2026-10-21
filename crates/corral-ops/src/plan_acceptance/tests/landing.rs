use super::*;
use corral_core::{AggregateAcceptance, AggregateLandingState};

const GATE: [&str; 2] = ["test -f first.txt", "test -f second.txt"];

fn plan_head(f: &Fixture) -> String {
    corral_git::rev_sha(&f.repo, "refs/heads/feature/plan").unwrap()
}

fn integration_checkout(f: &Fixture) -> PathBuf {
    f.repo
        .join(".worktrees")
        .join(format!("{}-integration", f.plan))
}

/// Simulate a crash after durable preparation and before finalization.
fn prepare(f: &Fixture, attempt: &str, before: &str, after: &str) {
    f.ctx
        .store
        .update(&f.plan, |task| {
            let stored = task
                .aggregate_attempts
                .iter_mut()
                .find(|stored| stored.id == attempt)
                .unwrap();
            stored.acceptance = Some(AggregateAcceptance {
                authorization: "interrupted".into(),
                automatic: false,
                by: "owner".into(),
                at: Timestamp::now(),
                strategy: AggregateStrategy::FastForward,
                before: Some(before.into()),
                after: Some(after.into()),
                state: AggregateLandingState::Prepared,
            });
            Ok(())
        })
        .unwrap();
}

fn fast_forward_main(f: &Fixture, head: &str) {
    let _lock = corral_git::landing_lock(&f.repo).unwrap();
    corral_git::land_checked(
        &f.repo,
        &integration_checkout(f),
        "feature/plan",
        "main",
        head,
        &f.main(),
    )
    .unwrap();
}

#[test]
fn accepted_integration_candidate_lands_on_main_exactly_once() {
    // Given an integration Plan whose verified combined candidate is ahead of main.
    let f = Fixture::new(Delivery::Integration, &GATE);
    let (before, head) = (f.main(), plan_head(&f));
    assert_ne!(before, head);
    let attempt = f.reviewed();
    assert_eq!(attempt.identity.head, head);
    // When the owner accepts it.
    let acceptance = plan_accept(&f.ctx, &f.plan, &attempt.id, false).unwrap();
    // Then main fast-forwards to exactly the checked candidate and the Plan completes.
    assert_eq!(acceptance.strategy, AggregateStrategy::FastForward);
    assert_eq!(acceptance.state, AggregateLandingState::Landed);
    assert_eq!(
        (acceptance.before.as_deref(), acceptance.after.as_deref()),
        (Some(before.as_str()), Some(head.as_str()))
    );
    assert_eq!(f.main(), head);
    assert_eq!(f.plan().status, Status::Done);
    assert!(!acceptance.automatic && acceptance.by == "owner");
    let commits = f.commits();
    // When acceptance is retried, it returns the same receipt without landing again.
    assert_eq!(
        plan_accept(&f.ctx, &f.plan, &attempt.id, false).unwrap(),
        acceptance
    );
    assert!(plan_verify(&f.ctx, &f.plan).is_err());
    assert_eq!((f.main(), f.commits()), (head, commits));
    assert_eq!(f.accepted_events(), 1);
}

#[test]
fn crash_after_main_moved_reconciles_without_a_second_landing() {
    // Given durable preparation whose fast-forward completed before the crash.
    let f = Fixture::new(Delivery::Integration, &GATE);
    let attempt = f.reviewed();
    let (before, head) = (f.main(), plan_head(&f));
    prepare(&f, &attempt.id, &before, &head);
    fast_forward_main(&f, &head);
    let commits = f.commits();
    // When acceptance runs again.
    let acceptance = plan_accept(&f.ctx, &f.plan, &attempt.id, false).unwrap();
    // Then it records the observed landing once instead of authorizing another.
    assert_eq!(acceptance.state, AggregateLandingState::Landed);
    assert_eq!(acceptance.authorization, "interrupted");
    assert_eq!((f.main(), f.commits()), (head, commits));
    assert_eq!(f.accepted_events(), 1);
    assert_eq!(f.plan().status, Status::Done);
}

#[test]
fn crash_before_main_moved_lands_once_after_fresh_authorization() {
    // Given durable preparation and no Git movement.
    let f = Fixture::new(Delivery::Integration, &GATE);
    let attempt = f.reviewed();
    let (before, head) = (f.main(), plan_head(&f));
    prepare(&f, &attempt.id, &before, &head);
    // When acceptance runs again, the stale intent is released and checked anew.
    let acceptance = plan_accept(&f.ctx, &f.plan, &attempt.id, false).unwrap();
    // Then main moves exactly once to the checked candidate.
    assert_ne!(acceptance.authorization, "interrupted");
    assert_eq!(f.main(), head);
    assert_eq!(f.accepted_events(), 1);
}

#[test]
fn main_moved_outside_a_prepared_landing_refuses_reconciliation() {
    // Given durable preparation, then an unrelated main commit.
    let f = Fixture::new(Delivery::Integration, &GATE);
    let attempt = f.reviewed();
    let (before, head) = (f.main(), plan_head(&f));
    prepare(&f, &attempt.id, &before, &head);
    std::fs::write(f.repo.join("other.txt"), "unrelated\n").unwrap();
    corral_git::commit_all(&f.repo, "unrelated").unwrap();
    let main = f.main();
    // Then neither landing nor completion is inferred from the moved ref.
    let error = message(plan_accept(&f.ctx, &f.plan, &attempt.id, false));
    assert!(error.contains("main moved outside the prepared"), "{error}");
    f.assert_not_completed(&main);
}

#[test]
fn integration_target_equal_to_main_completes_without_a_merge() {
    // Given an integration Plan whose branch is already on main.
    let f = Fixture::new(Delivery::Integration, &GATE);
    let head = plan_head(&f);
    fast_forward_main(&f, &head);
    let commits = f.commits();
    let attempt = f.reviewed();
    // Then acceptance records the exact head without a fake commit.
    let acceptance = plan_accept(&f.ctx, &f.plan, &attempt.id, false).unwrap();
    assert_eq!(acceptance.strategy, AggregateStrategy::AlreadyOnMain);
    assert_eq!((f.main(), f.commits()), (head, commits));
}
