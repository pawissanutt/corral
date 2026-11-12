#![cfg(any(target_os = "linux", target_os = "android", target_os = "macos"))]
use super::{Event, Session};
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
    json!({"daemon":"healthy", "reasons":[], "runtimes":[], "pm":"absent",
        "loop":{"pass_seq":1,"last_pass_at":null,"last_pass_ms":null,"age_secs":null},
        "pid":1,"version":"test"})
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
    assert_eq!(hello.params["caps"], json!(["board.v1"]));
    respond(
        &mut stream,
        &hello,
        json!({
            "proto":{"major":wire::PROTO_MAJOR,"minor":wire::PROTO_MINOR},
            "caps":["board.v1"],"daemon":{"version":"test","store":"st-test","epoch":1},
            "health":health()
        }),
    );
    let health_open = request(&mut stream);
    let board_open = request(&mut stream);
    assert_eq!(health_open.method, wire::SUB_OPEN);
    assert_eq!(board_open.method, wire::SUB_OPEN);
    assert_eq!(health_open.params["topic"], "health");
    assert_eq!(board_open.params["topic"], "board");
    respond(&mut stream, &board_open, json!({}));
    respond(&mut stream, &health_open, json!({}));
    stream
}
fn row(id: &str, title: &str) -> Value {
    json!({"id":id,"title":title,"project":null,"plan":null,"goal":null,
        "group":"queued", "rail":[
            {"glyph":"pending","sha7":null,"at":null},
            {"glyph":"pending","sha7":null,"at":null},
            {"glyph":"pending","sha7":null,"at":null},
            {"glyph":"pending","sha7":null,"at":null}],
        "now":{"code":"queued","text":"queued","since":null},
        "held_reason":null,"questions":0,"attention":[],"agent":null,
        "cost_usd":null,"cost_basis":null,"last_event":null,"rev":1})
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
        json!({"topic":"board","epoch":epoch,
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
fn board(session: &mut Session) -> (u64, Vec<wire::board::BoardRow>, Vec<String>, bool) {
    loop {
        match session.next() {
            Event::Board {
                epoch,
                rows,
                left,
                replacement,
            } => return (epoch, rows, left, replacement),
            Event::Health(_) => {}
            Event::Idle => panic!("board was not delivered"),
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
        let first_ack = ack(&mut stream, Topic::Board, 1, Some("T-9"));
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
        let second_ack = ack(&mut stream, Topic::Board, 1, None);
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
        let delta_ack = ack(&mut stream, Topic::Board, 2, None);
        respond(&mut stream, &delta_ack, json!({}));
    });
    let (mut session, _) = Session::connect(&path).unwrap();
    let (epoch, rows, left, replacement) = board(&mut session);
    assert_eq!(epoch, 1);
    assert_eq!(rows.len(), 2);
    assert!(left.is_empty());
    assert!(replacement);
    let (epoch, rows, left, replacement) = board(&mut session);
    assert_eq!(epoch, 1);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].title, "updated");
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
        let old_ack = ack(&mut stream, Topic::Board, 1, None);
        respond(&mut stream, &old_ack, json!({}));
        notification(
            &mut stream,
            wire::SUB_RESET,
            json!({"topic":"board","epoch":2}),
        );
        let reopened = request(&mut stream);
        assert_eq!(reopened.method, wire::SUB_OPEN);
        let open: SubOpen = serde_json::from_value(reopened.params.clone()).unwrap();
        assert_eq!(open.topic, Topic::Board);
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
        let first_ack = ack(&mut stream, Topic::Board, 1, Some("T-10"));
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
        let last_ack = ack(&mut stream, Topic::Board, 1, None);
        respond(&mut stream, &last_ack, json!({}));
    });
    let (mut session, _) = Session::connect(&path).unwrap();
    let (epoch, rows, _, _) = board(&mut session);
    assert_eq!(epoch, 1);
    assert_eq!(rows[0].id, "T-9");
    let (epoch, rows, left, replacement) = board(&mut session);
    assert_eq!(epoch, 2);
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|row| row.id != "T-9"));
    assert!(left.is_empty());
    assert!(replacement);
    server.join().unwrap();
}
