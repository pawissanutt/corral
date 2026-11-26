use std::{
    cell::Cell,
    sync::Arc,
    time::{Duration, Instant},
};

use super::{page, service};
use crate::{
    topics::app::session::{
        SESSION_POLL, SESSION_STREAM_LINGER, SESSION_STREAM_POLL,
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
    let reads = Cell::new(0);
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
    let full = LivePage {
        more: true,
        ..page(1, 1, 32, true)
    };
    assert_eq!(at(Duration::ZERO, full), Duration::ZERO);
    assert_eq!(
        at(Duration::ZERO, page(1, 33, 33, true)),
        SESSION_STREAM_POLL
    );
    assert_eq!(
        at(SESSION_STREAM_POLL / 2, quiet(33)),
        SESSION_STREAM_POLL / 2
    );
    assert_eq!(reads.get(), 2, "a scope was read before it was due");
    assert_eq!(at(SESSION_STREAM_POLL, quiet(33)), SESSION_STREAM_POLL);
    assert_eq!(at(SESSION_STREAM_LINGER, quiet(33)), SESSION_POLL);
}
