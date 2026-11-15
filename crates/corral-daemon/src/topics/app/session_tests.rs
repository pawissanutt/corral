use super::*;
use corral_protocol::{rpc, subscription::SessionLiveKind};

fn service() -> Arc<Service> {
    Arc::new(Service {
        entries: Mutex::default(),
        revisions: Arc::default(),
        thread: Mutex::default(),
    })
}

fn event(generation: u32, seq: u64, text: bool) -> SessionLiveEvent {
    SessionLiveEvent {
        generation,
        seq,
        at: "2026-11-15T00:00:00Z".into(),
        kind: if text {
            SessionLiveKind::LiveText {
                turn: 1,
                message: 1,
                offset: (seq - 1) * 1024,
                text: "\0".repeat(1024),
            }
        } else {
            SessionLiveKind::LiveQueue {
                steer: 0,
                follow_up: 0,
            }
        },
    }
}

fn page(generation: u32, start: u64, end: u64, text: bool) -> LivePage {
    LivePage {
        generation,
        events: (start..=end)
            .map(|seq| event(generation, seq, text))
            .collect(),
        last: end,
        next: end,
        more: false,
        gap: None,
    }
}

fn delta(subscriber: &mut Subscriber) -> SubDelta {
    let frame = subscriber.next(7).unwrap().unwrap();
    assert!(frame.payload.len() <= APP_CONTROL_MAX);
    let notification: rpc::Notification<SubDelta> = serde_json::from_slice(&frame.payload).unwrap();
    notification.params
}

fn receive(
    subscriber: &mut Subscriber,
) -> (Option<u32>, Vec<SessionLiveEvent>, Option<SessionLiveGap>) {
    let SubDelta::Session {
        generation,
        events,
        gap,
        cursor,
        ..
    } = delta(subscriber)
    else {
        panic!("wrong live topic")
    };
    assert!(
        subscriber.next(7).unwrap().is_none(),
        "second page escaped before ack"
    );
    subscriber.ack(cursor.clone()).unwrap();
    assert!(
        subscriber.ack(cursor).is_err(),
        "duplicate ack advanced the stream"
    );
    (generation, events, gap)
}

#[test]
fn subscribers_do_not_split_one_host_cursor() {
    let service = service();
    let mut sources = BTreeMap::new();
    let mut calls = 0;
    poll(
        &service,
        &mut sources,
        |_, _| {
            calls += 1;
            Ok(page(1, 1, 32, true))
        },
        || false,
    );
    assert_eq!(calls, 0, "unwatched scopes must not read bridges");
    let mut first = subscribe(Arc::clone(&service), "pm".into(), 7, None).unwrap();
    let mut second = subscribe(Arc::clone(&service), "pm".into(), 7, None).unwrap();
    poll(
        &service,
        &mut sources,
        |scope, _| {
            assert_eq!(scope, "pm");
            calls += 1;
            Ok(page(1, 1, 32, true))
        },
        || false,
    );
    assert_eq!(calls, 1, "each subscriber opened its own reader");
    assert_eq!(sources.len(), 1);
    let mut received = [Vec::new(), Vec::new()];
    for (index, subscriber) in [&mut first, &mut second].into_iter().enumerate() {
        while received[index].len() < 32 {
            let (_, events, gap) = receive(subscriber);
            assert!(gap.is_none());
            received[index].extend(events.into_iter().map(|event| event.seq));
        }
    }
    assert_eq!(received[0], (1..=32).collect::<Vec<_>>());
    assert_eq!(received[0], received[1]);
    drop(first);
    drop(second);
    poll(
        &service,
        &mut sources,
        |_, _| {
            calls += 1;
            Ok(page(1, 33, 33, false))
        },
        || false,
    );
    assert_eq!(calls, 1, "closed scope kept polling its host");
    assert!(sources.is_empty());
    assert!(lock(&service.entries).is_empty());
}

