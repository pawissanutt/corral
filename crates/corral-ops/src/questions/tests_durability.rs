use corral_core::Status;
use corral_core::agent::{ContainerRef, QuestionState};
use corral_core::inbox::{Inbox, Recipient, TerminalState};
use jiff::{SignedDuration, Timestamp};
use serde_json::{Value, json};

use super::tests_support::*;
use super::{Actor, AnswerInput};
use crate::agent::{CallResponse, fence_generation};

const PM_OPS: [&str; 3] = ["question_answer", "question_escalate", "question_cancel"];

fn pm_payload(fx: &Fx, op: &str, key: &str) -> Value {
    let task = key.split('/').next().unwrap();
    match op {
        "question_answer" => json!({
            "key": key, "rev": 1, "text": "use the default", "basis_refs": [spec_ref(fx, task)],
            "scope_note": "inside the approved write scope",
        }),
        "question_escalate" => json!({"key": key, "rev": 1, "note": "owner call"}),
        _ => json!({"key": key, "rev": 1, "reason": "not needed"}),
    }
}

fn replayed(response: &CallResponse) -> bool {
    matches!(response, CallResponse::Receipt { replayed: true, .. })
}

#[test]
fn committed_pm_question_ops_replay_after_a_fence_and_from_a_replacement_generation() {
    for op in PM_OPS {
        let fx = fx();
        let (_, worker) = worker(&fx);
        let key = ask_ok(&fx, &worker, "c1", true);
        let pm = pm_host(&fx);
        let payload = pm_payload(&fx, op, &key);
        let request = request_id(&pm, "t1");
        let first = send_request(&fx, &pm, op, &request, payload.clone()).unwrap();
        let receipt = first
            .receipt()
            .unwrap_or_else(|| panic!("{op}: {first:?}"))
            .clone();
        let generation = pm.identity.generation;
        assert!(
            fence_generation(&fx.ctx.store, &ContainerRef::Pm, generation, "host lost").unwrap()
        );

        let replay = send_request(&fx, &pm, op, &request, payload.clone()).unwrap();
        assert!(replayed(&replay), "{op}: {replay:?}");
        assert_eq!(replay.receipt(), Some(&receipt), "{op}");
        let other = ask_ok(&fx, &worker, "c2", false);
        let fresh = send(&fx, &pm, op, "t2", pm_payload(&fx, op, &other));
        assert_eq!(
            code(&fresh),
            Some("agent_fenced"),
            "{op}: a new request is fenced"
        );

        let replacement = pm_host(&fx);
        assert_eq!(replacement.identity.generation, generation + 1);
        let retry = send_request(&fx, &replacement, op, &request, payload).unwrap();
        assert!(replayed(&retry), "{op}: {retry:?}");
        assert_eq!(retry.receipt(), Some(&receipt), "{op}");
        assert_eq!(
            question(&fx, &key).revision,
            2,
            "{op}: committed exactly once"
        );
    }
}

#[test]
fn pm_request_ids_are_unique_across_target_tasks_and_ops() {
    for op in PM_OPS {
        let fx = fx();
        let (a, worker_a) = worker(&fx);
        let (b, worker_b) = worker(&fx);
        let key_a = ask_ok(&fx, &worker_a, "c1", true);
        let key_b = ask_ok(&fx, &worker_b, "c1", true);
        let pm = pm_host(&fx);
        assert!(
            send(&fx, &pm, op, "same", pm_payload(&fx, op, &key_a))
                .receipt()
                .is_some()
        );

        let other_task = send(&fx, &pm, op, "same", pm_payload(&fx, op, &key_b));
        assert_eq!(code(&other_task), Some("payload_mismatch"), "{op}");
        let untouched = fx.ctx.store.get(&b).unwrap();
        assert!(untouched.agent_pm_receipts.is_empty(), "{op}");
        assert_eq!(question(&fx, &key_b).revision, 1, "{op}");
        assert!(messages(&fx, &b, "R1").is_empty(), "{op}");

        let other_op = PM_OPS.iter().find(|o| **o != op).unwrap();
        let cross = send(
            &fx,
            &pm,
            other_op,
            "same",
            pm_payload(&fx, other_op, &key_a),
        );
        assert_eq!(
            code(&cross),
            Some("payload_mismatch"),
            "{op} then {other_op}"
        );
        assert_eq!(
            fx.ctx.store.get(&a).unwrap().agent_pm_receipts.len(),
            1,
            "{op}"
        );
    }
}

