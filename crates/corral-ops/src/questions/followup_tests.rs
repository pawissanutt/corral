//! Cheap service-level regressions: no live host, timing sleeps or concurrent processes.
use super::tests_support::*;
use super::{Actor, AnswerInput, QuestionRefusal, RefusalKind};
use corral_core::agent::{QuestionState, owner_question::owner_question_container};
use serde_json::json;

#[test]
fn worker_followup_and_answer_compare_the_same_revision() {
    let fx = fx();
    fx.ctx
        .store
        .set_attribution(corral_core::Actor::Owner, "app:device-one")
        .unwrap();
    let (_, host) = worker(&fx);
    let key = ask_ok(&fx, &host, "race", false);
    super::followup(&fx.ctx, &key, 1, "Explain the rollback.").unwrap();
    let error = super::answer(
        &fx.ctx,
        &key,
        1,
        Actor::Owner,
        AnswerInput {
            text: "Proceed".into(),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert_eq!(
        error.downcast_ref::<QuestionRefusal>().unwrap().kind,
        RefusalKind::StaleRevision
    );
    let current = question(&fx, &key);
    assert_eq!(current.revision, 2);
    assert_eq!(
        current.followups[0].attribution.device.as_deref(),
        Some("device-one")
    );
    assert!(current.answer.is_none());
    super::answer(
        &fx.ctx,
        &key,
        2,
        Actor::Owner,
        AnswerInput {
            text: "Proceed after rollback".into(),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(super::followup(&fx.ctx, &key, 2, "late").is_err());
    assert!(super::followup(&fx.ctx, &key, 3, "cannot reopen").is_err());
    let current = question(&fx, &key);
    assert_eq!(current.state, QuestionState::Answered);
    assert_eq!(current.followups.len(), 1);
    assert_eq!(
        current.answer.unwrap().attribution.unwrap().via,
        "app:device-one"
    );
}

#[test]
fn pm_authored_mirror_is_not_a_pm_answer_to_a_worker_and_polling_is_unstored() {
    let fx = fx();
    let pm = pm_host(&fx);
    let opened = send(
        &fx,
        &pm,
        "owner_ask",
        "open-owner-ask",
        json!({
            "action": "open", "text": "Which acceptance criterion?", "options": ["A", "B"]
        }),
    );
    let key = opened.receipt().unwrap().result["key"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        owner_question_container(&key),
        Some(corral_core::agent::ContainerRef::Pm)
    );
    let before = corral_core::agent::pm_state::child_count(&fx.ctx.store, "receipts").unwrap();
    let poll = send(
        &fx,
        &pm,
        "owner_ask",
        "poll-owner-ask",
        json!({"action": "get", "key": key}),
    );
    let value = poll.to_json().unwrap();
    assert_eq!(
        value["result"]["question"]["text"],
        "Which acceptance criterion?"
    );
    assert_eq!(
        corral_core::agent::pm_state::child_count(&fx.ctx.store, "receipts").unwrap(),
        before
    );
    super::answer(
        &fx.ctx,
        &key,
        1,
        Actor::Owner,
        AnswerInput {
            text: "A".into(),
            option: Some("A".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let stale = send(
        &fx,
        &pm,
        "owner_ask",
        "late-dialog-answer",
        json!({
            "action": "answer", "key": key, "revision": 1, "text": "B", "option": "B"
        }),
    );
    assert_eq!(code(&stale), Some("stale_revision"));
    assert_eq!(question(&fx, &key).answer.unwrap().text, "A");
    assert!(
        super::list(&fx.ctx, None, true)
            .unwrap()
            .iter()
            .any(|row| row.question.key == key)
    );
}
