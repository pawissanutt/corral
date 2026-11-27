//! The board a held move waits for, at the client boundary: the real core's request order and
//! the app's handling of its events (`client::walks`).

use super::*;
use crate::client::core::Core;
use crate::client::requests::ClientRequest;
use crate::model::act::{self, Act, ActStatus};
use corral_protocol::frame::{Frame, FrameKind};
use corral_protocol::rpc::Request;
use corral_protocol::subscription::{SubOpen, Topic};
use corral_types::{Op, Status};
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

/// Every event the core produced goes to the app, and every request the app makes to the core.
fn pump(core: &mut Core, app: &mut AppState, feed: Signal<Feed>) {
    while let Some(event) = core.next_event() {
        if super::handles(&event) {
            for (method, params, tag) in app.event(feed, event).unwrap() {
                core.request_tagged(method, params, tag, 0).unwrap();
            }
        }
    }
}

/// The core's requests sent since the last call, oldest first; acknowledgements are answered.
fn sent(core: &mut Core) -> Vec<Request> {
    let mut requests = Vec::new();
    while let Some(frame) = core.take_outbox() {
        let request: Request = serde_json::from_slice(&frame.payload).unwrap();
        if request.method == corral_protocol::SUB_ACK {
            answer(core, &request);
        } else {
            requests.push(request);
        }
    }
    requests
}

fn answer(core: &mut Core, request: &Request) {
    receive(
        core,
        json!({"jsonrpc": "2.0", "id": request.id, "result": {}}),
    );
}

fn methods(requests: &[Request]) -> Vec<&str> {
    requests
        .iter()
        .map(|request| request.method.as_str())
        .collect()
}

/// One complete Tasks snapshot holding T-1 in `status`.
fn snapshot(core: &mut Core, status: Status) {
    let row = crate::model::daemon::task_row(crate::model::daemon::bare_task(
        "T-1",
        "t",
        status,
        jiff::Timestamp::UNIX_EPOCH,
    ));
    receive(
        core,
        json!({"jsonrpc": "2.0", "method": "sub.delta", "params": {
            "topic": "tasks", "epoch": 1, "rows": [row], "left": [],
            "cursor": {"rev": 1, "after": null}, "more": false
        }}),
    );
}

fn open_tasks(core: &mut Core) {
    let open = SubOpen {
        topic: Topic::Tasks,
        since: None,
        scope: None,
    };
    core.request(
        corral_protocol::SUB_OPEN,
        serde_json::to_value(open).unwrap(),
        0,
    )
    .unwrap();
}

/// The step button offered the same move again once corrald confirmed it while the board still
/// showed Backlog. A board subscription opened before the answer (a reset's reopen) must not
/// release it; the app's ordered refresh must, even when the rows did not change.
#[test]
fn a_held_move_is_released_only_by_a_board_opened_after_its_answer() {
    let runtime = nsh_core::element::Runtime::default();
    let _entered = runtime.enter();
    let feed = signal(Feed::default());
    let mut app = AppState::new();
    app.ready = true;
    let mut core = Core::new();
    open_tasks(&mut core);
    let opened = sent(&mut core);
    assert_eq!(methods(&opened), ["sub.open"]);
    answer(&mut core, &opened[0]);
    snapshot(&mut core, Status::Backlog);
    pump(&mut core, &mut app, feed);
    sent(&mut core);
    assert!(feed.with_untracked(|feed| feed.board.task("T-1").is_some()));

    // A reset's reopen is admitted before the move's answer is handled.
    open_tasks(&mut core);
    let reopen = sent(&mut core);
    let move_ready = Act::with_key(
        Id::new("k"),
        Op::Move {
            id: "T-1".into(),
            status: Status::Ready,
        },
        false,
    );
    let behind = feed
        .with_untracked(|feed| {
            act::board_behind(
                feed,
                &move_ready,
                &ActStatus::Done,
                crate::client::walks::opens(),
            )
        })
        .unwrap();
    // Its snapshot may predate the move: still held.
    answer(&mut core, &reopen[0]);
    snapshot(&mut core, Status::Backlog);
    pump(&mut core, &mut app, feed);
    sent(&mut core);
    assert!(!feed.with_untracked(|feed| act::board_caught_up(feed, &move_ready.op, behind)));

    // The refresh closes first; its open waits for the close's answer.
    for (method, params, tag) in app.request(feed, ClientRequest::RefreshTasks) {
        core.request_tagged(method, params, tag, 0).unwrap();
    }
    let close = sent(&mut core);
    assert_eq!(methods(&close), ["sub.close"]);
    // A page of the subscription being closed is dropped, not counted as fresh.
    snapshot(&mut core, Status::Backlog);
    pump(&mut core, &mut app, feed);
    assert!(!feed.with_untracked(|feed| act::board_caught_up(feed, &move_ready.op, behind)));
    assert!(sent(&mut core).is_empty());
    answer(&mut core, &close[0]);
    let refresh = sent(&mut core);
    assert_eq!(methods(&refresh), ["sub.open"]);
    answer(&mut core, &refresh[0]);
    // The rows did not change, and the board opened after the answer releases the move.
    snapshot(&mut core, Status::Backlog);
    pump(&mut core, &mut app, feed);
    assert!(feed.with_untracked(|feed| act::board_caught_up(feed, &move_ready.op, behind)));
}
