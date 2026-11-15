use super::*;
use crate::model::{Act, act::Confirmable, daemon::CommandStatus, effect::Effect};
use corral_protocol::{
    assistant::*,
    cmd::{Command, question::QuestionCommand, task::TaskCommand},
};

fn snapshot() -> Feed {
    feed(
        1_794_744_000,
        "2026-11-15".into(),
        "12:00".into(),
        Some(jiff::tz::TimeZone::UTC),
    )
}

#[test]
fn preview_sessions_never_borrow_another_tasks_transcript() {
    let feed = snapshot();
    for agent in feed.agents.values() {
        let Some(DetailBody::Session(page)) = feed
            .detail(&DetailRef::Session {
                id: agent.id.clone(),
            })
            .and_then(|cache| cache.body())
        else {
            panic!("missing session {}", agent.id);
        };
        assert_eq!(page.id, agent.id);
        assert_eq!(page.task, agent.task);
        assert_eq!(page.role, agent.role);
        let questions = page
            .messages
            .iter()
            .flat_map(|message| &message.items)
            .filter_map(|item| {
                if let SessionItem::QuestionCard { key, .. } = item {
                    Some(key)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        if agent.id != "T-868" {
            assert!(questions.is_empty());
        }
    }
    assert!(feed.ready());
    assert_eq!(feed.date, "2026-11-15");
    assert_eq!(feed.topics["assistant"], TopicState::default());
}

#[test]
fn answering_preview_question_updates_every_live_projection_once() {
    let mut feed = snapshot();
    let command = Act::with_key(
        "answer-1".into(),
        "Answer restore policy",
        Command::Question(QuestionCommand::Answer {
            key: "T-868/R1/Q1".into(),
            rev: 2,
            text: "Pane-origin only".into(),
            option: Some("1".into()),
            basis: vec![],
            scope_note: None,
            owner: true,
            json: false,
        }),
    );
    act(&mut feed, &command).unwrap();
    let head = feed.changes_head;
    act(&mut feed, &command).unwrap();
    assert_eq!(feed.changes_head, head);
    assert_eq!(feed.commands["answer-1"].status, CommandStatus::Done);
    assert_eq!(feed.board.task("T-868").unwrap().questions, 0);
    assert!(!feed.inbox["T-868/R1/Q1"].answerable);
    assert!(
        !feed.plans["T-870"]
            .blockers
            .iter()
            .any(|blocker| blocker.contains("your answer"))
    );
    let Some(DetailBody::Session(page)) = feed
        .detail(&DetailRef::Session { id: "T-868".into() })
        .and_then(|cache| cache.body())
    else {
        panic!("missing session");
    };
    assert!(
        page.messages
            .iter()
            .flat_map(|message| &message.items)
            .any(|item| matches!(
                item,
                SessionItem::QuestionCard {
                    answer: Some(_),
                    ..
                }
            ))
    );
    let stale = Act::with_key("answer-2".into(), "Answer again", command.command.clone());
    assert!(act(&mut feed, &stale).is_err());
    assert_eq!(feed.commands["answer-2"].status, CommandStatus::Conflict);
}

#[test]
fn preview_hold_and_confirmation_use_canonical_local_state() {
    let mut feed = snapshot();
    let command = Act::with_key(
        "hold-1".into(),
        "Hold T-871",
        Command::Task(TaskCommand::Hold {
            id: "T-871".into(),
            reason: Some("budget".into()),
        }),
    );
    let preview = confirmation(&feed, &Confirmable::Act(command.clone())).unwrap();
    assert!(
        preview
            .effects
            .iter()
            .any(|effect| effect.contains("PREVIEW ONLY"))
    );
    assert!(
        preview
            .effects
            .iter()
            .any(|effect| effect.contains("budget"))
    );
    act(&mut feed, &command).unwrap();
    assert_eq!(feed.board.task("T-871").unwrap().group, Group::Held);
    let Some(DetailBody::Task(detail)) = feed
        .detail(&DetailRef::Task { id: "T-871".into() })
        .and_then(|cache| cache.body())
    else {
        panic!("missing task");
    };
    assert_eq!(
        detail.hold.as_ref().unwrap().reason.as_deref(),
        Some("budget")
    );
    assert_eq!(detail.owner_revision, "preview-2");
}

#[test]
fn preview_send_retires_its_draft_without_inventing_a_model_reply() {
    let mut feed = snapshot();
    effect(
        &mut feed,
        &Effect::Draft {
            thread: "quick".into(),
            text: "Keep these exact words".into(),
        },
    )
    .unwrap();
    let request = AssistantSend {
        cmd_id: "send-1".into(),
        thread: "quick".into(),
        target: AskTarget::Assistant,
        text: "Keep these exact words".into(),
        typed_at: "2026-11-15T12:00:00Z".into(),
        context: vec![],
        attachments: vec![],
    };
    effect(&mut feed, &Effect::Send(request.clone())).unwrap();
    effect(&mut feed, &Effect::Send(request)).unwrap();
    assert_eq!(feed.send_acceptance["send-1"], SendAcceptance::Accepted);
    assert!(!feed.drafts.contains_key("quick"));
    assert!(!feed.commands.contains_key("send-1"));
    let messages: Vec<_> = feed.threads["quick"]
        .entries
        .values()
        .filter(|entry| entry.text.as_deref() == Some("Keep these exact words"))
        .collect();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].text.as_deref(), Some("Keep these exact words"));
    assert_eq!(
        messages[0].typed_at.as_deref(),
        Some("2026-11-15T12:00:00Z")
    );
}

#[test]
fn preview_session_operations_settle_under_the_callers_identity() {
    let mut feed = snapshot();
    let key = "new-session-1".into();
    effect(
        &mut feed,
        &Effect::Session {
            key,
            op: AssistantSessionOp::New {
                task: Some("T-868".into()),
                profile: None,
            },
        },
    )
    .unwrap();
    let row = feed.operations["new-session-1"]
        .decode::<corral_protocol::rows::AssistantRow>()
        .unwrap();
    assert_eq!(row.focus.as_deref(), Some("T-868"));
    assert!(feed.threads.contains_key(row.id.as_str()));
    let target = DetailRef::UsageSeries {
        period: "month".into(),
    };
    detail(&mut feed, &target).unwrap();
    let Some(DetailBody::UsageSeries(series)) = feed.detail(&target).and_then(|cache| cache.body())
    else {
        panic!("missing usage");
    };
    assert_eq!(series.days.len(), 30);
    assert!(!series.completeness.partial);
}

#[test]
fn preview_exposed_dispatch_and_membership_actions_leave_navigable_records() {
    use corral_protocol::cmd::plan::PlanCommand;
    let mut feed = snapshot();
    let dispatch = Act::with_key(
        "dispatch-1".into(),
        "Dispatch T-891",
        Command::Task(TaskCommand::Dispatch {
            id: "T-891".into(),
            profile: None,
            model: None,
            effort: None,
            base: None,
            headless: true,
        }),
    );
    act(&mut feed, &dispatch).unwrap();
    assert_eq!(feed.agents["T-891"].task.as_deref(), Some("T-891"));
    assert!(
        feed.detail(&DetailRef::Session { id: "T-891".into() })
            .is_some()
    );
    let move_member = Act::with_key(
        "member-1".into(),
        "Move T-891",
        Command::Plan(PlanCommand::MoveMember {
            tracker: "T-893".into(),
            member: "T-891".into(),
            to: "T-898".into(),
            gate_file: None,
            reason: None,
            json: false,
        }),
    );
    act(&mut feed, &move_member).unwrap();
    assert_eq!(
        feed.board.task("T-891").unwrap().plan.as_deref(),
        Some("T-898")
    );
    for (plan, present) in [("T-893", false), ("T-898", true)] {
        let Some(DetailBody::Plan(detail)) = feed
            .detail(&DetailRef::Plan { id: plan.into() })
            .and_then(|cache| cache.body())
        else {
            panic!("missing tracker");
        };
        assert_eq!(
            detail.members.iter().any(|member| member.id == "T-891"),
            present
        );
    }
    let Some(DetailBody::Task(task)) = feed
        .detail(&DetailRef::Task { id: "T-891".into() })
        .and_then(|cache| cache.body())
    else {
        panic!("missing task");
    };
    assert_eq!(task.plan_context.as_ref().unwrap().plan, "T-898");
}

#[test]
fn preview_reasonless_hold_remains_a_releasable_canonical_hold() {
    let mut feed = snapshot();
    act(
        &mut feed,
        &Act::with_key(
            "hold-without-reason".into(),
            "Hold T-891",
            Command::Task(TaskCommand::Hold {
                id: "T-891".into(),
                reason: None,
            }),
        ),
    )
    .unwrap();
    let Some(DetailBody::Task(task)) = feed
        .detail(&DetailRef::Task { id: "T-891".into() })
        .and_then(|cache| cache.body())
    else {
        panic!("missing task");
    };
    assert!(task.hold.is_some());
}

#[test]
fn disconnected_live_review_approval_refuses_without_a_command_or_confirmation() {
    use crate::model::{
        effect::{Conn, Env},
        nav::Form,
        review,
    };
    let mut feed = snapshot();
    let selection = crate::model::live::Selection {
        review: Some("T-868".into()),
        ..Default::default()
    };
    let env = Env {
        conn: Conn::Healthy,
        form: Form::Desktop,
    };
    let connected =
        review::State::default().apply_live(review::Action::Approve, &feed, &selection, env);
    assert!(
        connected
            .iter()
            .any(|effect| matches!(effect, Effect::Act(act) if act.confirm))
    );
    feed.link = Link::Down;
    let refused =
        review::State::default().apply_live(review::Action::Approve, &feed, &selection, env);
    assert!(matches!(&refused[..], [Effect::Toast(_)]));
}

#[test]
fn preview_plan_and_review_owner_moves_settle_with_their_canonical_records() {
    use corral_protocol::{
        cmd::{plan::PlanCommand, review::ReviewCommand},
        rows::{AttentionCounts, InboxState},
    };
    let mut feed = snapshot();
    let counts = AttentionCounts::from_rows(&feed.inbox.values().cloned().collect::<Vec<_>>());
    assert_eq!((counts.total, counts.plans, counts.review), (3, 1, 1));
    assert_eq!(
        feed.board.task("T-868").unwrap().rail[0].sha7.as_deref(),
        Some("85b45e1e")
    );
    assert_eq!(feed.inbox["review:T-868:85b45e1e"].state, InboxState::Open);
    act(
        &mut feed,
        &Act::with_key(
            "accept-plan".into(),
            "Accept T-885 P2",
            Command::Plan(PlanCommand::Accept {
                id: "T-885".into(),
                revision: "P2".into(),
                owner_ok: true,
                notes: None,
            }),
        ),
    )
    .unwrap();
    act(
        &mut feed,
        &Act::with_key(
            "approve-review".into(),
            "Approve T-868",
            Command::Review(ReviewCommand::Approve {
                id: "T-868".into(),
                sha: "85b45e1e".into(),
            }),
        ),
    )
    .unwrap();
    let counts = AttentionCounts::from_rows(&feed.inbox.values().cloned().collect::<Vec<_>>());
    assert_eq!((counts.total, counts.plans, counts.review), (1, 0, 0));
    assert_eq!(feed.inbox["T-868/R1/Q1"].state, InboxState::Open);
}
