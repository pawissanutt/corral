//! S1-S7, S9, S17 and S18: ladder levels, scripted PM policies and restarts.
use corral_core::Status;
use corral_ops::attention;

use crate::fixture::{Fx, at};
use crate::support::{attend, invariants, only_key, pm_live, record, stats, sync, ts};

#[test]
fn s1_dropped_wakes_still_list_the_condition_and_notify_the_owner_once_at_the_bound() {
    // Given a live PM that drops every attention_due from its view.
    let fx = Fx::new();
    pm_live(&fx);
    let task = fx.stalled(Some("app"));
    // When passes run every 15 minutes through the owner bound and one pass after it.
    for seconds in [0, 900, 1800, 2700] {
        sync(&fx, seconds);
    }
    // Then the board still lists it, and exactly one owner notification fired at the bound.
    let key = only_key(&fx, &task.id);
    let board = attention::board(&fx.reopen(), at(2700)).unwrap();
    let row = board.tasks.iter().find(|row| row.task == task.id).unwrap();
    assert_eq!(row.assessment.conditions[0].key.as_ref().unwrap().0, key);
    let levels = record(&fx, &task.id, &key).levels;
    assert_eq!(levels.announced_at, ts(0));
    assert_eq!(levels.rewoke_at, ts(900));
    assert_eq!(levels.owner_claimed_at, ts(1800));
    assert_eq!(fx.calls(), 1);
    let stats = stats(&fx, 2700);
    let condition = &stats.conditions[&key];
    assert_eq!((condition.wakes, condition.owner_levels), (2, 1));
    assert_eq!(stats.population.total, 1);
    assert_eq!(stats.population.pm_available, 1);
    assert_eq!((stats.handling.observed, stats.handling.censored), (0, 1));
    assert_eq!(stats.wakes.attention_due, 2);
    assert_eq!(stats.wakes.due_pairs, 2);
    let notices = &stats.notifications;
    assert_eq!(
        (notices.claims, notices.batches, notices.success),
        (1, 1, 1)
    );
    assert_eq!(notices.calls, [1, 1]);
    assert_eq!(notices.by_producer["attention_ladder"].batches, 1);
    assert_eq!(notices.condition_links, 1);
    assert_eq!(stats.backlog.end_open, 1);
    assert_eq!(stats.backlog.end_max_secs, Some(2700));
    invariants(&stats);
}

#[test]
fn s2_each_wake_delivered_twice_to_a_responsive_pm_yields_one_handling_record() {
    // Given a live PM that receives every wake twice and answers each delivery.
    let fx = Fx::new();
    pm_live(&fx);
    let task = fx.stalled(Some("app"));
    sync(&fx, 0);
    let key = only_key(&fx, &task.id);
    let wake = format!("{key}/attention_opened");
    // When both deliveries of the same wake are handled with the wake's identity.
    let first = attend(&fx, &key, &wake, 1800, 60);
    let second = attend(&fx, &key, &wake, 1800, 61);
    sync(&fx, 900);
    // Then one handling revision exists and no duplicate action is counted.
    assert!(first.contains("EVAL_REVISION=1") && second.contains("EVAL_REVISION=1"));
    assert_eq!(record(&fx, &task.id, &key).handling.unwrap().revision, 1);
    assert_eq!(fx.logical("attention_handled").len(), 1);
    let stats = stats(&fx, 900);
    assert_eq!(stats.conditions[&key].handled, 1);
    assert_eq!(stats.handling.observed, 1);
    assert_eq!(stats.handling.p95_secs, Some(60));
    assert_eq!(
        stats.conditions[&key].wakes, 1,
        "handling paused the re-wake"
    );
    assert_eq!(stats.actions.duplicates, 0);
    invariants(&stats);
}

#[test]
fn s3_push_kinds_without_the_condition_kind_lose_no_condition() {
    // Given push enabled for an unrelated kind only, and no PM consuming anything.
    let mut fx = Fx::new();
    fx.ctx.store.config.pm.push_kinds = vec!["question_opened".into()];
    fx.ctx.store.save_config().unwrap();
    let task = fx.stalled(Some("app"));
    // When sync runs, then the ledger and the stats population both carry the condition.
    sync(&fx, 0);
    sync(&fx, 1800);
    let key = only_key(&fx, &task.id);
    let stats = stats(&fx, 1800);
    assert_eq!(stats.population.total, 1);
    assert_eq!(stats.conditions[&key].kind, "unexpected_stall");
    assert_eq!(stats.conditions[&key].owner_levels, 1);
    assert_eq!(fx.calls(), 1);
    invariants(&stats);
}

