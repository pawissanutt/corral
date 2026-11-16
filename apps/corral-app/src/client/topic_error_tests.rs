use super::*;
use crate::client::app::{self, AppState};
use crate::client::core::Event;
use crate::model::daemon::Feed;
use nsh_core::prelude::*;
use serde_json::json;

fn receive(core: &mut Core, value: Value) {
    core.receive(
        Frame {
            kind: FrameKind::Control,
            stream: 0,
            payload: serde_json::to_vec(&value).unwrap(),
        },
        0,
    )
    .unwrap();
}
fn publish(core: &mut Core, state: &mut AppState, feed: Signal<Feed>) {
    while let Some(event) = core.next_event() {
        if app::handles(&event) {
            state.event(feed, event).unwrap();
        }
    }
}
fn drain_opens(core: &mut Core) -> usize {
    let mut opens = 0;
    while let Some(frame) = core.take_outbox() {
        let request: Request = serde_json::from_slice(&frame.payload).unwrap();
        if request.method == wire::SUB_OPEN {
            opens += 1;
        }
        receive(core, json!({"jsonrpc":"2.0","id":request.id,"result":{}}));
    }
    opens
}
fn failure(core: &mut Core, scope: &str) {
    receive(
        core,
        json!({"jsonrpc":"2.0","method":"sub.reset","params":{
            "topic":"thread","scope":scope,"epoch":1
        }}),
    );
    receive(
        core,
        json!({"jsonrpc":"2.0","id":null,"error":{
            "code":"unavailable","message":"thread source is unavailable"
        }}),
    );
}

