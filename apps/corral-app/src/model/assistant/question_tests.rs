use corral_protocol::assistant::{EntryKind, MessageState, ThreadEntry, TurnCompletion};
use corral_protocol::rows::{CostBasis, WaitReason};
use corral_types::{Op, Status};

use super::{Action, AssistantMessage, State, ThreadItem, live_items};
use crate::model::daemon::{Feed, ThreadSnapshot};
use crate::model::effect::Effect;
use crate::model::hue::Hue;

fn entry(id: &str, kind: EntryKind) -> ThreadEntry {
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
        card: None,
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
    live_items(state, feed, "A-1")
        .into_iter()
        .filter_map(|item| match item {
            ThreadItem::Assistant(message) => Some(message),
            _ => None,
        })
        .collect()
}

/// A message waiting because the assistant cannot run offers File as idea, which files its
/// words as an Idea task with no card from the daemon.
#[test]
fn a_waiting_message_is_filed_as_an_idea_in_the_owners_words() {
    let mut waiting = entry("A-1/M1/text", EntryKind::Message);
    waiting.text = Some("Retry the flaky gate\nIt failed twice on main.".into());
    waiting.state = Some(MessageState::Waiting {
        reason: WaitReason {
            code: "host_down".into(),
            text: "The host is down.".into(),
            retry_at: None,
        },
    });
    let feed = feed_with(vec![waiting]);
    let mut state = State::default();
    let offered = live_items(&state, &feed, "A-1").into_iter().any(|item| {
        matches!(item, ThreadItem::Owner(owner)
            if owner.file.as_ref().is_some_and(|file| file.action == Action::FileEntry("A-1/M1/text".into())))
    });
    assert!(offered, "the waiting message offers File as idea");
    let effects = state.act_live(Action::FileEntry("A-1/M1/text".into()), &feed);
    let [Effect::Act(act)] = effects.as_slice() else {
        panic!("one engine operation, got {effects:?}");
    };
    let Op::New { task } = &act.op else {
        panic!("filing creates a task, got {:?}", act.op);
    };
    assert_eq!(task.title, "Retry the flaky gate");
    assert_eq!(task.body, "Retry the flaky gate\nIt failed twice on main.");
    assert_eq!(task.status, Status::Idea);
}

#[test]
fn each_settled_answer_says_its_own_cost_and_how_its_turn_ended() {
    let mut reported = entry("A-1/M1/answer/0", EntryKind::Answer);
    reported.cost_usd = Some(0.04);
    reported.cost_basis = Some(CostBasis::Reported);
    reported.completion = Some(TurnCompletion::Completed);
    let mut estimated = entry("A-1/M2/answer/0", EntryKind::Answer);
    estimated.at = "2026-11-15T10:02:00Z".into();
    estimated.cost_usd = Some(0.12);
    estimated.cost_basis = Some(CostBasis::Estimated);
    estimated.completion = Some(TurnCompletion::Failed);
    let mut unknown = entry("A-1/M3/answer/0", EntryKind::Answer);
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
