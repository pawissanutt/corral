//! Provider-overload stalls against recorded OmO/OMP session logs, fake herdr and an injected clock.
use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use corral_core::{ObligationSource, Status, WaitHandle, WaitRequest};
use corral_ops::register_wait;
use jiff::Timestamp;
use serde_json::Value;

use super::{Fx, slug};

const ERROR_AT: &str = "2026-10-15T09:10:00.000Z";

fn fixture(name: &str) -> String {
    let path = format!(
        "{}/tests/fixtures/provider-stall/{name}.jsonl",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(path).unwrap()
}

fn write_log(dir: &Path, file: &str, log: &str, modified: Timestamp) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(file);
    std::fs::write(&path, log).unwrap();
    let handle = File::options().write(true).open(&path).unwrap();
    handle.set_modified(SystemTime::from(modified)).unwrap();
    path
}

fn omo_dir(fx: &Fx) -> PathBuf {
    fx.agent_dir
        .join("sessions")
        .join(format!("--{}--", slug(&fx.worktree)))
}

fn omp_dir(fx: &Fx) -> PathBuf {
    let relative = fx.worktree.strip_prefix(std::env::temp_dir()).unwrap();
    let bucket = relative.to_string_lossy().replace('/', "-");
    fx.omp_dir.join("sessions").join(format!("-tmp-{bucket}"))
}

/// The OmO session the run owns; the log is replaced in place, as a live harness appends to it.
fn install(fx: &Fx, name: &str) {
    write_log(&omo_dir(fx), "session.jsonl", &fixture(name), fx.at(1));
}

fn retries(fx: &Fx) -> Vec<Value> {
    let events = fx.ctx.store.events().unwrap();
    events
        .iter()
        .filter(|e| e.kind == "provider_retry")
        .map(|e| e.detail.clone())
        .collect()
}

fn replies(fx: &Fx) -> usize {
    std::fs::read_to_string(fx.bin_dir.join("replies.log"))
        .unwrap_or_default()
        .lines()
        .count()
}

/// Ticks that deliver the three retries (t+660, t+780, t+1080) for an error logged at t+600.
fn send_all_retries(fx: &Fx) {
    for secs in [600, 660, 780, 1080] {
        fx.sync(secs);
    }
    assert_eq!(retries(fx).len(), 3);
}

fn escalated_reason(fx: &Fx) -> String {
    let task = fx.sync(1380);
    assert_eq!(task.status, Status::NeedsMe);
    task.current_run().unwrap().blocked_excerpt.clone().unwrap()
}

#[test]
fn overloaded_agent_is_retried_with_backoff_then_escalated_with_the_real_reason() {
    let fx = Fx::new("omo");
    fx.herdr_says("blocked");
    install(&fx, "overloaded");
    for secs in [600, 630, 659] {
        let task = fx.sync(secs);
        assert_eq!(task.status, Status::Running, "t+{secs}");
    }
    assert_eq!(replies(&fx), 0);
    assert_eq!(fx.events("provider_stall"), 1);
    for (secs, sent) in [
        (660, 1),
        (779, 1),
        (780, 2),
        (1079, 2),
        (1080, 3),
        (1379, 3),
    ] {
        let task = fx.sync(secs);
        assert_eq!(task.status, Status::Running, "t+{secs}");
        assert_eq!(replies(&fx), sent, "t+{secs}");
    }
    let sent = retries(&fx);
    for (index, detail) in sent.iter().enumerate() {
        assert_eq!(detail["attempt"], index + 1);
        assert_eq!(detail["of"], 3);
        assert_eq!(detail["submitted"], true);
        assert_eq!(detail["pane"], "w1:p1");
    }
    assert_eq!(fx.events("needs_me"), 0);

    let excerpt = escalated_reason(&fx);
    assert!(excerpt.contains("provider overloaded since "), "{excerpt}");
    assert!(excerpt.contains("currently overloaded"), "{excerpt}");
    assert!(!excerpt.contains("QUESTION"), "{excerpt}");
    assert_eq!(fx.events("provider_escalated"), 1);
    assert_eq!(fx.events("needs_me"), 1);
    assert!(fx.notifications().contains("T-001"));

    let later = fx.sync(1500);
    assert_eq!(later.status, Status::NeedsMe);
    assert_eq!(
        later.current_run().unwrap().blocked_excerpt.as_ref(),
        Some(&excerpt)
    );
    assert_eq!(replies(&fx), 3);
}

