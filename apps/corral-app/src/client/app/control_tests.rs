use super::*;
use crate::model::agents::{self, Control};
use crate::model::hue::Hue;

/// corrald refuses Pause for a session without a pi host; the session shows the refusal with
/// corrald's reason, and it is not taken for a connection problem.
#[test]
fn pause_on_a_session_without_a_pi_host_shows_corralds_reason() {
    let runtime = nsh_core::element::Runtime::default();
    let _entered = runtime.enter();
    let feed = signal(Feed::default());
    let mut app = AppState::new();
    app.ready = true;
    let sent = app.operation(
        feed,
        "pause-1".into(),
        control::SESSION_PAUSE,
        Control::Pause.params("T-1_R1"),
    );
    assert_eq!(sent.len(), 1);
    let reason = "session has no Pi socket; use Terminate to stop its host";
    let refusal = Refusal {
        reason: reason.into(),
        next: Vec::new(),
    };
    app.event(
        feed,
        Event::Reply {
            method: control::SESSION_PAUSE,
            tag: Some("pause-1".into()),
            params: sent[0].1.clone(),
            outcome: Err((corral_protocol::rpc::ErrorCode::Refused, reason.into())),
            error_data: Some(serde_json::to_value(&refusal).unwrap()),
        },
    )
    .unwrap();
    let mut state = agents::State::default();
    state.control_sent(
        Id::from("T-1_R1"),
        Id::from("pause-1"),
        Control::Pause,
        None,
    );
    let shown = feed.with_untracked(|feed| agents::control_status(&state, "T-1_R1", feed));
    assert_eq!(shown, Some((Hue::Fail, format!("Pause refused: {reason}"))));
    assert_eq!(feed.with_untracked(|feed| feed.problem.clone()), None);
}