#[test]
fn withdrawal_settles_question_owned_needs_me_and_keeps_other_blocks() {
    let fx = fx();
    let withdraw = |host: &Host, key: &str, call: &str| {
        let payload = json!({"key": key, "reason": "resolved it from the spec"});
        assert!(
            send(&fx, host, "withdraw", call, payload)
                .receipt()
                .is_some()
        );
    };
    let (escalated, first) = worker(&fx);
    let key = ask_ok(&fx, &first, "c1", true);
    super::escalate(&fx.ctx, &key, 1, Actor::Pm(None), None, None).unwrap();
    assert_eq!(
        fx.ctx.store.get(&escalated).unwrap().status,
        Status::NeedsMe
    );
    withdraw(&first, &key, "w1");
    let task = fx.ctx.store.get(&escalated).unwrap();
    assert_eq!(
        (task.status, task.runs[0].blocked_excerpt.clone()),
        (Status::Running, None)
    );

    let (quiet, quiet_worker) = worker(&fx);
    let blocking = ask_ok(&fx, &quiet_worker, "c1", true);
    let later = Timestamp::now() + SignedDuration::from_secs(601);
    assert_eq!(
        super::unattended(&fx.ctx, &quiet, later).unwrap(),
        vec![blocking.clone()]
    );
    assert_eq!(fx.ctx.store.get(&quiet).unwrap().status, Status::NeedsMe);
    withdraw(&quiet_worker, &blocking, "w1");
    assert_eq!(fx.ctx.store.get(&quiet).unwrap().status, Status::Running);

    let (two, two_worker) = worker(&fx);
    let first = ask_ok(&fx, &two_worker, "c1", true);
    let second = ask_ok(&fx, &two_worker, "c2", false);
    for key in [&first, &second] {
        super::escalate(&fx.ctx, key, 1, Actor::Pm(None), None, None).unwrap();
    }
    withdraw(&two_worker, &first, "w1");
    let task = fx.ctx.store.get(&two).unwrap();
    assert_eq!(
        task.status,
        Status::NeedsMe,
        "another owner question still waits"
    );
    assert!(
        task.runs[0]
            .blocked_excerpt
            .as_deref()
            .unwrap()
            .contains(&second)
    );

    let (other, other_worker) = worker(&fx);
    let key = ask_ok(&fx, &other_worker, "c1", false);
    fx.ctx
        .store
        .update(&other, |t| {
            t.status = Status::NeedsMe;
            t.runs[0].blocked_excerpt = Some("process stuck".into());
            Ok(())
        })
        .unwrap();
    withdraw(&other_worker, &key, "w1");
    let task = fx.ctx.store.get(&other).unwrap();
    assert_eq!(task.status, Status::NeedsMe, "an unrelated block stays");
    assert_eq!(
        task.runs[0].blocked_excerpt.as_deref(),
        Some("process stuck")
    );
}

#[test]
fn question_events_recover_after_a_post_commit_logging_failure() {
    let cases = [
        ("ask", "question_opened"),
        ("withdraw", "question_withdrawn"),
        ("question_answer", "question_answered"),
        ("question_escalate", "question_escalated"),
        ("question_cancel", "question_cancelled"),
    ];
    for (op, kind) in cases {
        let fx = fx();
        let (_, worker) = worker(&fx);
        let pm = pm_host(&fx);
        let (host, payload) = match op {
            "ask" => (&worker, json!({"kind": "clarification", "text": "which?"})),
            "withdraw" => {
                let key = ask_ok(&fx, &worker, "c0", false);
                (&worker, json!({"key": key, "reason": "done"}))
            }
            _ => {
                let key = ask_ok(&fx, &worker, "c0", false);
                (&pm, pm_payload(&fx, op, &key))
            }
        };
        let request = request_id(host, "t1");
        break_events(&fx);
        let failed = send_request(&fx, host, op, &request, payload.clone());
        restore_events(&fx);
        assert!(failed.is_err(), "{op}: the post-commit event write failed");
        assert_eq!(events(&fx, kind), 0, "{op}");
        let replay = send_request(&fx, host, op, &request, payload.clone()).unwrap();
        assert!(replayed(&replay), "{op}: {replay:?}");
        assert_eq!(events(&fx, kind), 1, "{op}: replay recovered the event");
        send_request(&fx, host, op, &request, payload).unwrap();
        assert_eq!(events(&fx, kind), 1, "{op}: recovery is idempotent");
    }
}

