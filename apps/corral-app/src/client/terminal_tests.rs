use super::*;
use serde_json::json;

fn frame(value: Value) -> Frame {
    Frame {
        kind: FrameKind::Control,
        stream: 0,
        payload: serde_json::to_vec(&value).unwrap(),
    }
}

fn outgoing(core: &mut Core) -> Value {
    serde_json::from_slice(&core.take_outbox().unwrap().payload).unwrap()
}

fn capture(scope: &str, epoch: u64) -> Value {
    json!({"jsonrpc":"2.0", "method":"sub.delta", "params": {
        "topic":"terminal", "scope":scope, "epoch":epoch,
        "snapshot":{"lines":["screen"], "rows":[], "captured_at":"2026-11-15T01:00:00Z", "live":true},
        "cursor":{"rev":epoch,"after":null}, "more":false, "error":null
    }})
}

fn open(core: &mut Core, scope: &str) {
    core.request(
        wire::SUB_OPEN,
        json!({"topic":"terminal","scope":scope,"since":null}),
        0,
    )
    .unwrap();
}

#[test]
fn terminal_captures_ack_their_scope_and_reopen_only_after_the_close_barrier() {
    let mut core = Core::new();
    open(&mut core, "session-a");
    assert_eq!(outgoing(&mut core)["params"]["scope"], "session-a");
    open(&mut core, "session-b");
    outgoing(&mut core);
    for scope in ["session-a", "session-b"] {
        core.receive(frame(capture(scope, 1)), 1).unwrap();
        assert!(matches!(core.next_event(), Some(Event::Rows {
            delta: SubDelta::Terminal { scope: received, snapshot: Some(_), .. }, replacement: true, ..
        }) if received == scope));
        let ack = outgoing(&mut core);
        assert_eq!(ack["method"], wire::SUB_ACK);
        assert_eq!(ack["params"]["topic"], "terminal");
        assert_eq!(ack["params"]["scope"], scope);
        assert_eq!(ack["params"]["cursor"]["rev"], 1);
    }
    core.receive(frame(capture("session-a", 1)), 2).unwrap();
    assert!(matches!(
        core.next_event(),
        Some(Event::Rows {
            replacement: false,
            ..
        })
    ));
    outgoing(&mut core);
    core.request(
        wire::SUB_CLOSE,
        json!({"topic":"terminal","scope":"session-a"}),
        3,
    )
    .unwrap();
    let close = outgoing(&mut core);
    open(&mut core, "session-a");
    assert!(core.take_outbox().is_none());
    core.receive(frame(capture("session-a", 1)), 4).unwrap();
    assert!(core.next_event().is_none());
    assert!(core.take_outbox().is_none());
    core.receive(
        frame(json!({"jsonrpc":"2.0","id":close["id"],"result":{}})),
        5,
    )
    .unwrap();
    assert_eq!(outgoing(&mut core)["method"], wire::SUB_OPEN);
    core.receive(frame(capture("session-a", 1)), 6).unwrap();
    assert!(matches!(
        core.next_event(),
        Some(Event::Rows {
            replacement: true,
            ..
        })
    ));
    outgoing(&mut core);
    core.receive(
        frame(json!({"jsonrpc":"2.0","method":"sub.reset","params":{
            "topic":"terminal","scope":"session-a","epoch":2
        }})),
        7,
    )
    .unwrap();
    assert!(matches!(
        core.next_event(),
        Some(Event::Reset {
            topic: Topic::Terminal,
            ..
        })
    ));
    let reopen = outgoing(&mut core);
    assert_eq!(reopen["params"]["scope"], "session-a");
    core.receive(frame(capture("session-a", 2)), 8).unwrap();
    assert!(matches!(
        core.next_event(),
        Some(Event::Rows {
            replacement: true,
            ..
        })
    ));
    outgoing(&mut core);
    assert!(core.receive(frame(capture("session-a", 3)), 9).is_err());

    let mut reconnected = Core::new();
    open(&mut reconnected, "session-a");
    outgoing(&mut reconnected);
    reconnected
        .receive(frame(capture("session-a", 4)), 10)
        .unwrap();
    assert!(matches!(
        reconnected.next_event(),
        Some(Event::Rows {
            replacement: true,
            ..
        })
    ));
}

#[test]
fn terminal_stream_rejects_partial_windows_instead_of_publishing_an_empty_capture() {
    let mut core = Core::new();
    let mut partial = capture("session-a", 1);
    partial["params"]["more"] = Value::Bool(true);
    assert!(core.receive(frame(partial), 0).is_err());
    assert!(core.next_event().is_none());
    assert!(core.take_outbox().is_none());
}