fn declare_wait(fx: &Fx) {
    let request = WaitRequest {
        version: 1,
        id: "event".into(),
        run: "R1".into(),
        condition: "CI result is available".into(),
        owner: "fixture".into(),
        source: ObligationSource::Worker,
        deadline: Some(fx.at(7200)),
        handle: WaitHandle::Channel {
            channel_id: "fixture:ci-1".into(),
            capability: Some("wait_v1".into()),
        },
    };
    register_wait(&fx.ctx, "T-001", request, fx.at(0)).unwrap();
}

#[test]
fn an_exhausted_escalation_survives_a_pending_wait() {
    let fx = Fx::new("omo");
    fx.herdr_says("blocked");
    declare_wait(&fx);
    install(&fx, "overloaded");
    send_all_retries(&fx);
    let reason = escalated_reason(&fx);
    assert!(reason.contains("provider overloaded since "), "{reason}");
    for secs in [1390, 1500, 3000] {
        let task = fx.sync(secs);
        assert_eq!(task.status, Status::NeedsMe, "t+{secs}");
        let kept = task.current_run().unwrap().blocked_excerpt.as_ref();
        assert_eq!(kept, Some(&reason), "t+{secs}");
    }
    assert_eq!(fx.events("resumed"), 0);
    assert_eq!(retries(&fx).len(), 3);
}

#[test]
fn an_escalation_with_a_pending_wait_clears_when_the_session_recovers() {
    let fx = Fx::new("omo");
    fx.herdr_says("blocked");
    declare_wait(&fx);
    install(&fx, "overloaded");
    send_all_retries(&fx);
    escalated_reason(&fx);
    install(&fx, "recovered");
    let task = fx.sync(1500);
    assert_eq!(task.status, Status::Running);
    assert_eq!(task.current_run().unwrap().blocked_excerpt, None);
    assert_eq!(fx.events("provider_recovered"), 1);
}

#[test]
fn an_escalation_with_a_pending_wait_clears_when_the_harness_resumes() {
    let fx = Fx::new("omo");
    fx.herdr_says("blocked");
    declare_wait(&fx);
    install(&fx, "overloaded");
    send_all_retries(&fx);
    escalated_reason(&fx);
    fx.herdr_says("working");
    let task = fx.sync(1500);
    assert_eq!(task.status, Status::Running);
    assert_eq!(task.current_run().unwrap().blocked_excerpt, None);
}

#[test]
fn omp_agent_log_is_retried_and_escalated_like_omo() {
    let fx = Fx::new("omp");
    fx.herdr_says("blocked");
    let log = fixture("omp-overloaded").replace(
        "/home/pawis/miti-workspace/corral/.worktrees/T-001",
        &fx.worktree.to_string_lossy(),
    );
    write_log(&omp_dir(&fx), "session.jsonl", &log, fx.at(1));
    send_all_retries(&fx);
    assert_eq!(replies(&fx), 3);
    let excerpt = escalated_reason(&fx);
    assert!(excerpt.contains("provider overloaded since "), "{excerpt}");
    assert_eq!(fx.events("provider_escalated"), 1);
}

#[test]
fn omp_session_of_another_worktree_is_not_this_runs_stall() {
    let fx = Fx::new("omp");
    fx.herdr_says("blocked");
    let log = fixture("omp-overloaded");
    write_log(&omp_dir(&fx), "session.jsonl", &log, fx.at(1));
    let task = fx.sync(900);
    assert_eq!(task.status, Status::NeedsMe);
    assert_eq!(fx.events("provider_stall"), 0);
    assert_eq!(replies(&fx), 0);
}

#[test]
fn a_busy_delivery_lock_defers_the_agent_retry_without_consuming_an_attempt() {
    let fx = Fx::new("omo");
    fx.herdr_says("blocked");
    install(&fx, "overloaded");
    fx.sync(600);
    let held = fx.ctx.store.try_lock("delivery-T-001").unwrap().unwrap();
    for secs in [660, 700] {
        assert_eq!(fx.sync(secs).status, Status::Running, "t+{secs}");
    }
    assert!(retries(&fx).is_empty());
    assert_eq!(replies(&fx), 0);
    drop(held);
    fx.sync(701);
    let sent = retries(&fx);
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["attempt"], 1);
    assert_eq!(replies(&fx), 1);
}

