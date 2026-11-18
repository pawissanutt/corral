use super::*;
use corral_types::{Next, Op};

/// A refused `op.submit` keeps the engine's typed refusal on the operation, so the app can
/// offer each next step; it is not reduced to its message.
#[test]
fn a_refused_submit_keeps_the_typed_next_steps() {
    let runtime = nsh_core::element::Runtime::default();
    let _entered = runtime.enter();
    let feed = signal(Feed::default());
    let mut app = AppState::new();
    app.ready = true;
    let dispatch = Op::Dispatch {
        id: "T-42".into(),
        profile: None,
        base: None,
    };
    let sent = app.operation(
        feed,
        "op-1".into(),
        OP_SUBMIT,
        json!({"op": dispatch, "force": false}),
    );
    assert_eq!(sent.len(), 1);
    let refusal = Refusal {
        reason: "T-41 is not done".into(),
        next: vec![Next::force(dispatch)],
    };
    app.event(
        feed,
        Event::Reply {
            method: OP_SUBMIT,
            tag: Some("op-1".into()),
            params: sent[0].1.clone(),
            outcome: Err((
                corral_protocol::rpc::ErrorCode::Refused,
                refusal.reason.clone(),
            )),
            error_data: Some(serde_json::to_value(&refusal).unwrap()),
        },
    )
    .unwrap();
    assert_eq!(
        feed.with_untracked(|feed| crate::model::act::status(feed, "op-1")),
        Some(crate::model::ActStatus::Refused(refusal))
    );
}
