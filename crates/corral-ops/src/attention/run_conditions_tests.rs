#![cfg(unix)]
use super::super::handling_tests::fixture::{Fx, at};
use super::*;
use corral_core::agent::{AgentRole, HostHealth, OutboxState, QuestionState, QuestionTerminal};
use corral_core::work::{self, Counter, HotScope};
use corral_core::{RunOutcome, WaitRecord};
use serde_json::json;

fn grow(run: &mut Run, scale: usize) {
    let host = run.agent_host.as_mut().unwrap();
    let template = host.questions[0].clone();
    for n in 0..2 * scale {
        let mut old = template.clone();
        let prefix = template.key.rsplit_once('/').unwrap().0;
        old.key = format!("{prefix}/Q{:06}", n + 100);
        old.state = QuestionState::Cancelled;
        old.terminal = Some(QuestionTerminal {
            state: QuestionState::Cancelled,
            reason: "settled".into(),
            at: at(0),
        });
        old.wake.state = OutboxState::Posted;
        host.questions.insert(0, old);
    }
    let template = run.obligations.waits[0].clone();
    for n in 0..2 * scale {
        let mut old = template.clone();
        old.request.id = format!("old-{n:06}");
        old.receipt = Some(
            serde_json::from_value(json!({"version":1,"id":format!("rc-{n:06}"),"run":run.id,
            "outcome":"ready","delivery":"acknowledged","observed_at":at(0)}))
            .unwrap(),
        );
        old.observation = corral_core::receipt_observation(old.receipt.as_ref().unwrap(), at(0));
        run.obligations.waits.insert(0, old);
    }
}

#[test]
fn active_run_condition_rows_match_full_facade_without_settled_history_reads() {
    for case in 0..7 {
        let mut measured = Vec::new();
        for scale in [1, 10] {
            let fx = Fx::new();
            let mut task = fx.questions(1);
            task.status = Status::NeedsMe;
            task.status_since = Some(at(0));
            let parent = task.runs.last_mut().unwrap();
            parent.idle_no_report = case >= 2;
            parent.idle_since = Some(at(0));
            parent.stuck = Some(
                serde_json::from_value(json!({"pid":40,"started":"50","argv":["foreign"]}))
                    .unwrap(),
            );
            parent.changes_round = Some(
                serde_json::from_value(
                    json!({"n":2,"at":at(0),"attempt":"rev","callback":"cb","sha":"abc"}),
                )
                .unwrap(),
            );
            parent.obligations.waits.push(serde_json::from_value::<WaitRecord>(json!({
                "request":{"version":1,"id":"W-live","run":"R1","condition":"Dependency","owner":"worker","source":"worker",
                    "handle":{"kind":"channel","channel_id":"task:T-999:landed","capability":"wait_v1"}},
                "registered_at":at(0),"observation":{"at":at(0),"state":"pending","detail":"waiting"},"receipt":null
            })).unwrap());
            if case != 3 {
                parent.obligations.decisions.push(serde_json::from_value(json!({
                    "request":{"version":1,"id":"D-live","run":"R1","question":"Proceed?","requested_action":"Answer","owner":"owner","source":"worker"},
                    "registered_at":at(0),"observation":{"at":at(0),"state":"pending","detail":"waiting"},"receipt":null
                })).unwrap());
            }
            if case == 1 {
                parent.agent_host.as_mut().unwrap().binding.health = HostHealth::Fenced;
            }
            if case >= 4 {
                let mut child = parent.clone();
                child.id = "R1-review1".into();
                child.changes_round = None;
                child.agent_host.as_mut().unwrap().binding.role = AgentRole::Reviewer;
                child.agent_host.as_mut().unwrap().questions[0].key =
                    format!("{}/R1/{}/Q1", task.id, child.id);
                for wait in &mut child.obligations.waits {
                    wait.request.run = child.id.clone();
                }
                for decision in &mut child.obligations.decisions {
                    decision.request.run = child.id.clone();
                }
                grow(&mut child, scale);
                if case == 4 {
                    parent.outcome = Some(RunOutcome::Done);
                }
                if case == 5 {
                    let mut second = child.clone();
                    second.id = "R1-review2".into();
                    for question in &mut second.agent_host.as_mut().unwrap().questions {
                        question.key = question.key.replace("/R1-review1/", "/R1-review2/");
                    }
                    for wait in &mut second.obligations.waits {
                        wait.request.run = second.id.clone();
                        if let Some(receipt) = &mut wait.receipt {
                            receipt.run = second.id.clone();
                        }
                    }
                    for decision in &mut second.obligations.decisions {
                        decision.request.run = second.id.clone();
                    }
                    parent.review_runs.push(child);
                    child = second;
                }
                if case == 6 {
                    child.ended = Some(at(1));
                }
                parent.review_runs.push(child);
            }
            grow(parent, scale);
            fx.ctx.store.fixture_put_task(&task).unwrap();
            let now = at(100);
            let mut expected = Vec::new();
            let parent = task.current_run().unwrap();
            if parent.outcome.is_none() {
                run_conditions(&task, parent, now, &mut expected, false);
            }
            for child in parent
                .review_runs
                .iter()
                .filter(|child| child.outcome.is_none() && child.ended.is_none())
            {
                run_conditions(&task, child, now, &mut expected, true);
            }
            assert!(
                expected
                    .iter()
                    .any(|condition| condition.kind == "obligation_wait")
            );
            if case == 3 {
                assert!(
                    !expected
                        .iter()
                        .any(|condition| condition.kind == "stuck_process")
                );
            }
            let head = fx.ctx.store.head(&task.id).unwrap();
            let _hot = HotScope::enter();
            let before = work::snapshot();
            assert_eq!(
                run_conditions_ref(&head, now).unwrap(),
                expected,
                "case={case} scale={scale}"
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
