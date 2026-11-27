use corral_protocol::session_state::{Capability, SessionState};
use corral_types::{Role, SessionRef, SessionStatus};

use super::{KeyAction, WEB_RESERVED, bound, press};
use crate::model::agents::Control;
use crate::model::agents::source_tests::agent;
use crate::model::daemon::Feed;
use crate::model::health::daemon_env;
use crate::model::nav::Form;
use crate::model::session::transcript::LiveSession;
use crate::model::session::{Action, Cx, Facts, SessionRole, State, Ui, actions, slash};
use crate::model::{Effect, Id};

/// An idle worker run `T-1_R1` on a Pi host that declares `rewind`, unavailable for `why` when
/// given: the session Pi checkpoints, so the one that can be rewound.
fn worker_run(why: Option<&str>) -> (Id, Feed, Facts) {
    let id = Id::from("T-1_R1");
    let mut feed = Feed::default();
    let mut run = agent("T-1", "R1", Role::Worker);
    run.status = SessionStatus::Idle;
    run.run.session = Some(SessionRef::Pi {
        socket: "/run/T-1-R1.sock".into(),
        inner: Box::new(SessionRef::Herdr {
            workspace: "w1".into(),
            pane: "w1:p1".into(),
        }),
        dir: "/runs/T-1/R1".into(),
    });
    feed.agents.insert(id.clone(), run);
    let mut live = LiveSession::default();
    live.replace_state(Some(Box::new(SessionState {
        can: vec![Capability {
            id: "rewind".into(),
            unavailable: why.map(str::to_owned),
        }],
        ..SessionState::default()
    })));
    feed.live.insert(id.clone(), live);
    let facts = Facts {
        role: SessionRole::Worker,
        task: Some(Id::from("T-1")),
    };
    (id, feed, facts)
}

/// The `/` menu read `/rewind 3` as the bare row and listed turns instead of going to turn 3,
/// and took any other text after `/rewind` as the list too.
#[test]
fn enter_on_the_open_menu_for_rewind_n_confirms_that_turn() {
    let (id, feed, facts) = worker_run(None);
    let run_of = |draft: &str| {
        let ui = Ui {
            draft: draft.into(),
            ..Ui::default()
        };
        let cx = Cx::new(&feed, &ui, &id, daemon_env(&feed, Form::Desktop), &facts);
        let menu = slash(&cx, &actions(&cx))?;
        Some(menu.rows.get(menu.at?)?.run.clone())
    };
    let run = run_of("/rewind 3").expect("`/rewind 3` opens the menu on Rewind");
    assert!(run.contains(&Action::Control {
        session: id.clone(),
        control: Control::rewind_turn(3),
    }));
    let listed = run_of("/rewind --list").expect("`/rewind --list` opens the menu on Rewind");
    assert!(listed.contains(&Action::Control {
        session: id.clone(),
        control: Control::Rewinds,
    }));
    assert_eq!(run_of("/rewind unrelated text"), None);
}

/// The ⋯ and `/` menus offered Rewind… as runnable while the host said it could not rewind now.
#[test]
fn rewind_is_muted_with_the_hosts_reason() {
    let why = "no checkpoint yet";
    let (id, feed, facts) = worker_run(Some(why));
    let ui = Ui::default();
    let cx = Cx::new(&feed, &ui, &id, daemon_env(&feed, Form::Desktop), &facts);
    let catalog = actions(&cx);
    let rewind = catalog
        .rows()
        .find(|row| row.id == "act-rewind")
        .expect("Rewind… stays listed");
    assert_eq!(rewind.unavailable.as_deref(), Some(why));
}

/// Esc on an idle, empty composer did nothing, so a worker run could not be rewound from its
/// page: the first press must only arm, the second must ask the host for its turns.
#[test]
fn esc_twice_on_an_idle_empty_composer_asks_for_the_rewind_list() {
    let (id, feed, facts) = worker_run(None);
    let mut state = State::default();
    let esc = |state: &mut State| {
        let cx = Cx::new(
            &feed,
            state.ui(id.as_str()),
            &id,
            daemon_env(&feed, Form::Desktop),
            &facts,
        );
        press("escape", &cx, false)
    };
    let first = esc(&mut state).expect("a first Esc arms the rewind");
    assert_eq!(first, Action::ArmRewind(id.clone()));
    state.act(first, &feed);
    let second = esc(&mut state).expect("a second Esc opens it");
    assert_eq!(
        second,
        Action::Control {
            session: id.clone(),
            control: Control::Rewinds
        }
    );
    let effects = state.act(second, &feed);
    assert!(matches!(
        effects.as_slice(),
        [Effect::Control {
            control: Control::Rewinds,
            ..
        }]
    ));
}

/// A session key that shadows one the browser keeps (F5 reload, Ctrl+L the address bar, Ctrl+T
/// and Ctrl+W tabs, Ctrl+P print…) either never reaches the page or takes the browser's own
/// command from the owner. The web build must bind none of them, whatever a part declares,
/// while the remapped keys that replace them are still bound.
#[test]
fn no_session_key_shadows_a_browser_reserved_key_on_the_web() {
    for form in [Form::Desktop, Form::Terminal] {
        let web = bound(form, true);
        for binding in &web {
            let key = binding.key.replace("mod+", "ctrl+");
            assert!(
                !WEB_RESERVED.contains(&key.as_str()),
                "{form:?}: {} shadows a browser key",
                binding.key
            );
        }
        for action in [
            KeyAction::Thinking,
            KeyAction::ExpandAll,
            KeyAction::Jump,
            KeyAction::Model,
            KeyAction::Effort,
        ] {
            assert!(
                web.iter().any(|binding| binding.action == action),
                "{form:?}: {action:?} lost its web binding"
            );
        }
    }
    // The native build keeps the terminal UI's F5 for the Task tab.
    assert!(
        bound(Form::Desktop, false)
            .iter()
            .any(|binding| binding.key == "f5")
    );
    assert!(bound(Form::Phone, true).is_empty());
}

/// A typed `/rewind 3` that the app did not open went to the host as text, where Pi's own
/// `/rewind` command restores the turn without the confirmation asked here; and the menu's
/// rewind asked the host while it said it could not rewind now.
#[test]
fn a_rewind_the_host_has_not_offered_never_leaves_the_app() {
    let send = |session: &Id| Action::Send {
        session: session.clone(),
        text: "/rewind 3".into(),
        policy: corral_protocol::session_state::SendPolicy::Steer,
        attachments: Vec::new(),
        restore: None,
    };
    let (id, feed, _) = worker_run(Some("no checkpoint yet"));
    let mut state = State::default();
    assert!(state.act(send(&id), &feed).is_empty());
    let menu = Action::Control {
        session: id.clone(),
        control: Control::rewind_turn(3),
    };
    assert!(state.act(menu, &feed).is_empty());
    let (id, mut feed, _) = worker_run(None);
    feed.live.remove(&id);
    assert!(State::default().act(send(&id), &feed).is_empty());
}
