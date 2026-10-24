//! The thread-local write-transaction guard (plan T-253 section 2): long work never runs inside
//! `apply`, so an external call (git, herdr, a process spawn) made while this thread holds an
//! open write transaction is a bug. Debug builds panic, naming the call.

use std::cell::Cell;

thread_local! {
    static IN_WRITE: Cell<bool> = const { Cell::new(false) };
}

pub(super) struct WriteScope(());

impl WriteScope {
    pub(super) fn enter() -> Self {
        IN_WRITE.with(|flag| flag.set(true));
        Self(())
    }
}

impl Drop for WriteScope {
    fn drop(&mut self) {
        IN_WRITE.with(|flag| flag.set(false));
    }
}

pub fn in_write_transaction() -> bool {
    IN_WRITE.with(Cell::get)
}

/// Wrappers of external effects call this before they run `call`. In debug builds it panics
/// when this thread holds an open storage write transaction.
pub fn external_call(call: &str) {
    if cfg!(debug_assertions) && in_write_transaction() {
        panic!("external call `{call}` attempted while a storage write transaction is open");
    }
}
