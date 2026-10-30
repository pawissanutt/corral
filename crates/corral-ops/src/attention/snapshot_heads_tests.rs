#![cfg(unix)]
use super::super::handling_tests::fixture::{Fx, at};
use super::*;
use corral_core::agent::QuestionState;
use corral_core::work::{self, Counter, HotScope};
use corral_core::{ResumeCheck, RunOutcome, WaitRecord};
use serde_json::json;

#[test]
fn hot_snapshot_preserves_named_terminal_park_and_wait_sources_without_board_history() {
    let mut measured = Vec::new();
    for scale in [1, 10] {
        let fx = Fx::new();
        let mut worker = fx.questions(1);
        worker.status = Status::NeedsMe;
        worker.status_since = Some(at(0));
        let mut source = fx.questions(1);
        source.status = Status::Done;
        let run = source.runs.last_mut().unwrap();
        run.outcome = Some(RunOutcome::Done);
        run.ended = Some(at(1));
        run.agent_host.as_mut().unwrap().binding.closing = true;
        run.inbox.settled = Some(at(1));
        let question = &mut run.agent_host.as_mut().unwrap().questions[0];
        question.state = QuestionState::Answered;
        question.answer =
            Some(serde_json::from_value(json!({"by":"owner","text":"Yes","at":at(0)})).unwrap());
        let key = question.key.clone();
        run.obligations.decisions.push(serde_json::from_value(json!({
            "request":{"version":1,"id":"selected","run":"R1","question":"Proceed?","requested_action":"Answer","owner":"owner","source":"worker"},
            "registered_at":at(0),"observation":{"at":at(0),"state":"ready","detail":"answered"},
            "receipt":{"version":1,"id":"answer","run":"R1","outcome":"answered","delivery":"acknowledged","observed_at":at(0)}
        })).unwrap());
        fx.ctx.store.fixture_put_task(&source).unwrap();
        let dependency = fx
            .ctx
            .store
            .create("Finished dependency", None, Status::Done, "")
            .unwrap();
        worker.runs.last_mut().unwrap().obligations.waits.push(serde_json::from_value::<WaitRecord>(json!({
            "request":{"version":1,"id":"W-live","run":"R1","condition":"Dependency","owner":"worker","source":"worker",
                "handle":{"kind":"channel","channel_id":format!("task:{}:landed",dependency.id),"capability":"wait_v1"}},
            "registered_at":at(0),"observation":{"at":at(0),"state":"pending","detail":"waiting"},"receipt":null
        })).unwrap());
        fx.ctx.store.fixture_put_task(&worker).unwrap();
        let mut parked = fx
            .ctx
            .store
            .create("Parked", None, Status::Ready, "")
            .unwrap();
        parked.park = Some(
            serde_json::from_value(
                json!({"version":1,"revision":1,"by":"owner","reason":"Wait","at":at(0),
            "resume_when":[],"resume_action":"Resume","review_by":at(700)}),
            )
            .unwrap(),
        );
        parked.park.as_mut().unwrap().resume_when = vec![
            ResumeCheck {
                kind: "answered".into(),
                target: Some(key),
                expected: Some("1".into()),
                description: "answer".into(),
                machine_checkable: true,
                satisfied: None,
            },
            ResumeCheck {
                kind: "decision".into(),
                target: Some("selected".into()),
                expected: None,
                description: "decision".into(),
                machine_checkable: true,
                satisfied: None,
            },
        ];
        fx.ctx.store.fixture_put_task(&parked).unwrap();
        for _ in 0..2 * scale {
            fx.ctx
                .store
                .create("Unrelated settled", None, Status::Done, "")
                .unwrap();
        }
        let cold = super::super::board(&fx.ctx, at(100)).unwrap();
        let expected = cold
            .tasks
            .into_iter()
            .filter(|row| !matches!(row.status, Status::Done | Status::Dropped))
            .collect::<Vec<_>>();
        let _hot = HotScope::enter();
        let before = work::snapshot();
        let (roots, hot) = snapshot(&fx.ctx, at(100)).unwrap();
        assert_eq!(hot.tasks, expected);
        assert_eq!(
            roots
                .iter()
                .map(|head| head.id.as_str())
                .collect::<Vec<_>>(),
            vec![worker.id.as_str(), parked.id.as_str()]
        );
        let wait = hot
            .tasks
            .iter()
            .find(|row| row.task == worker.id)
            .unwrap()
            .assessment
            .conditions
            .iter()
            .find(|row| row.kind == "obligation_wait")
            .unwrap();
        assert_eq!(wait.reason_code, "wait_receipt_missing");
        assert!(
            wait.resume
                .iter()
                .any(|check| check.kind == "task_status" && check.satisfied == Some(true))
        );
        let after = work::snapshot();
        measured.push(Counter::ALL.map(|counter| after.total(counter) - before.total(counter)));
        assert_eq!(
            after.total(Counter::FullTaskHydrations) - before.total(Counter::FullTaskHydrations),
            0
        );
    }
    assert_eq!(measured[0], measured[1]);
}
