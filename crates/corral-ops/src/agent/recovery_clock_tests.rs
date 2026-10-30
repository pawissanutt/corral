//! Generation boundaries are session positions, not cross-process wall-clock comparisons.
use serde_json::json;

use super::tests::{classes, resume, run_recipient, state};
use crate::agent::bridge::fixture::{self as f, HostProc, SESSION};

fn backward_clock(tool: &str) {
    let fx = f::fixture(false, 5);
    let mut host = HostProc::spawn(true);
    let bound = fx.bind(&fx.worker_target(), "w0:p1", &host);
    let _control = f::bridge(
        &fx.ctx.store,
        &run_recipient(&fx),
        1,
        Some(f::CAP),
        f::healthy,
    );
    let binding = state(&fx, &fx.worker()).generation(1).unwrap().clone();
    let created = binding.created;
    let earlier = created - jiff::SignedDuration::from_secs(60);
    let marker = |generation, nonce: &str| {
        json!({"type":"custom", "customType":"corral_host_bound",
        "timestamp": earlier, "data":{"generation":generation,"session_id":SESSION,
            "bind_nonce_sha256":nonce}})
    };
    let call = |id, tool| {
        json!({"type":"message", "timestamp":earlier,
        "message":{"role":"assistant", "content":[{"type":"toolCall", "id":id,
            "name":tool, "arguments":{}}]}})
    };
    let session = [
        marker(1, "previous-child-g1"),
        call("old-edit", "edit"),
        marker(2, "previous-child-g2"),
        call("old-bash", "bash"),
        marker(1, &binding.bind_nonce_sha256),
        call("interrupted", tool),
        call("completed", tool),
        json!({"type":"message", "timestamp":earlier,
            "message":{"role":"toolResult", "toolCallId":"completed", "content":[]}}),
    ]
    .iter()
    .map(|entry| format!("{entry}\n"))
    .collect::<String>();
    f::write_session(&bound, &session);
    host.kill();

    let outcome = resume(&fx, &fx.worker(), false).unwrap();
    assert_eq!(
        classes(&outcome["calls"]),
        [(tool.into(), "outcome_unknown".into())]
    );
    assert_eq!(outcome["calls"][0]["tool_call_id"], "interrupted");
    assert_eq!(
        std::fs::read_to_string(f::session_file(&bound)).unwrap(),
        session
    );
}

#[test]
fn edit_after_bind_survives_a_backward_wall_clock_step() {
    backward_clock("edit");
}

#[test]
fn bash_after_bind_survives_a_backward_wall_clock_step() {
    backward_clock("bash");
}
