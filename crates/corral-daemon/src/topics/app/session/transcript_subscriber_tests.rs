use super::*;
use corral_protocol::{
    detail::SessionMessage,
    subscription::{BoardCursor, TranscriptOp as Op},
};

/// A fleet and a message that do not fit one page together.
fn long_fleet() -> LivePage {
    let mut source = fleet_replacement(1, 1, false);
    let message = |seq| wire::subscription::SessionAgentMessage {
        seq,
        at: seq,
        from: "parent".into(),
        to: "all".into(),
        text: "m".repeat(400),
        delivery: "delivered".into(),
    };
    let fleet = SessionFleet {
        children: Vec::new(),
        messages: (1..=200).map(message).collect(),
        messages_total: 200,
        max_parallel: 3,
    };
    source.fleet = Some(bound_fleet(fleet).unwrap());
    source
}

fn long_put() -> Op {
    Op::Put {
        message: SessionMessage {
            id: "long".into(),
            author: "assistant".into(),
            at: None,
            items: vec![corral_protocol::detail::SessionItem::Text {
                text: "w".repeat(40 * 1024),
            }],
            model: None,
            stop_reason: None,
            usage: None,
            input: None,
            attachments: Vec::new(),
        },
        before: None,
        replaces: None,
        open: false,
        cut: false,
    }
}

/// The next page of a client that follows the fleet and the transcript, not yet acked.
fn send(subscriber: &mut Subscriber) -> (serde_json::Value, BoardCursor) {
    let frame = subscriber.next(7, true, false, true).unwrap().unwrap();
    let notice: rpc::Notification<serde_json::Value> =
        serde_json::from_slice(&frame.payload).unwrap();
    let cursor = serde_json::from_value(notice.params["cursor"].clone()).unwrap();
    (notice.params, cursor)
}

fn next(subscriber: &mut Subscriber) -> serde_json::Value {
    let (page, cursor) = send(subscriber);
    subscriber.ack(cursor).unwrap();
    page
}

/// A page of operations with no room for the fleet came without it, which says the session
/// has none, and nothing sent the fleet again until something else changed.
#[test]
fn a_fleet_left_off_a_page_of_operations_follows_it() {
    let mut subscriber = subscribe(service(), "pm".into(), 7, None).unwrap();
    let entry = Arc::clone(&subscriber.watch.entry);
    lock(&entry).publish(long_fleet());
    let held = next(&mut subscriber)["fleet"].clone();
    assert!(held.is_object());
    lock(&entry).publish_transcript(vec![Op::Reset { older: None }, long_put()]);
    assert_eq!(next(&mut subscriber)["transcript"][0]["op"], "reset");
    let operations = next(&mut subscriber);
    assert_eq!(operations["transcript"][0]["message"]["id"], "long");
    assert!(operations.get("fleet").is_none());
    assert_eq!(next(&mut subscriber)["fleet"], held);
    assert!(subscriber.next(7, true, false, true).unwrap().is_none());
}

/// The `quick` scope names no chat once its chat ends, so no journal was followed and
/// nothing reset the transcript: a subscriber was given the ended chat's messages again.
#[test]
fn a_scope_that_names_no_transcript_any_more_shows_none() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("journal.jsonl");
    std::fs::write(
        &path,
        "{\"type\":\"message\",\"id\":\"u\",\"parentId\":null,\"message\":{\"role\":\"user\",\"content\":\"hi\"}}\n",
    )
    .unwrap();
    let service = service();
    let mut sources = Sources::default();
    let mut subscriber = subscribe(Arc::clone(&service), "quick".into(), 7, None).unwrap();
    let mut read = |journal: Option<&std::path::Path>, seq| {
        poll(
            &service,
            &mut sources,
            |_, reader| {
                reader.follow("A-1", journal, None, None);
                Ok(page(1, seq, seq, false))
            },
            || false,
        );
    };
    read(Some(&path), 1);
    let shown = next(&mut subscriber);
    assert_eq!(shown["transcript"][1]["message"]["id"], "A-1/entry/u");
    read(None, 2);
    let ended = next(&mut subscriber);
    assert_eq!(ended["transcript"], serde_json::json!([{"op": "reset"}]));
    read(None, 3);
    assert!(next(&mut subscriber).get("transcript").is_none());
}

