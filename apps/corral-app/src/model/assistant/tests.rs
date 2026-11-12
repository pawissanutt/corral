use super::*;
use crate::model::effect::DataMode;

#[test]
fn offline_send_queues_instead_of_refusing() {
    for conn in [Conn::Offline, Conn::Down] {
        for thread in ["A-7", "A-17"] {
            for target in [AskTarget::Assistant, AskTarget::Pm] {
                let mut state = State {
                    thread,
                    ..State::default()
                };
                let env = Env {
                    conn,
                    mode: DataMode::Live,
                    form: Form::Desktop,
                };
                let effects = if thread == "A-7" {
                    state.send("queue T-873", target, env)
                } else {
                    state.send_to_thread(thread, "queue T-873", env)
                };
                assert_eq!(state.sent.len(), 1);
                assert_eq!(state.sent[0].state, MessageState::QueuedOnDevice);
                assert!(effects.is_empty());
            }
        }
    }
}

#[test]
fn late_input_is_answerable_but_never_authoritative() {
    let items = thread(
        &State::default(),
        "A-7",
        Env {
            conn: Conn::Healthy,
            mode: DataMode::Live,
            form: Form::Desktop,
        },
    );
    assert!(!MessageState::Late.can_instruct());
    assert!(items.iter().any(|item| matches!(item, ThreadItem::Assistant(message) if message.at == "21:47" && message.body.iter().any(|body| matches!(body, Body::Paragraph(_))))));
    assert!(items.iter().any(|item| matches!(item, ThreadItem::Assistant(message) if message.at == "21:47" && message.body.iter().any(|body| matches!(body, Body::Action(ActionCard { class: ActionClass::Late, outcome: ActionOutcome::Late { .. }, .. }))))));
}

#[test]
fn model_cycle_wraps_without_losing_the_conversation() {
    let mut state = State {
        model: Some(MODELS[MODELS.len() - 1].name),
        last_q: Some("kept".into()),
        ..State::default()
    };
    state.cycle_model();
    assert_eq!(state.model_name(), MODELS[0].name);
    assert_eq!(state.last_q.as_deref(), Some("kept"));
}

#[test]
fn hotkey_send_keeps_the_actual_delivery_state() {
    let mut state = State::default();
    let env = Env {
        conn: Conn::Offline,
        mode: DataMode::Live,
        form: Form::Desktop,
    };
    state.send_from_hotkey("queue T-873", env);
    assert_eq!(state.hotkey, HotkeyState::Typing);
    assert_eq!(state.hotkey_sent, Some(MessageState::QueuedOnDevice));
    assert_eq!(
        hotkey_window(&state, env).sent.map(|chip| chip.hue),
        Some(crate::model::hue::Hue::Held)
    );
    state.hotkey = HotkeyState::Filed;
    let sent = state.sent.len();
    state.send_from_hotkey("  ", env);
    assert_eq!(state.hotkey, HotkeyState::Filed);
    assert_eq!(state.sent.len(), sent);
}
