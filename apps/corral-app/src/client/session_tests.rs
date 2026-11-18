#![cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
use super::Session;
use crate::client::core::Event;
use corral_protocol::rpc::Request;
use corral_protocol::subscription::{SubAck, SubOpen, Topic};
use corral_protocol::{
    self as wire,
    frame::{Frame, FrameKind, read_frame, write_frame},
};
use serde_json::{Value, json};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

struct SocketDir(PathBuf);
impl SocketDir {
    fn new() -> Self {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let nonce = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "corral-app-{}-{nonce}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn socket(&self) -> PathBuf {
        self.0.join("daemon.sock")
    }
}
impl Drop for SocketDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn send(stream: &mut UnixStream, value: Value) {
    write_frame(
        stream,
        &Frame {
            kind: FrameKind::Control,
            stream: 0,
            payload: serde_json::to_vec(&value).unwrap(),
        },
    )
    .unwrap();
}
fn request(stream: &mut UnixStream) -> Request {
    let frame = read_frame(stream).unwrap().unwrap();
    assert_eq!(frame.kind, FrameKind::Control);
    assert_eq!(frame.stream, 0);
    serde_json::from_slice(&frame.payload).unwrap()
}
fn respond(stream: &mut UnixStream, request: &Request, result: Value) {
    send(
        stream,
        json!({"jsonrpc":"2.0", "id":request.id, "result":result}),
    );
}
fn notification(stream: &mut UnixStream, method: &str, params: Value) {
    send(
        stream,
        json!({"jsonrpc":"2.0", "method":method, "params":params}),
    );
}
fn health() -> Value {
    json!({"daemon":"healthy", "runner_reachable":true, "last_sync_at":null,
        "last_sync_error":null, "pid":1, "version":"test"})
}
fn handshake(listener: UnixListener) -> UnixStream {
    let (mut stream, _) = listener.accept().unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let hello = request(&mut stream);
    assert_eq!(hello.method, wire::HELLO);
    assert_eq!(hello.params["client"]["kind"], "app");
    assert_eq!(hello.params["caps"], json!(crate::client::app::CAPS));
    respond(
        &mut stream,
        &hello,
        json!({
            "proto":{"major":wire::PROTO_MAJOR,"minor":wire::PROTO_MINOR},
            "caps":["tasks.v1"],"daemon":{"version":"test","store":"st-test","epoch":1},
            "health":health()
        }),
    );
    let health_open = request(&mut stream);
    let tasks_open = request(&mut stream);
    assert_eq!(health_open.method, wire::SUB_OPEN);
    assert_eq!(tasks_open.method, wire::SUB_OPEN);
    assert_eq!(health_open.params["topic"], "health");
    assert_eq!(tasks_open.params["topic"], "tasks");
    respond(&mut stream, &tasks_open, json!({}));
    respond(&mut stream, &health_open, json!({}));
    stream
}
fn row(id: &str, title: &str) -> Value {
    let task = crate::model::daemon::bare_task(
        id,
        title,
        corral_types::Status::Ready,
        jiff::Timestamp::UNIX_EPOCH,
    );
    serde_json::to_value(crate::model::daemon::task_row(task)).unwrap()
}
fn page(
    stream: &mut UnixStream,
    epoch: u64,
    rev: u64,
    after: Value,
    rows: Vec<Value>,
    left: Vec<&str>,
    more: bool,
) {
    notification(
        stream,
        wire::SUB_DELTA,
        json!({"topic":"tasks","epoch":epoch,
        "rows":rows,"left":left,"cursor":{"rev":rev,"after":after},"more":more}),
    );
}
fn ack(stream: &mut UnixStream, topic: Topic, rev: u64, after: Option<&str>) -> Request {
    let request = request(stream);
    assert_eq!(request.method, wire::SUB_ACK);
    let ack: SubAck = serde_json::from_value(request.params.clone()).unwrap();
    assert_eq!(ack.topic, topic);
    assert_eq!(ack.cursor.rev, rev);
    assert_eq!(ack.cursor.after.as_deref(), after);
    request
}
type TasksPage = (u64, Vec<wire::rows::TaskRow>, Vec<String>, bool);

fn tasks(session: &mut Session) -> TasksPage {
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    loop {
        match session.next() {
            Event::Rows {
                delta: wire::subscription::SubDelta::Tasks { delta },
                replacement,
            } => return (delta.epoch, delta.rows, delta.left, replacement),
            Event::Health(_)
            | Event::Reply { .. }
            | Event::Rows { .. }
            | Event::Reset { .. }
            | Event::TopicError { .. }
            | Event::Live { .. }
            | Event::Notice { .. } => {}
            Event::Idle => assert!(
                std::time::Instant::now() < deadline,
                "tasks were not delivered"
            ),
            Event::Closed(reason) => panic!("session closed: {reason}"),
        }
    }
}

