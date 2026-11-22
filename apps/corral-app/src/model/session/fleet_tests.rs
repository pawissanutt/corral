use corral_protocol::detail::CardKind;
use corral_protocol::session_state::{CardBrief, Counts, SessionState};
use corral_protocol::subscription::{SessionAgentMessage, SessionChild, SessionFleet};

use super::{AgentsTab, agents_tab};
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
fn selecting_a_stopped_parents_child_requests_its_saved_detail() {
    let feed = feed(None, None);
    let ui = Ui {
        agent: Some("a".into()),
        ..Ui::default()
    };
    with(&feed, &ui, |cx| {
        let cx = Cx {
            host: super::Host::NotRunning,
            ..*cx
        };
        assert!(
            super::fetches(&cx, &State::default())
                .contains(&Action::Refresh(super::child_target(cx.id, "a")))
        );
    });
}

#[test]
fn a_typed_agent_id_opens_only_what_corrald_would_read_under_this_session() {
    let lookup = shown(None).lookup;
    let opens = |child: &str| {
        Ok(Action::Fleet(super::FleetAction::Open {
            session: Id::from(ID),
            child: child.into(),
        }))
    };
    assert_eq!(lookup.check(" a2 "), opens("a2"));
    assert_eq!(lookup.check("B-k3x9/child/rev:1.b-c"), opens("rev:1.b-c"));
    assert_eq!(lookup.check(&"a".repeat(128)), opens(&"a".repeat(128)));
    assert_eq!(lookup.check("  "), Err(None));
    for malformed in [
        "..",
        ".",
        "a/b",
        "../a",
        "B-k3x9/child/",
        "B-k3x9/child/a/child/b",
        "a b",
        "ä",
        &"a".repeat(129),
        "B-other/child/a2",
    ] {
        assert!(
            matches!(lookup.check(malformed), Err(Some(_))),
            "{malformed:?}"
        );
    }
}

#[test]
fn an_agent_opened_by_id_outside_the_fleet_is_read_only_and_claims_no_record_corrald_lacks() {
    use crate::model::daemon::{self, ClientError, DetailCache};
    use crate::model::session::Opened;

    let mut feed = feed(None, None);
    let mut state = State::default();
    let open = shown(None)
        .lookup
        .check("a9")
        .expect("a well-formed id opens");
    let effects = state.act(open, &feed);
    assert!(effects.is_empty(), "{effects:?}");
    let ui = state.ui(ID).clone();
    assert_eq!(ui.to, None, "an unlisted agent never becomes the receiver");
    let conversation = |feed: &Feed| {
        let tab = with(feed, &ui, |cx| agents_tab(cx, &state));
        assert!(tab.rows.is_empty());
        match tab.opened {
            Some(Opened::Conversation(conversation)) => *conversation,
            other => panic!("the conversation opens without a row: {other:?}"),
        }
    };
    let reading = conversation(&feed);
    assert!(reading.buttons.is_empty() && reading.receiver.is_none());
    let wanted = with(&feed, &ui, |cx| super::fetches(cx, &state));
    assert!(wanted.contains(&Action::Refresh(super::child_target(&Id::from(ID), "a9"))));

    let target = super::child_target(&Id::from(ID), "a9");
    feed.details.insert(
        daemon::detail_key(&target),
        DetailCache {
            transcript_seams: Default::default(),
            target,
            pages: Vec::new(),
            loading: false,
            stale: false,
            error: Some(ClientError {
                code: "not_found".into(),
                message: "child session not found".into(),
            }),
            merged: None,
        },
    );
    let missing = conversation(&feed);
    assert_ne!(missing.state, reading.state);
    assert_ne!(missing.state, "done");
    assert!(missing.buttons.is_empty() && missing.end.is_none());

    let key = with(&feed, &ui, |cx| super::press("i", cx)).expect("i opens the id field");
    let effects = state.act(key, &feed);
    assert!(state.ui(ID).sub.is_none(), "the field is on the list");
    assert!(
        effects.contains(&crate::model::effect::Effect::Focus(
            shown(None).lookup.field
        )),
        "{effects:?}"
    );
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
    let tab = with(&feed, &ui, |cx| agents_tab(cx, &State::default()));
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

#[test]
fn the_part_of_the_agents_tab_buttons_are_built_from_is_equal_across_a_clock_tick() {
    use corral_protocol::detail::{DetailBody, DetailRef, SessionJob, SessionJobs};

    use crate::model::daemon::{self, DetailCache};
    use crate::model::session::{FleetUi, SubView};

    let started: jiff::Timestamp = "2026-11-19T10:00:00Z".parse().expect("a test time parses");
    let started_s = u64::try_from(started.as_second()).expect("after the epoch");
    let running = SessionChild {
        state: "running".into(),
        started_at: Some(started_s * 1_000),
        ended_at: None,
        steerable: true,
        activity: Some("running tests".into()),
        ..child("a1", Some(0.21))
    };
    let fleet = SessionFleet {
        children: vec![running, child("a0", Some(0.33))],
        messages: vec![],
        messages_total: 0,
        max_parallel: 2,
    };
    let jobs = DetailRef::SessionJobs { id: ID.into() };
    let at = |now: u64, ui: &Ui| {
        let mut feed = feed(Some(fleet.clone()), None);
        feed.now = now;
        feed.details.insert(
            daemon::detail_key(&jobs),
            DetailCache {
                transcript_seams: Default::default(),
                target: jobs.clone(),
                pages: Vec::new(),
                loading: false,
                stale: false,
                error: None,
                merged: Some(DetailBody::SessionJobs(SessionJobs {
                    jobs: vec![SessionJob {
                        id: "j1".into(),
                        command: "cargo test".into(),
                        dir: None,
                        state: "running".into(),
                        started_at: Some(started.to_string()),
                        ended_at: None,
                        exit_code: None,
                        output_bytes: 2_048,
                        tail: vec!["running 12 tests".into()],
                        monitors: Vec::new(),
                        actions: vec!["stop".into(), "background".into()],
                    }],
                    unavailable: None,
                })),
            },
        );
        with(&feed, ui, |cx| agents_tab(cx, &State::default()))
    };
    let list = Ui {
        agent: Some("a1".into()),
        ..Ui::default()
    };
    let conversation = Ui {
        agent: Some("a1".into()),
        sub: Some(SubView::Agent("a1".into())),
        ..Ui::default()
    };
    let job = Ui {
        fleet: FleetUi {
            job: Some("j1".into()),
            ..FleetUi::default()
        },
        sub: Some(SubView::Job("j1".into())),
        ..Ui::default()
    };
    for (view, ui) in [("list", list), ("conversation", conversation), ("job", job)] {
        let before = at(started_s + 30, &ui);
        let after = at(started_s + 31, &ui);
        assert_ne!(before.clock, after.clock, "{view}: the clock moved");
        assert_eq!(before.steady(), after.steady(), "{view}");
    }
}
