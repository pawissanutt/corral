use super::*;
use corral_protocol::{rpc, subscription::Topic};

fn watch(service: &Arc<Service>, scope: &str) -> Subscriber {
    let entry = {
        let mut entries = lock(&service.entries);
        let entry = entries.entry(scope.into()).or_default();
        lock(entry).users += 1;
        Arc::clone(entry)
    };
    Subscriber {
        watch: Watch {
            service: Arc::clone(service),
            scope: scope.into(),
            entry,
        },
        delivered: 0,
        in_flight: None,
    }
}

fn snapshot(text: &str) -> TerminalSnapshot {
    TerminalSnapshot {
        lines: vec![text.into()],
        rows: Vec::new(),
        live: true,
        captured_at: "2026-11-15T00:00:00Z".into(),
    }
}

fn delta(frame: &Frame) -> SubDelta {
    assert!(frame.payload.len() <= APP_CONTROL_MAX);
    serde_json::from_slice::<rpc::Notification<SubDelta>>(&frame.payload)
        .unwrap()
        .params
}

#[test]
fn terminal_stream_bounds_rate_size_and_slow_consumers_without_output_queues() {
    let service = Arc::new(Service::default());
    let mut fast = watch(&service, "pm");
    let slow = watch(&service, "pm");
    let mut topics = TerminalTopics::default();
    topics.subscribers.insert("pm".into(), slow);
    let now = Instant::now();
    let mut reads = 0;
    service.poll(now, 7, |_| {
        reads += 1;
        Ok(snapshot("first"))
    });
    let first = fast.next().unwrap();
    let slow_first = topics.subscribers.get_mut("pm").unwrap().next().unwrap();
    assert!(
        Arc::ptr_eq(&first, &slow_first),
        "one capture must serve both devices"
    );
    let SubDelta::Terminal {
        cursor,
        snapshot: Some(first),
        ..
    } = delta(&first)
    else {
        panic!("missing first terminal window");
    };
    assert_eq!(first.lines, ["first"]);
    assert!(fast.next().is_none(), "second frame escaped before ack");
    for millis in [1, 10, 100, 999] {
        service.poll(now + Duration::from_millis(millis), 7, |_| {
            reads += 1;
            Ok(snapshot("too early"))
        });
    }
    assert_eq!(reads, 1);
    for second in 1..=3 {
        service.poll(now + Duration::from_secs(second), 7, |_| {
            reads += 1;
            Ok(snapshot(&format!("window-{second}")))
        });
    }
    assert_eq!(reads, 4, "read once per scope, not per device");
    assert!(topics.subscribers.get_mut("pm").unwrap().next().is_none());
    let ack = SubAck {
        topic: Topic::Terminal,
        scope: Some("pm".into()),
        cursor,
    };
    topics.ack(ack.clone()).unwrap();
    assert!(topics.ack(ack).is_err(), "duplicate ack must not advance");
    let SubDelta::Terminal {
        snapshot: Some(latest),
        cursor,
        ..
    } = delta(&topics.subscribers.get_mut("pm").unwrap().next().unwrap())
    else {
        panic!("missing latest terminal window");
    };
    assert_eq!(
        latest.lines,
        ["window-3"],
        "slow device receives only the newest replacement"
    );
    topics
        .ack(SubAck {
            topic: Topic::Terminal,
            scope: Some("pm".into()),
            cursor,
        })
        .unwrap();
    service.poll(now + Duration::from_secs(4), 7, |_| {
        Ok(snapshot(&"x".repeat(APP_CONTROL_MAX)))
    });
    let SubDelta::Terminal {
        snapshot, error, ..
    } = delta(&topics.subscribers.get_mut("pm").unwrap().next().unwrap())
    else {
        panic!("missing bounded refusal");
    };
    assert!(snapshot.is_none());
    assert!(error.is_some());
    drop(fast);
    topics.close(Some("pm".into()));
    assert!(
        lock(&service.entries).is_empty(),
        "closing the final watch releases its buffer"
    );
    service.poll(now + Duration::from_secs(5), 7, |_| {
        panic!("closed scope was read")
    });
}
