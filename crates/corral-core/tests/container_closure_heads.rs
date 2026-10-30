//! Closure is not host health or acceptance of a new mutation: preserve native/legacy delivery.
mod store_support;
use corral_core::agent::{self, ClosureReason, ContainerRef};
use corral_core::inbox::{Inbox, Recipient};
use corral_core::work::{self, Counter, HotScope};
use corral_core::{RunOutcome, Status, Verdict};
use serde_json::json;

#[test]
fn worker_and_reviewer_closure_match_cold_facades_without_history_reads() {
    for reviewer in [false, true] {
        for native in [false, true] {
            for case in 0..14 {
                let mut measured = Vec::new();
                for scale in [1, 10] {
                    let (_dir, store) = store_support::store();
                    let mut task = store_support::rich(
                        &store,
                        2 * scale + 1,
                        2 * scale + 1,
                        2 * scale + 1,
                        scale,
                    );
                    task.status = Status::Review;
                    let task_id = task.id.clone();
                    let parent = task.runs.last_mut().unwrap();
                    parent.outcome = None;
                    parent.ended = None;
                    parent.review = None;
                    parent.harness = Some(if native { "pi-native" } else { "omo" }.into());
                    for child in &mut parent.review_runs {
                        child.review = None;
                    }
                    let last = parent.review_runs.last_mut().unwrap();
                    last.outcome = None;
                    last.ended = None;
                    let mut review = last.id.clone();
                    let mut run = parent.id.clone();
                    let mut id = task_id.clone();
                    let mut expected = None;
                    match case {
                        0 => {}
                        1 => {
                            parent.outcome = Some(RunOutcome::Discarded);
                            expected = Some(ClosureReason::RunEnded);
                        }
                        2 => {
                            task.status = Status::Dropped;
                            expected = Some(ClosureReason::RunEnded);
                        }
                        3 => {
                            parent.outcome = Some(RunOutcome::Done);
                        }
                        4 => {
                            parent.outcome = Some(RunOutcome::Done);
                            task.status = Status::Done;
                            expected = Some(ClosureReason::RunEnded);
                        }
                        5 => {
                            parent.outcome = Some(RunOutcome::Failed);
                        }
                        6 => {
                            run = "missing".into();
                            expected = Some(ClosureReason::RunEnded);
                        }
                        7 => {
                            run = task.runs[0].id.clone();
                            expected = Some(ClosureReason::RunEnded);
                        }
                        8 => {
                            parent.review_runs[0].outcome = Some(RunOutcome::Superseded);
                            review = parent.review_runs[0].id.clone();
                            if reviewer {
                                expected = Some(ClosureReason::ReviewSuperseded);
                            }
                        }
                        9 => {
                            last.review=Some(serde_json::from_value(json!({"reviewer":"rev","verdict":Verdict::Approve,"sha":"abc","at":store_support::at(0)})).unwrap());
                            if reviewer {
                                expected = Some(ClosureReason::ReviewClosed);
                            }
                        }
                        10 => {
                            last.outcome = Some(RunOutcome::Abandoned);
                            if reviewer {
                                expected = Some(ClosureReason::ReviewInterrupted);
                            }
                        }
                        11 => {
                            last.review=Some(serde_json::from_value(json!({"reviewer":"rev","sha":"abc","at":store_support::at(0),"process_lost":true})).unwrap());
                            if reviewer {
                                expected = Some(ClosureReason::ReviewInterrupted);
                            }
                        }
                        12 => {
                            parent.ended = Some(store_support::at(900));
                        }
                        13 => {
                            id = "T-999".into();
                            expected = Some(ClosureReason::RunEnded);
                        }
                        _ => unreachable!(),
                    }
                    let (container, recipient) = if reviewer {
                        (
                            ContainerRef::Reviewer {
                                task: id.clone(),
                                run: run.clone(),
                                review: review.clone(),
                            },
                            Recipient::ReviewRun {
                                task: id,
                                run,
                                review,
                            },
                        )
                    } else {
                        (
                            ContainerRef::Worker {
                                task: id.clone(),
                                run: run.clone(),
                            },
                            Recipient::Run { task: id, run },
                        )
                    };
                    assert_eq!(
                        agent::container_closure(&task, &container),
                        expected,
                        "cold reviewer={reviewer} native={native} case={case}"
                    );
                    assert_eq!(
                        agent::container_closure_ref(&task, &container).unwrap(),
                        expected
                    );
                    let inbox = Inbox::new(&store, recipient).unwrap();
                    let ended = inbox.recipient_ended(&task);
                    assert_eq!(inbox.recipient_ended_ref(&task).unwrap(), ended);
                    store.fixture_put_task(&task).unwrap();
                    let head = store.head(&task_id).unwrap();
                    let _hot = HotScope::enter();
                    let before = work::snapshot();
                    assert_eq!(
                        agent::container_closure_ref(&head, &container).unwrap(),
                        expected
                    );
                    assert_eq!(inbox.recipient_ended_ref(&head).unwrap(), ended);
                    let after = work::snapshot();
                    measured.push(
                        Counter::ALL.map(|counter| after.total(counter) - before.total(counter)),
                    );
                    assert_eq!(
                        after.total(Counter::FullTaskHydrations)
                            - before.total(Counter::FullTaskHydrations),
                        0
                    );
                }
                assert_eq!(
                    measured[0], measured[1],
                    "reviewer={reviewer} native={native} case={case}"
                );
            }
        }
    }
}
