//! T-252 recovery: Pi reviewers. A reviewer whose host dies while its pane stays is never timed
//! out by the idle watchdog: it waits in Needs me for `corral agent resume`, which returns the
//! task to Review so its next generation is admitted and nudged. A reviewer whose provider
//! authentication fails goes to the owner. One whose pane is gone is closed by the existing
//! process_lost path, and a new review child is retried in its place.
use std::collections::HashMap;

use corral_herdr::{AgentStatus, PaneInfo};
use jiff::{SignedDuration, Timestamp};
use serde_json::json;

use super::tests::{ask, event, messages, resume, state, sync};
use super::*;
use crate::agent::bridge::fixture::{self as f, Fx, HostProc};
use crate::agent::bridge::liveness::{self, LossReason, Probe};
use crate::agent::hooks::review::LOST_PREFIX;
use crate::agent::hooks::sync::AUTH_PREFIX;
use crate::agent::test_support as ts;
use corral_core::Status;
use corral_core::agent::ClosureReason;

fn review_recipient(fx: &Fx, review: &str) -> Recipient {
    Recipient::ReviewRun {
        task: fx.id.clone(),
        run: "R1".into(),
        review: review.into(),
    }
}

fn panes(ids: &[&str]) -> HashMap<String, PaneInfo> {
    let pane = |id: &&str| PaneInfo {
        pane_id: (*id).into(),
        workspace_id: "w0".into(),
        tab_id: "w0:t1".into(),
        agent: None,
        status: AgentStatus::Idle,
        title: None,
        cwd: None,
    };
    ids.iter().map(|id| ((*id).to_owned(), pane(id))).collect()
}

fn watchdog(fx: &Fx, at: Timestamp, inventory: &[&str]) -> Option<crate::Change> {
    crate::reviewer::reconcile_review_at(&fx.ctx, &fx.id, at, &panes(inventory)).unwrap()
}

fn turn(fx: &Fx, host: &ts::Host, seq: u64, turn: u64, ends: bool) {
    let origin = json!({"kind": "bridge", "idempotency_key": format!("r{turn}")});
    event(
        fx,
        host,
        seq,
        json!({"kind": "turn_started", "turn": turn, "origin": origin}),
    );
    if ends {
        event(
            fx,
            host,
            seq + 1,
            json!({"kind": "turn_ended", "turn": turn}),
        );
    }
}

/// A Review task whose Pi reviewer is bound in `w0:p2` and in its first turn.
fn reviewing(fx: &Fx, host: &HostProc, since: Timestamp) -> (ContainerRef, ts::Host) {
    let target = f::reviewing(fx);
    fx.pending_review("w0:p2", since);
    let bound = fx.bind(&target, "w0:p2", host);
    f::write_session(&bound, &f::session(&[], &[]));
    let to = review_recipient(fx, "R1-review1");
    f::bridge(&fx.ctx.store, &to, 1, Some(f::CAP), f::healthy);
    (target.container(), bound)
}

