//! Per-thread count of herdr subprocess invocations, read by the board work-budget benchmark.

use std::cell::Cell;

thread_local! {
    static INVOCATIONS: Cell<u64> = const { Cell::new(0) };
}

/// Records one herdr invocation on the current thread.
pub fn record() {
    INVOCATIONS.with(|count| count.set(count.get().saturating_add(1)));
}

/// The current thread's running total of herdr invocations.
pub fn invocations() -> u64 {
    INVOCATIONS.with(Cell::get)
}
