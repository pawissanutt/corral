//! Nested Code Mode calls (T-683): a child names its parent cell and transcript call, its request
//! ID is derived from them, its receipt keeps that identity, and a replay never repeats the effect.
use serde_json::{Value, json};

use super::test_support::{self as ts, Host, code, committed, envelope, hello, send};
use super::{BindTarget, CallResponse, tool_request_id};

fn init() -> Value {
    json!({"op":"init","phases":[{"id":"work","label":"Work","steps":[
        {"id":"build","label":"Build the feature"}
    ]}]})
}

fn child(host: &Host, call: &str) -> (Host, String) {
    let mut nested = host.clone();
    nested.identity.parent = Some(
        serde_json::from_value(json!({
            "tool_call_id": "call_code|fc_1", "cell": "cell-2", "call": call,
        }))
        .unwrap(),
    );
    let key = format!("call_code|fc_1/{call}");
    let request = tool_request_id(&host.identity.session_id, &key);
    (nested, request)
}

fn bound_worker(fx: &ts::Fx) -> (String, Host) {
    let task = ts::worker_task(fx, "p1");
    let target = BindTarget::Worker {
        task: task.clone(),
        run: "R1".into(),
    };
    let host = ts::bind(fx, &target, "p1", "worker");
    committed(&hello(fx, &host));
    (task, host)
}

#[test]
fn a_nested_mutation_keeps_its_parent_and_replays_its_one_effect() {
    // Given a bound worker whose Code Mode cell makes its first nested call.
    let fx = ts::fx();
    let (task, host) = bound_worker(&fx);
    let (nested, request) = child(&host, "cell-2.1");
    let input = envelope(&nested, "todo", &request, init(), false);

    // When the child commits and the same child identity is delivered again.
    let first = committed(&send(&fx, &input, "todo"));
    let again = send(&fx, &input, "todo");

    // Then the receipt names the parent cell and call, the replay is that receipt, and the
    // canonical ledger moved once.
    let parent = first.identity.parent.clone().unwrap();
    assert_eq!(
        (parent.cell.as_str(), parent.call.as_str()),
        ("cell-2", "cell-2.1")
    );
    assert_eq!(parent.child_key(), "call_code|fc_1/cell-2.1");
    assert!(matches!(
        again,
        CallResponse::Receipt { replayed: true, .. }
    ));
    assert_eq!(again.receipt(), Some(&first));
    let progress = fx
        .ctx
        .store
        .get(&task)
        .unwrap()
        .current_run()
        .unwrap()
        .progress
        .clone()
        .unwrap();
    assert_eq!(progress.revision, 1);
    assert_eq!(
        ts::lookup(&fx, &host, &request).and_then(|r| r.identity.parent),
        Some(parent)
    );

    // And a sibling child is a distinct request that never reuses the first receipt.
    let (sibling, other) = child(&host, "cell-2.2");
    assert_ne!(other, request);
    match send(
        &fx,
        &envelope(&sibling, "todo", &other, init(), false),
        "todo",
    ) {
        CallResponse::Receipt { receipt, replayed } => {
            assert!(!replayed);
            assert_eq!(receipt.request_id, other);
            assert_eq!(
                receipt.identity.parent.map(|p| p.call),
                Some("cell-2.2".into())
            );
        }
        refused => assert_ne!(code(&refused), Some("payload_mismatch")),
    }
}

#[test]
fn a_parent_must_bind_the_request_id_and_never_rides_a_lifecycle_call() {
    let fx = ts::fx();
    let (task, host) = bound_worker(&fx);
    let before = ts::task_bytes(&fx, &task);
    let (nested, request) = child(&host, "cell-2.1");
    // A request ID that is not derived from the parent, a call outside its cell, and a parent on
    // a lifecycle event are all malformed, and none changes the run.
    let forged = tool_request_id(&host.identity.session_id, "call_code|fc_1/2");
    let (outside, _) = child(&host, "cell-3.1");
    let mut event = nested.clone();
    event.identity.turn = Some(1);
    for (input, op) in [
        (envelope(&nested, "todo", &forged, init(), false), "todo"),
        (envelope(&outside, "todo", &request, init(), false), "todo"),
        (
            envelope(
                &event,
                "event",
                "ev-1-1",
                json!({"kind":"turn_started","turn":1}),
                false,
            ),
            "event",
        ),
    ] {
        assert_eq!(code(&send(&fx, &input, op)), Some("bad_request"));
    }
    assert_eq!(ts::task_bytes(&fx, &task), before);
}
