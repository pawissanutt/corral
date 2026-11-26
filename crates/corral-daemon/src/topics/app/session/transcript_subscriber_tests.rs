use super::*;
use corral_protocol::{detail::SessionMessage, subscription::TranscriptOp as Op};

/// A page of operations with no room for the fleet came without it, which says the session
/// has none, and nothing sent the fleet again until something else changed.
#[test]
fn a_fleet_left_off_a_page_of_operations_follows_it() {
    let mut subscriber = subscribe(service(), "pm".into(), 7, None).unwrap();
    let entry = Arc::clone(&subscriber.watch.entry);
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
    lock(&entry).publish(source);
    let next = |subscriber: &mut Subscriber| {
        let frame = subscriber.next(7, true, false, true).unwrap().unwrap();
        let notice: rpc::Notification<serde_json::Value> =
            serde_json::from_slice(&frame.payload).unwrap();
        subscriber
            .ack(serde_json::from_value(notice.params["cursor"].clone()).unwrap())
            .unwrap();
        notice.params
    };
    let held = next(&mut subscriber)["fleet"].clone();
    assert!(held.is_object());
    let reset = Op::Reset { older: None };
    lock(&entry).publish_transcript(vec![
        reset,
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
            again: false,
        },
    ]);
    let mut operations = next(&mut subscriber);
    if operations["transcript"][0]["op"] == "reset" {
        operations = next(&mut subscriber);
    }
    assert_eq!(operations["transcript"][0]["message"]["id"], "long");
    assert!(operations.get("fleet").is_none());
    assert_eq!(next(&mut subscriber)["fleet"], held);
    assert!(subscriber.next(7, true, false, true).unwrap().is_none());
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
        again: false,
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