#[test]
fn a_lost_reviewer_waits_in_needs_me_for_resume_and_its_next_generation_is_admitted() {
    // Given a reviewer killed inside its first turn while its pane stays:
    let fx = f::fixture(false, 5);
    let mut host = HostProc::spawn(true);
    let (child, bound) = reviewing(&fx, &host, Timestamp::now());
    turn(&fx, &bound, 1, 1, false);
    host.kill();
    let loss = Probe::Lost {
        generation: 1,
        reason: LossReason::HostExited,
        detail: "killed".into(),
    };
    liveness::record(&fx.ctx.store, &child, &loss).unwrap();

    // When the idle watchdog runs past review_idle_after_secs over its idle pane:
    let idle = SignedDuration::from_secs(
        i64::try_from(fx.ctx.store.config.sync.review_idle_after_secs).unwrap() + 1,
    );
    let now = Timestamp::now();
    assert_eq!(watchdog(&fx, now, &["w0:p2"]), None);
    assert_eq!(watchdog(&fx, now + idle, &["w0:p2"]), None);

    // Then it does not time the lost reviewer out: the review and its child stay open:
    let task = fx.task();
    assert_eq!(task.status, Status::Review);
    assert_eq!(task.runs[0].idle_since, None);
    assert_eq!(agent::container_closure(&task, &child), None);

    // When sync runs, the review waits in Needs me for resume, its pane and child kept:
    sync(&fx);
    let task = fx.task();
    assert_eq!(task.status, Status::NeedsMe);
    let excerpt = task.runs[0].blocked_excerpt.clone().unwrap();
    assert!(excerpt.starts_with(LOST_PREFIX), "{excerpt}");
    let resume_hint = format!("corral agent resume {} --review R1-review1", fx.id);
    assert!(excerpt.contains(&resume_hint), "{excerpt}");
    let review = task.runs[0].review.as_ref().unwrap();
    assert!(review.verdict.is_none() && !review.process_lost);
    assert_eq!(agent::container_closure(&task, &child), None);

    // When the owner resumes it, the task is back in Review before generation 2 launches, and
    // generation 2's hello on the same Pi session is admitted:
    resume(&fx, &child, false).unwrap();
    assert_eq!(fx.task().status, Status::Review);
    assert_eq!(fx.task().runs[0].blocked_excerpt, None);
    let to = review_recipient(&fx, "R1-review1");
    let relaunched = HostProc::spawn(true);
    let next = fx.hello_next(&bound, &to, &relaunched);
    f::bridge(&fx.ctx.store, &to, 2, Some(f::CAP), f::quiet);
    assert_eq!(state(&fx, &child).binding.health, HostHealth::Healthy);

    // Then its silent first turn gets generation 2's nudge though generation 1's turn never
    // ended, and the notice reached the same review child:
    turn(&fx, &next, 1, 1, true);
    sync(&fx);
    let nudges = messages(&fx, &to, "turn_nudge");
    assert_eq!(nudges.len(), 1);
    assert_eq!(nudges[0].subject.as_ref().unwrap()["generation"], 2);
    let turns = state(&fx, &child).turns;
    assert!(turns.iter().any(|t| t.generation == 1 && t.ended.is_none()));
    assert_eq!(messages(&fx, &to, NOTICE_KIND).len(), 1);
    assert_eq!(fx.task().status, Status::Review);
}

#[test]
fn a_reviewer_whose_provider_authentication_fails_goes_to_the_owner() {
    // Given a healthy reviewer whose turn failed provider authentication:
    let fx = f::fixture(false, 5);
    let host = HostProc::spawn(true);
    let (_, bound) = reviewing(&fx, &host, Timestamp::now());
    turn(&fx, &bound, 1, 1, false);
    event(
        &fx,
        &bound,
        2,
        json!({"kind": "provider_error", "turn": 1, "class": "auth"}),
    );
    event(&fx, &bound, 3, json!({"kind": "turn_ended", "turn": 1}));

    // When the review watchdog runs over its live pane:
    let change = watchdog(&fx, Timestamp::now(), &["w0:p2"]).unwrap();

    // Then the review goes to the owner with the login step, never a retry or a paid fallback:
    assert_eq!(change.to, Status::NeedsMe);
    let excerpt = fx.task().runs[0].blocked_excerpt.clone().unwrap();
    assert!(excerpt.contains(AUTH_PREFIX), "{excerpt}");
    assert!(excerpt.contains("corral agent login") && excerpt.contains("no paid fallback"));
    let escalated = fx.events("provider_escalated");
    assert_eq!(escalated.len(), 1);
    assert_eq!(escalated[0].detail["class"], "auth");
    let to = review_recipient(&fx, "R1-review1");
    assert!(messages(&fx, &to, "turn_nudge").is_empty());
    assert!(fx.events("provider_retry").is_empty());
}

