//! S19 kill barriers through the real sync path. Compiled in-crate because the barriers are
//! cfg(test) seams; restart is a new Ctx on the same store. The fake engine's call log is the
//! independent transport oracle and is never derived from persisted state.
use corral_core::Status;
use serde_json::json;

use super::{Stats, Window, stats};
use crate::attention::handling::{CRASH as HANDLING_CRASH, CrashPoint as HandlingPoint};
use crate::owner_notice::{CRASH, CrashPoint};

use crate::attention::handling_tests::fixture::{Fx, at};

fn window(end: i64) -> Window {
    Window {
        start: "2000-01-01T00:00:00Z".parse().unwrap(),
        end: at(end),
    }
}

fn crashed_sync(fx: &Fx, seconds: i64, point: CrashPoint) {
    CRASH.with(|crash| crash.set(Some(point)));
    let error = crate::sync_at(&fx.reopen(), at(seconds)).unwrap_err();
    assert!(format!("{error:#}").contains("injected crash"), "{error:#}");
    CRASH.with(|crash| crash.set(None));
}

fn sync(fx: &Fx, seconds: i64) {
    crate::sync_at(&fx.reopen(), at(seconds)).unwrap();
}

fn finish(fx: &Fx, id: &str, seconds: i64) -> Stats {
    fx.ctx
        .store
        .update(id, |task| {
            task.status = Status::Done;
            Ok(())
        })
        .unwrap();
    sync(fx, seconds);
    sync(fx, seconds + 1);
    let task = fx.ctx.store.get(id).unwrap();
    assert!(task.attention.records.is_empty(), "records pruned");
    assert!(task.attention.notice_outbox.is_empty(), "claims pruned");
    stats(&fx.reopen(), window(seconds + 1)).unwrap()
}

type Row = (u64, u64, u64, u64, u64, u64, [u64; 2]);

fn counts(stats: &Stats) -> Row {
    let n = &stats.notifications;
    (
        n.claims,
        n.batches,
        n.success,
        n.failure,
        n.outcome_unknown,
        n.certainly_not_invoked,
        n.calls,
    )
}

#[test]
fn single_claim_matrix_matches_the_design_table_after_replay_and_prune() {
    for (case, expected, oracle) in [
        ("before_notify", (1, 1, 0, 0, 1, 0, [0, 1]), 0),
        ("after_notify", (1, 1, 0, 0, 1, 0, [0, 1]), 1),
        ("after_outcome_save", (1, 1, 1, 0, 0, 0, [1, 1]), 1),
        ("returned_failure", (1, 1, 0, 1, 0, 0, [1, 1]), 1),
        ("disabled", (1, 1, 0, 0, 0, 1, [0, 0]), 0),
    ] {
        // Given one PM condition reaching its owner level in one pass.
        let mut fx = Fx::new();
        if case == "disabled" {
            fx.ctx.store.config.pm.attention_notify = false;
            fx.ctx.store.save_config().unwrap();
        }
        if case == "returned_failure" {
            std::fs::write(fx.tmp.path().join("notify-fail"), "").unwrap();
        }
        let task = fx.stalled(None);
        sync(&fx, 0);
        // When the process stops at the named barrier, then restarts on the same store.
        match case {
            "before_notify" => crashed_sync(&fx, 1800, CrashPoint::BeforeNotify),
            "after_notify" => crashed_sync(&fx, 1800, CrashPoint::AfterNotify),
            "after_outcome_save" => crashed_sync(&fx, 1800, CrashPoint::AfterOutcomeSave),
            _ => sync(&fx, 1800),
        }
        sync(&fx, 1801);
        let stats = finish(&fx, &task.id, 1900);
        // Then durable counts match the table and the oracle stays independent of them.
        assert_eq!(counts(&stats), expected, "{case}");
        assert_eq!(fx.calls(), oracle, "{case}");
        assert_eq!(
            stats.notifications.by_producer["attention_ladder"].batches,
            1
        );
    }
}

fn question(fx: &Fx, id: &str, index: usize, asked: i64) {
    fx.ctx
        .store
        .update(id, |task| {
            let run = task.current_run_mut().unwrap();
            let state = run.agent_host.as_mut().unwrap();
            let key = format!("{id}/R1/Q{index}");
            state.questions.push(serde_json::from_value(json!({
                "version":1,"key":key,"revision":1,"kind":"clarification","blocking":false,
                "text":"Which constraint applies?","state":"open",
                "asked":{"request_id":key,"payload_sha256":"sha","session_id":"session","generation":1,"at":at(asked)},
                "wake":{"route":"none","key":key,"state":"pending"}
            }))?);
            Ok(())
        })
        .unwrap();
}

fn abc(post_send: bool) -> (Stats, usize) {
    // Given two due conditions on one task (A), then one (B), then two more (C).
    let fx = Fx::new();
    let task = fx.questions(2);
    // A's pass-0 publication stops after appending only one of its two opening facts.
    HANDLING_CRASH.with(|crash| crash.set(Some(HandlingPoint::AfterAppend)));
    assert!(crate::sync_at(&fx.reopen(), at(0)).is_err());
    HANDLING_CRASH.with(|crash| crash.set(None));
    assert_eq!(fx.logical("attention_opened").len(), 1);
    sync(&fx, 1);
    assert_eq!(fx.logical("attention_opened").len(), 2);
    sync(&fx, 1800);
    question(&fx, &task.id, 3, 2000);
    sync(&fx, 2000);
    std::fs::write(fx.tmp.path().join("notify-fail"), "").unwrap();
    sync(&fx, 3800);
    std::fs::remove_file(fx.tmp.path().join("notify-fail")).unwrap();
    question(&fx, &task.id, 4, 4000);
    question(&fx, &task.id, 5, 4000);
    sync(&fx, 4000);
    // When C stops before (pre-send) or after (post-send) transport, before its outcome.
    let point = if post_send {
        CrashPoint::AfterNotify
    } else {
        CrashPoint::BeforeNotify
    };
    crashed_sync(&fx, 5800, point);
    sync(&fx, 5801);
    let stats = finish(&fx, &task.id, 5900);
    (stats, fx.calls())
}

#[test]
fn abc_batches_keep_exact_coverage_and_identical_persisted_metrics_in_both_windows() {
    let (pre, pre_calls) = abc(false);
    let (post, post_calls) = abc(true);
    // Then the fake oracle differs while persisted metrics are identical.
    assert_eq!((pre_calls, post_calls), (2, 3));
    assert_eq!(pre.notifications, post.notifications);
    assert_eq!(counts(&pre), (5, 3, 1, 1, 1, 0, [2, 3]));
    let ladder = &pre.notifications.by_producer["attention_ladder"];
    assert_eq!((ladder.batches, ladder.claims), (3, 5));
    assert_eq!(pre.notifications.coverage_edges, 5);
    assert_eq!(pre.notifications.condition_links, 5);
    assert_eq!(pre.population.total, 5);
    assert_eq!(pre.population.resolved, 5);
    let batches: std::collections::BTreeSet<_> = pre
        .conditions
        .values()
        .filter_map(|row| row.owner_claim.clone())
        .collect();
    assert_eq!(
        batches.len(),
        3,
        "A's two claims stay in one physical batch"
    );
}
