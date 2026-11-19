use std::fs;

use corral_engine::SessionRef;
use corral_protocol::{rpc, subscription::SubDelta};

use super::*;
use crate::views::tests::Fixture;

#[test]
fn process_terminal_replaces_bounded_tail_after_ack_even_without_events() {
    let fixture = Fixture::new();
    fixture.project();
    let task = fixture.named_task("Terminal run", Some("demo"));
    let mut run = fixture.dispatch(&task);
    let dir = fixture.runner.starts().pop().unwrap().dir;
    let log = dir.join("session.log");
    fs::write(&log, "first\n").unwrap();
    run.session = Some(SessionRef::Process {
        pid: 1,
        dir: dir.clone(),
    });
    fixture
        .shared
        .engine()
        .unwrap()
        .store
        .update_run(&run)
        .unwrap();
    let scope = format!("{}_{}", run.task, run.id);
    let mut topics = TerminalTopics::default();
    topics
        .open(
            &fixture.shared,
            SubOpen {
                topic: Topic::Terminal,
                scope: Some(scope.clone()),
                since: None,
            },
        )
        .unwrap();

    let now = Instant::now();
    let delta = |topics: &mut TerminalTopics, at| {
        topics
            .subscribers
            .get_mut(&scope)
            .unwrap()
            .next_at(&fixture.shared, &scope, at)
            .unwrap()
            .map(|frame| {
                assert!(frame.payload.len() <= APP_CONTROL_MAX);
                serde_json::from_slice::<rpc::Notification<SubDelta>>(&frame.payload)
                    .unwrap()
                    .params
            })
    };
    let Some(SubDelta::Terminal {
        snapshot: Some(first),
        cursor,
        ..
    }) = delta(&mut topics, now)
    else {
        panic!("expected initial process log");
    };
    assert_eq!(first.lines, ["first"]);
    assert!(first.live);
    let mut other = TerminalTopics::default();
    other
        .open(
            &fixture.shared,
            SubOpen {
                topic: Topic::Terminal,
                scope: Some(scope.clone()),
                since: None,
            },
        )
        .unwrap();
    assert!(delta(&mut other, now).is_some());
    assert!(
        Arc::ptr_eq(
            topics.subscribers[&scope].sent.as_ref().unwrap(),
            other.subscribers[&scope].sent.as_ref().unwrap(),
        ),
        "watchers on different connections share one capture"
    );
    fs::write(&log, "second\n").unwrap();
    assert!(
        delta(&mut topics, now + POLL).is_none(),
        "backpressure requires an ack"
    );
    let ack = SubAck {
        topic: Topic::Terminal,
        scope: Some(scope.clone()),
        cursor,
    };
    topics.ack(ack.clone()).unwrap();
    assert!(topics.ack(ack).is_err());
    assert!(
        delta(&mut topics, now + POLL / 2).is_none(),
        "terminal poll rate exceeded"
    );

    let log_text = format!(
        "{}\n{}\n\x1b[32mnewest\x1b[0m\n",
        "discarded".repeat(APP_CONTROL_MAX),
        "tail\n".repeat(210)
    );
    fs::write(&log, log_text).unwrap();
    fs::write(dir.join("exit_code"), "0").unwrap();
    let Some(SubDelta::Terminal {
        snapshot: Some(latest),
        cursor,
        ..
    }) = delta(&mut topics, now + POLL)
    else {
        panic!("expected latest process log replacement");
    };
    assert_eq!(
        latest.lines.len(),
        corral_protocol::detail::TERMINAL_ROWS_MAX
    );
    assert_eq!(latest.lines.last().unwrap(), "newest");
    assert!(!latest.live);
    topics
        .ack(SubAck {
            topic: Topic::Terminal,
            scope: Some(scope.clone()),
            cursor,
        })
        .unwrap();
    assert!(
        delta(&mut topics, now + 2 * POLL).is_none(),
        "capture time is not a content delta"
    );
    topics.close(Some(scope.clone()));
    assert!(topics.subscribers.is_empty());
    other.close(Some(scope.clone()));
    assert!(
        fixture.shared.terminal_cache.entries.lock().unwrap()[&scope]
            .upgrade()
            .is_none()
    );
}
