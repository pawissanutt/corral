use super::*;
use crate::client::app::{self, AppState};
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
}
