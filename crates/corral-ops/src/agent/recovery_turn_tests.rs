//! T-252 recovery: turns around a host loss. A host killed inside a turn leaves that turn open
//! as it was, and generation 2 still gets OD11's nudge, its escalation and provider retries; an
//! automatic retry in flight keeps its episode's budget across sync passes until the retries
//! are used up, and a retry turn that succeeds recovers the stall.
use jiff::{SignedDuration, Timestamp};
use serde_json::json;

use super::tests::{event, messages, resume, run_recipient, state, sync};
use super::*;
use crate::agent::bridge::fixture::{self as f, Fx, HostProc};
use crate::agent::test_support as ts;
use corral_core::Status;

fn started(fx: &Fx, host: &ts::Host, seq: u64, turn: u64) {
    let origin = json!({"kind": "bridge", "idempotency_key": format!("k{turn}")});
    event(
        fx,
        host,
        seq,
        json!({"kind": "turn_started", "turn": turn, "origin": origin}),
    );
}

/// Ends `turn` from event `seq` on, failed on `class` when given; returns the next event seq.
fn ended(fx: &Fx, host: &ts::Host, seq: u64, turn: u64, class: Option<&str>) -> u64 {
    let mut seq = seq;
    if let Some(class) = class {
        event(
            fx,
            host,
            seq,
            json!({"kind": "provider_error", "turn": turn, "class": class}),
        );
        seq += 1;
    }
    event(fx, host, seq, json!({"kind": "turn_ended", "turn": turn}));
    seq + 1
}

fn end_of(fx: &Fx, generation: u32, seq: u64) -> Timestamp {
    let turns = state(fx, &fx.worker()).turns;
    let turn = turns
        .iter()
        .find(|t| t.generation == generation && t.seq == seq);
    turn.and_then(|t| t.ended).unwrap()
}

fn advance(fx: &Fx, at: Timestamp) -> Option<crate::Change> {
    let task = fx.task();
    crate::agent::hooks::stall::advance_agent(&fx.ctx, &task, &task.runs[0], at).unwrap()
}

fn retries(fx: &Fx) -> usize {
    let nudges = messages(fx, &run_recipient(fx), "turn_nudge");
    let retry = |m: &&Message| {
        m.subject.as_ref().is_some_and(|s| {
            s["key"]
                .as_str()
                .is_some_and(|k| k.starts_with("provider-retry:"))
        })
    };
    nudges.iter().filter(retry).count()
}

#[test]
fn a_host_killed_inside_a_turn_leaves_it_open_and_generation_2_is_still_nudged_and_retried() {
    // Given a busy host: its turn 1 started and never ended before the host was killed:
    let fx = f::fixture(false, 5);
    let mut host = HostProc::spawn(true);
    let bound = fx.bind(&fx.worker_target(), "w0:p1", &host);
    f::write_session(&bound, &f::session(&[], &[]));
    started(&fx, &bound, 1, 1);
    host.kill();
    let exit = crate::launch_exit::receipt(&fx.ctx, &fx.id, "R1");
    std::fs::create_dir_all(exit.parent().unwrap()).unwrap();
    std::fs::write(&exit, "137\n").unwrap();
    sync(&fx);
    assert_eq!(state(&fx, &fx.worker()).binding.health, HostHealth::Lost);

    // When the owner resumes the run and generation 2 binds:
    resume(&fx, &fx.worker(), false).unwrap();
    let relaunched = HostProc::spawn(true);
    let next = fx.hello_next(&bound, &run_recipient(&fx), &relaunched);
    f::bridge(
        &fx.ctx.store,
        &run_recipient(&fx),
        2,
        Some(f::CAP),
        f::quiet,
    );
    assert_eq!(fx.task().status, Status::Running);

    // Then generation 1's interrupted turn stays recorded as it was, never ended or classified:
    let turns = state(&fx, &fx.worker()).turns;
    let lost = turns
        .iter()
        .find(|t| t.generation == 1 && t.seq == 1)
        .unwrap();
    assert_eq!((lost.ended, lost.class), (None, None));

    // When generation 2's first turn ends without a report, it gets the generation's one nudge:
    started(&fx, &next, 1, 1);
    let seq = ended(&fx, &next, 2, 1, None);
    sync(&fx);
    let nudges = messages(&fx, &run_recipient(&fx), "turn_nudge");
    assert_eq!(nudges.len(), 1);
    assert_eq!(nudges[0].subject.as_ref().unwrap()["generation"], 2);

    // And its next silent turn goes to the owner:
    started(&fx, &next, seq, 2);
    let seq = ended(&fx, &next, seq + 1, 2, None);
    let attention = sync(&fx).attention.unwrap();
    assert!(
        attention.starts_with(crate::agent::turns::HOLD_PREFIX),
        "{attention}"
    );

    // And a turn that then fails on provider overload is retried on provider_stall's backoff:
    started(&fx, &next, seq, 3);
    ended(&fx, &next, seq + 1, 3, Some("overload"));
    let failed = end_of(&fx, 2, 3);
    assert_eq!(advance(&fx, failed + SignedDuration::from_secs(61)), None);
    assert_eq!(retries(&fx), 1);
}

