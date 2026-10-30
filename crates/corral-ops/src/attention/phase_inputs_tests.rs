#![cfg(unix)]
use super::super::handling_tests::fixture::{Fx, at};
use super::*;
use corral_core::work::{self, Counter, HotScope};

#[test]
fn phase_inputs_match_full_draft_and_latest_active_reviewer_with_constant_work() {
    for last_closed in [false, true] {
        let mut measured = Vec::new();
        for scale in [1, 10] {
            let fx = Fx::new();
            let mut task = fx.questions(0);
            let parent = task.runs.last_mut().unwrap();
            let mut child = parent.clone();
            child.id = "active-review".into();
            child.agent_host = None;
            let mut closed = child.clone();
            closed.outcome = Some(RunOutcome::Done);
            closed.ended = Some(at(1));
            if last_closed {
                parent.review_runs.push(child.clone());
            }
            for n in 0..2 * scale {
                closed.id = format!("closed-{n:06}");
                parent.review_runs.push(closed.clone());
            }
            if !last_closed {
                parent.review_runs.push(child);
            }
            fx.ctx.store.fixture_put_task(&task).unwrap();
            let expected = Phases::full(&task);
            assert!(expected.run.as_ref().unwrap().latest_reviewer.is_some());
            let head = fx.ctx.store.head(&task.id).unwrap();
            let _hot = HotScope::enter();
            let before = work::snapshot();
            assert_eq!(Phases::head(&head).unwrap(), expected);
            let after = work::snapshot();
            measured.push(Counter::ALL.map(|counter| after.total(counter) - before.total(counter)));
            assert_eq!(
                after.total(Counter::FullTaskHydrations)
                    - before.total(Counter::FullTaskHydrations),
                0
            );
        }
        assert_eq!(measured[0], measured[1]);
    }
}
