//! Inbox reads and established answer operations against real, isolated canonical records.
use super::tests_support::*;
use super::*;
use corral_core::{DecisionRequest, ObligationSource, RunOutcome, Status};
use serde_json::json;

#[test]
fn canonical_inbox_unkeyed_conditions_have_distinct_inspection_identities() {
    let fx = fx();
    let task = fx
        .ctx
        .store
        .create("Blocked dependencies", None, Status::Ready, "")
        .unwrap();
    fx.ctx
        .store
        .update(&task.id, |task| {
            task.depends_on = vec!["T-998".into(), "T-999".into()];
            Ok(())
        })
        .unwrap();
    let inbox = crate::attention::inbox::list(&fx.ctx, None, jiff::Timestamp::now()).unwrap();
    let conditions: Vec<_> = inbox
        .items
        .iter()
        .filter(|item| item.state == "prerequisite")
        .collect();
    assert_eq!(conditions.len(), 2);
    assert_ne!(conditions[0].key, conditions[1].key);
    assert!(
        conditions
            .iter()
            .all(|item| item.key.contains("unkeyed:") && item.response.is_none())
    );
}

#[test]
fn canonical_inbox_keeps_question_history_exact_and_never_mutates_any_task() {
    let fx = fx();
    let (id, host) = worker(&fx);
    let (unrelated, _) = worker_in(&fx, "other");
    let open = ask_ok(&fx, &host, "open", false);
    let answered = ask_ok(&fx, &host, "answered", false);
    pm_answer(&fx, &answered, 1).unwrap();
    let cancelled = ask_ok(&fx, &host, "cancelled", false);
    cancel(&fx.ctx, &cancelled, 1, Actor::Owner, "no longer needed").unwrap();
    let (closed_id, closed_host) = worker(&fx);
    let closed = ask_ok(&fx, &closed_host, "closed", false);
    fx.ctx
        .store
        .update(&closed_id, |task| {
            task.runs[0].outcome = Some(RunOutcome::Superseded);
            Ok(())
        })
        .unwrap();
    let before = fx.ctx.store.list().unwrap();
    let events_before = fx.ctx.store.events().unwrap();
    let inbox = crate::attention::inbox::list(&fx.ctx, None, jiff::Timestamp::now()).unwrap();
    let item = |key: &str| inbox.items.iter().find(|item| item.key == key).unwrap();
    assert_eq!(item(&open).task, id);
    assert_eq!(
        item(&open).container.as_deref(),
        Some(format!("{id}/R1").as_str())
    );
    assert_eq!(item(&open).revision, "1");
    assert_eq!(item(&open).scope, "corral");
    assert!(
        item(&open)
            .response
            .as_ref()
            .unwrap()
            .contains("--rev 1 --owner")
    );
    assert_eq!(item(&answered).state, "answered");
    assert_eq!(item(&cancelled).state, "cancelled");
    assert_eq!(item(&closed).state, "cancelled");
    assert!(!item(&closed).pending);
    assert!(item(&closed).response.is_none());
    assert!(
        item(&closed)
            .detail
            .iter()
            .any(|detail| detail.contains("closed"))
    );
    assert_eq!(fx.ctx.store.list().unwrap(), before);
    assert_eq!(fx.ctx.store.events().unwrap(), events_before);
    let scoped =
        crate::attention::inbox::list(&fx.ctx, Some("corral"), jiff::Timestamp::now()).unwrap();
    assert!(scoped.items.iter().all(|item| item.task != unrelated));
    assert_eq!(fx.ctx.store.list().unwrap(), before);
}

#[test]
fn canonical_inbox_response_routes_retain_stale_closed_and_role_refusals() {
    let fx = fx();
    let (id, host) = worker(&fx);
    let (unrelated, _) = worker_in(&fx, "other");
    let key = ask_ok(&fx, &host, "question", false);
    let captured = crate::attention::inbox::list(&fx.ctx, None, jiff::Timestamp::now()).unwrap();
    let revision: u32 = captured
        .items
        .iter()
        .find(|item| item.key == key)
        .unwrap()
        .revision
        .parse()
        .unwrap();
    let before = fx.ctx.store.list().unwrap();
    let denied = send(
        &fx,
        &host,
        "question_answer",
        "forged-worker-answer",
        json!({"key":key,"revision":revision,"text":"yes"}),
    );
    assert!(denied.rejection().is_some());
    assert_eq!(
        fx.ctx.store.get(&id).unwrap(),
        before.iter().find(|task| task.id == id).unwrap().clone()
    );
    // Existing PM answers still require canonical basis and scope, regardless of panel inspection.
    let error = answer(
        &fx.ctx,
        &key,
        revision,
        Actor::Pm(None),
        AnswerInput {
            text: "yes".into(),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert_eq!(refusal(&error), "question_rule");
    pm_answer(&fx, &key, revision).unwrap();
    let after_answer = fx.ctx.store.get(&id).unwrap();
    assert_eq!(
        refusal(&pm_answer(&fx, &key, revision).unwrap_err()),
        "stale_revision"
    );
    assert_eq!(fx.ctx.store.get(&id).unwrap(), after_answer);
    fx.ctx
        .store
        .update(&id, |task| {
            task.runs[0].outcome = Some(RunOutcome::Superseded);
            Ok(())
        })
        .unwrap();
    let closed = fx.ctx.store.get(&id).unwrap();
    assert_eq!(
        refusal(
            &answer(
                &fx.ctx,
                &key,
                revision + 1,
                Actor::Owner,
                AnswerInput {
                    text: "correction".into(),
                    ..Default::default()
                }
            )
            .unwrap_err()
        ),
        "question_closed"
    );
    assert_eq!(fx.ctx.store.get(&id).unwrap(), closed);
    assert_eq!(
        fx.ctx.store.get(&unrelated).unwrap(),
        before
            .into_iter()
            .find(|task| task.id == unrelated)
            .unwrap()
    );
}

#[test]
fn canonical_inbox_portable_decisions_keep_digest_receipt_and_old_run_scope() {
    let fx = fx();
    let (id, _) = worker(&fx);
    let now = jiff::Timestamp::now();
    crate::register_decision(
        &fx.ctx,
        &id,
        DecisionRequest {
            version: 1,
            id: "choice".into(),
            run: "R1".into(),
            question: "Choose a route?".into(),
            requested_action: "Owner answer".into(),
            owner: "owner".into(),
            source: ObligationSource::Worker,
            deadline: None,
        },
        now,
    )
    .unwrap();
    let read = || {
        crate::attention::inbox::list(&fx.ctx, None, jiff::Timestamp::now())
            .unwrap()
            .items
            .into_iter()
            .find(|item| item.kind == "portable_decision")
            .unwrap()
    };
    let pending = read();
    assert_eq!(pending.key, format!("{id}/R1/choice"));
    assert_eq!(pending.revision_kind, "source_digest");
    assert!(pending.response.unwrap().contains("--decision 'choice'"));
    crate::obligations::answer_reply(&fx.ctx, &id, "R1", "choice", "Use A").unwrap();
    let answered = read();
    assert_eq!(answered.state, "answered");
    assert!(!answered.pending);
    assert_ne!(answered.revision, pending.revision);
    assert!(
        answered
            .detail
            .iter()
            .any(|detail| detail.contains("Use A"))
    );
    fx.ctx
        .store
        .update(&id, |task| {
            task.status = Status::Running;
            task.runs.push(run("R2", "p2"));
            Ok(())
        })
        .unwrap();
    assert_eq!(
        read().container.as_deref(),
        Some(format!("{id}/R1").as_str())
    );
}
