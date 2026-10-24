use corral_core::Status;
use corral_core::agent::{AnswerBy, QuestionKind, QuestionState};
use jiff::{SignedDuration, Timestamp};
use serde_json::json;

use super::tests_support::*;
use super::{Actor, AnswerInput};
use crate::agent::CallResponse;
use crate::pm_next::{PmActionKind, PmObservation, observe, pm_card};

#[test]
fn ask_opens_keyed_question_at_revision_one_and_logs_it_once() {
    let fx = fx();
    let (id, worker) = worker(&fx);
    let first = ask(
        &fx,
        &worker,
        "c1",
        json!({"kind": "clarification", "text": "Which flag?", "options": ["a", "b"]}),
    );
    let receipt = first.receipt().unwrap();
    assert_eq!(receipt.result["key"], json!(format!("{id}/R1/Q1")));
    assert_eq!(receipt.result["revision"], json!(1));
    let blocking = ask_ok(&fx, &worker, "c2", true);
    assert_eq!(blocking, format!("{id}/R1/Q2"));
    let q = question(&fx, &blocking);
    assert_eq!(
        (q.revision, q.state, q.blocking),
        (1, QuestionState::Open, true)
    );
    assert_eq!(events(&fx, "question_opened"), 2);

    let replay = ask(
        &fx,
        &worker,
        "c1",
        json!({"kind": "clarification", "text": "Which flag?", "options": ["a", "b"]}),
    );
    assert!(matches!(
        replay,
        CallResponse::Receipt { replayed: true, .. }
    ));
    assert_eq!(super::list(&fx.ctx, Some(&id), true).unwrap().len(), 2);
    assert_eq!(events(&fx, "question_opened"), 2);

    let (review_task, reviewer) = reviewer(&fx);
    let key = ask_ok(&fx, &reviewer, "r1", false);
    assert_eq!(key, format!("{review_task}/R1/R1-review1/Q1"));
}