#[test]
fn s4_absent_pm_for_two_hours_gets_one_owner_notice_per_condition_and_no_rewake() {
    // Given two PM-responsible conditions and no PM pane at all.
    let fx = Fx::new();
    let tasks = [fx.stalled(Some("app")), fx.stalled(Some("app"))];
    // When passes run every 15 minutes for two hours.
    for seconds in (0..=7200).step_by(900) {
        sync(&fx, seconds);
    }
    // Then each condition got one owner notification and no re-wake, in the pm_absent cohort.
    assert_eq!(fx.calls(), 2);
    for task in &tasks {
        let key = only_key(&fx, &task.id);
        let levels = record(&fx, &task.id, &key).levels;
        assert_eq!(levels.rewoke_at, None);
        assert_eq!(levels.owner_claimed_at, ts(1800));
    }
    let stats = stats(&fx, 7200);
    assert_eq!(stats.population.pm_absent, 2);
    assert_eq!(stats.population.pm_available, 0);
    assert_eq!(stats.handling.observed + stats.handling.censored, 0);
    assert_eq!(stats.handling_pm_absent.censored, 2);
    assert_eq!(stats.wakes.attention_levels, 2);
    assert_eq!(stats.notifications.batches, 2);
    assert_eq!(stats.notifications.calls, [2, 2]);
    invariants(&stats);
}

#[test]
fn s5_pm_restart_during_handling_keeps_handling_and_never_reannounces() {
    // Given a handled condition.
    let fx = Fx::new();
    pm_live(&fx);
    let task = fx.stalled(Some("app"));
    sync(&fx, 0);
    let key = only_key(&fx, &task.id);
    attend(&fx, &key, "pm-1", 1800, 60);
    // When the PM pane disappears and a new one starts mid-handling.
    crate::support::pm_gone(&fx);
    sync(&fx, 600);
    pm_live(&fx);
    sync(&fx, 900);
    // Then handling persists, its pause holds, and nothing is announced again.
    let held = record(&fx, &task.id, &key);
    assert_eq!(held.handling.unwrap().request, "pm-1");
    assert_eq!(held.levels.rewoke_at, None);
    assert_eq!(fx.logical("attention_opened").len(), 1);
    let stats = stats(&fx, 900);
    assert_eq!(stats.conditions[&key].wakes, 1);
    assert_eq!(stats.conditions[&key].first_handled_at, ts(60));
    invariants(&stats);
}

#[test]
fn s6_board_restart_between_ladder_levels_never_repeats_a_level() {
    // Given an announced condition.
    let fx = Fx::new();
    pm_live(&fx);
    let task = fx.stalled(Some("app"));
    sync(&fx, 0);
    // When every pass is repeated on a freshly opened board.
    for seconds in [900, 900, 1800, 1800, 1801] {
        sync(&fx, seconds);
    }
    // Then each level is recorded once and the owner is invoked once.
    let key = only_key(&fx, &task.id);
    assert_eq!(fx.logical("attention_overdue").len(), 2);
    assert_eq!(fx.calls(), 1);
    let stats = stats(&fx, 1801);
    assert_eq!(stats.conditions[&key].wakes, 2);
    assert_eq!(stats.conditions[&key].owner_levels, 1);
    assert_eq!(stats.wakes.attention_due, 2);
    invariants(&stats);
}

#[test]
fn s7_muted_pm_with_push_and_desktop_off_still_meets_the_owner_bound() {
    // Given push and desktop notifications off and a PM muted for ten minutes.
    let fx = Fx::new();
    assert!(!fx.ctx.store.config.pm.push_events);
    pm_live(&fx);
    let task = fx.stalled(Some("app"));
    let mut key = String::new();
    // When five-minute passes run and the PM acknowledges after the mute with a short check.
    for seconds in (0..=2100).step_by(300) {
        sync(&fx, seconds);
        if seconds == 0 {
            key = only_key(&fx, &task.id);
        }
        if seconds == 600 {
            attend(&fx, &key, "after-mute", 60, 600);
        }
    }
    // Then every level was recorded and the owner bound held to the pass.
    let levels = record(&fx, &task.id, &key).levels;
    assert_eq!(levels.announced_at, ts(0));
    assert_eq!(levels.rewoke_at, ts(900));
    assert_eq!(levels.owner_claimed_at, ts(1800));
    assert_eq!(fx.calls(), 1);
    let stats = stats(&fx, 2100);
    assert_eq!(stats.handling.observed, 1);
    assert_eq!(stats.handling.p50_secs, Some(600));
    invariants(&stats);
}

