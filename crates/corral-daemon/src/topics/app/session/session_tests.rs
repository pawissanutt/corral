use super::{
    APP_CONTROL_MAX, SESSION_BUFFER_BYTES, SESSION_BUFFER_EVENTS, SESSION_SCOPE_BYTES,
    buffer::Watches,
    lock,
    reader::{
        Cursors, HostPage, LIVE_EVENT_BYTES, LivePage, LiveReader, RingPage, Sources, bound_fleet,
        bound_state, event_bytes, poll,
    },
    subscriber::{Subscriber, subscribe},
};
use corral_protocol::{
    self as wire, rpc,
    session_state::{Capability, SessionState},
    subscription::{
        LIVE_FLEET_BYTES, LIVE_STATE_BYTES, SessionAgentMessage, SessionFleet, SessionLiveEvent,
        SessionLiveGap, SessionLiveKind, SessionLiveToolState, SubAck, SubDelta, SubOpen,
    },
};
use serde_json::json;
use std::sync::Arc;

fn service() -> Arc<Watches> {
    Arc::default()
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
            .map(|seq| {
                let event = event(generation, seq, text);
                let bytes = event_bytes(&event).unwrap();
                (event, bytes)
            })
            .collect(),
        next: end,
        more: false,
        gap: None,
        fleet: None,
        state: None,
    }
}

fn delta(subscriber: &mut Subscriber) -> SubDelta {
    let frame = subscriber.next(7, false, false).unwrap().unwrap();
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
        subscriber.next(7, false, false).unwrap().is_none(),
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
    let mut sources = Sources::default();
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
    assert_eq!(sources.readers.len(), 1);
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
    assert!(sources.readers.is_empty());
    assert!(lock(&service.entries).is_empty());
}

