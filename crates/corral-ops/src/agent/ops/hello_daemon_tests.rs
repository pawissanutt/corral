//! Regression: pane-less hello is an explicit generation binding, not a pane exemption.
use serde_json::json;

use crate::agent::admission::{self, BindTarget};
use crate::agent::test_support::*;
use corral_core::agent::paths;

fn hello_with_pane(fx: &Fx, host: &Host, pane: Option<&str>) -> crate::agent::CallResponse {
    let payload = json!({
        "ctl_cap_sha256": paths::sha256_hex(host.ctl_cap.as_bytes()),
        "session_file": host.sessions.join("session.jsonl"),
        "pid": 4242, "start_time": "123456", "pane": pane,
    });
    let request = format!("bind-host-{}", host.identity.generation);
    send(
        fx,
        &envelope(host, "hello", &request, payload, true),
        "hello",
    )
}

#[test]
fn daemon_hello_requires_absent_pane_and_terminal_hello_still_requires_exact_pane() {
    let fx = fx();
    let session = fx
        .ctx
        .store
        .create_assistant("pi", None, None, None)
        .unwrap();
    let target = BindTarget::Assistant {
        session: session.id.clone(),
    };
    let mut host = bind(&fx, &target, "owner-pane", "assistant-session");
    assert_eq!(
        code(&hello_with_pane(&fx, &host, None)),
        Some("pane_mismatch")
    );
    committed(&hello_with_pane(&fx, &host, Some("owner-pane")));

    let grant =
        admission::open_generation_host(&fx.ctx.store, &target, None, false, None, None, false)
            .unwrap();
    host.identity.generation = grant.generation;
    host.nonce = grant.bind_nonce;
    host.host_cap = secret("remote-host", grant.generation);
    host.ctl_cap = secret("remote-ctl", grant.generation);
    assert_eq!(
        code(&hello_with_pane(&fx, &host, Some("owner-pane"))),
        Some("pane_mismatch")
    );
    committed(&hello_with_pane(&fx, &host, None));
    let saved = fx.ctx.store.get_assistant_container(&session.id).unwrap();
    assert!(saved.pane.is_none());
    assert!(saved.state.current_generation().unwrap().daemon_hosted);
    committed(&send(
        &fx,
        &event_input(&host, 1, "turn_started", 1),
        "event",
    ));
    admission::fence_generation(
        &fx.ctx.store,
        &target.container(),
        grant.generation,
        "stopped",
    )
    .unwrap();
    assert_eq!(
        code(&send(&fx, &event_input(&host, 2, "turn_ended", 1), "event")),
        Some("agent_fenced")
    );
}