#[test]
fn a_held_reviewer_whose_pane_then_goes_away_is_closed_by_process_lost_and_escalated() {
    // Given a reviewer started an hour ago with a pending question and an undelivered message,
    // held in Needs me by a full sync after its host died while its pane stayed:
    let fx = f::fixture(false, 5);
    let mut host = HostProc::spawn(true);
    let started = Timestamp::now() - SignedDuration::from_hours(1);
    let (child, _) = reviewing(&fx, &host, started);
    let key = ask(&fx, &child);
    let to = review_recipient(&fx, "R1-review1");
    let note = Inbox::new(&fx.ctx.store, to.clone())
        .unwrap()
        .post(Post {
            kind: "owner_note",
            urgency: Urgency::Wake,
            author: "owner",
            body: "Check the parser too.",
            binding: MessageBinding::default(),
            subject: None,
            max_payload: corral_core::inbox::MAX_PAYLOAD,
        })
        .unwrap();
    host.kill();
    liveness::probe_hosts_in_tests();
    let mut at = Timestamp::now();
    let mut pass = || {
        crate::sync_at(&fx.ctx, at).unwrap();
        at += SignedDuration::from_secs(1);
    };
    pass();
    let held = fx.task();
    assert_eq!(held.status, Status::NeedsMe);
    let excerpt = held.runs[0].blocked_excerpt.clone().unwrap_or_default();
    assert!(excerpt.starts_with(LOST_PREFIX), "{excerpt}");

    // When its pane then goes away and normal reconciliation runs:
    std::fs::remove_file(fx.root().join("w0:p2")).unwrap();
    let escalated = |t: &corral_core::Task| {
        t.status == Status::NeedsMe
            && t.runs[0]
                .blocked_excerpt
                .as_deref()
                .is_some_and(|e| e.contains("closed without a verdict"))
    };
    for _ in 0..8 {
        if escalated(&fx.task()) {
            break;
        }
        pass();
    }

    // Then the existing path records the review process lost and closes the child, the hold no
    // longer offers resume, and the child's question and message are settled:
    let task = fx.task();
    assert!(escalated(&task), "{:?}", task.runs[0].blocked_excerpt);
    assert!(task.runs[0].review.as_ref().unwrap().process_lost);
    assert_eq!(
        agent::container_closure(&task, &child),
        Some(ClosureReason::ReviewInterrupted)
    );
    let question = state(&fx, &child)
        .questions
        .into_iter()
        .find(|q| q.key == key)
        .unwrap();
    assert_eq!(question.state, corral_core::agent::QuestionState::Cancelled);
    let inbox = Inbox::new(&fx.ctx.store, to).unwrap();
    assert!(inbox.terminal(note.seq).unwrap().is_some());
    let error = resume(&fx, &child, false).unwrap_err().to_string();
    assert!(
        error.starts_with("container_closed") || error.starts_with("pane_gone"),
        "{error}"
    );
    assert!(fx.events("agent_fenced").is_empty());
}

#[test]
fn a_reviewer_whose_pane_is_gone_is_closed_by_process_lost_and_a_new_child_is_retried() {
    // Given a reviewer started an hour ago whose host died and whose pane is gone:
    let fx = f::fixture(false, 5);
    let mut host = HostProc::spawn(true);
    let started = Timestamp::now() - SignedDuration::from_hours(1);
    let (child, _) = reviewing(&fx, &host, started);
    host.kill();
    std::fs::remove_file(fx.root().join("w0:p2")).unwrap();

    // When sync runs:
    sync(&fx);
    crate::sync_at(&fx.ctx, Timestamp::now()).unwrap();

    // Then no recovery hold is taken; the existing path records the review process lost and
    // closes the child, which resume refuses:
    let task = fx.task();
    assert!(task.runs[0].review.as_ref().unwrap().process_lost);
    let blocked = task.runs[0].blocked_excerpt.clone().unwrap_or_default();
    assert!(!blocked.starts_with(LOST_PREFIX), "{blocked}");
    assert_eq!(
        agent::container_closure(&task, &child),
        Some(ClosureReason::ReviewInterrupted)
    );
    let error = resume(&fx, &child, false).unwrap_err().to_string();
    assert!(
        error.starts_with("container_closed") || error.starts_with("pane_gone"),
        "{error}"
    );

    // When the review is retried as a new review child, that child binds and is live while the
    // lost one stays closed:
    let herdr = corral_herdr::Herdr::with_bin(fx.root().join("herdr"), None);
    let retried: corral_core::Run =
        serde_json::from_value(f::run_json("R1-review2", &fx.app(), "w0:p3", &herdr)).unwrap();
    std::fs::write(fx.root().join("w0:p3"), "").unwrap();
    fx.ctx
        .store
        .update(&fx.id, |t| {
            t.status = Status::Review;
            t.runs[0].review_runs.push(retried);
            Ok(())
        })
        .unwrap();
    fx.pending_review("w0:p3", Timestamp::now());
    let target = BindTarget::Reviewer {
        task: fx.id.clone(),
        run: "R1".into(),
        review: "R1-review2".into(),
        callback: "g1:c2".into(),
    };
    let replacement = HostProc::spawn(true);
    fx.bind(&target, "w0:p3", &replacement);
    let task = fx.task();
    assert_eq!(agent::container_closure(&task, &target.container()), None);
    assert!(agent::container_closure(&task, &child).is_some());
}