#[test]
fn s9_replayed_attend_and_replayed_append_count_one_handling_fact() {
    // Given an accepted attend.
    let fx = Fx::new();
    pm_live(&fx);
    let task = fx.stalled(Some("app"));
    sync(&fx, 0);
    let key = only_key(&fx, &task.id);
    attend(&fx, &key, "lost-ack", 1800, 60);
    // When the acknowledgement is lost, the PM resends, and the event append is replayed.
    let replay = attend(&fx, &key, "lost-ack", 1800, 120);
    let handled = fx
        .ctx
        .store
        .events()
        .unwrap()
        .into_iter()
        .find(|event| event.kind == "attention_handled")
        .unwrap();
    fx.ctx
        .store
        .log_event(Some(&task.id), &handled.kind, handled.detail.clone())
        .unwrap();
    // Then there is one handling record and one deduplicated fact.
    assert!(replay.contains("EVAL_REVISION=1"));
    assert_eq!(record(&fx, &task.id, &key).handling.unwrap().revision, 1);
    let stats = stats(&fx, 120);
    assert_eq!(stats.conditions[&key].handled, 1);
    assert_eq!(stats.conditions[&key].first_handled_at, ts(60));
    invariants(&stats);
}

#[test]
fn s17_twenty_conditions_opened_and_resolved_in_one_pass_survive_pruning() {
    // Given twenty question conditions opened by one live-PM pass.
    let fx = Fx::new();
    pm_live(&fx);
    let task = fx.questions(20);
    sync(&fx, 0);
    assert_eq!(crate::support::keys(&fx, &task.id).len(), 20);
    // When the task finishes before any overdue level and the records are pruned.
    fx.ctx
        .store
        .update(&task.id, |task| {
            task.status = Status::Done;
            Ok(())
        })
        .unwrap();
    sync(&fx, 60);
    assert!(
        fx.ctx
            .store
            .get(&task.id)
            .unwrap()
            .attention
            .records
            .is_empty()
    );
    // Then 20 opened and 20 resolved facts, all PM-live, give complete stats.
    assert_eq!(fx.logical("attention_opened").len(), 20);
    assert_eq!(fx.logical("attention_resolved").len(), 20);
    let stats = stats(&fx, 60);
    assert_eq!(stats.population.total, 20);
    assert_eq!(stats.population.resolved, 20);
    assert_eq!(stats.population.pm_available, 20);
    assert_eq!(stats.handling.observed, 20);
    assert_eq!(stats.resolution.observed, 20);
    assert_eq!(stats.wakes.attention_due, 1);
    assert_eq!(stats.wakes.attention_levels, 20);
    assert_eq!(stats.notifications.batches, 0);
    assert_eq!(stats.backlog.end_open, 0);
    invariants(&stats);
}

#[test]
fn s18_herdr_unavailable_across_a_deadline_records_the_level_and_logs_one_failure() {
    // Given a known condition, then an engine that is offline and cannot notify.
    let fx = Fx::new();
    let task = fx.stalled(None);
    sync(&fx, 0);
    std::fs::write(fx.tmp.path().join("offline"), "").unwrap();
    std::fs::write(fx.tmp.path().join("notify-fail"), "").unwrap();
    // When the owner deadline crosses during the unavailable branch.
    sync(&fx, 1800);
    sync(&fx, 1900);
    // Then the level is recorded, the failure is logged once and no pane loss is invented.
    let key = only_key(&fx, &task.id);
    assert_eq!(
        record(&fx, &task.id, &key).levels.owner_claimed_at,
        ts(1800)
    );
    assert_eq!(fx.calls(), 1);
    let stats = stats(&fx, 1900);
    assert!(!stats.population.by_kind.contains_key("agent_lost"));
    assert_eq!(stats.notifications.failure, 1);
    assert_eq!(stats.notifications.calls, [1, 1]);
    invariants(&stats);
}
