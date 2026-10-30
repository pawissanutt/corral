//! A stopping selector reads named questions in its current run, even retired children.
mod store_support;
use corral_core::agent::AgentRunState;
use corral_core::attention::{
    self, Assessment, Condition, NextAction, Progress, Responsible, ResumeCheck, SinceSource,
    WaitClass,
};
use corral_core::work::{self, Counter, HotScope};
use corral_core::{Status, TaskRef};
use serde_json::json;

#[test]
fn typed_blocked_selection_matches_full_scope_and_constant_work() {
    for case in 0..10 {
        let mut measured = Vec::new();
        for scale in [1, 10] {
            let (_dir, store) = store_support::store();
            let mut task = store_support::rich(
                &store,
                2 * scale + 1,
                2 * scale + 1,
                2 * scale + 1,
                2 * scale,
            );
            task.status = Status::NeedsMe;
            let current = task.runs.last().unwrap().id.clone();
            let owner = match case {
                2 => task.runs.last().unwrap().review_runs[0].id.clone(),
                3 => task.runs[0].id.clone(),
                4 => task.plan_runs.last().unwrap().id.clone(),
                _ => current.clone(),
            };
            let key = format!("{}/{owner}/Q1", task.id);
            let question=serde_json::from_value(json!({
                "version":1,"key":key,"revision":1,"kind":"clarification","blocking":case==1,"text":"Question","state":"open",
                "asked":{"request_id":"rq-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","payload_sha256":"a".repeat(64),"session_id":"s","generation":1,"at":store_support::at(0)},
                "wake":{"route":"none","key":key,"state":"pending"}
            })).unwrap();
            let mut host=AgentRunState::new(serde_json::from_value(json!({"role":if case==4 {"planner"}else{"worker"},"store":"fixture","project":"app","generation":1})).unwrap());
            host.questions.push(question);
            for n in 0..2 * scale {
                let mut old = host.questions[0].clone();
                old.key = format!("{}/{owner}/Q{:06}", task.id, n + 100);
                old.state = corral_core::agent::QuestionState::Cancelled;
                host.questions.push(old);
            }
            match case {
                0 | 1 => task.runs.last_mut().unwrap().agent_host = Some(Box::new(host)),
                2 => task.runs.last_mut().unwrap().review_runs[0].agent_host = Some(Box::new(host)),
                3 => task.runs[0].agent_host = Some(Box::new(host)),
                4 => task.plan_runs.last_mut().unwrap().agent_host = Some(Box::new(host)),
                _ => {}
            }
            let condition = Condition {
                key: None,
                kind: if case == 7 {
                    "decision_pending"
                } else {
                    "question_open"
                }
                .into(),
                class: if case == 6 {
                    WaitClass::Capacity
                } else {
                    WaitClass::PmAction
                },
                reason_code: "test".into(),
                reason: "test".into(),
                responsible: Responsible::Pm,
                since: None,
                since_source: SinceSource::Discovered,
                age_secs: None,
                resume: vec![ResumeCheck {
                    kind: "question_answered".into(),
                    target: Some(key.clone()),
                    expected: Some("1".into()),
                    description: "answer".into(),
                    machine_checkable: true,
                    satisfied: None,
                }],
                next_action: NextAction {
                    command: None,
                    description: "inspect".into(),
                    requires_owner: false,
                    recipient: None,
                },
            };
            if case == 8 {
                task.status = Status::Ready;
                task.runs.clear();
                task.plan_runs.clear();
                task.plans.clear();
                task.tracker = false;
            }
            if case == 9 {
                task.status = Status::Idea;
            }
            let assessment = Assessment {
                progress: (case == 7).then(|| Progress {
                    executor: "worker".into(),
                    since: None,
                    reason: "working".into(),
                    liveness: "working".into(),
                }),
                conditions: vec![condition.clone()],
                primary: Some(condition),
            };
            let expected = attention::blocked(&task, &assessment).cloned();
            assert_eq!(
                expected.is_some(),
                matches!(case, 1 | 3 | 4 | 5 | 7),
                "case={case}"
            );
            assert_eq!(
                attention::blocked_ref(&task, &assessment).unwrap().cloned(),
                expected
            );
            let question_expected = task.run_question_by_key(&current, &key).unwrap();
            assert_eq!(question_expected.is_some(), case <= 2);
            store.fixture_put_task(&task).unwrap();
            let head = store.head(&task.id).unwrap();
            let _hot = HotScope::enter();
            let before = work::snapshot();
            assert_eq!(
                attention::blocked_ref(&head, &assessment).unwrap().cloned(),
                expected,
                "case={case}"
            );
            let after = work::snapshot();
            measured.push(Counter::ALL.map(|counter| after.total(counter) - before.total(counter)));
            assert_eq!(
                after.total(Counter::FullTaskHydrations)
                    - before.total(Counter::FullTaskHydrations),
                0
            );
        }
        assert_eq!(measured[0], measured[1], "case={case}");
    }
}
