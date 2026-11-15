use corral_protocol::assistant::{EntryCard, EntryKind, ThreadEntry, TurnCompletion};
use corral_protocol::board::CostBasis;
use corral_protocol::cmd::Command;
use corral_protocol::cmd::question::QuestionCommand;
use corral_protocol::detail::question::QuestionAnswer;

use super::{Action, AskUserCard, AssistantMessage, Body, State, ThreadItem, live_items};
use crate::model::daemon::{Feed, ThreadSnapshot};
use crate::model::effect::Effect;
use crate::model::hue::Hue;

fn entry(id: &str, kind: EntryKind, card: Option<EntryCard>) -> ThreadEntry {
    ThreadEntry {
        id: id.into(),
        rev: 1,
        at: "2026-11-15T10:00:00Z".into(),
        typed_at: None,
        kind,
        device: None,
        via: None,
        text: Some("Which branch should I land this on?".into()),
        state: None,
        model: None,
        cost_usd: None,
        tools: Vec::new(),
        part: None,
        completion: None,
        cost_basis: None,
        links: Vec::new(),
        card,
        attachments: Vec::new(),
    }
}

fn feed_with(entries: Vec<ThreadEntry>) -> Feed {
    let mut thread = ThreadSnapshot::default();
    for entry in entries {
        thread.entries.insert(entry.id.as_str().into(), entry);
    }
    let mut feed = Feed::default();
    feed.threads.insert("A-1".into(), thread);
    feed
}

fn messages(state: &State, feed: &Feed) -> Vec<AssistantMessage> {
    live_items(state, feed, "A-1", true)
        .into_iter()
        .filter_map(|item| match item {
            ThreadItem::Assistant(message) => Some(message),
            _ => None,
        })
        .collect()
}

fn question(state: &State, feed: &Feed) -> AskUserCard {
    messages(state, feed)
        .into_iter()
        .flat_map(|message| message.body)
        .find_map(|body| match body {
            Body::AskUser(card) => Some(card),
            _ => None,
        })
        .expect("the question card")
}

fn free_text(answer: Option<QuestionAnswer>) -> EntryCard {
    EntryCard::Question {
        key: "Q-1".into(),
        rev: 3,
        options: Vec::new(),
        answer,
        accepts_text: true,
    }
}

#[test]
fn a_free_text_question_is_answered_in_the_thread_at_its_revision() {
    let feed = feed_with(vec![entry(
        "A-1/M1/question",
        EntryKind::Question,
        Some(free_text(None)),
    )]);
    let mut state = State::default();
    let reply = question(&state, &feed)
        .reply
        .expect("a question that takes words has a field in the thread");
    assert!(reply.send.enabled);
    let Action::AnswerText { key, rev, .. } = reply.send.action else {
        panic!("Send answers the question in the owner's words");
    };
    assert!(
        state
            .act_live(
                Action::AnswerText {
                    key: key.clone(),
                    rev,
                    text: "   ".into(),
                },
                &feed,
            )
            .is_empty(),
        "blank words send nothing"
    );
    let effects = state.act_live(
        Action::AnswerText {
            key,
            rev,
            text: "  main, after the gate  ".into(),
        },
        &feed,
    );
    let [Effect::Act(act)] = effects.as_slice() else {
        panic!("one canonical answer, got {effects:?}");
    };
    assert_eq!(
        act.command,
        Command::Question(QuestionCommand::Answer {
            key: "Q-1".into(),
            rev: 3,
            text: "main, after the gate".into(),
            option: None,
            basis: Vec::new(),
            scope_note: None,
            owner: true,
            json: false,
        })
    );

    // Another device answered first: the field goes, so nothing can race that answer.
    let answered = feed_with(vec![entry(
        "A-1/M1/question",
        EntryKind::Question,
        Some(free_text(Some(QuestionAnswer {
            by: "owner".into(),
            via: "phone".into(),
            device: None,
            at: "2026-11-15T10:01:00Z".into(),
            text: "release branch".into(),
            option: None,
        }))),
    )]);
    assert!(question(&state, &answered).reply.is_none());
}

#[test]
fn each_settled_answer_says_its_own_cost_and_how_its_turn_ended() {
    let mut reported = entry("A-1/M1/answer/0", EntryKind::Answer, None);
    reported.cost_usd = Some(0.04);
    reported.cost_basis = Some(CostBasis::Reported);
    reported.completion = Some(TurnCompletion::Completed);
    let mut estimated = entry("A-1/M2/answer/0", EntryKind::Answer, None);
    estimated.at = "2026-11-15T10:02:00Z".into();
    estimated.cost_usd = Some(0.12);
    estimated.cost_basis = Some(CostBasis::Estimated);
    estimated.completion = Some(TurnCompletion::Failed);
    let mut unknown = entry("A-1/M3/answer/0", EntryKind::Answer, None);
    unknown.at = "2026-11-15T10:04:00Z".into();
    unknown.completion = Some(TurnCompletion::Aborted);
    let feed = feed_with(vec![reported, estimated, unknown]);
    let shown = messages(&State::default(), &feed);
    let [reported, estimated, unknown] = shown.as_slice() else {
        panic!("three settled turns, got {}", shown.len());
    };
    let cost = |message: &AssistantMessage| message.cost.clone().unwrap_or_default();
    assert!(cost(reported).contains("0.04") && !cost(reported).contains("estimated"));
    assert!(cost(estimated).contains("0.12") && cost(estimated).contains("estimated"));
    // An unattributed turn is unknown, never zero or a share of the session's total.
    assert!(cost(unknown).contains("unknown") && !cost(unknown).contains('$'));
    assert!(reported.state.is_none() && reported.stopped_note.is_none());
    assert_eq!(
        estimated.state.as_ref().map(|chip| chip.hue),
        Some(Hue::Fail)
    );
    assert!(unknown.state.is_some() && unknown.stopped_note.is_some());
    assert!(
        shown
            .iter()
            .all(|message| !message.streaming && message.stop.is_none())
    );
}