#[test]
fn paged_snapshot_and_delta_are_two_batches_and_every_page_is_acknowledged() {
    let dir = SocketDir::new();
    let path = dir.socket();
    let listener = UnixListener::bind(&path).unwrap();
    let server = std::thread::spawn(move || {
        let mut stream = handshake(listener);
        page(
            &mut stream,
            1,
            1,
            json!("T-9"),
            vec![row("T-9", "nine")],
            vec![],
            true,
        );
        let first_ack = ack(&mut stream, Topic::Tasks, 1, Some("T-9"));
        send(
            &mut stream,
            json!({"jsonrpc":"2.0","method":"future.notification"}),
        );
        notification(&mut stream, wire::SUB_DELTA, json!({"topic":"future"}));
        notification(&mut stream, wire::SUB_RESET, json!({"topic":"future"}));
        notification(
            &mut stream,
            wire::SUB_DELTA,
            json!({"topic":"health","epoch":1,
            "rows":[health()],"left":[],"cursor":{"rev":7,"after":null},"more":false}),
        );
        respond(&mut stream, &first_ack, json!({}));
        let health_ack = ack(&mut stream, Topic::Health, 7, None);
        page(
            &mut stream,
            1,
            1,
            Value::Null,
            vec![row("T-10", "ten")],
            vec![],
            false,
        );
        respond(&mut stream, &health_ack, json!({}));
        let second_ack = ack(&mut stream, Topic::Tasks, 1, None);
        respond(&mut stream, &second_ack, json!({}));
        page(
            &mut stream,
            1,
            2,
            Value::Null,
            vec![row("T-10", "updated")],
            vec!["T-9"],
            false,
        );
        let delta_ack = ack(&mut stream, Topic::Tasks, 2, None);
        respond(&mut stream, &delta_ack, json!({}));
    });
    let (mut session, _) = Session::connect(&path).unwrap();
    session.open(Topic::Health).unwrap();
    session.open(Topic::Tasks).unwrap();
    let (epoch, rows, left, replacement) = tasks(&mut session);
    assert_eq!(epoch, 1);
    assert_eq!(rows.len(), 2);
    assert!(left.is_empty());
    assert!(replacement);
    let (epoch, rows, left, replacement) = tasks(&mut session);
    assert_eq!(epoch, 1);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].task.title, "updated");
    assert_eq!(left, ["T-9"]);
    assert!(!replacement);
    server.join().unwrap();
}

#[test]
fn sub_reset_reopens_without_since_and_delivers_a_complete_replacement() {
    let dir = SocketDir::new();
    let path = dir.socket();
    let listener = UnixListener::bind(&path).unwrap();
    let server = std::thread::spawn(move || {
        let mut stream = handshake(listener);
        page(
            &mut stream,
            1,
            1,
            Value::Null,
            vec![row("T-9", "old")],
            vec![],
            false,
        );
        let old_ack = ack(&mut stream, Topic::Tasks, 1, None);
        respond(&mut stream, &old_ack, json!({}));
        notification(
            &mut stream,
            wire::SUB_RESET,
            json!({"topic":"tasks","epoch":2}),
        );
        let reopened = request(&mut stream);
        assert_eq!(reopened.method, wire::SUB_OPEN);
        let open: SubOpen = serde_json::from_value(reopened.params.clone()).unwrap();
        assert_eq!(open.topic, Topic::Tasks);
        assert!(open.since.is_none());
        page(
            &mut stream,
            2,
            1,
            json!("T-10"),
            vec![row("T-10", "new")],
            vec![],
            true,
        );
        let first_ack = ack(&mut stream, Topic::Tasks, 1, Some("T-10"));
        respond(&mut stream, &reopened, json!({}));
        respond(&mut stream, &first_ack, json!({}));
        page(
            &mut stream,
            2,
            1,
            Value::Null,
            vec![row("T-11", "newer")],
            vec![],
            false,
        );
        let last_ack = ack(&mut stream, Topic::Tasks, 1, None);
        respond(&mut stream, &last_ack, json!({}));
    });
    let (mut session, _) = Session::connect(&path).unwrap();
    session.open(Topic::Health).unwrap();
    session.open(Topic::Tasks).unwrap();
    let (epoch, rows, _, _) = tasks(&mut session);
    assert_eq!(epoch, 1);
    assert_eq!(rows[0].task.id, "T-9");
    let (epoch, rows, left, replacement) = tasks(&mut session);
    assert_eq!(epoch, 2);
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|row| row.task.id != "T-9"));
    assert!(left.is_empty());
    assert!(replacement);
    server.join().unwrap();
}

