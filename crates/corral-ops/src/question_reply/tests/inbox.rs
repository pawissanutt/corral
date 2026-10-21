//! Native answers for runs launched with inbox delivery, coding and planning alike.

use corral_core::inbox::{DeliveryMode, Inbox, Recipient};

use super::*;

fn inbox(fixture: Fixture, planner: bool) -> (Fixture, Recipient) {
    let fixture = if planner { fixture.planner() } else { fixture };
    fixture
        .ctx
        .store
        .update(&fixture.task.id, |task| {
            task.runs[0].delivery = DeliveryMode::Inbox;
            if let Some(run) = task.plan_runs.last_mut() {
                run.delivery = DeliveryMode::Inbox;
            }
            Ok(())
        })
        .unwrap();
    let recipient = if planner {
        Recipient::PlanRun {
            task: fixture.task.id.clone(),
            plan_run: "D1".into(),
        }
    } else {
        Recipient::Run {
            task: fixture.task.id.clone(),
            run: "R1".into(),
        }
    };
    (fixture, recipient)
}

fn messages(fixture: &Fixture, recipient: &Recipient) -> Vec<corral_core::inbox::Message> {
    Inbox::new(&fixture.ctx.store, recipient.clone())
        .unwrap()
        .messages()
        .unwrap()
}

fn observed(fixture: &Fixture, recipient: &Recipient, seq: u64) -> bool {
    crate::delivery::owner::is_observed(&fixture.ctx, recipient, seq).unwrap()
}

/// Request A settles without our answer and a new request B opens.
fn replace_question(fixture: &Fixture) {
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&fixture.session)
        .unwrap();
    for line in [
        json!({"type":"custom","customType":"ask-user:settlement","data":{"requestId":REQUEST,"status":"cancelled"}}),
        json!({"type":"message","message":{"role":"assistant","content":[{"type":"toolCall","id":"call-b","name":"request_user_input","arguments":{"questions":[{"id":"other","question":"Other?"}]}}]}}),
        json!({"type":"custom","customType":"ask-user:question","data":{"requestId":"call-b","headers":["Other"]}}),
    ] {
        writeln!(file, "{line}").unwrap();
    }
}

#[test]
fn a_replaced_question_is_never_answered_with_the_old_reply() {
    for planner in [false, true] {
        let (fixture, recipient) = inbox(Fixture::new(), planner);
        fixture.state.lock().unwrap().refuse = true;
        assert!(crate::reply(&fixture.ctx, &fixture.task.id, ANSWER).is_err());
        fixture.state.lock().unwrap().refuse = false;
        let bound = &messages(&fixture, &recipient)[0];
        assert_eq!(bound.binding.question_request.as_deref(), Some(REQUEST));
        replace_question(&fixture);
        // Resend recovers this message's own preparation, which names request A only.
        let error = crate::resend_reply(&fixture.ctx, &fixture.task.id).unwrap_err();
        assert!(format!("{error:#}").contains(REQUEST), "{error:#}");
        // A different reply cannot bypass the unsettled question-bound message either.
        let error = crate::reply(&fixture.ctx, &fixture.task.id, "answer B").unwrap_err();
        assert!(format!("{error:#}").contains("pending"), "{error:#}");
        assert!(fixture.state.lock().unwrap().received.is_empty());
        assert!(!observed(&fixture, &recipient, bound.seq));
        fixture.no_terminal_writes();
    }
}

#[test]
fn an_answer_bound_to_another_request_is_refused_before_sending() {
    let (fixture, _) = inbox(Fixture::new(), false);
    let task = fixture.ctx.store.get(&fixture.task.id).unwrap();
    let delivery = crate::question_reply::answer(
        &fixture.ctx,
        &task,
        ANSWER,
        false,
        &crate::question_reply::Bound {
            request: "call-somewhere-else",
            message: "T-001_R1#9",
        },
    )
    .unwrap();
    assert!(!delivery.submitted);
    assert!(delivery.error.unwrap().contains("call-somewhere-else"));
    assert!(fixture.state.lock().unwrap().received.is_empty());
}

#[test]
fn lost_ack_resend_finds_its_own_preparation_past_other_observations() {
    for planner in [false, true] {
        let (fixture, recipient) = inbox(Fixture::new(), planner);
        fixture.state.lock().unwrap().lose_ack = true;
        assert!(crate::reply(&fixture.ctx, &fixture.task.id, ANSWER).is_err());
        let bound = messages(&fixture, &recipient)[0].clone();
        // An unrelated owner message is observed in between.
        let note = crate::delivery::owner::post(
            &fixture.ctx,
            &recipient,
            "owner_note",
            "unrelated note",
            Default::default(),
            |_, _| Ok(()),
        )
        .unwrap();
        Inbox::new(&fixture.ctx.store, recipient.clone())
            .unwrap()
            .mark_observed(note.seq, "fixture: delivered elsewhere")
            .unwrap();
        // An ordinary reply cannot jump the unsettled native answer.
        assert!(crate::reply(&fixture.ctx, &fixture.task.id, "something else").is_err());
        crate::resend_reply(&fixture.ctx, &fixture.task.id).unwrap();
        assert_eq!(fixture.state.lock().unwrap().received, [ANSWER]);
        assert!(
            observed(&fixture, &recipient, bound.seq),
            "planner={planner}"
        );
        assert!(fixture.pending().is_none());
        fixture.no_terminal_writes();
    }
}

#[test]
fn refused_answer_resends_the_exact_request_once() {
    for planner in [false, true] {
        let (fixture, recipient) = inbox(Fixture::new(), planner);
        fixture.state.lock().unwrap().refuse = true;
        assert!(crate::reply(&fixture.ctx, &fixture.task.id, ANSWER).is_err());
        fixture.state.lock().unwrap().refuse = false;
        crate::resend_reply(&fixture.ctx, &fixture.task.id).unwrap();
        assert_eq!(fixture.state.lock().unwrap().received, [ANSWER]);
        let bound = &messages(&fixture, &recipient)[0];
        let receipt = Inbox::new(&fixture.ctx.store, recipient.clone())
            .unwrap()
            .entry(bound.seq)
            .unwrap()
            .observed
            .unwrap();
        assert_eq!(receipt.via, "omo_question");
        assert_eq!(receipt.detail["request"], REQUEST);
        assert_eq!(messages(&fixture, &recipient).len(), 1);
        fixture.no_terminal_writes();
    }
}
