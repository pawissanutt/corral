use corral_protocol::subscription::{SessionAgentMessage, SessionChild, SessionFleet};

use super::{Fleet, fleet};
use crate::model::Id;
use crate::model::agents::State;
use crate::model::daemon::Feed;
use crate::model::session_feed::LiveSession;

const ID: &str = "T-1";

fn child(id: &str, cost_usd: Option<f64>) -> SessionChild {
    SessionChild {
        id: id.into(),
        kind: "slice".into(),
        title: format!("Child {id}"),
        state: "done".into(),
        model: "provider/model".into(),
        started_at: Some(1_000),
        ended_at: Some(61_000),
        activity: None,
        tokens: 1_200,
        cost_usd,
        verdict: None,
        question: None,
    }
}

fn message(seq: u64) -> SessionAgentMessage {
    SessionAgentMessage {
        seq,
        at: seq * 1_000,
        from: "parent".into(),
        to: "a".into(),
        text: format!("message {seq}"),
        delivery: "delivered".into(),
    }
}

fn shown(fleet_value: Option<SessionFleet>) -> Option<Fleet> {
    let mut feed = Feed::default();
    let mut live = LiveSession::default();
    live.replace_fleet(fleet_value);
    feed.live.insert(Id::from(ID), live);
    fleet(&State::default(), &Id::from(ID), &feed)
}

fn words(fleet: &Fleet) -> Vec<String> {
    fleet
        .children
        .iter()
        .chain(&fleet.messages)
        .flat_map(|section| &section.entries)
        .flat_map(|entry| &entry.line.body)
        .map(|rich| rich.iter().map(|span| span.text.as_str()).collect())
        .collect()
}

#[test]
fn a_watched_session_without_a_fleet_shows_no_children_section() {
    assert_eq!(shown(None), None);
}

#[test]
fn truncated_messages_say_the_total_corrald_holds() {
    let fleet = shown(Some(SessionFleet {
        children: vec![],
        messages: (141..=240).map(message).collect(),
        messages_total: 240,
        max_parallel: 3,
    }))
    .expect("a fleet with messages is shown");
    let messages = fleet.messages.expect("the messages are listed");
    assert_eq!(messages.entries.len(), 100);
    assert!(
        messages
            .note
            .as_deref()
            .is_some_and(|note| note.contains("240")),
        "{:?}",
        messages.note
    );
}

#[test]
fn a_child_with_unknown_cost_is_not_shown_as_costing_nothing() {
    let unknown = shown(Some(SessionFleet {
        children: vec![child("a", None)],
        messages: vec![],
        messages_total: 0,
        max_parallel: 1,
    }))
    .expect("a fleet with a child is shown");
    let free = shown(Some(SessionFleet {
        children: vec![child("a", Some(0.0))],
        messages: vec![],
        messages_total: 0,
        max_parallel: 1,
    }))
    .expect("a fleet with a child is shown");
    assert_ne!(words(&unknown), words(&free));
    assert!(words(&unknown).iter().all(|line| !line.contains("$0")));
}
