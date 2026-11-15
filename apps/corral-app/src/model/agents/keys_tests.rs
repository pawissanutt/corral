use corral_protocol::app::{AppSession, DeviceAuthority, SeenCursor};
use corral_protocol::cmd::Command;
use corral_protocol::cmd::session::SessionCommand;
use corral_protocol::device::{DeviceInfo, DevicePlatform};
use corral_protocol::rows::{AgentRow, AgentStatus, ControlState, SessionControls};

use super::super::{Action, State};
use super::{KeyPress, Keys, keys};
use crate::model::daemon::{
    CommandState, CommandStatus, DaemonHealth, DaemonLevel, Feed, Link, PmLevel,
};
use crate::model::effect::Effect;
use crate::model::health::daemon_env;
use crate::model::{Form, Id, Selection};

const ID: &str = "T-1/run/R1";

/// A foreign harness's session in a pane, with corrald's `keys` control as given, seen from an
/// admin device after the welcome, authenticated app session and complete board have arrived.
fn feed(keys: ControlState) -> Feed {
    let mut feed = Feed::default();
    feed.welcome_received(
        "/workspace/.corral".into(),
        DaemonHealth {
            level: DaemonLevel::Healthy,
            reasons: Vec::new(),
            runtimes: Vec::new(),
            pm: PmLevel::Absent,
            pass_seq: 1,
            last_pass: None,
            last_pass_ms: None,
            version: "test".into(),
        },
    );
    feed.session = Some(AppSession {
        device: DeviceInfo {
            id: "dev-1".into(),
            name: "desk".into(),
            platform: DevicePlatform::Native,
            app_version: "test".into(),
            enrolled_at: "2026-11-15T10:00:00Z".into(),
            revoked_at: None,
            authority: None,
            last_seen: None,
            reach: None,
        },
        authority: DeviceAuthority {
            reply: true,
            act: true,
            assistant: true,
            assistant_act: true,
            admin: true,
        },
        seen: SeenCursor::default(),
        server_time: "2026-11-15T10:00:00Z".into(),
    });
    feed.board_received(1, Vec::new(), &[], true);
    assert!(feed.ready());
    feed.agents.insert(
        Id::from(ID),
        AgentRow {
            id: ID.into(),
            role: "worker".into(),
            task: Some("T-1".into()),
            run: Some("R1".into()),
            title: "Foreign harness".into(),
            profile: None,
            runtime: Some("claude".into()),
            machine: None,
            pane: Some("w1:p1".into()),
            status: AgentStatus::Working,
            now: String::new(),
            since: None,
            tail: Vec::new(),
            cost_usd: None,
            cost_basis: None,
            tokens: None,
            rev: 1,
            generation: None,
            turn: None,
            controls: SessionControls {
                keys,
                ..SessionControls::default()
            },
        },
    );
    feed
}

fn press(state: &mut State, feed: &Feed, press: KeyPress) -> Vec<Effect> {
    let selection = Selection {
        session: Some(Id::from(ID)),
        ..Selection::default()
    };
    state.act_live(
        Action::Keys(press),
        feed,
        &selection,
        daemon_env(feed, Form::Desktop),
    )
}

#[test]
fn keys_refused_on_the_row_are_neither_offered_nor_sent() {
    let feed = feed(ControlState {
        available: false,
        refusal: Some("has_bridge".into()),
    });
    let mut state = State::default();
    let env = daemon_env(&feed, Form::Desktop);
    let row = &feed.agents[ID];
    assert!(keys(&state, &Id::from(ID), row, &feed, env).is_none());
    assert!(press(&mut state, &feed, KeyPress::Add("enter")).is_empty());
    assert!(press(&mut state, &feed, KeyPress::Send("ls".into())).is_empty());
}

#[test]
fn keys_wait_for_the_authenticated_projection_even_if_the_link_says_up() {
    for missing in ["welcome", "session", "board"] {
        let mut feed = feed(ControlState {
            available: true,
            refusal: None,
        });
        match missing {
            "welcome" => feed.transport_ready = false,
            "session" => feed.session = None,
            "board" => feed.projection_ready = false,
            _ => unreachable!(),
        }
        assert_eq!(feed.link, Link::Up);
        assert!(!feed.ready());
        let mut state = State::default();
        assert!(press(&mut state, &feed, KeyPress::Add("ctrl-c")).is_empty());
        assert!(press(&mut state, &feed, KeyPress::Send("ls".into())).is_empty());
        let env = daemon_env(&feed, Form::Desktop);
        let Some(Keys::Open(bar)) = keys(&state, &Id::from(ID), &feed.agents[ID], &feed, env)
        else {
            panic!("the offered keys bar remains visible while waiting for readiness");
        };
        assert!(!bar.enabled);
    }
}

#[test]
fn a_second_send_while_one_is_in_flight_sends_nothing() {
    let mut feed = feed(ControlState {
        available: true,
        refusal: None,
    });
    let mut state = State::default();
    press(&mut state, &feed, KeyPress::Add("ctrl-c"));
    let effects = press(&mut state, &feed, KeyPress::Send(String::new()));
    let [Effect::Act(act)] = effects.as_slice() else {
        panic!("one keys command, got {effects:?}");
    };
    assert!(matches!(
        &act.command,
        Command::Session(SessionCommand::Keys { keys, text: None, .. }) if keys == &["ctrl-c"]
    ));
    feed.commands.insert(
        act.key.clone(),
        CommandState {
            device: None,
            method: "cmd.submit".into(),
            request: serde_json::Value::Null,
            status: CommandStatus::Pending,
            result: None,
            error: None,
            entry: None,
        },
    );
    assert!(press(&mut state, &feed, KeyPress::Send("next".into())).is_empty());
    let env = daemon_env(&feed, Form::Desktop);
    let Some(Keys::Open(bar)) = keys(&state, &Id::from(ID), &feed.agents[ID], &feed, env) else {
        panic!("keys are offered");
    };
    assert!(!bar.enabled);
}

#[test]
fn a_key_outside_the_daemon_list_is_never_sent() {
    let feed = feed(ControlState {
        available: true,
        refusal: None,
    });
    let mut state = State::default();
    press(&mut state, &feed, KeyPress::Add("f13"));
    assert!(press(&mut state, &feed, KeyPress::Send(String::new())).is_empty());
}
