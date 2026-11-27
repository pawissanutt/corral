use corral_protocol::session_state::{Capability, SessionState};

use super::{KeyAction, WEB_RESERVED, bound, press};
use crate::model::agents::Control;
use crate::model::daemon::Feed;
use crate::model::health::daemon_env;
use crate::model::nav::Form;
use crate::model::session::transcript::LiveSession;
use crate::model::session::{Action, Cx, Facts, SessionRole, State};
use crate::model::{Effect, Id};

/// Esc on an idle, empty composer did nothing, so a chat could not be rewound from its page: the
/// first press must only arm, the second must ask the host for its turns.
#[test]
fn esc_twice_on_an_idle_empty_composer_asks_for_the_rewind_list() {
    let id = Id::from("A-1");
    let mut feed = Feed::default();
    let mut live = LiveSession::default();
    live.replace_state(Some(Box::new(SessionState {
        can: vec![Capability {
            id: "rewind".into(),
            unavailable: None,
        }],
        ..SessionState::default()
    })));
    feed.live.insert(id.clone(), live);
    let facts = Facts {
        role: SessionRole::Assistant,
        task: None,
    };
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
