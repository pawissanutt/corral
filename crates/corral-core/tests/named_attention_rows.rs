//! Exact park/attention selectors must not enumerate container or obligation history.
mod store_support;
use corral_core::TaskRef;
use corral_core::agent::{AgentQuestion, AgentRunState};
use corral_core::work::{self, Counter, HotScope};
use serde_json::json;

#[test]
fn named_question_and_first_decision_are_keyed_despite_wait_id_collisions() {
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
        for run in &mut task.runs {
            run.obligations.waits[0].request.id = "chosen".into();
            run.obligations.decisions[0].request.id = "chosen".into();
        }
        let key = format!("{}/R0001/Q1", task.id);
        let question:AgentQuestion=serde_json::from_value(json!({
            "version":1,"key":key,"revision":1,"kind":"clarification","blocking":false,"text":"Historical question",
            "state":"cancelled","terminal":{"state":"cancelled","reason":"settled","at":store_support::at(0)},
            "asked":{"request_id":"rq-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","payload_sha256":"a".repeat(64),"session_id":"s","generation":1,"at":store_support::at(0)},
            "wake":{"route":"none","key":key,"state":"posted"}
        })).unwrap();
        let mut host=AgentRunState::new(serde_json::from_value(json!({
            "role":"worker","store":"fixture","project":"app","generation":1,"health":"fenced","closing":true
        })).unwrap());
        host.questions.push(question.clone());
        for n in 0..2 * scale {
            let mut old = question.clone();
            old.key = format!("{}/R0001/Q{:06}", task.id, n + 100);
            host.questions.push(old);
        }
        task.runs[0].agent_host = Some(Box::new(host));
        let expected = task.decision_by_id("chosen").unwrap().unwrap();
        assert_eq!(expected.request.run, "R0001");
        assert_eq!(task.question_by_key(&key).unwrap(), Some(question.clone()));
        store.fixture_put_task(&task).unwrap();
        let head = store.head(&task.id).unwrap();
        {
            let _hot = HotScope::enter();
            let before = work::snapshot();
            assert_eq!(
                head.decision_by_id("chosen").unwrap(),
                Some(expected.clone())
            );
            assert_eq!(head.question_by_key(&key).unwrap(), Some(question));
            assert!(head.decision_by_id("missing").unwrap().is_none());
            assert!(head.question_by_key("missing").unwrap().is_none());
            let after = work::snapshot();
            measured.push(Counter::ALL.map(|counter| after.total(counter) - before.total(counter)));
            assert_eq!(
                after.total(Counter::FullTaskHydrations)
                    - before.total(Counter::FullTaskHydrations),
                0
            );
        }
        store
            .update_head(&task.id, |head| {
                assert_eq!(
                    TaskRef::decision_by_id(head, "chosen")?,
                    Some(expected.clone())
                );
                Ok(())
            })
            .unwrap();
    }
    assert_eq!(measured[0], measured[1]);
}