#[test]
fn scoped_reset_errors_surface_and_bound_reopens_until_explicit_retry_or_delta() {
    let runtime = nsh_core::element::Runtime::default();
    let _entered = runtime.enter();
    let feed = signal(Feed::default());
    let mut state = AppState::new();
    let mut core = Core::new();
    let mut reopened = 0;
    for _ in 0..=MAX_TOPIC_RESET_ERRORS {
        failure(&mut core, "A-3");
        publish(&mut core, &mut state, feed);
        feed.with_untracked(|feed| {
            let error = feed.topics["thread:A-3"]
                .error
                .as_ref()
                .expect("scoped source error must reach the feed");
            assert_eq!(error.code, "unavailable");
            assert_eq!(error.message, "thread source is unavailable");
            assert!(!feed.topics.contains_key("thread:A-4"));
        });
        reopened += drain_opens(&mut core);
    }
    assert_eq!(reopened, usize::from(MAX_TOPIC_RESET_ERRORS));
    assert!(!feed.with_untracked(|feed| feed.threads["A-3"].loading));
    // A stopped scope cannot suppress another thread's independent recovery.
    failure(&mut core, "A-4");
    publish(&mut core, &mut state, feed);
    assert_eq!(drain_opens(&mut core), 1);

    // An explicit same-scope open is the owner's retry, not another automatic loop.
    core.request(
        wire::SUB_OPEN,
        json!({"topic":"thread","scope":"A-3","since":null}),
        0,
    )
    .unwrap();
    assert_eq!(drain_opens(&mut core), 1);
    for _ in 0..MAX_TOPIC_RESET_ERRORS.saturating_sub(1) {
        failure(&mut core, "A-3");
        publish(&mut core, &mut state, feed);
        assert_eq!(drain_opens(&mut core), 1);
    }
    // A real delta also starts a fresh failure streak and clears the displayed error.
    receive(
        &mut core,
        json!({"jsonrpc":"2.0","method":"sub.delta","params":{
            "topic":"thread","scope":"A-3","epoch":1,"rows":[],"left":[],
            "cursor":{"rev":1,"after":null},"more":false,"live":null
        }}),
    );
    publish(&mut core, &mut state, feed);
    assert!(feed.with_untracked(|feed| feed.topics["thread:A-3"].error.is_none()));
    drain_opens(&mut core);
    for _ in 0..MAX_TOPIC_RESET_ERRORS.saturating_sub(1) {
        failure(&mut core, "A-3");
        publish(&mut core, &mut state, feed);
        assert_eq!(drain_opens(&mut core), 1);
    }
    // An unrelated intervening frame breaks correlation with a preceding reset.
    receive(
        &mut core,
        json!({"jsonrpc":"2.0","method":"sub.reset","params":{
            "topic":"thread","scope":"A-7","epoch":1
        }}),
    );
    receive(
        &mut core,
        json!({"jsonrpc":"2.0","method":"sub.delta","params":{
            "topic":"health","epoch":1,"rows":[],"left":[],
            "cursor":{"rev":1,"after":null},"more":false
        }}),
    );
    receive(
        &mut core,
        json!({"jsonrpc":"2.0","id":null,"error":{
            "code":"unavailable","message":"unrelated unavailable response"
        }}),
    );
    publish(&mut core, &mut state, feed);
    assert!(feed.with_untracked(|feed| feed.topics["thread:A-7"].error.is_none()));
    receive(
        &mut core,
        json!({"jsonrpc":"2.0","method":"sub.reset","params":{
            "topic":"thread","scope":"A-8","epoch":1
        }}),
    );
    failure(&mut core, "A-9");
    publish(&mut core, &mut state, feed);
    assert!(feed.with_untracked(|feed| feed.topics["thread:A-8"].error.is_none()));
    assert!(feed.with_untracked(|feed| feed.topics["thread:A-9"].error.is_some()));
    drain_opens(&mut core);
    let key = "thread:A-9";
    let scope = json!({"topic":"thread","scope":"A-9"});
    core.request(wire::SUB_CLOSE, scope.clone(), 0).unwrap();
    let close: Request = serde_json::from_slice(&core.take_outbox().unwrap().payload).unwrap();
    assert!(core.pages.contains_key(key));
    assert!(core.reset_errors.contains_key(key));
    core.request(wire::SUB_OPEN, scope.clone(), 1).unwrap();
    core.request(wire::SUB_OPEN, scope.clone(), 2).unwrap();
    assert!(core.take_outbox().is_none());
    assert!(core.reset_errors.contains_key(key));
    assert_eq!(core.deferred_opens.len(), 1);
    let reopen_id = core.deferred_opens[key].0;
    assert_eq!(core.pending[&reopen_id].deadline, Some(5_001));
    receive(
        &mut core,
        json!({"jsonrpc":"2.0","id":close.id,"result":{}}),
    );
    assert!(!core.reset_errors.contains_key(key));
    assert!(core.pages[key].replacement);
    assert_eq!(core.pages[key].epoch, None);
    assert_eq!(drain_opens(&mut core), 1);
    core.request(wire::SUB_CLOSE, scope, 3).unwrap();
    assert!(core.pages.contains_key(key));
    drain_opens(&mut core);
    assert!(!core.pages.contains_key(key));
}

