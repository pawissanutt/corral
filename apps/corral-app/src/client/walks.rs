//! Whether the Tasks board was taken after a moment this device knows of, so a move can wait
//! for a board taken after corrald answered it.
//!
//! Every Tasks open the core admits gets the next number here. The core admits an open only
//! after corrald answered the close of the subscription before it (dropping that one's pages
//! meanwhile), after a reset retired it, or on a new connection; corrald takes the snapshot of
//! an open after reading it. So a board from a subscription numbered past `opens()` read at
//! some moment was taken after that moment. The core tags each Tasks batch with its
//! subscription's number (`Event::Rows::subscription`).

use std::sync::atomic::{AtomicU64, Ordering};

static OPENS: AtomicU64 = AtomicU64::new(0);

/// A Tasks open is admitted: its subscription's number.
pub(super) fn opened() -> u64 {
    OPENS.fetch_add(1, Ordering::AcqRel) + 1
}

/// The number of the last Tasks open admitted.
pub(crate) fn opens() -> u64 {
    OPENS.load(Ordering::Acquire)
}