/// A gap found while a large message waited its turn went out on the page of that message
/// alone, without the fleet: across a gap a client trusts no fleet it holds, and cleared it.
#[test]
fn a_gap_is_told_on_a_page_that_carries_the_fleet() {
    let mut subscriber = subscribe(service(), "pm".into(), 7, None).unwrap();
    let entry = Arc::clone(&subscriber.watch.entry);
    lock(&entry).publish(long_fleet());
    let held = next(&mut subscriber)["fleet"].clone();
    lock(&entry).publish_transcript(vec![Op::Reset { older: None }, long_put()]);
    assert_eq!(next(&mut subscriber)["transcript"][0]["op"], "reset");
    // The message gives way to a page of events, and the ring overflows before its ack.
    lock(&entry).publish(page(1, 2, 2, false));
    let (events, cursor) = send(&mut subscriber);
    assert!(events.get("transcript").is_none());
    lock(&entry).publish(page(1, 3, 3 + SESSION_BUFFER_EVENTS as u64, false));
    subscriber.ack(cursor).unwrap();
    let gap = next(&mut subscriber);
    assert!(gap["gap"].is_object(), "{gap}");
    assert_eq!(gap["fleet"], held);
    assert!(gap.get("transcript").is_none());
    assert_eq!(
        next(&mut subscriber)["transcript"][0]["message"]["id"],
        "long"
    );
}

#[test]
fn capable_subscriber_snapshots_and_recovers_without_live_words() {
    let service = service();
    let mut capable = subscribe(service.clone(), "pm".into(), 7, None).unwrap();
    let mut legacy = subscribe(service, "pm".into(), 7, None).unwrap();
    let put = |id: &str| Op::Put {
        message: SessionMessage {
            id: id.into(),
            author: "assistant".into(),
            at: None,
            items: Vec::new(),
            model: None,
            stop_reason: None,
            usage: None,
            input: None,
            attachments: Vec::new(),
        },
        before: None,
        replaces: None,
        open: false,
        cut: false,
    };
    let receive = |subscriber: &mut Subscriber, cap| {
        let frame = subscriber.next(7, false, false, cap).unwrap().unwrap();
        assert!(frame.payload.len() <= APP_CONTROL_MAX);
        let notification: rpc::Notification<SubDelta> =
            serde_json::from_slice(&frame.payload).unwrap();
        let SubDelta::Session {
            events,
            transcript,
            cursor,
            ..
        } = notification.params
        else {
            panic!("session delta");
        };
        assert!(subscriber.next(7, false, false, cap).unwrap().is_none());
        subscriber.ack(cursor).unwrap();
        (events, transcript)
    };
    let entry = capable.watch.entry.clone();
    {
        let mut buffer = lock(&entry);
        buffer.publish(page(1, 1, 1, true));
        buffer.publish_transcript(vec![Op::Reset { older: None }, put("first")]);
    }
    let (events, ops) = receive(&mut capable, true);
    assert!(events.is_empty());
    assert!(
        matches!(ops.as_slice(), [Op::Reset {..},Op::Put {message,..}] if message.id == "first")
    );
    let (events, ops) = receive(&mut legacy, false);
    assert!(
        matches!(events.as_slice(), [event] if matches!(event.kind, SessionLiveKind::LiveText {..}))
    );
    assert!(ops.is_empty());

    lock(&entry).publish_transcript(vec![put("second")]);
    let (_, ops) = receive(&mut capable, true);
    assert!(matches!(ops.as_slice(), [Op::Put {message,..}] if message.id == "second"));
    lock(&entry).publish_transcript((0..=SESSION_BUFFER_EVENTS).map(|_| put("second")).collect());
    let (_, ops) = receive(&mut capable, true);
    assert!(
        matches!(ops.as_slice(), [Op::Reset {..}, Op::Put{..}, Op::Put{message,..}] if message.id == "second")
    );
}