#[test]
fn slow_subscriber_gets_gap_instead_of_unbounded_buffer() {
    // Queue events exercise the count ceiling; escaped text exercises the byte ceiling.
    for text in [false, true] {
        let service = service();
        let mut sources = BTreeMap::new();
        let mut slow = subscribe(Arc::clone(&service), "pm".into(), 7, None).unwrap();
        poll(
            &service,
            &mut sources,
            |_, _| Ok(page(1, 1, 1, text)),
            || false,
        );
        let SubDelta::Session { cursor, .. } = delta(&mut slow) else {
            panic!("wrong topic")
        };
        poll(
            &service,
            &mut sources,
            |_, _| Ok(page(1, 2, 513, text)),
            || false,
        );
        poll(
            &service,
            &mut sources,
            |_, _| Ok(page(1, 514, 514, text)),
            || false,
        );
        {
            let buffer = lock(&slow.watch.entry);
            assert!(buffer.events.len() <= SESSION_BUFFER_EVENTS);
            assert!(buffer.bytes <= SESSION_BUFFER_BYTES);
            assert!(buffer.events.front().unwrap().0.seq > 2);
        }
        assert!(slow.next(7).unwrap().is_none());
        slow.ack(cursor).unwrap();
        let (_, events, gap) = receive(&mut slow);
        assert!(
            events.is_empty(),
            "lagging subscriber must resume from the present"
        );
        let gap = gap.unwrap();
        assert!(gap.reconcile);
        assert_eq!(gap.after, 1);
        assert_eq!(gap.resumed, 515);
        poll(
            &service,
            &mut sources,
            |_, _| Ok(page(1, 515, 515, text)),
            || false,
        );
        let (_, events, gap) = receive(&mut slow);
        assert!(gap.is_none());
        assert_eq!(events[0].seq, 515);
    }
}

#[test]
fn generation_change_never_reuses_stale_offsets() {
    let service = service();
    let mut sources = BTreeMap::new();
    let mut subscriber = subscribe(Arc::clone(&service), "pm".into(), 7, None).unwrap();
    poll(
        &service,
        &mut sources,
        |_, _| Ok(page(1, 1, 1, true)),
        || false,
    );
    let (generation, _, gap) = receive(&mut subscriber);
    assert_eq!(generation, Some(1));
    assert!(gap.is_none());
    poll(
        &service,
        &mut sources,
        |_, _| Ok(page(2, 1, 1, true)),
        || false,
    );
    let (generation, events, gap) = receive(&mut subscriber);
    assert_eq!(generation, Some(2));
    assert!(gap.unwrap().reconcile);
    assert!(events.iter().all(|event| event.generation == 2));
    assert!(matches!(
        events[0].kind,
        SessionLiveKind::LiveText { offset: 0, .. }
    ));
    poll(
        &service,
        &mut sources,
        |_, _| anyhow::bail!("host ended"),
        || false,
    );
    let (generation, events, gap) = receive(&mut subscriber);
    assert_eq!(generation, None);
    assert!(events.is_empty());
    assert!(gap.unwrap().reconcile);
    assert_eq!(
        lock(&subscriber.watch.entry).users,
        1,
        "failure closed the subscription"
    );
    poll(
        &service,
        &mut sources,
        |_, _| Ok(page(3, 1, 1, true)),
        || false,
    );
    let SubDelta::Session {
        generation,
        events,
        gap,
        cursor,
        ..
    } = delta(&mut subscriber)
    else {
        panic!("wrong topic")
    };
    assert_eq!(generation, Some(3));
    assert!(gap.unwrap().reconcile);
    assert_eq!(events[0].seq, 1);
    drop(subscriber);
    let mut reopened = subscribe(
        Arc::clone(&service),
        "pm".into(),
        7,
        Some(wire::subscription::SubSince { epoch: 7, cursor }),
    )
    .unwrap();
    poll(
        &service,
        &mut sources,
        |_, _| Ok(page(4, 1, 1, true)),
        || false,
    );
    let (generation, _, gap) = receive(&mut reopened);
    assert_eq!(generation, Some(4));
    assert!(
        gap.unwrap().reconcile,
        "recreated scope reused an old cursor token"
    );
}