#[test]
fn two_concurrent_due_ticks_submit_one_agent_retry() {
    let fx = Fx::new("omo");
    fx.herdr_says("blocked");
    install(&fx, "overloaded");
    fx.sync(600);
    let barrier = std::sync::Barrier::new(2);
    let ticks = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..2)
            .map(|_| {
                let ctx = fx.ctx.clone();
                let (barrier, now) = (&barrier, fx.at(660));
                scope.spawn(move || {
                    barrier.wait();
                    corral_ops::sync_at(&ctx, now).is_ok()
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert!(ticks.contains(&true), "no tick completed");
    let sent = retries(&fx);
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["attempt"], 1);
    assert_eq!(replies(&fx), 1);
}

#[test]
fn recovery_stops_the_retries_and_is_recorded() {
    let fx = Fx::new("omo");
    fx.herdr_says("blocked");
    install(&fx, "overloaded");
    fx.sync(600);
    fx.sync(660);
    assert_eq!(replies(&fx), 1);
    install(&fx, "recovered");
    fx.sync(700);
    fx.sync(1500);
    assert_eq!(fx.events("provider_recovered"), 1);
    assert_eq!(replies(&fx), 1);
    assert_eq!(fx.events("provider_escalated"), 0);
}

#[test]
fn a_newer_unrelated_session_neither_cancels_nor_feeds_the_retries() {
    let fx = Fx::new("omo");
    fx.herdr_says("blocked");
    install(&fx, "overloaded");
    fx.sync(600);
    fx.sync(660);
    assert_eq!(retries(&fx).len(), 1);
    write_log(
        &omo_dir(&fx),
        "other.jsonl",
        &fixture("recovered"),
        fx.at(700),
    );
    fx.sync(780);
    assert_eq!(retries(&fx).len(), 2);
    fx.sync(1080);
    assert_eq!(retries(&fx).len(), 3);
    assert_eq!(fx.events("provider_recovered"), 0);
}

#[test]
fn an_error_in_a_session_older_than_the_run_is_ignored() {
    let fx = Fx::new("omo");
    fx.herdr_says("blocked");
    let old = fixture("overloaded")
        .replace("2026-10-15T09:00:00.000Z", "2026-10-15T08:00:00.000Z")
        .replace(ERROR_AT, "2026-10-15T08:10:00.000Z");
    write_log(&omo_dir(&fx), "old.jsonl", &old, fx.at(1));
    let task = fx.sync(900);
    assert_eq!(task.status, Status::NeedsMe);
    assert_eq!(fx.events("provider_stall"), 0);
    assert_eq!(replies(&fx), 0);
}

#[test]
fn a_working_agent_defers_the_retry_until_it_stops() {
    let fx = Fx::new("omo");
    fx.herdr_says("working");
    install(&fx, "overloaded");
    fx.sync(700);
    assert_eq!(replies(&fx), 0);
    fx.herdr_says("idle");
    fx.sync(710);
    assert_eq!(replies(&fx), 1);
}

#[test]
fn a_final_retry_that_is_still_working_is_not_escalated_and_may_still_succeed() {
    let fx = Fx::new("omo");
    fx.herdr_says("blocked");
    install(&fx, "overloaded");
    send_all_retries(&fx);
    fx.herdr_says("working");
    for secs in [1380, 2000] {
        assert_eq!(fx.sync(secs).status, Status::Running, "t+{secs}");
    }
    assert_eq!(fx.events("provider_escalated"), 0);
    install(&fx, "recovered");
    fx.herdr_says("idle");
    fx.sync(2100);
    assert_eq!(fx.events("provider_recovered"), 1);
    assert_eq!(fx.events("provider_escalated"), 0);
    assert_eq!(retries(&fx).len(), 3);
}

#[test]
fn a_final_retry_that_failed_again_escalates_once_the_agent_stops() {
    let fx = Fx::new("omo");
    fx.herdr_says("blocked");
    install(&fx, "overloaded");
    send_all_retries(&fx);
    install(&fx, "retry-failed");
    fx.herdr_says("working");
    assert_eq!(fx.sync(1100).status, Status::Running);
    assert_eq!(fx.events("provider_escalated"), 0);
    fx.herdr_says("blocked");
    let task = fx.sync(1110);
    assert_eq!(task.status, Status::NeedsMe);
    assert_eq!(fx.events("provider_escalated"), 1);
    assert_eq!(retries(&fx).len(), 3);
}

#[test]
fn permanent_errors_are_left_alone() {
    let fx = Fx::new("omo");
    fx.herdr_says("blocked");
    install(&fx, "credentials");
    let task = fx.sync(900);
    assert_eq!(task.status, Status::NeedsMe);
    assert_eq!(fx.events("provider_stall"), 0);
    assert_eq!(replies(&fx), 0);
}

#[test]
fn harnesses_without_an_adapter_are_left_alone() {
    let other = Fx::new("fake");
    other.herdr_says("blocked");
    install(&other, "overloaded");
    let task = other.sync(900);
    assert_eq!(task.status, Status::NeedsMe);
    assert_eq!(
        task.current_run().unwrap().agent_state.as_deref(),
        Some("blocked")
    );
    assert_eq!(other.events("provider_stall"), 0);
    assert_eq!(replies(&other), 0);
}