#[test]
fn sweep_and_owner_transition_events_recover_through_reconciliation() {
    let fx = fx();
    let (id, worker) = worker(&fx);
    let key = ask_ok(&fx, &worker, "c1", false);
    let owner = AnswerInput {
        text: "ship it".into(),
        ..AnswerInput::default()
    };
    break_events(&fx);
    let failed = super::answer(&fx.ctx, &key, 1, Actor::Owner, owner);
    restore_events(&fx);
    assert!(failed.is_err());
    assert_eq!(
        question(&fx, &key).state,
        QuestionState::Answered,
        "committed"
    );
    assert_eq!(events(&fx, "question_answered"), 0);
    assert_eq!(super::reconcile_events(&fx.ctx.store, &id).unwrap(), 1);
    assert_eq!(events(&fx, "question_answered"), 1);
    assert_eq!(super::reconcile_events(&fx.ctx.store, &id).unwrap(), 0);

    let open = ask_ok(&fx, &worker, "c2", false);
    fx.ctx
        .store
        .update(&id, |t| {
            t.runs[0].outcome = Some(corral_core::RunOutcome::Discarded);
            Ok(())
        })
        .unwrap();
    break_events(&fx);
    assert!(super::terminalize(&fx.ctx, &id).is_err());
    restore_events(&fx);
    assert_eq!(question(&fx, &open).terminal.unwrap().reason, "run_ended");
    assert_eq!(events(&fx, "question_cancelled"), 0);
    assert!(super::terminalize(&fx.ctx, &id).unwrap().is_empty());
    assert_eq!(
        events(&fx, "question_cancelled"),
        1,
        "the next sweep recovered it"
    );
}

#[test]
fn reviewer_answers_corrections_and_expiries_reach_the_exact_child_only() {
    let fx = fx();
    let (id, reviewer) = reviewer(&fx);
    let answered = ask_ok(&fx, &reviewer, "r1", true);
    let deadline = ask_ok(&fx, &reviewer, "r2", false);
    pm_answer(&fx, &answered, 1).unwrap();
    let correction = AnswerInput {
        text: "use the other option".into(),
        ..AnswerInput::default()
    };
    super::answer(&fx.ctx, &answered, 2, Actor::Owner, correction).unwrap();
    super::escalate(&fx.ctx, &deadline, 1, Actor::Pm(None), None, None).unwrap();
    let at = Timestamp::now() + SignedDuration::from_hours(1);
    super::deadline(&fx.ctx, &deadline, 2, at, "approve it").unwrap();
    let after = at + SignedDuration::from_secs(1);
    assert_eq!(
        super::expire_deadlines(&fx.ctx, &id, after).unwrap(),
        vec![deadline.clone()]
    );

    let inbox = review_messages(&fx, &id, "R1-review1");
    let kinds: Vec<&str> = inbox.iter().map(|m| m.kind.as_str()).collect();
    assert_eq!(
        kinds,
        ["question_answer", "question_correction", "question_answer"]
    );
    assert!(
        inbox
            .iter()
            .all(|m| m.binding.run.as_deref() == Some("R1-review1"))
    );
    assert_eq!(
        question(&fx, &deadline).answer.unwrap().message_id,
        Some(inbox[2].id.clone())
    );
    assert!(
        messages(&fx, &id, "R1").is_empty(),
        "the parent run receives nothing"
    );

    fx.ctx
        .store
        .update(&id, |t| {
            t.runs[0].review_runs[0].outcome = Some(corral_core::RunOutcome::Superseded);
            t.runs[0].review_runs.push(run("R1-review2", "p3"));
            Ok(())
        })
        .unwrap();
    let next = review_child(&fx, &id, "R1-review2", "p3", "g2:c2");
    let fresh = ask_ok(&fx, &next, "r1", true);
    super::terminalize(&fx.ctx, &id).unwrap();
    let closed = Inbox::new(
        &fx.ctx.store,
        Recipient::ReviewRun {
            task: id.clone(),
            run: "R1".into(),
            review: "R1-review1".into(),
        },
    )
    .unwrap();
    for message in &inbox {
        let terminal = closed.terminal(message.seq).unwrap();
        assert_eq!(terminal, Some(TerminalState::Cancelled), "{}", message.id);
    }
    pm_answer(&fx, &fresh, 1).unwrap();
    let next_inbox = review_messages(&fx, &id, "R1-review2");
    assert_eq!(next_inbox.len(), 1, "only the new child's own answer");
    assert_eq!(
        next_inbox[0].subject,
        Some(json!({"key": fresh, "revision": 2}))
    );
}

#[test]
fn review_run_inbox_keys_parse_apart_from_plan_review_keys() {
    let review = Recipient::ReviewRun {
        task: "T-001".into(),
        run: "R1".into(),
        review: "R1-review1".into(),
    };
    assert_eq!(review.key().unwrap(), "T-001_R1_rv-R1-review1");
    assert_eq!(
        Recipient::parse_key("T-001_R1_rv-R1-review1").unwrap(),
        review
    );
    let plan = Recipient::PlanReview {
        task: "T-001".into(),
        plan: "P1".into(),
        review: "PR1".into(),
    };
    assert_eq!(Recipient::parse_key("T-001_P1_PR1").unwrap(), plan);
    let colliding = Recipient::PlanReview {
        task: "T-001".into(),
        plan: "P1".into(),
        review: "rv-x".into(),
    };
    assert!(colliding.key().is_err());
}
