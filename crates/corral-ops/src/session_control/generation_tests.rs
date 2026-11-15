use crate::agent::bridge::fixture::{self as f, HostProc};
use corral_core::TaskRef;
use corral_core::inbox::Recipient;

#[test]
fn stale_generation_cancel_refuses_before_touching_host() {
    let fx = f::fixture(false, 1);
    let host = HostProc::spawn(true);
    let bound = fx.bind(&fx.worker_target(), "w0:p1", &host);
    let to = Recipient::Run {
        task: fx.id.clone(),
        run: "R1".into(),
    };
    let bridge = f::bridge(
        &fx.ctx.store,
        &to,
        bound.identity.generation,
        Some(&bound.ctl_cap),
        |_| serde_json::json!({"result": {"cancelled": true, "turn": 1}}),
    );
    let id = format!("{}/run/R1", fx.id);
    let target = super::resolve(&fx.ctx, &id).unwrap();
    let before = fx
        .ctx
        .store
        .head(&fx.id)
        .unwrap()
        .agent_head(&fx.worker())
        .unwrap();
    let error = super::cancel::cancel(&fx.ctx, &target, Some(bound.identity.generation + 1), None)
        .unwrap_err();
    assert!(error.to_string().starts_with("conflict:"));
    assert!(matches!(
        bridge.frames.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    ));
    assert_eq!(
        fx.ctx
            .store
            .head(&fx.id)
            .unwrap()
            .agent_head(&fx.worker())
            .unwrap(),
        before
    );
}
