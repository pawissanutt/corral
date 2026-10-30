#![cfg(unix)]
use super::handling_tests::fixture::{Fx, at};
use super::*;
use corral_core::ResumeCheck;
use corral_core::agent::QuestionState;
use corral_core::work::{self, Counter, HotScope};
use serde_json::json;

fn check(kind: &str, target: &str, expected: Option<&str>) -> ResumeCheck {
    ResumeCheck {
        kind: kind.into(),
        target: Some(target.into()),
        expected: expected.map(str::to_owned),
        description: "fixture".into(),
        machine_checkable: true,
        satisfied: None,
    }
}

#[test]
fn typed_neighborhood_matches_cold_park_facts_without_scanning_settled_history() {
    let mut measured = Vec::new();
    for scale in [1, 10] {
        let fx = Fx::new();
        let mut source = fx.questions(1);
        source.status = Status::Done;
        let run = source.runs.last_mut().unwrap();
        run.outcome = Some(corral_core::RunOutcome::Done);
        run.ended = Some(at(1));
        run.autopilot =
            Some(serde_json::from_value(json!({"stage":"done","at":at(0),"sha":"abc"})).unwrap());
        run.inbox.settled = Some(at(1));
        let host = run.agent_host.as_mut().unwrap();
        host.binding.closing = true;
        host.questions[0].state = QuestionState::Answered;
        host.questions[0].answer =
            Some(serde_json::from_value(json!({"by":"owner","text":"Yes","at":at(0)})).unwrap());
        let question = host.questions[0].key.clone();
        let template = host.questions[0].clone();
        for n in 0..2 * scale {
            let mut old = template.clone();
            old.key = format!("{}/R1/Q{:06}", source.id, n + 100);
            host.questions.push(old);
        }
        let decision:corral_core::DecisionRecord=serde_json::from_value(json!({
            "request":{"version":1,"id":"chosen","run":"R1","question":"Proceed?","requested_action":"Answer","owner":"owner","source":"worker"},
            "registered_at":at(0),"observation":{"at":at(0),"state":"ready","detail":"answered"},
            "receipt":{"version":1,"id":"answer","run":"R1","outcome":"answered","delivery":"acknowledged","observed_at":at(0)}
        })).unwrap();
        run.obligations.decisions.push(decision.clone());
        for n in 0..2 * scale {
            let mut old = decision.clone();
            old.request.id = format!("old-{n:06}");
            run.obligations.decisions.push(old);
        }
        fx.ctx.store.fixture_put_task(&source).unwrap();
        let mut parked = fx
            .ctx
            .store
            .create("Parked", None, Status::Ready, "")
            .unwrap();
        let park:corral_core::Park=serde_json::from_value(json!({
            "version":1,"revision":1,"by":"owner","reason":"Wait","at":at(0),"resume_when":[],"resume_action":"Resume","review_by":at(700)
        })).unwrap();
        parked.park = Some(corral_core::Park {
            resume_when: vec![
                check("status", &source.id, Some("done")),
                check("landed", &source.id, None),
                check("answered", &question, Some("1")),
                check("answered", &question, Some("2")),
                check("decision", "chosen", None),
                check("hold-lifted", &source.id, None),
                check("status", "T-999", Some("done")),
            ],
            ..park
        });
        fx.ctx.store.fixture_put_task(&parked).unwrap();
        let tasks = vec![source, parked];
        let policy = fx.ctx.store.effective_operating_policy().unwrap();
        let cold = Neighborhood::new(&tasks, &fx.ctx.store.config, &policy);
        let park = tasks[1].park.as_ref().unwrap();
        let expected = crate::park::checks(park, &cold);
        assert_eq!(
            expected
                .iter()
                .map(|check| check.satisfied)
                .collect::<Vec<_>>(),
            vec![
                Some(true),
                Some(true),
                Some(true),
                None,
                Some(true),
                Some(true),
                None
            ]
        );
        let view = fx.ctx.store.read_only_snapshot().unwrap();
        let heads = tasks
            .iter()
            .map(|task| view.head(&task.id).unwrap())
            .collect::<Vec<_>>();
        let _hot = HotScope::enter();
        let before = work::snapshot();
        let hot = Neighborhood::new_heads(&heads, &fx.ctx.store.config, &policy).unwrap();
        assert_eq!(crate::park::checks(park, &hot), expected);
        assert_eq!(hot.slots, cold.slots);
        assert_eq!(hot.review_slots, cold.review_slots);
        assert_eq!(hot.project_slots, cold.project_slots);
        let after = work::snapshot();
        measured.push(Counter::ALL.map(|counter| after.total(counter) - before.total(counter)));
        assert_eq!(
            after.total(Counter::FullTaskHydrations) - before.total(Counter::FullTaskHydrations),
            0
        );
    }
    assert_eq!(measured[0], measured[1]);
}
