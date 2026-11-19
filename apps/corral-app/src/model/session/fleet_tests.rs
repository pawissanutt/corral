use corral_protocol::detail::CardKind;
use corral_protocol::session_state::{CardBrief, Counts, SessionState};
use corral_protocol::subscription::{SessionAgentMessage, SessionChild, SessionFleet};

use super::{AgentsTab, agents_tab, badge};
use crate::model::Id;
use crate::model::daemon::{Feed, Link};
use crate::model::health::daemon_env;
use crate::model::hue::Hue;
use crate::model::nav::Form;
use crate::model::session::transcript::LiveSession;
use crate::model::session::{Action, Cx, DockAction, Facts, SessionRole, State, Ui};

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

fn feed(fleet: Option<SessionFleet>, state: Option<SessionState>) -> Feed {
    let mut feed = Feed {
        link: Link::Up,
        ..Feed::default()
    };
    let mut live = LiveSession::default();
    live.replace_fleet(fleet);
    live.replace_state(state.map(Box::new));
    feed.live.insert(Id::from(ID), live);
    feed
}

fn with<T>(feed: &Feed, ui: &Ui, read: impl FnOnce(&Cx<'_>) -> T) -> T {
    let id = Id::from(ID);
    let facts = Facts {
        role: SessionRole::Build,
        task: None,
    };
    let cx = Cx::new(feed, ui, &id, daemon_env(feed, Form::Desktop), &facts);
    read(&cx)
}

fn shown(fleet: Option<SessionFleet>) -> AgentsTab {
    let feed = feed(fleet, None);
    with(&feed, &Ui::default(), |cx| {
        agents_tab(cx, &State::default())
    })
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

#[test]
fn a_child_that_asks_is_counted_as_waiting_and_names_its_card() {
    let asking = SessionChild {
        state: "needs".into(),
        question: Some(true),
        steerable: true,
        ended_at: None,
        ..child("a2", Some(0.04))
    };
    let fleet = SessionFleet {
        children: vec![child("a1", Some(0.21)), asking],
        messages: vec![],
        messages_total: 0,
        max_parallel: 2,
    };
    let state = SessionState {
        cards: vec![CardBrief {
            id: "card-7".into(),
            kind: CardKind::ChildQuestion,
            title: "a2 asks which cache to use".into(),
            child: Some("a2".into()),
            expires_at: None,
        }],
        children: Counts {
            live: 1,
            waiting: 1,
            total: 2,
        },
        ..SessionState::default()
    };
    let feed = feed(Some(fleet), Some(state));
    let ui = Ui {
        agent: Some("a2".into()),
        ..Ui::default()
    };
    let (tab, count) = with(&feed, &ui, |cx| {
        (agents_tab(cx, &State::default()), badge(cx))
    });
    let count = count.expect("the tab counts the agents at work");
    assert_eq!(count.hue, Hue::You, "{count:?}");
    assert!(count.text.contains("▲1"), "{count:?}");
    let row = tab
        .rows
        .iter()
        .find(|row| row.child == "a2")
        .expect("the asking child is listed");
    assert!(row.asks);
    assert_eq!(row.hue, Hue::You);
    assert_eq!(row.card.as_deref(), Some("card-7"));
    let detail = tab.detail.expect("the selected child shows its detail");
    let answer = detail
        .buttons
        .iter()
        .find(|button| button.id == "btn-child-answer")
        .expect("an asking child offers Answer");
    assert!(answer.unavailable.is_none(), "{answer:?}");
    assert_eq!(
        answer.action,
        Action::Dock(DockAction::Bring {
            session: Id::from(ID),
            card: "card-7".into(),
        })
    );
}
