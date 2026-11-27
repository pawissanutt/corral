//! Where the Tasks board stands relative to the replies this device received, so a move can
//! wait for a board taken after it.
//!
//! A board position is `(subscription, walk)`. A Tasks subscription is numbered when the core
//! admits its open (after corrald answered a close, the core dropping pages of a subscription it
//! is closing, or retired the old subscriber with a reset, or on a new connection) and takes
//! effect at the first page of the replacement batch begun then. So no page of an earlier
//! subscription is counted in it, a late or expired `sub.open` reply does not matter, and an
//! open admitted after some moment is known to be later than it (`opens`).
//! Within a subscription corrald pins one walk at a time and writes a reply before any walk it
//! pins after handling the request. The client reads frames in order and hands each event to
//! the app before reading the next, so a walk past the fence read while a reply is handled was
//! taken after corrald answered it. Subscriptions are numbered across connections.
//!
//! The core records here as it reads; the app reads while it handles the event that produced it.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// A board position: the Tasks subscription and the walk completed within it.
pub(crate) type Position = (u64, u64);

static OPENS: AtomicU64 = AtomicU64::new(0);
static SUBSCRIPTION: AtomicU64 = AtomicU64::new(0);
static WALKS: AtomicU64 = AtomicU64::new(0);
static PARTIAL: AtomicBool = AtomicBool::new(false);

/// A Tasks open is admitted: its subscription's number.
pub(super) fn opened() -> u64 {
    OPENS.fetch_add(1, Ordering::AcqRel) + 1
}

/// The number of the last Tasks open admitted: a subscription numbered past it was opened
/// later.
pub(crate) fn opens() -> u64 {
    OPENS.load(Ordering::Acquire)
}

/// The first page of subscription `number` is being read.
pub(super) fn subscribed(number: u64) {
    SUBSCRIPTION.store(number, Ordering::Release);
    WALKS.store(0, Ordering::Release);
    PARTIAL.store(false, Ordering::Release);
}

/// A Tasks page of the current subscription was read; `more` says its walk continues.
pub(super) fn page(more: bool) {
    PARTIAL.store(more, Ordering::Release);
    if !more {
        WALKS.fetch_add(1, Ordering::AcqRel);
    }
}

/// The last completed walk.
pub(crate) fn completed() -> Position {
    (
        SUBSCRIPTION.load(Ordering::Acquire),
        WALKS.load(Ordering::Acquire),
    )
}

/// What a walk must be past to have been taken after the reply being handled: a walk partly
/// read may have been pinned before it.
pub(crate) fn fence() -> Position {
    let (subscription, walks) = completed();
    (
        subscription,
        walks + u64::from(PARTIAL.load(Ordering::Acquire)),
    )
}
