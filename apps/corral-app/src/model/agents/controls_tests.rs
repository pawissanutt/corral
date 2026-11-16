use corral_protocol::assistant::AssistantStop;
use corral_protocol::cmd::Command;
use corral_protocol::cmd::session::SessionCommand;
use corral_protocol::rows::{AgentRow, AgentStatus, ControlState, SessionControls};

use super::super::{Action, State};
use super::ControlKind;
use crate::model::daemon::Feed;
use crate::model::effect::Effect;
use crate::model::health::daemon_env;
use crate::model::{Form, Id, Selection};

const OFFERED: ControlState = ControlState {
    available: true,
    refusal: None,
};

fn row(id: &str, role: &str, controls: SessionControls) -> AgentRow {
    AgentRow {
        id: id.into(),
        role: role.into(),
        task: None,
        run: None,
        title: "Session".into(),
        profile: None,
        runtime: Some("corral-pi".into()),
        machine: None,
        pane: None,
        status: AgentStatus::Working,
        now: String::new(),
        since: None,
        tail: Vec::new(),
        last_output_at: None,
        activity: None,
        steps: None,
        cost_usd: None,
        cost_basis: None,
        tokens: None,
        rev: 1,
        generation: Some(2),
        turn: Some(4),
        controls,
    }
}

/// Presses `kind` on the bar of session `id` in `feed`.
fn press(feed: &Feed, id: &str, kind: ControlKind) -> Vec<Effect> {
    let selection = Selection {
        session: Some(Id::from(id)),
        ..Selection::default()
    };
    State::default().act_live(
        Action::Control(kind),
        feed,
        &selection,
        daemon_env(feed, Form::Desktop),
    )
}

#[test]
fn a_control_the_daemon_refused_is_not_sent() {
    let mut feed = Feed::default();
    let controls = SessionControls {
        pause: ControlState {
            available: false,
            refusal: Some("no_bridge".into()),
        },
        ..SessionControls::default()
    };
    feed.agents.insert(
        Id::from("T-1/run/R1"),
        row("T-1/run/R1", "worker", controls),
    );
    assert!(press(&feed, "T-1/run/R1", ControlKind::Pause).is_empty());
    assert!(press(&feed, "T-1/run/R1", ControlKind::Cancel).is_empty());
}

#[test]
fn terminate_is_never_sent_without_a_confirmation() {
    let mut feed = Feed::default();
    let controls = SessionControls {
        stop: OFFERED,
        ..SessionControls::default()
    };
    feed.agents.insert(
        Id::from("T-1/run/R1"),
        row("T-1/run/R1", "worker", controls),
    );
    let effects = press(&feed, "T-1/run/R1", ControlKind::Stop);
    let [Effect::Act(act)] = effects.as_slice() else {
        panic!("Terminate sends one command, got {effects:?}");
    };
    assert!(matches!(
        act.command,
        Command::Session(SessionCommand::Stop { .. })
    ));
    assert!(act.confirm);
}

#[test]
fn an_assistant_cancel_is_not_sent_as_a_session_command() {
    let mut feed = Feed::default();
    let controls = SessionControls {
        cancel: OFFERED,
        ..SessionControls::default()
    };
    feed.agents
        .insert(Id::from("A-3"), row("A-3", "assistant", controls));
    let effects = press(&feed, "A-3", ControlKind::Cancel);
    assert_eq!(
        effects,
        vec![Effect::Stop(AssistantStop {
            session: "A-3".into(),
            turn: 4,
        })]
    );
}
