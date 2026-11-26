//! Where the Tasks board stands relative to the replies this device received, so a move can
//! wait for a board taken after it.
//!
//! corrald pins one Tasks walk at a time per subscription and writes a reply before any walk it
//! pins after handling the request; the client reads frames in order and hands each event to
//! the app before reading the next. So a walk completed after a reply's fence was pinned after
//! the reply, which corrald sends once the operation committed. Walks are numbered across
//! connections: a new connection's walks are later than any earlier reply.
//!
//! The core records each Tasks page here as it reads it; the app reads the numbers while it
//! handles the event that page or reply produced.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

static WALKS: AtomicU64 = AtomicU64::new(0);
static PARTIAL: AtomicBool = AtomicBool::new(false);

/// A Tasks page was read; `more` says another page of the same walk follows.
pub(super) fn page(more: bool) {
    if more {
        PARTIAL.store(true, Ordering::Release);
    } else {
        PARTIAL.store(false, Ordering::Release);
        WALKS.fetch_add(1, Ordering::AcqRel);
    }
}

/// A new connection: a walk the old one was reading never completes.
pub(super) fn connected() {
    PARTIAL.store(false, Ordering::Release);
}

/// The number of the last completed Tasks walk.
pub(crate) fn completed() -> u64 {
    WALKS.load(Ordering::Acquire)
}

/// The number a walk must exceed to have been taken after the reply being handled: a walk
/// partly read may have been pinned before it.
pub(crate) fn fence() -> u64 {
    completed() + u64::from(PARTIAL.load(Ordering::Acquire))
}
