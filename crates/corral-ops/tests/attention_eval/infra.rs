//! S8, S13 and S16: Git contention and abandoned operations during review never lose a round.
use corral_core::{InfraWaitClass, Status, Verdict};
use corral_ops::attention::metrics::{self, Window};

use crate::git_fixture::{GitFx, at, reviewing};

/// Autopilot events carry the real append time, which follows this fixture's fixed clock, so
/// the window ends at a fixed later instant to include them.
fn stats(fx: &GitFx) -> metrics::Stats {
    metrics::stats(
        &fx.ctx,
        Window {
            start: at(-3600),
            end: "2027-01-01T00:00:00Z".parse().unwrap(),
        },
    )
    .unwrap()
}

fn sync(fx: &GitFx, seconds: i64) {
    corral_ops::sync_at(&fx.ctx, at(seconds)).unwrap();
}

fn land(fx: &GitFx, seconds: i64) {
    fx.verdict(Verdict::Approve, "Reviewed candidate.");
    sync(fx, seconds);
    assert_eq!(fx.task().status, Status::Done);
}

#[test]
fn s8_short_and_long_index_lock_lose_no_round_and_hold_once_only_past_the_bound() {
    for (held_for, holds) in [(20, 0), (400, 1)] {
        // Given a live reviewer and an external writer's index.lock on the Plan target.
        let fx = reviewing();
        let rounds = fx.task().rounds;
        let lock = fx.marker("index.lock");
        // When ten-second passes run while the lock is held, then it is released.
        for seconds in (0..=held_for).step_by(10) {
            sync(&fx, seconds);
        }
        std::fs::remove_file(&lock).unwrap();
        sync(&fx, held_for + 1);
        land(&fx, held_for + 2);
        // Then no review round was lost; only the long case held once with one notice.
        assert_eq!(fx.task().rounds, rounds, "{held_for}");
        assert_eq!(fx.events("review_started"), 1, "{held_for}");
        assert_eq!(fx.events("infra_hold"), holds, "{held_for}");
        assert_eq!(fx.notifications(), holds, "{held_for}");
        let stats = stats(&fx);
        assert_eq!(stats.lost_review_rounds, 0);
        let infra = stats.notifications.by_producer.get("infra_hold");
        assert_eq!(infra.map_or(0, |p| p.batches), holds as u64, "{held_for}");
        assert_eq!(stats.notifications.calls, [holds as u64; 2], "{held_for}");
        crate::support::invariants(&stats);
    }
}

#[test]
fn s13_t318_lock_at_224309z_holds_at_the_bound_instead_of_escalating() {
    // Given the T-318 instant: a reviewer in flight, a lock appearing and no PM.
    let fx = reviewing();
    let rounds = fx.task().rounds;
    let lock = fx.marker("index.lock");
    // When five-minute passes cover the 101 minutes the incident took to reach the owner,
    // with the reviewer's approval arriving at minute 25 while the lock is still held.
    for seconds in (0..=6060).step_by(300).chain([6060]) {
        sync(&fx, seconds);
        if seconds == 1500 {
            fx.verdict(Verdict::Approve, "Reviewed candidate.");
        }
    }
    // Then nothing escalated or landed, and the owner heard at the 300-second bound.
    let task = fx.task();
    let wait = task.infra_wait.as_ref().unwrap();
    assert_eq!(wait.first_seen, at(0));
    assert_eq!(wait.held_at, Some(at(300)));
    assert_eq!(task.status, Status::Review);
    let escalations = fx
        .ctx
        .store
        .events()
        .unwrap()
        .iter()
        .filter(|event| event.kind == "autopilot" && event.detail["to"] == "escalated")
        .count();
    assert_eq!(escalations, 0);
    assert_eq!(fx.events("approved"), 0);
    assert_eq!(fx.notifications(), 1);
    // When the lock clears, the retained verification lands with no defect round.
    std::fs::remove_file(&lock).unwrap();
    sync(&fx, 6061);
    assert_eq!(fx.task().status, Status::Done);
    assert_eq!(fx.task().rounds, rounds);
    assert_eq!(fx.events("review_started"), 1);
    assert_eq!(fx.notifications(), 1);
    let stats = stats(&fx);
    assert_eq!(stats.lost_review_rounds, 0);
    let infra = &stats.notifications.by_producer["infra_hold"];
    assert_eq!((infra.batches, infra.success), (1, 1));
    assert_eq!(
        stats.notifications.batches, 1,
        "the ladder honours the hold's claim"
    );
    crate::support::invariants(&stats);
}

#[test]
fn s16_abandoned_merge_marker_is_an_operation_with_one_notice_and_is_untouched() {
    // Given an abandoned MERGE_HEAD in the target checkout.
    let fx = reviewing();
    let marker = fx.marker("MERGE_HEAD");
    let bytes = std::fs::read(&marker).unwrap();
    // When its bound passes over several passes.
    for seconds in [1, 301, 302, 900] {
        sync(&fx, seconds);
    }
    // Then it is an operation wait with one owner notification and an untouched marker.
    let wait = fx.task().infra_wait.unwrap();
    assert_eq!(wait.class, InfraWaitClass::Operation);
    assert_eq!(wait.held_at, Some(at(301)));
    assert_eq!(std::fs::read(&marker).unwrap(), bytes);
    assert_eq!(fx.events("infra_hold"), 1);
    assert_eq!(fx.notifications(), 1);
    let stats = stats(&fx);
    assert_eq!(stats.notifications.by_producer["infra_hold"].batches, 1);
    assert_eq!(stats.lost_review_rounds, 0);
    crate::support::invariants(&stats);
}
