use corral_protocol::subscription::{SessionAgentMessage, SessionChild, SessionFleet};

use super::{AgentsTab, agents_tab};
use crate::model::Id;
use crate::model::daemon::{Feed, Link};
use crate::model::health::daemon_env;
use crate::model::nav::Form;
use crate::model::session::transcript::LiveSession;
use crate::model::session::{Cx, Facts, SessionRole, Ui};

const ID: &str = "B-k3x9";

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
        profile: None,
        effort: None,
        pending_model: None,
        turns: 0,
        context_percent: None,
        files: 0,
        outside: 0,
        owns: 0,
        after: Vec::new(),
        steerable: false,
        error: None,
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

fn shown(fleet: Option<SessionFleet>) -> AgentsTab {
    let id = Id::from(ID);
    let mut feed = Feed {
        link: Link::Up,
        ..Feed::default()
    };
    let mut live = LiveSession::default();
    live.replace_fleet(fleet);
    feed.live.insert(id.clone(), live);
    let facts = Facts {
        role: SessionRole::Build,
        task: None,
    };
    let ui = Ui::default();
    let cx = Cx::new(&feed, &ui, &id, daemon_env(&feed, Form::Desktop), &facts);
    agents_tab(&cx)
}

#[test]
fn a_watched_session_without_a_fleet_lists_no_agents_and_says_so() {
    let tab = shown(None);
    assert!(tab.rows.is_empty());
    assert!(tab.empty.is_some());
}

#[test]
fn truncated_messages_say_the_total_corrald_holds() {
    let tab = shown(Some(SessionFleet {
        children: vec![],
        messages: (141..=240).map(message).collect(),
        messages_total: 240,
        max_parallel: 3,
    }));
    let messages = tab.messages.expect("the messages are listed");
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
    let fleet = |cost| SessionFleet {
        children: vec![child("a", cost)],
        messages: vec![],
        messages_total: 0,
        max_parallel: 1,
    };
    let unknown = shown(Some(fleet(None)));
    let free = shown(Some(fleet(Some(0.0))));
    assert_ne!(unknown.rows[0].cost, free.rows[0].cost);
    assert!(!unknown.rows[0].cost.contains("$0"));
}