#[test]
fn rc05_slow_reads_expire_as_correlated_replies_without_killing_the_session() {
    for (method, params, duration) in [
        (wire::app::DETAIL_GET, json!({"detail":"slow"}), 30_000),
        (wire::app::CHANGES_PAGE, json!({"limit":20}), 30_000),
        (wire::app::SEEN_ADVANCE, json!({"through":1}), 30_000),
        (
            wire::app::ACT_RESULT,
            json!({"cmd_id":"test","wait_ms":5_000}),
            10_000,
        ),
    ] {
        let mut core = Core::new();
        core.request_tagged(method, params.clone(), Some("read".into()), 100)
            .unwrap();
        assert!(core.has_pending());
        assert_eq!(core.expired(5_100), None);
        assert!(core.next_event().is_none());
        assert_eq!(core.expired(100 + duration - 1), None);
        assert_eq!(core.expired(100 + duration), None);
        assert!(matches!(core.next_event(), Some(Event::Reply {
            method: actual, tag: Some(tag), params: actual_params,
            outcome: Err((ErrorCode::Unavailable, _)), error_data: Some(data),
        }) if actual == method && tag == "read" && actual_params == params
            && data["code"] == DEADLINE_EXPIRED));
        assert!(!core.has_pending());
        assert!(core.next_event().is_none());
        receive(
            &mut core,
            json!({"jsonrpc":"2.0","id":1,"result":{"late":true}}),
        );
        assert!(core.next_event().is_none());
        core.request(wire::PING, json!({}), 100 + duration).unwrap();
        receive(&mut core, json!({"jsonrpc":"2.0","id":2,"result":{}}));
        assert!(matches!(
            core.next_event(),
            Some(Event::Reply {
                method: wire::PING,
                outcome: Ok(_),
                ..
            })
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn frame(value: Value) -> Frame {
        Frame {
            kind: FrameKind::Control,
            stream: 0,
            payload: serde_json::to_vec(&value).unwrap(),
        }
    }

    #[test]
    fn device_errors_are_replies_but_subscription_errors_close_the_session() {
        let mut core = Core::new();
        core.request(
            corral_protocol::device::DEVICE_ATTACH,
            json!({"device":"dev-test"}),
            10,
        )
        .unwrap();
        core.receive(
            frame(json!({
                "jsonrpc":"2.0", "id":1,
                "error":{"code":"revoked", "message":"device_revoked"}
            })),
            11,
        )
        .unwrap();
        assert!(matches!(
            core.next_event(),
            Some(Event::Reply {
                method: corral_protocol::device::DEVICE_ATTACH,
                outcome: Err((ErrorCode::Revoked, _)),
                ..
            })
        ));
        core.open(Topic::Board, 12).unwrap();
        assert!(
            core.receive(
                frame(json!({
                    "jsonrpc":"2.0", "id":2,
                    "error":{"code":"refused", "message":"topic_refused"}
                })),
                13
            )
            .is_err()
        );
    }

    #[test]
    fn notices_survive_and_requests_expire_at_the_supplied_deadline() {
        let mut core = Core::new();
        core.request(wire::PING, json!({}), 100).unwrap();
        assert_eq!(core.expired(5_099), None);
        assert_eq!(core.expired(5_100), Some(wire::PING));
        core.receive(
            frame(json!({
                "jsonrpc":"2.0", "method":"bye", "params":{"reason":"revoked"}
            })),
            101,
        )
        .unwrap();
        assert!(
            matches!(core.next_event(), Some(Event::Notice { method, params })
            if method == "bye" && params["reason"] == "revoked")
        );
        core.receive(frame(json!({"jsonrpc":"2.0", "id":1, "result":{}})), 102)
            .unwrap();
        assert_eq!(core.expired(5_100), None);
    }

    #[test]
    fn live_frames_never_finish_or_replace_a_scoped_history_snapshot() {
        let mut core = Core::new();
        for (more, live) in [
            (true, Value::Null),
            (
                false,
                json!({
                    "entry":"reply-1","turn":1,"offset":0,"text":"answer","done":false
                }),
            ),
        ] {
            core.receive(
                frame(json!({"jsonrpc":"2.0","method":"sub.delta","params":{
                    "topic":"thread","scope":"quick","epoch":1,"rows":[],"left":[],
                    "cursor":{"rev":1,"after":null},"more":more,"live":live
                }})),
                0,
            )
            .unwrap();
        }
        assert!(matches!(core.next_event(), Some(Event::Live { scope, .. }) if scope == "quick"));
        assert!(core.next_event().is_none());
        core.receive(
            frame(json!({"jsonrpc":"2.0","method":"sub.delta","params":{
                "topic":"thread","scope":"quick","epoch":1,"rows":[],"left":[],
                "cursor":{"rev":1,"after":null},"more":false,"live":null
            }})),
            1,
        )
        .unwrap();
        assert!(matches!(
            core.next_event(),
            Some(Event::Rows {
                replacement: true,
                ..
            })
        ));
    }
}