#[test]
fn concurrent_first_questions_get_distinct_keys_and_answers_reach_only_their_recipient() {
    let fx = fx();
    let (a, worker_a) = worker(&fx);
    let (b, worker_b) = worker(&fx);
    let (c, reviewer_c) = reviewer(&fx);
    let keys: Vec<String> = std::thread::scope(|scope| {
        let handles = [&worker_a, &worker_b, &reviewer_c].map(|host| {
            let fx = &fx;
            scope.spawn(move || ask_ok(fx, host, "first", true))
        });
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    assert_eq!(
        keys,
        [
            format!("{a}/R1/Q1"),
            format!("{b}/R1/Q1"),
            format!("{c}/R1/R1-review1/Q1"),
        ]
    );
    let answered_a = pm_answer(&fx, &keys[0], 1).unwrap();
    let answered_c = pm_answer(&fx, &keys[2], 1).unwrap();
    let inbox_a = messages(&fx, &a, "R1");
    assert_eq!(inbox_a.len(), 1);
    assert_eq!(inbox_a[0].kind, "question_answer");
    assert_eq!(
        inbox_a[0].subject,
        Some(json!({"key": keys[0], "revision": 2}))
    );
    assert_eq!(
        answered_a.outcome().unwrap().message_id.as_deref(),
        Some(inbox_a[0].id.as_str())
    );
    assert!(messages(&fx, &b, "R1").is_empty(), "B received A's answer");
    let inbox_c = review_messages(&fx, &c, "R1-review1");
    assert_eq!(inbox_c.len(), 1, "the reviewer receives its own answer");
    assert_eq!(inbox_c[0].kind, "question_answer");
    assert_eq!(
        inbox_c[0].subject,
        Some(json!({"key": keys[2], "revision": 2}))
    );
    assert_eq!(
        answered_c.outcome().unwrap().message_id.as_deref(),
        Some(inbox_c[0].id.as_str())
    );
    assert!(
        messages(&fx, &c, "R1").is_empty(),
        "the reviewer's parent run got its answer"
    );
    assert!(pending(&fx, &keys[1]));
}

#[test]
fn pi_pm_answer_commits_state_receipt_and_message_in_the_target_task() {
    let fx = fx();
    let (id, worker) = worker(&fx);
    let pm = pm_host(&fx);
    let key = ask_ok(&fx, &worker, "c1", true);
    let payload = json!({
        "key": key, "rev": 1, "text": "Use the default", "basis_refs": [spec_ref(&fx, &id)],
        "scope_note": "inside the approved write scope",
    });
    let response = send(&fx, &pm, "question_answer", "a1", payload.clone());
    let receipt = response
        .receipt()
        .unwrap_or_else(|| panic!("{response:?}"))
        .clone();
    assert!(matches!(
        response,
        CallResponse::Receipt {
            replayed: false,
            ..
        }
    ));
    let task = fx.ctx.store.get(&id).unwrap();
    assert_eq!(task.agent_pm_receipts, vec![receipt.clone()]);
    let q = question(&fx, &key);
    let inbox = messages(&fx, &id, "R1");
    assert_eq!((q.state, q.revision), (QuestionState::Answered, 2));
    assert_eq!(q.answer.as_ref().unwrap().by, AnswerBy::Pm);
    assert_eq!(q.answer.unwrap().message_id, Some(inbox[0].id.clone()));
    assert_eq!(receipt.result["message_id"], json!(inbox[0].id));

    let replay = send(&fx, &pm, "question_answer", "a1", payload);
    assert!(matches!(
        replay,
        CallResponse::Receipt { replayed: true, .. }
    ));
    assert_eq!(messages(&fx, &id, "R1").len(), 1);
    let mut changed =
        json!({"key": key, "rev": 2, "text": "other", "basis_refs": [spec_ref(&fx, &id)]});
    changed["scope_note"] = json!("x");
    assert_eq!(
        code(&send(&fx, &pm, "question_answer", "a1", changed)),
        Some("payload_mismatch")
    );

    let second = ask_ok(&fx, &worker, "c2", false);
    let escalate = json!({"key": second, "rev": 2});
    assert_eq!(
        code(&send(&fx, &pm, "question_escalate", "e1", escalate)),
        Some("stale_revision")
    );
    assert_eq!(question(&fx, &second).revision, 1);
    let escalate = json!({"key": second, "rev": 1, "note": "owner call"});
    assert!(
        send(&fx, &pm, "question_escalate", "e2", escalate)
            .receipt()
            .is_some()
    );
    assert_eq!(question(&fx, &second).state, QuestionState::Escalated);

    let (other, other_worker) = worker_in(&fx, "elsewhere");
    let foreign = ask_ok(&fx, &other_worker, "c1", false);
    let cancel = json!({"key": foreign, "rev": 1, "reason": "not ours"});
    assert_eq!(
        code(&send(&fx, &pm, "question_cancel", "x1", cancel)),
        Some("out_of_scope")
    );
    assert!(
        fx.ctx
            .store
            .get(&other)
            .unwrap()
            .agent_pm_receipts
            .is_empty()
    );
}

#[test]
fn pm_answer_authority_follows_od7_and_reclassification_goes_upward_only() {
    let fx = fx();
    let (id, worker) = worker(&fx);
    let key = ask_ok(&fx, &worker, "c1", false);
    let answer = |input: AnswerInput| super::answer(&fx.ctx, &key, 1, Actor::Pm(None), input);
    let mut input = pm_input(&fx, &id, "yes");
    input.basis_refs.clear();
    assert_eq!(refusal(&answer(input).unwrap_err()), "question_rule");
    let mut input = pm_input(&fx, &id, "yes");
    input.basis_refs = vec![format!("comment:{id}#1"), format!("plan:{id}@P9")];
    assert_eq!(refusal(&answer(input).unwrap_err()), "question_rule");
    let mut input = pm_input(&fx, &id, "yes");
    input.scope_note = None;
    assert_eq!(refusal(&answer(input).unwrap_err()), "question_rule");
    assert_eq!(question(&fx, &key).revision, 1);

    for kind in ["owner_only", "permission"] {
        let key = ask_kind(&fx, &worker, kind, kind, false);
        let error = super::answer(&fx.ctx, &key, 1, Actor::Pm(None), pm_input(&fx, &id, "y"));
        assert_eq!(refusal(&error.unwrap_err()), "question_rule", "{kind}");
        let down = super::escalate(
            &fx.ctx,
            &key,
            1,
            Actor::Pm(None),
            None,
            Some(QuestionKind::Clarification),
        );
        assert_eq!(refusal(&down.unwrap_err()), "question_rule", "{kind}");
    }
    let choice = ask_kind(&fx, &worker, "choice", "technical_choice", false);
    super::escalate(
        &fx.ctx,
        &choice,
        1,
        Actor::Pm(None),
        None,
        Some(QuestionKind::OwnerOnly),
    )
    .unwrap();
    let q = question(&fx, &choice);
    assert_eq!(
        (q.kind, q.state, q.revision),
        (QuestionKind::OwnerOnly, QuestionState::Escalated, 2)
    );
    let pm = super::answer(
        &fx.ctx,
        &choice,
        2,
        Actor::Pm(None),
        pm_input(&fx, &id, "y"),
    );
    assert_eq!(refusal(&pm.unwrap_err()), "question_rule");
    let owner = AnswerInput {
        text: "do it".into(),
        ..AnswerInput::default()
    };
    super::answer(&fx.ctx, &choice, 2, Actor::Owner, owner).unwrap();
    assert_eq!(question(&fx, &choice).answer.unwrap().by, AnswerBy::Owner);
}

#[test]
fn escalated_question_is_held_for_the_owner_and_shown_as_an_owner_card() {
    let fx = fx();
    let (id, worker) = worker(&fx);
    let key = ask_ok(&fx, &worker, "c1", true);
    super::escalate(
        &fx.ctx,
        &key,
        1,
        Actor::Pm(None),
        Some("owner call".into()),
        None,
    )
    .unwrap();
    assert_eq!(fx.ctx.store.get(&id).unwrap().status, Status::NeedsMe);
    let later = Timestamp::now() + SignedDuration::from_hours(24 * 365);
    assert!(
        super::expire_deadlines(&fx.ctx, &id, later)
            .unwrap()
            .is_empty()
    );
    assert!(super::unattended(&fx.ctx, &id, later).unwrap().is_empty());
    assert_eq!(question(&fx, &key).state, QuestionState::Escalated);

    let observation = observe::observations(&fx.ctx)
        .unwrap()
        .into_iter()
        .find(|o| matches!(o, PmObservation::OwnerQuestion { key: k, .. } if *k == key))
        .expect("owner question card");
    let card = pm_card(&observation, "corral");
    assert_eq!(card.kind, PmActionKind::OwnerQuestion);
    assert!(card.requires_owner);
    assert_eq!(
        card.evidence[0],
        format!("question {key} revision 2 (clarification)")
    );
    assert_eq!(card.evidence[1], "question c1?");
    assert!(card.command.contains("question answer") && card.command.contains(&key));
    assert!(card.command.contains("--rev 2 --owner"));

    let owner = AnswerInput {
        text: "ship it".into(),
        ..AnswerInput::default()
    };
    super::answer(&fx.ctx, &key, 2, Actor::Owner, owner).unwrap();
    let task = fx.ctx.store.get(&id).unwrap();
    assert_eq!(task.status, Status::Running);
    assert_eq!(task.runs[0].blocked_excerpt, None);
}

#[test]
fn owner_deadline_expires_only_at_the_unchanged_revision() {
    let fx = fx();
    let (id, worker) = worker(&fx);
    let key = ask_ok(&fx, &worker, "c1", false);
    super::escalate(&fx.ctx, &key, 1, Actor::Pm(None), None, None).unwrap();
    let at = Timestamp::now() + SignedDuration::from_hours(1);
    let stale = super::deadline(&fx.ctx, &key, 1, at, "go");
    assert_eq!(refusal(&stale.unwrap_err()), "stale_revision");
    super::deadline(&fx.ctx, &key, 2, at, "use option a").unwrap();
    assert_eq!(question(&fx, &key).revision, 2);
    let before = at - SignedDuration::from_secs(1);
    assert!(
        super::expire_deadlines(&fx.ctx, &id, before)
            .unwrap()
            .is_empty()
    );

    let voided = ask_ok(&fx, &worker, "c2", false);
    super::deadline(&fx.ctx, &voided, 1, at, "void me").unwrap();
    super::escalate(&fx.ctx, &voided, 1, Actor::Pm(None), None, None).unwrap();

    let after = at + SignedDuration::from_secs(1);
    assert_eq!(
        super::expire_deadlines(&fx.ctx, &id, after).unwrap(),
        vec![key.clone()]
    );
    let q = question(&fx, &key);
    let answer = q.answer.unwrap();
    assert_eq!((q.state, q.revision), (QuestionState::Expired, 3));
    assert_eq!(
        (answer.by, answer.text.as_str()),
        (AnswerBy::OwnerDeadline, "use option a")
    );
    let inbox = messages(&fx, &id, "R1");
    assert_eq!(inbox.len(), 1);
    assert_eq!(inbox[0].kind, "question_answer");
    assert_eq!(answer.message_id, Some(inbox[0].id.clone()));
    assert_eq!(events(&fx, "question_expired"), 1);
    assert_eq!(question(&fx, &voided).state, QuestionState::Escalated);
    assert!(
        super::expire_deadlines(&fx.ctx, &id, after)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn owner_supersedes_an_answer_with_a_correction_and_the_agent_withdraws_with_a_reason() {
    let fx = fx();
    let (id, worker) = worker(&fx);
    let key = ask_ok(&fx, &worker, "c1", false);
    pm_answer(&fx, &key, 1).unwrap();
    let again = pm_answer(&fx, &key, 2);
    assert_eq!(refusal(&again.unwrap_err()), "question_state");
    let correction = AnswerInput {
        text: "use option b instead".into(),
        ..AnswerInput::default()
    };
    let applied = super::answer(&fx.ctx, &key, 2, Actor::Owner, correction).unwrap();
    let q = question(&fx, &key);
    let supersede = q.triage.last().unwrap();
    assert_eq!((supersede.action.as_str(), q.revision), ("supersede", 3));
    assert!(supersede.note.as_deref().unwrap().contains("revision 2"));
    let inbox = messages(&fx, &id, "R1");
    assert_eq!(
        inbox.iter().map(|m| m.kind.as_str()).collect::<Vec<_>>(),
        ["question_answer", "question_correction"]
    );
    assert_eq!(
        applied.outcome().unwrap().message_id,
        Some(inbox[1].id.clone())
    );
    let answered = fx.ctx.store.events().unwrap();
    let last = answered
        .iter()
        .rev()
        .find(|e| e.kind == "question_answered")
        .unwrap();
    assert_eq!(last.detail["supersedes_revision"], json!(2));

    let second = ask_ok(&fx, &worker, "c2", true);
    let withdraw = json!({"key": second, "reason": "found it in the spec"});
    let response = send(&fx, &worker, "withdraw", "w1", withdraw);
    assert_eq!(
        response.receipt().unwrap().result["state"],
        json!("withdrawn")
    );
    let q = question(&fx, &second);
    assert_eq!((q.state, q.revision), (QuestionState::Withdrawn, 2));
    assert_eq!(
        q.triage.last().unwrap().note.as_deref(),
        Some("found it in the spec")
    );
    assert_eq!(events(&fx, "question_withdrawn"), 1);
    assert_eq!(
        refusal(&pm_answer(&fx, &second, 2).unwrap_err()),
        "question_withdrawn"
    );
    let foreign = json!({"key": format!("{id}/R9/Q1"), "reason": "x"});
    assert_eq!(
        code(&send(&fx, &worker, "withdraw", "w2", foreign)),
        Some("question_not_found")
    );
}

#[test]
fn bounds_refuse_overflow_without_truncation() {
    let fx = fx();
    let (id, worker) = worker(&fx);
    ask_ok(&fx, &worker, "blocking", true);
    let second = json!({"kind": "clarification", "blocking": true, "text": "again?"});
    assert_eq!(
        code(&ask(&fx, &worker, "b2", second)),
        Some("question_limit")
    );
    let long = json!({"kind": "clarification", "text": "x".repeat(4 * 1024 + 1)});
    assert_eq!(code(&ask(&fx, &worker, "long", long)), Some("bad_request"));
    let options = json!({"kind": "clarification", "text": "pick", "options": ["1", "2", "3", "4", "5", "6", "7"]});
    assert_eq!(
        code(&ask(&fx, &worker, "opts", options)),
        Some("bad_request")
    );
    for n in 2..=8 {
        ask_ok(&fx, &worker, &format!("n{n}"), false);
    }
    let ninth = json!({"kind": "clarification", "text": "ninth?"});
    assert_eq!(
        code(&ask(&fx, &worker, "n9", ninth)),
        Some("question_limit")
    );
    let all = super::list(&fx.ctx, Some(&id), true).unwrap();
    assert_eq!(all.len(), 8);
    assert!(all.iter().all(|v| v.question.text.len() < 64));
}

#[test]
fn config_accepts_the_question_push_kinds_without_defaulting_them_and_agent_defaults() {
    let fx = fx();
    let root = fx.ctx.store.root().to_path_buf();
    let mut config = corral_core::Config::default_for(&root);
    let added = [
        "question_opened",
        "question_escalated",
        "turn_ended_without_report",
    ];
    assert!(
        added
            .iter()
            .all(|k| !config.pm.push_kinds.iter().any(|d| d == k))
    );
    let agent = &config.agent;
    assert_eq!(
        (
            agent.question_unattended_secs,
            agent.gate_start_uncertain_secs,
            agent.drain_secs
        ),
        (600, 300, 60)
    );
    assert!(!config.to_toml().contains("[agent]"));
    config.pm.push_kinds.extend(added.map(String::from));
    let problems = config
        .validate(&root)
        .err()
        .map(|e| e.0)
        .unwrap_or_default();
    assert!(
        problems.iter().all(|p| !p.contains("push_kinds")),
        "{problems:?}"
    );
    config.agent.question_unattended_secs = 0;
    let problems = config.validate(&root).unwrap_err().0;
    assert!(
        problems
            .iter()
            .any(|p| p == "agent.question_unattended_secs must be > 0")
    );
}