#[test]
fn reopened_thread_does_not_ack_or_publish_the_retired_live_page() {
    let dir = SocketDir::new();
    let path = dir.socket();
    let listener = UnixListener::bind(&path).unwrap();
    let (queued, ready) = std::sync::mpsc::channel();
    let server = std::thread::spawn(move || {
        let mut stream = handshake(listener);
        let open = request(&mut stream);
        respond(&mut stream, &open, Value::Null);
        notification(
            &mut stream,
            wire::SUB_DELTA,
            json!({"topic":"thread","scope":"quick","epoch":1,"rows":[],"left":[],
                "cursor":{"rev":155,"after":"live:1"},"more":false,
                "live":{"entry":"old","turn":1,"offset":0,"text":"retired","done":false}}),
        );
        queued.send(()).unwrap();
        let close = request(&mut stream);
        assert_eq!(close.method, wire::SUB_CLOSE);
        respond(&mut stream, &close, Value::Null);
        let reopened = request(&mut stream);
        assert_eq!(reopened.method, wire::SUB_OPEN);
        respond(&mut stream, &reopened, Value::Null);
        notification(
            &mut stream,
            wire::SUB_DELTA,
            json!({"topic":"thread","scope":"quick","epoch":1,"rows":[],"left":[],
                "cursor":{"rev":156,"after":null},"more":false,"live":null}),
        );
        let accepted = ack(&mut stream, Topic::Thread, 156, None);
        assert_eq!(accepted.params["scope"], "quick");
        respond(&mut stream, &accepted, Value::Null);
    });
    let (mut session, _) = Session::connect(&path).unwrap();
    session.open(Topic::Health).unwrap();
    session.open(Topic::Tasks).unwrap();
    let scope = json!({"topic":"thread","scope":"quick","since":null});
    session.request(wire::SUB_OPEN, scope.clone()).unwrap();
    ready.recv_timeout(Duration::from_secs(3)).unwrap();
    session
        .request(wire::SUB_CLOSE, json!({"topic":"thread","scope":"quick"}))
        .unwrap();
    session.request(wire::SUB_OPEN, scope).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    loop {
        match session.next() {
            Event::Rows {
                delta: wire::subscription::SubDelta::Thread { scope, delta, .. },
                replacement,
            } => {
                assert_eq!(scope, "quick");
                assert_eq!(delta.cursor.rev, 156);
                assert!(replacement);
                break;
            }
            Event::Idle => assert!(std::time::Instant::now() < deadline),
            Event::Live { .. } => panic!("retired live text reached the reopened thread"),
            Event::Closed(reason) => panic!("session closed: {reason}"),
            _ => {}
        }
    }
    server.join().unwrap();
}

#[test]
fn tagged_request_preserves_correlation_and_original_params_in_reply() {
    let dir = SocketDir::new();
    let path = dir.socket();
    let listener = UnixListener::bind(&path).unwrap();
    let server = std::thread::spawn(move || {
        let mut stream = handshake(listener);
        let ping = request(&mut stream);
        assert_eq!(ping.method, wire::PING);
        assert_eq!(ping.params, json!({"nonce":"original"}));
        assert_eq!(serde_json::to_value(&ping).unwrap().get("tag"), None);
        respond(&mut stream, &ping, json!({"pong":true}));
    });
    let (mut session, _) = Session::connect(&path).unwrap();
    session.open(Topic::Health).unwrap();
    session.open(Topic::Tasks).unwrap();
    session
        .request_tagged(
            wire::PING,
            json!({"nonce":"original"}),
            Some("caller-identity".into()),
        )
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    loop {
        match session.next() {
            Event::Reply {
                method,
                params,
                tag,
                outcome,
                ..
            } => {
                assert_eq!(method, wire::PING);
                assert_eq!(params, json!({"nonce":"original"}));
                assert_eq!(tag.as_deref(), Some("caller-identity"));
                assert_eq!(outcome, Ok(json!({"pong":true})));
                break;
            }
            Event::Idle => assert!(std::time::Instant::now() < deadline),
            Event::Closed(reason) => panic!("session closed: {reason}"),
            _ => panic!("unexpected event before tagged reply"),
        }
    }
    server.join().unwrap();
}
