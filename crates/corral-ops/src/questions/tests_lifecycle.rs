use corral_core::agent::{
    AgentTurn, QuestionState, TurnClass, TurnOrigin, TurnOriginKind, container_state_mut,
};
use corral_core::inbox::{Inbox, Recipient, TerminalState};
use corral_core::{RunOutcome, Status, Task, Verdict};
use jiff::{SignedDuration, Timestamp};
use serde_json::json;

use super::tests_support::*;
use super::{Actor, AnswerInput};
use crate::agent::fence_generation;

type Closing = (&'static str, fn(&mut Task));

fn review_record(verdict: Option<Verdict>, process_lost: bool) -> corral_core::ReviewRecord {
    let mut record = json!({"reviewer": "pi", "sha": "abc", "at": "2026-10-23T00:00:00Z"});
    if let Some(verdict) = verdict {
        record["verdict"] = json!(verdict);
    }
    if process_lost {
        record["process_lost"] = json!(true);
    }
    serde_json::from_value(record).unwrap()
}

fn cancel_reason(fx: &Fx, key: &str) -> Option<String> {
    question(fx, key).terminal.map(|t| t.reason)
}

fn assert_closed_everywhere(fx: &Fx, key: &str, revision: u32) {
    let owner = AnswerInput {
        text: "late".into(),
        ..AnswerInput::default()
    };
    let attempts = [
        pm_answer(fx, key, revision),
        super::answer(&fx.ctx, key, revision, Actor::Owner, owner),
        super::escalate(&fx.ctx, key, revision, Actor::Pm(None), None, None),
        super::cancel(&fx.ctx, key, revision, Actor::Owner, "late"),
        super::deadline(&fx.ctx, key, revision, Timestamp::now(), "late"),
    ];
    for attempt in attempts {
        assert_eq!(refusal(&attempt.unwrap_err()), "question_closed", "{key}");
    }
}

#[test]
fn worker_questions_close_only_on_canonical_run_outcomes() {
    let closings: [Closing; 4] = [
        ("discard", |t| {
            t.runs[0].outcome = Some(RunOutcome::Discarded)
        }),
        ("retry", |t| {
            t.runs[0].outcome = Some(RunOutcome::Superseded);
            t.runs.push(run("R2", "p9"));
        }),
        ("drop", |t| t.status = Status::Dropped),
        ("landing", |t| {
            t.runs[0].outcome = Some(RunOutcome::Done);
            t.status = Status::Done;
        }),
    ];
    for (name, close) in closings {
        let fx = fx();
        let (id, worker) = worker(&fx);
        let answered = ask_ok(&fx, &worker, "c1", false);
        pm_answer(&fx, &answered, 1).unwrap();
        let open = ask_ok(&fx, &worker, "c2", true);
        fx.ctx
            .store
            .update(&id, |t| {
                close(t);
                Ok(())
            })
            .unwrap();

        let view = super::show(&fx.ctx, &open).unwrap();
        assert!(
            !view.pending,
            "{name}: readers see it closed before terminalize"
        );
        assert_eq!(view.effective_state, QuestionState::Cancelled, "{name}");
        assert_eq!(
            super::terminalize(&fx.ctx, &id).unwrap(),
            vec![open.clone()],
            "{name}"
        );
        assert_eq!(
            cancel_reason(&fx, &open).as_deref(),
            Some("run_ended"),
            "{name}"
        );
        assert!(
            super::terminalize(&fx.ctx, &id).unwrap().is_empty(),
            "{name}"
        );
        assert_eq!(events(&fx, "question_cancelled"), 1, "{name}");
        let inbox = Inbox::new(
            &fx.ctx.store,
            Recipient::Run {
                task: id.clone(),
                run: "R1".into(),
            },
        )
        .unwrap();
        let seq = messages(&fx, &id, "R1")[0].seq;
        assert_eq!(
            inbox.terminal(seq).unwrap(),
            Some(TerminalState::Cancelled),
            "{name}"
        );
        assert_closed_everywhere(&fx, &open, 2);
    }
}

#[test]
fn refused_done_cancelled_turn_and_host_loss_leave_questions_pending() {
    let fx = fx();
    let (id, worker) = worker(&fx);
    let key = ask_ok(&fx, &worker, "c1", true);
    let refused = json!({"key": "nope", "reason": "x"});
    assert!(
        send(&fx, &worker, "withdraw", "bad", refused)
            .rejection()
            .is_some()
    );
    fx.ctx
        .store
        .update(&id, |t| {
            let state = container_state_mut(t, &worker_container(&id)).unwrap();
            state.turns.push(AgentTurn {
                generation: 1,
                seq: 1,
                origin: TurnOrigin {
                    kind: TurnOriginKind::Bridge,
                    idempotency_key: None,
                },
                started: Some(Timestamp::now()),
                ended: Some(Timestamp::now()),
                class: Some(TurnClass::Cancelled),
                provider_error: None,
            });
            Ok(())
        })
        .unwrap();
    assert!(fence_generation(&fx.ctx.store, &worker_container(&id), 1, "host lost").unwrap());
    assert!(pending(&fx, &key));
    fx.ctx
        .store
        .update(&id, |t| {
            t.runs[0].outcome = Some(RunOutcome::Done);
            t.status = Status::Review;
            Ok(())
        })
        .unwrap();
    assert!(pending(&fx, &key), "done in Review does not close the run");
    assert!(super::terminalize(&fx.ctx, &id).unwrap().is_empty());
    assert_eq!(question(&fx, &key).state, QuestionState::Open);
}

#[test]
fn review_child_closure_paths_cancel_with_their_reason_and_refuse_later_actions() {
    let closings: [Closing; 7] = [
        ("approve", |t| {
            t.runs[0].review_runs[0].review = Some(review_record(Some(Verdict::Approve), false))
        }),
        ("changes", |t| {
            t.runs[0].review_runs[0].review = Some(review_record(Some(Verdict::Changes), false))
        }),
        ("escalate", |t| {
            t.runs[0].review_runs[0].review = Some(review_record(Some(Verdict::Escalate), false))
        }),
        ("supersede_review", |t| {
            t.runs[0].review_runs[0].outcome = Some(RunOutcome::Superseded)
        }),
        ("process_lost", |t| {
            t.runs[0].review_runs[0].review = Some(review_record(None, true))
        }),
        ("newer_attempt", |t| {
            t.runs[0].review_runs.push(run("R1-review2", "p3"))
        }),
        ("terminal_parent", |t| {
            t.runs[0].outcome = Some(RunOutcome::Discarded)
        }),
    ];
    let expected = [
        "review_closed",
        "review_closed",
        "review_closed",
        "review_superseded",
        "review_interrupted",
        "review_superseded",
        "run_ended",
    ];
    for ((name, close), reason) in closings.into_iter().zip(expected) {
        let fx = fx();
        let (id, reviewer) = reviewer(&fx);
        let key = ask_ok(&fx, &reviewer, "r1", true);
        fx.ctx
            .store
            .update(&id, |t| {
                close(t);
                Ok(())
            })
            .unwrap();
        let view = super::show(&fx.ctx, &key).unwrap();
        assert!(!view.pending, "{name}");
        assert_eq!(view.closure.map(|c| c.as_str()), Some(reason), "{name}");
        assert_eq!(
            super::terminalize(&fx.ctx, &id).unwrap(),
            vec![key.clone()],
            "{name}"
        );
        assert_eq!(cancel_reason(&fx, &key).as_deref(), Some(reason), "{name}");
        assert!(
            super::terminalize(&fx.ctx, &id).unwrap().is_empty(),
            "{name}"
        );
        assert_closed_everywhere(&fx, &key, 2);
    }
}

#[test]
fn withdrawn_reviewer_questions_refuse_new_triage_as_closed_after_closure() {
    let closings: [Closing; 4] = [
        ("verdict", |t| {
            t.runs[0].review_runs[0].review = Some(review_record(Some(Verdict::Approve), false))
        }),
        ("supersession", |t| {
            t.runs[0].review_runs[0].outcome = Some(RunOutcome::Superseded)
        }),
        ("process_loss", |t| {
            t.runs[0].review_runs[0].review = Some(review_record(None, true))
        }),
        ("terminal_parent", |t| {
            t.runs[0].outcome = Some(RunOutcome::Discarded)
        }),
    ];
    for (name, close) in closings {
        {
            let fx = fx();
            let (id, reviewer) = reviewer(&fx);
            let key = ask_ok(&fx, &reviewer, "r1", true);
            withdraw_question(&fx, &reviewer, &key);
            fx.ctx
                .store
                .update(&id, |t| {
                    close(t);
                    Ok(())
                })
                .unwrap();
            let before = serde_json::to_value(fx.ctx.store.get(&id).unwrap()).unwrap();
            let events_before = std::fs::read(fx.ctx.store.dir().join("events.jsonl")).unwrap();
            let inbox_before = review_messages(&fx, &id, "R1-review1");
            assert_closed_everywhere(&fx, &key, question(&fx, &key).revision);
            assert!(super::terminalize(&fx.ctx, &id).unwrap().is_empty());
            assert_eq!(
                serde_json::to_value(fx.ctx.store.get(&id).unwrap()).unwrap(),
                before,
                "{name}"
            );
            assert_eq!(
                std::fs::read(fx.ctx.store.dir().join("events.jsonl")).unwrap(),
                events_before,
                "{name}"
            );
            assert_eq!(
                review_messages(&fx, &id, "R1-review1"),
                inbox_before,
                "{name}"
            );
        }
    }
}

fn withdraw_question(fx: &Fx, host: &Host, key: &str) {
    let withdraw = json!({"key": key, "reason": "answered by the diff"});
    assert!(
        send(fx, host, "withdraw", "w1", withdraw)
            .receipt()
            .is_some()
    );
}

#[test]
fn withdrawn_question_in_open_container_still_refuses_as_withdrawn() {
    let fx = fx();
    let (_, reviewer) = reviewer(&fx);
    let key = ask_ok(&fx, &reviewer, "r1", true);
    withdraw_question(&fx, &reviewer, &key);
    let attempts = [
        pm_answer(&fx, &key, 2),
        super::answer(
            &fx.ctx,
            &key,
            2,
            Actor::Owner,
            AnswerInput {
                text: "late".into(),
                ..AnswerInput::default()
            },
        ),
        super::escalate(&fx.ctx, &key, 2, Actor::Pm(None), None, None),
        super::cancel(&fx.ctx, &key, 2, Actor::Owner, "late"),
        super::deadline(&fx.ctx, &key, 2, Timestamp::now(), "late"),
    ];
    for attempt in attempts {
        assert_eq!(refusal(&attempt.unwrap_err()), "question_withdrawn");
    }
}

#[test]
fn withdrawn_worker_question_refuses_as_closed_after_run_ends() {
    let fx = fx();
    let (id, worker) = worker(&fx);
    let key = ask_ok(&fx, &worker, "c1", true);
    withdraw_question(&fx, &worker, &key);
    fx.ctx
        .store
        .update(&id, |t| {
            t.runs[0].outcome = Some(RunOutcome::Discarded);
            Ok(())
        })
        .unwrap();
    assert_closed_everywhere(&fx, &key, 2);
}

#[test]
fn committed_triage_receipt_replays_after_withdrawal_and_container_closure() {
    for superseded in [false, true] {
        let fx = fx();
        let (id, reviewer) = reviewer(&fx);
        let key = ask_ok(&fx, &reviewer, "r1", true);
        let pm = pm_host(&fx);
        let payload = json!({"key": key, "rev": 1, "note": "owner call"});
        let first = send(&fx, &pm, "question_escalate", "e1", payload.clone());
        let receipt = first.receipt().unwrap().clone();
        withdraw_question(&fx, &reviewer, &key);
        fx.ctx
            .store
            .update(&id, |t| {
                if superseded {
                    t.runs[0].review_runs[0].outcome = Some(RunOutcome::Superseded);
                } else {
                    t.runs[0].review_runs[0].review =
                        Some(review_record(Some(Verdict::Approve), false));
                }
                Ok(())
            })
            .unwrap();
        let before = serde_json::to_value(fx.ctx.store.get(&id).unwrap()).unwrap();
        let replay = send(&fx, &pm, "question_escalate", "e1", payload.clone());
        assert!(matches!(
            replay,
            crate::agent::CallResponse::Receipt { replayed: true, .. }
        ));
        assert_eq!(replay.receipt(), Some(&receipt));
        assert_eq!(
            code(&send(&fx, &pm, "question_escalate", "e2", payload)),
            Some("question_closed")
        );
        assert_eq!(
            serde_json::to_value(fx.ctx.store.get(&id).unwrap()).unwrap(),
            before
        );
    }
}

#[test]
fn superseded_reviewer_questions_never_reach_the_next_attempt() {
    let fx = fx();
    let (id, first) = reviewer(&fx);
    let old = ask_ok(&fx, &first, "r1", true);
    fx.ctx
        .store
        .update(&id, |t| {
            t.runs[0].review_runs[0].outcome = Some(RunOutcome::Superseded);
            t.runs[0].review_runs.push(run("R1-review2", "p3"));
            Ok(())
        })
        .unwrap();
    let second = review_child(&fx, &id, "R1-review2", "p3", "g2:c2");
    let new = ask_ok(&fx, &second, "r1", true);
    assert_eq!(new, format!("{id}/R1/R1-review2/Q1"));
    super::terminalize(&fx.ctx, &id).unwrap();
    assert_eq!(
        cancel_reason(&fx, &old).as_deref(),
        Some("review_superseded")
    );
    assert_eq!(
        refusal(&pm_answer(&fx, &old, 2).unwrap_err()),
        "question_closed"
    );
    let fresh = question(&fx, &new);
    assert_eq!(
        (fresh.state, fresh.revision, fresh.answer),
        (QuestionState::Open, 1, None)
    );
}

fn awaiting_turn(fx: &Fx, id: &str) {
    fx.ctx
        .store
        .update(id, |t| {
            let state = container_state_mut(t, &worker_container(id)).unwrap();
            state.turns.push(AgentTurn {
                generation: 1,
                seq: 1,
                origin: TurnOrigin {
                    kind: TurnOriginKind::Bridge,
                    idempotency_key: None,
                },
                started: Some(Timestamp::now()),
                ended: Some(Timestamp::now()),
                class: Some(TurnClass::AwaitingAnswer),
                provider_error: None,
            });
            Ok(())
        })
        .unwrap();
}

#[test]
fn unattended_marks_blocking_or_awaiting_questions_once_per_revision() {
    let fx = fx();
    let (id, first) = worker(&fx);
    let blocking = ask_ok(&fx, &first, "c1", true);
    let (quiet, quiet_worker) = worker(&fx);
    let waiting = ask_ok(&fx, &quiet_worker, "c1", false);
    let (review_task, reviewer) = reviewer(&fx);
    let reviewing = ask_ok(&fx, &reviewer, "r1", true);
    let soon = Timestamp::now() + SignedDuration::from_secs(60);
    assert!(super::unattended(&fx.ctx, &id, soon).unwrap().is_empty());
    let later = Timestamp::now() + SignedDuration::from_secs(601);

    assert_eq!(
        super::unattended(&fx.ctx, &id, later).unwrap(),
        vec![blocking.clone()]
    );
    assert!(super::unattended(&fx.ctx, &id, later).unwrap().is_empty());
    assert_eq!(events(&fx, "question_unattended"), 1);
    let task = fx.ctx.store.get(&id).unwrap();
    assert_eq!(task.status, Status::NeedsMe);
    assert!(
        task.runs[0]
            .blocked_excerpt
            .as_deref()
            .unwrap()
            .contains(&blocking)
    );
    assert_eq!(question(&fx, &blocking).revision, 1);

    assert!(
        super::unattended(&fx.ctx, &quiet, later)
            .unwrap()
            .is_empty()
    );
    awaiting_turn(&fx, &quiet);
    assert_eq!(
        super::unattended(&fx.ctx, &quiet, later).unwrap(),
        vec![waiting]
    );

    assert_eq!(
        super::unattended(&fx.ctx, &review_task, later).unwrap(),
        vec![reviewing.clone()]
    );
    // The reviewer's own question holds its task in Needs me; its binding stays live.
    let held = fx.ctx.store.get(&review_task).unwrap();
    assert_eq!(held.status, Status::NeedsMe);
    assert!(corral_core::agent::question_hold(&held, &held.runs[0]));
    assert!(
        held.runs[0]
            .blocked_excerpt
            .as_deref()
            .unwrap()
            .contains(&reviewing)
    );
    let cards = crate::pm_next::observe::observations(&fx.ctx).unwrap();
    assert!(cards.iter().any(|o| matches!(o,
        crate::pm_next::PmObservation::OwnerQuestion { key, reason, .. }
            if *key == reviewing && reason == "unattended")));

    pm_answer(&fx, &blocking, 1).unwrap();
    let task = fx.ctx.store.get(&id).unwrap();
    assert_eq!(
        (task.status, task.runs[0].blocked_excerpt.clone()),
        (Status::Running, None)
    );
}