#[test]
fn slow_subscriber_gets_gap_instead_of_unbounded_buffer() {
    // Queue events exercise the count ceiling; escaped text exercises the byte ceiling.
    for text in [false, true] {
        let service = service();
        let mut sources = Sources::default();
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
        assert!(slow.next(7, false, false).unwrap().is_none());
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
    let mut sources = Sources::default();
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

#[test]
fn fleet_is_connection_scoped_bounded_and_can_be_renegotiated_off() {
    use corral_protocol::frame::read_frame;
    use std::os::unix::net::UnixStream;

    let fixture = crate::views::tests::Fixture::new();
    let shared = &fixture.shared;
    // Each Topics instance is owned by one connection, just as in both HELLO loops.
    let mut current = crate::topics::Subscriptions::new(shared);
    let mut legacy = crate::topics::Subscriptions::new(shared);
    current.set_capabilities(&[wire::app::SESSION_FLEET_V1.into()]);
    for topics in [&mut current, &mut legacy] {
        topics
            .open(SubOpen {
                topic: wire::subscription::Topic::Session,
                scope: Some("pm".into()),
                since: None,
            })
            .unwrap();
    }
    let mut source = page(1, 1, 32, true);
    source.events[0].0.kind = SessionLiveKind::LiveFleet {};
    source.events[0].1 = event_bytes(&source.events[0].0).unwrap();
    let fleet = serde_json::from_value(json!({
        "children": [{"id": "review", "kind": "review", "title": "Review", "state": "running",
            "model": "provider/model", "tokens": 0}],
        "messages": (1..=101).map(|seq| json!({
            "seq": seq, "at": seq, "from": "parent", "to": "review",
            "text": "\0".repeat(2048), "delivery": "delivered"
        })).collect::<Vec<_>>(),
        "messages_total": 101, "max_parallel": 3
    }))
    .unwrap();
    source.fleet = Some(bound_fleet(fleet).unwrap());
    lock(&lock(&shared.session_live.watches.entries)["pm"]).publish(source);
    let (mut current_server, mut current_client) = UnixStream::pair().unwrap();
    let (mut legacy_server, mut legacy_client) = UnixStream::pair().unwrap();
    let receive = |client: &mut UnixStream| {
        let frame = read_frame(client).unwrap().unwrap();
        assert!(frame.payload.len() <= APP_CONTROL_MAX);
        let notice: rpc::Notification<SubDelta> = serde_json::from_slice(&frame.payload).unwrap();
        notice.params
    };
    current.flush(&mut current_server).unwrap();
    legacy.flush(&mut legacy_server).unwrap();
    let SubDelta::Session {
        fleet,
        events,
        cursor,
        ..
    } = receive(&mut current_client)
    else {
        panic!("wrong topic")
    };
    let fleet = fleet.unwrap();
    assert_eq!(fleet.children.len(), 1);
    assert!(fleet.messages.len() < 100);
    assert_eq!(fleet.messages.last().unwrap().seq, 101);
    assert_eq!(fleet.messages_total, 101);
    assert!(
        fleet
            .messages
            .iter()
            .all(|message| message.text.len() == 2048)
    );
    assert!(matches!(events[0].kind, SessionLiveKind::LiveFleet {}));
    let SubDelta::Session { fleet, events, .. } = receive(&mut legacy_client) else {
        panic!("wrong topic")
    };
    assert!(fleet.is_none());
    assert!(
        events
            .iter()
            .all(|event| !matches!(event.kind, SessionLiveKind::LiveFleet {}))
    );
    current
        .ack(SubAck {
            topic: wire::subscription::Topic::Session,
            scope: Some("pm".into()),
            cursor,
        })
        .unwrap();
    // This is the same replacement performed by a second HELLO without the capability.
    current.set_capabilities(&[]);
    current.flush(&mut current_server).unwrap();
    let SubDelta::Session { fleet, .. } = receive(&mut current_client) else {
        panic!("wrong topic")
    };
    assert!(fleet.is_none());
}

fn fleet_replacement(generation: u32, seq: u64, empty: bool) -> LivePage {
    let mut page = page(generation, seq, seq, false);
    page.events[0].0.kind = SessionLiveKind::LiveFleet {};
    page.events[0].1 = event_bytes(&page.events[0].0).unwrap();
    let fleet = SessionFleet {
        children: Vec::new(),
        messages: if empty {
            Vec::new()
        } else {
            vec![wire::subscription::SessionAgentMessage {
                seq: 1,
                at: 1,
                from: "parent".into(),
                to: "all".into(),
                text: "Ready?".into(),
                delivery: "delivered".into(),
            }]
        },
        messages_total: u64::from(!empty),
        max_parallel: 3,
    };
    page.fleet = Some(bound_fleet(fleet).unwrap());
    page
}

fn fleet_wire(subscriber: &mut Subscriber) -> serde_json::Value {
    let frame = subscriber.next(7, true, false).unwrap().unwrap();
    assert!(frame.payload.len() <= APP_CONTROL_MAX);
    let notice: rpc::Notification<serde_json::Value> =
        serde_json::from_slice(&frame.payload).unwrap();
    subscriber
        .ack(serde_json::from_value(notice.params["cursor"].clone()).unwrap())
        .unwrap();
    notice.params
}

#[test]
fn empty_fleet_clears_clients_but_transcript_pages_keep_the_current_projection() {
    let service = service();
    let mut subscriber = subscribe(service, "pm".into(), 7, None).unwrap();
    let entry = Arc::clone(&subscriber.watch.entry);
    lock(&entry).publish(fleet_replacement(1, 1, false));
    let initial = fleet_wire(&mut subscriber);
    assert_eq!(initial["fleet"]["messages_total"], 1);
    // None on the bridge is no update: ordinary live pages must retain the replacement.
    lock(&entry).publish(page(1, 2, 2, true));
    let transcript = fleet_wire(&mut subscriber);
    assert_eq!(transcript["fleet"], initial["fleet"]);
    lock(&entry).publish(fleet_replacement(1, 3, true));
    assert!(lock(&entry).fleet.is_none());
    let cleared = fleet_wire(&mut subscriber);
    assert!(cleared.get("fleet").is_none());
    assert_eq!(cleared["events"][0]["kind"], "live_fleet");
    lock(&entry).publish(page(1, 4, 3, false));
    assert!(subscriber.next(7, true, false).unwrap().is_none());
}

#[test]
fn new_generations_and_bridge_loss_discard_previous_fleets() {
    let service = service();
    let mut subscriber = subscribe(service, "pm".into(), 7, None).unwrap();
    let entry = Arc::clone(&subscriber.watch.entry);
    lock(&entry).publish(fleet_replacement(1, 1, false));
    assert!(fleet_wire(&mut subscriber).get("fleet").is_some());
    // A new host has no events or projection yet, but the generation still clears the old data.
    lock(&entry).publish(page(2, 1, 0, false));
    assert!(lock(&entry).fleet.is_none());
    let generation = fleet_wire(&mut subscriber);
    assert_eq!(generation["generation"], 2);
    assert_eq!(generation["gap"]["reconcile"], true);
    assert!(generation.get("fleet").is_none());
    lock(&entry).publish(fleet_replacement(2, 1, false));
    assert!(fleet_wire(&mut subscriber).get("fleet").is_some());
    lock(&entry).unavailable();
    assert!(lock(&entry).fleet.is_none());
    let cleared = fleet_wire(&mut subscriber);
    assert!(cleared["generation"].is_null());
    assert_eq!(cleared["gap"]["reconcile"], true);
    assert!(cleared.get("fleet").is_none());
}

#[test]
fn legacy_hosts_emit_no_fleet_updates_or_spurious_clears() {
    let service = service();
    let mut subscriber = subscribe(service, "pm".into(), 7, None).unwrap();
    let entry = Arc::clone(&subscriber.watch.entry);
    lock(&entry).publish(page(1, 1, 0, false));
    assert!(subscriber.next(7, true, false).unwrap().is_none());
    lock(&entry).publish(page(1, 1, 1, false));
    let ordinary = fleet_wire(&mut subscriber);
    assert!(ordinary.get("fleet").is_none());
    assert!(ordinary["gap"].is_null());
    assert_eq!(ordinary["events"][0]["kind"], "live_queue");
    lock(&entry).publish(page(1, 2, 1, false));
    assert!(subscriber.next(7, true, false).unwrap().is_none());
}

fn lifecycle(last: u64, next: u64, gap: Option<SessionLiveGap>) -> RingPage {
    RingPage { last, next, gap }
}

fn gap(after: u64, resumed: u64) -> SessionLiveGap {
    SessionLiveGap {
        after,
        resumed,
        missing: resumed - after - 1,
        reconcile: true,
    }
}

fn host(last: u64, next: u64, sequences: &[u64], gap: Option<SessionLiveGap>) -> HostPage {
    serde_json::from_value(json!({
        "last": last, "next": next, "more": next < last, "gap": gap,
        "events": sequences.iter().map(|seq| json!({
            "seq": seq, "at": "2026-11-15T00:00:00Z",
            "kind": "live_text", "turn": 1, "message": 1, "offset": seq - 1, "text": "a"
        })).collect::<Vec<_>>(),
    }))
    .unwrap()
}

#[test]
fn ring_gaps_reconcile_in_live_sequence_space() {
    let mut reader = LiveReader::default();
    let mut subscriber = subscribe(service(), "A-1".into(), 7, None).unwrap();
    let entry = Arc::clone(&subscriber.watch.entry);
    let page = reader
        .advance(
            Cursors::default(),
            lifecycle(4, 4, None),
            host(1, 1, &[1], None),
            false,
        )
        .unwrap();
    lock(&entry).publish(page);
    assert!(receive(&mut subscriber).2.is_none());

    // Only the lifecycle ring overflowed; its offsets must not leak to the app.
    let page = reader
        .advance(
            Cursors {
                lifecycle: 4,
                live: 1,
            },
            lifecycle(9, 9, Some(gap(4, 9))),
            host(1, 1, &[], None),
            false,
        )
        .unwrap();
    lock(&entry).publish(page);
    let (generation, events, reconciled) = receive(&mut subscriber);
    assert_eq!(generation, Some(1));
    assert!(events.is_empty());
    assert_eq!(reconciled, Some(gap(1, 2)));

    // The live ring can overflow independently and retains its real missing count.
    let page = reader
        .advance(
            Cursors {
                lifecycle: 9,
                live: 1,
            },
            lifecycle(9, 9, None),
            host(5, 5, &[5], Some(gap(1, 5))),
            false,
        )
        .unwrap();
    lock(&entry).publish(page);
    let (generation, events, reconciled) = receive(&mut subscriber);
    assert_eq!(generation, Some(1));
    assert_eq!(events[0].seq, 5);
    assert_eq!(reconciled, Some(gap(1, 5)));
}

#[test]
fn regression_in_either_host_ring_resets_both_cursors() {
    for lifecycle_regressed in [false, true] {
        let mut reader = LiveReader::default();
        let expected = Cursors {
            lifecycle: 3,
            live: 2,
        };
        let first = reader
            .advance(
                Cursors::default(),
                lifecycle(3, 3, None),
                host(2, 2, &[1, 2], None),
                false,
            )
            .unwrap();
        assert_eq!(first.generation, 1);
        assert_eq!(reader.cursors, expected);
        let page = reader
            .advance(
                expected,
                lifecycle(if lifecycle_regressed { 1 } else { 3 }, 3, None),
                host(if lifecycle_regressed { 2 } else { 1 }, 2, &[], None),
                false,
            )
            .unwrap();
        assert_eq!(page.generation, 2);
        assert_eq!(page.next, 0);
        assert!(page.events.is_empty());
        assert_eq!(reader.cursors, Cursors::default());
        let replayed = reader
            .advance(
                Cursors::default(),
                lifecycle(3, 3, None),
                host(2, 2, &[1, 2], None),
                false,
            )
            .unwrap();
        assert_eq!(replayed.events[0].0.generation, 2);
        assert_eq!(replayed.events[0].0.seq, 1);
        assert_eq!(reader.cursors, expected);
    }
}

fn state_delta(subscriber: &mut Subscriber) -> SubDelta {
    let frame = subscriber.next(7, false, true).unwrap().unwrap();
    let notification: rpc::Notification<SubDelta> = serde_json::from_slice(&frame.payload).unwrap();
    let SubDelta::Session { cursor, .. } = &notification.params else {
        panic!("wrong topic");
    };
    subscriber.ack(cursor.clone()).unwrap();
    notification.params
}

#[test]
fn state_is_sent_once_per_change_and_again_after_a_gap() {
    let watches = service();
    let mut current = subscribe(Arc::clone(&watches), "A-1".into(), 7, None).unwrap();
    let mut legacy = subscribe(Arc::clone(&watches), "A-1".into(), 7, None).unwrap();
    let entry = Arc::clone(&current.watch.entry);
    let mut first = host(0, 0, &[], None);
    first.state = Some(SessionState {
        turn: 1,
        ..SessionState::default()
    });
    let initial = LiveReader::default()
        .advance(Cursors::default(), lifecycle(0, 0, None), first, false)
        .unwrap();
    let first = initial.state.as_ref().unwrap().0.clone();
    lock(&entry).publish(initial);
    assert!(matches!(
        state_delta(&mut current),
        SubDelta::Session { state: Some(state), events, gap: None, .. }
            if *state == first && events.is_empty()
    ));
    assert!(legacy.next(7, false, false).unwrap().is_none());

    let mut duplicate = page(1, 1, 1, false);
    let event = &mut duplicate.events[0];
    event.0.kind = SessionLiveKind::LiveState {};
    event.1 = event_bytes(&event.0).unwrap();
    duplicate.state = Some(bound_state(first).unwrap());
    lock(&entry).publish(duplicate);
    assert!(matches!(
        state_delta(&mut current),
        SubDelta::Session { state: None, events, .. }
            if matches!(events[0].kind, SessionLiveKind::LiveState {})
    ));
    assert!(matches!(
        delta(&mut legacy),
        SubDelta::Session { state: None, events, cursor, .. }
            if events.is_empty() && cursor.rev == 1
    ));

    let mut replacement = page(1, 2, 1, false);
    replacement.state = Some(
        bound_state(SessionState {
            turn: 2,
            ..SessionState::default()
        })
        .unwrap(),
    );
    lock(&entry).publish(replacement);
    assert!(matches!(
        state_delta(&mut current),
        SubDelta::Session { state: Some(state), events, .. } if state.turn == 2 && events.is_empty()
    ));
    assert!(current.next(7, false, true).unwrap().is_none());

    let mut reconciling = page(1, 3, 3, false);
    reconciling.gap = Some(gap(1, 3));
    lock(&entry).publish(reconciling);
    let SubDelta::Session {
        state: Some(state),
        gap: Some(_),
        cursor,
        ..
    } = state_delta(&mut current)
    else {
        panic!("the gap did not resend the cached state");
    };
    assert_eq!(state.turn, 2);
    assert!(current.next(7, false, true).unwrap().is_none());

    let mut resumed = subscribe(
        watches,
        "A-1".into(),
        7,
        Some(wire::subscription::SubSince { epoch: 7, cursor }),
    )
    .unwrap();
    assert!(matches!(
        state_delta(&mut resumed),
        SubDelta::Session { state: Some(state), gap: None, .. } if state.turn == 2
    ));

    lock(&entry).unavailable();
    assert!(matches!(
        state_delta(&mut current),
        SubDelta::Session {
            generation: None,
            state: None,
            gap: Some(_),
            ..
        }
    ));
}

#[test]
fn fleet_state_and_one_event_fit_the_control_frame() {
    use crate::views::json_len;

    let mut fleet = SessionFleet {
        children: Vec::new(),
        messages: (1..=3)
            .map(|seq| SessionAgentMessage {
                seq,
                at: seq,
                from: "parent".into(),
                to: "review".into(),
                text: if seq < 3 {
                    "\0".repeat(2048)
                } else {
                    String::new()
                },
                delivery: "delivered".into(),
            })
            .collect(),
        messages_total: 3,
        max_parallel: 3,
    };
    let room = LIVE_FLEET_BYTES - json_len(&fleet).unwrap();
    fleet.messages[2].text = "\0".repeat(room / 6) + &"x".repeat(room % 6);
    let fleet = bound_fleet(fleet).unwrap();
    assert_eq!(json_len(&fleet.0).unwrap(), LIVE_FLEET_BYTES);

    let mut state = SessionState {
        can: vec![Capability {
            id: "send".into(),
            unavailable: Some(String::new()),
        }],
        ..SessionState::default()
    };
    state.can[0].unavailable = Some("x".repeat(LIVE_STATE_BYTES - json_len(&state).unwrap()));
    let state = bound_state(state).unwrap();
    assert_eq!(json_len(&state.0).unwrap(), LIVE_STATE_BYTES);

    let mut event = SessionLiveEvent {
        kind: SessionLiveKind::LiveTool {
            turn: 1,
            call: "call".into(),
            tool: String::new(),
            state: SessionLiveToolState::Started {
                args: Some("\0".repeat(1024)),
                args_truncated: None,
            },
        },
        ..event(1, 1, false)
    };
    let room = LIVE_EVENT_BYTES - json_len(&event).unwrap();
    let SessionLiveKind::LiveTool { tool, .. } = &mut event.kind else {
        unreachable!();
    };
    *tool = "x".repeat(room);
    assert_eq!(event_bytes(&event).unwrap(), LIVE_EVENT_BYTES);

    let scope = "\"".repeat(SESSION_SCOPE_BYTES);
    let mut subscriber = subscribe(service(), scope.into(), 7, None).unwrap();
    let entry = Arc::clone(&subscriber.watch.entry);
    lock(&entry).publish(LivePage {
        events: vec![(event, LIVE_EVENT_BYTES)],
        fleet: Some(fleet),
        state: Some(state),
        ..page(1, 2, 1, false)
    });
    let frame = subscriber.next(7, true, true).unwrap().unwrap();
    assert!(frame.payload.len() <= APP_CONTROL_MAX);
    let notification: rpc::Notification<SubDelta> = serde_json::from_slice(&frame.payload).unwrap();
    assert!(matches!(
        notification.params,
        SubDelta::Session { events, state: Some(_), fleet: Some(_), more: false, .. }
            if events.len() == 1
    ));
}
