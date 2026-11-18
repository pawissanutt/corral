use corral_protocol::assistant::{EntryKind, ThreadEntry, TurnCompletion};
use corral_protocol::rows::CostBasis;

use super::{AssistantMessage, State, ThreadItem, live_items};
use crate::model::daemon::{Feed, ThreadSnapshot};
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
