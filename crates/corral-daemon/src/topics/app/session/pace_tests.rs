use std::{
    cell::Cell,
    sync::Arc,
    time::{Duration, Instant},
};

use super::{page, service};
use crate::{
    topics::app::session::{
        SESSION_POLL, SESSION_STREAM_BURST, SESSION_STREAM_LINGER, SESSION_STREAM_POLL,
        reader::{LivePage, Sources, pass, poll},
        subscriber::subscribe,
    },
    wake::Waker,
};

/// No events, the host's cursor still at `next`.
fn quiet(next: u64) -> LivePage {
    page(1, next + 1, next, true)
}

/// A page buffered while its connection waited for client input was sent only once that
/// wait ran out, up to half a second later, so a reply arrived in lumps.
#[test]
fn a_published_page_ends_its_watchers_wait() {
    let service = service();
    let mut sources = Sources::default();
    let waker = Arc::new(Waker::new().unwrap());
    let mut subscriber = subscribe(Arc::clone(&service), "pm".into(), 7, None).unwrap();
    subscriber.watch.wake(&waker);
    poll(
        &service,
        &mut sources,
        |_, _| Ok(page(1, 1, 1, true)),
        || false,
    );
    assert!(waker.take(), "events were buffered without waking");
    poll(&service, &mut sources, |_, _| Ok(quiet(1)), || false);
    assert!(!waker.take(), "an idle read woke the connection");
    poll(
        &service,
        &mut sources,
        |_, _| Ok(page(1, 2, 2, true)),
        || false,
    );
    assert!(waker.take());
    drop(subscriber);
    let _other = subscribe(Arc::clone(&service), "pm".into(), 7, None).unwrap();
    poll(
        &service,
        &mut sources,
        |_, _| Ok(page(1, 1, 1, true)),
        || false,
    );
    assert!(
        !waker.take(),
        "a closed subscription kept waking its connection"
    );
}

/// A host writing a reply was read ten times a second like a quiet one, and a full page
/// waited a whole pass for the rest.
#[test]
fn a_streaming_scope_is_read_sooner_than_a_quiet_one() {
    let service = service();
    let mut sources = Sources::default();
    let _watch = subscribe(Arc::clone(&service), "pm".into(), 7, None).unwrap();
    let start = Instant::now();
    let now = Cell::new(start);
    let reads = Cell::new(0u64);
    let mut at = |after: Duration, next: LivePage| {
        now.set(start + after);
        let mut next = Some(next);
        pass(
            &service,
            &mut sources,
            || Some(now.get()),
            |_, _| {
                reads.set(reads.get() + 1);
                Ok(next.take().unwrap())
            },
            || false,
        )
    };
    let full = |n: u64| LivePage {
        more: true,
        ..page(1, n * 32 + 1, (n + 1) * 32, true)
    };
    let burst = u64::from(SESSION_STREAM_BURST);
    for n in 0..burst {
        assert_eq!(at(Duration::ZERO, full(n)), Duration::ZERO);
    }
    assert_eq!(
        at(Duration::ZERO, full(burst)),
        SESSION_STREAM_POLL,
        "a backlog held the worker without end"
    );
    let last = (burst + 1) * 32;
    assert_eq!(
        at(SESSION_STREAM_POLL / 2, quiet(last)),
        SESSION_STREAM_POLL / 2
    );
    assert_eq!(reads.get(), burst + 1, "a scope was read before it was due");
    assert_eq!(at(SESSION_STREAM_POLL, quiet(last)), SESSION_STREAM_POLL);
    assert_eq!(at(SESSION_STREAM_LINGER, quiet(last)), SESSION_POLL);
}

/// How long until a scope was due was measured as it was visited, so the time the reads
/// after it took was waited a second time.
#[test]
fn the_time_later_reads_took_is_not_waited_again() {
    let service = service();
    let mut sources = Sources::default();
    let _streaming = subscribe(Arc::clone(&service), "a".into(), 7, None).unwrap();
    let _slow = subscribe(Arc::clone(&service), "b".into(), 7, None).unwrap();
    let now = Cell::new(Instant::now());
    let wait = pass(
        &service,
        &mut sources,
        || Some(now.get()),
        |scope, _| {
            if scope == "a" {
                return Ok(page(1, 1, 1, true));
            }
            now.set(now.get() + SESSION_STREAM_POLL * 2);
            Ok(quiet(0))
        },
        || false,
    );
    assert_eq!(wait, Duration::ZERO, "an overdue scope waited");
}