#[test]
fn an_automatic_retry_in_flight_keeps_its_budget_until_the_retries_are_used_up() {
    // Given a healthy host whose turn 1 failed on provider overload:
    let fx = f::fixture(false, 5);
    let host = HostProc::spawn(true);
    let bound = fx.bind(&fx.worker_target(), "w0:p1", &host);
    started(&fx, &bound, 1, 1);
    let mut seq = ended(&fx, &bound, 2, 1, Some("overload"));
    let first = end_of(&fx, 1, 1);
    let at = |secs: i64| first + SignedDuration::from_secs(secs);
    let key = crate::provider_stall::agent_key(&fx.id, &fx.task().runs[0]);
    assert_eq!(advance(&fx, at(61)), None);
    assert_eq!(retries(&fx), 1);

    // When each retry's own turn is in flight at a sync pass and then fails again:
    for (turn, held, due) in [(2, 100, 182), (3, 300, 483)] {
        started(&fx, &bound, seq, turn);
        assert_eq!(advance(&fx, at(held)), None);
        assert!(
            crate::provider_stall::retrying(&fx.ctx, &key),
            "turn {turn}"
        );
        assert!(fx.events("provider_recovered").is_empty(), "turn {turn}");
        seq = ended(&fx, &bound, seq + 1, turn, Some("overload"));
        assert_eq!(advance(&fx, at(due - 2)), None);
        assert_eq!(retries(&fx), usize::try_from(turn - 1).unwrap());
        assert_eq!(advance(&fx, at(due)), None);
        assert_eq!(retries(&fx), usize::try_from(turn).unwrap(), "turn {turn}");
    }

    // Then the third retry failing too uses the allowance up and goes to the owner once:
    started(&fx, &bound, seq, 4);
    assert_eq!(advance(&fx, at(600)), None);
    ended(&fx, &bound, seq + 1, 4, Some("overload"));
    let change = advance(&fx, at(784)).unwrap();
    assert_eq!(change.to, Status::NeedsMe);
    assert!(crate::provider_stall::escalated(&fx.ctx, &key));
    assert_eq!(fx.events("provider_retry").len(), 3);
    assert_eq!(fx.events("provider_escalated").len(), 1);
    assert!(fx.events("provider_recovered").is_empty());
}

#[test]
fn a_retry_turn_that_succeeds_recovers_the_stall_once_it_ends() {
    // Given a turn that failed on provider overload and its first automatic retry, in flight:
    let fx = f::fixture(false, 5);
    let host = HostProc::spawn(true);
    let bound = fx.bind(&fx.worker_target(), "w0:p1", &host);
    started(&fx, &bound, 1, 1);
    let seq = ended(&fx, &bound, 2, 1, Some("rate_limit"));
    let first = end_of(&fx, 1, 1);
    let key = crate::provider_stall::agent_key(&fx.id, &fx.task().runs[0]);
    advance(&fx, first + SignedDuration::from_secs(61));
    started(&fx, &bound, seq, 2);
    advance(&fx, first + SignedDuration::from_secs(90));
    assert!(crate::provider_stall::retrying(&fx.ctx, &key));

    // When the retry turn ends without a provider error:
    ended(&fx, &bound, seq + 1, 2, None);
    advance(&fx, first + SignedDuration::from_secs(95));

    // Then the stall is recovered once and its state is gone:
    assert!(!crate::provider_stall::retrying(&fx.ctx, &key));
    assert_eq!(fx.events("provider_recovered").len(), 1);
    assert_eq!(retries(&fx), 1);
}
