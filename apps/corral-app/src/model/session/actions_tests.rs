use corral_protocol::session_state::{Capability, SessionState};

use super::{Cx, Facts, SessionRole, Ui, actions, slash};
use crate::model::commands;
use crate::model::daemon::Feed;
use crate::model::health::daemon_env;
use crate::model::nav::Form;
use crate::model::session::transcript::LiveSession;
use crate::model::{DataState, Id};

const WAIT: &str = "Wait for the current turn to finish";

/// A build whose host declares Compact but cannot run it while a turn is open.
fn working_build(id: &Id) -> Feed {
    let mut feed = Feed::default();
    let mut live = LiveSession::default();
    live.replace_state(Some(Box::new(SessionState {
        open: true,
        can: vec![
            Capability {
                id: "send".into(),
                unavailable: None,
            },
            Capability {
                id: "compact".into(),
                unavailable: Some(WAIT.into()),
            },
        ],
        ..SessionState::default()
    })));
    feed.live.insert(id.clone(), live);
    feed
}

/// An action the host marks unavailable was dropped from the catalog, or ran anyway from the
/// palette or the `/` menu. It must stay listed with the host's reason in all three, and choosing
/// it must run nothing.
#[test]
fn an_unavailable_action_stays_listed_with_its_reason_and_does_not_run() {
    let id = Id::from("B-k3x9");
    let feed = working_build(&id);
    let facts = Facts {
        role: SessionRole::Build,
        task: None,
    };
    let ui = Ui {
        draft: "/compact".into(),
        ..Ui::default()
    };
    let cx = Cx::new(&feed, &ui, &id, daemon_env(&feed, Form::Desktop), &facts);
    let catalog = actions(&cx);

    let compact = catalog
        .rows()
        .find(|row| row.id == "act-compact")
        .expect("Compact stays in the catalog while the host cannot run it");
    assert_eq!(compact.unavailable.as_deref(), Some(WAIT));

    let menu = slash(&cx, &catalog).expect("`/compact` opens the menu");
    let listed = menu
        .rows
        .iter()
        .find(|row| row.command == "/compact")
        .expect("the `/` menu lists Compact");
    assert_eq!(listed.unavailable.as_deref(), Some(WAIT));

    let palette = commands::filtered(
        "",
        true,
        DataState::Daemon,
        &feed,
        None,
        Form::Desktop,
        Some(&catalog),
    );
    let row = palette
        .iter()
        .find(|row| row.id == "session:act-compact")
        .expect("the palette lists Compact");
    assert!(row.unavailable && row.label.contains(WAIT));
    assert_eq!(
        commands::run(
            &row.id,
            "",
            true,
            DataState::Daemon,
            &feed,
            None,
            Form::Desktop,
            Some(&catalog),
        ),
        None,
        "choosing an unavailable row runs nothing"
    );
}
