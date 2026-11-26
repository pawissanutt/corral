use super::*;
use corral_protocol::{detail::SessionMessage, subscription::TranscriptOp as Op};

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
