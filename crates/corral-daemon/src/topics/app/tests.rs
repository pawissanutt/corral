use super::*;
use corral_protocol::{
    assistant::{EntryKind, ThreadEntry},
    rpc,
    subscription::SubDelta,
};

fn entry(id: &str) -> ThreadEntry {
    ThreadEntry {
        id: id.into(),
        rev: 1,
        at: "2026-11-14T00:00:00Z".into(),
        typed_at: None,
        kind: EntryKind::Answer,
        device: None,
        via: None,
        text: Some("\0".repeat(8 * 1024)),
        state: None,
        model: None,
        cost_usd: None,
        tools: Vec::new(),
        links: Vec::new(),
        card: None,
        attachments: Vec::new(),
    }
}

#[test]
fn replacement_pages_all_removals_and_requires_exact_single_ack() {
    let shared = Shared::new();
    let _fixture = views::tests::install(&shared);
    let key = Key::new(Topic::Thread, Some("quick".into())).unwrap();
    let watch = views::watch(&shared, key.clone()).unwrap();
    views::tests::publish(&shared, &key, Rows::Thread(Vec::new()), 1);
    let known: BTreeSet<_> = (0..205).map(|n| format!("removed-{n}")).collect();
    let mut sub = Subscriber {
        watch,
        epoch: shared.epoch(),
        reset: false,
        cursor: BoardCursor::default(),
        live_next: true,
        in_flight: false,
        delivered: 0,
        known: known.clone(),
        live: None,
        pending_live: None,
        serial: 0,
        transfer: Some(Transfer {
            rev: 1,
            page: Some(Arc::new(Snapshot {
                rev: 1,
                rows: Rows::Thread(vec![entry("one"), entry("two"), entry("three")]),
            })),
            index: 0,
            pending: None,
            next: None,
            head: Some(1),
            last: true,
            remaining: known.clone(),
            seen: BTreeSet::new(),
            left: None,
            left_index: 0,
        }),
    };
    let mut topics = AppTopics::default();
    let mut removed = BTreeSet::new();
    let mut received = BTreeSet::new();
    loop {
        let (frame, close) = sub.next_transfer(&shared).unwrap().unwrap();
        assert!(!close);
        assert!(frame.payload.len() <= APP_CONTROL_MAX);
        assert!(
            sub.next(&shared).unwrap().is_none(),
            "second page leaked before ack"
        );
        let message: rpc::Notification<SubDelta> = serde_json::from_slice(&frame.payload).unwrap();
        let SubDelta::Thread { delta, .. } = message.params else {
            panic!("wrong topic")
        };
        for row in delta.rows {
            assert!(received.insert(row.id));
        }
        for id in delta.left {
            assert!(removed.insert(id));
        }
        let more = delta.more;
        topics.subscribers.insert(key.clone(), sub);
        let wrong = SubAck {
            topic: Topic::Thread,
            scope: key.scope.clone(),
            cursor: BoardCursor {
                rev: 999,
                after: None,
            },
        };
        assert!(topics.ack(wrong).is_err());
        let ack = SubAck {
            topic: Topic::Thread,
            scope: key.scope.clone(),
            cursor: delta.cursor,
        };
        topics.ack(ack.clone()).unwrap();
        assert!(topics.ack(ack).is_err(), "duplicate ack accepted");
        sub = topics.subscribers.remove(&key).unwrap();
        if !more {
            break;
        }
    }
    assert_eq!(
        received,
        ["one", "two", "three"]
            .into_iter()
            .map(str::to_owned)
            .collect()
    );
    assert_eq!(removed, known);
    assert_eq!(sub.known, received);
}
