//! Bounded idle pool and revocable, age-limited WAL read transactions.
use std::ops::Deref;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, Instant};

use rusqlite::Connection;

use crate::backend::{Clock, ViewError};

pub(super) const MAX_IDLE_READERS: usize = 2;
pub(super) type ActiveReaders = Mutex<Vec<Weak<Mutex<ReadLease>>>>;

pub(super) struct ReadLease {
    pub conn: Connection,
    opened: jiff::Timestamp,
    started: Instant,
    clock: Arc<dyn Clock>,
    max_age: Duration,
    pub expired: bool,
}

impl ReadLease {
    pub fn new(conn: Connection, clock: Arc<dyn Clock>, max_age: Duration) -> Self {
        Self {
            conn,
            opened: clock.now(),
            started: Instant::now(),
            clock,
            max_age,
            expired: false,
        }
    }

    fn age(&self) -> Duration {
        super::codec::elapsed(self.opened, self.clock.now()).max(self.started.elapsed())
    }

    fn expire(&mut self) -> Result<(), ViewError> {
        if !self.expired && self.age() > self.max_age {
            // Drop the file handle as well as its WAL read mark. The inert in-memory
            // connection keeps guard borrowing simple; expired views can never query it.
            let closed = Connection::open_in_memory()?;
            self.conn.execute_batch("ROLLBACK")?;
            self.conn = closed;
            self.expired = true;
        }
        Ok(())
    }
}

pub(super) struct ReadGuard<'a>(MutexGuard<'a, ReadLease>);

impl Deref for ReadGuard<'_> {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        &self.0.conn
    }
}

pub(super) fn fresh(lease: &Mutex<ReadLease>) -> Result<ReadGuard<'_>, ViewError> {
    let mut lease = lease.lock().unwrap_or_else(PoisonError::into_inner);
    lease.expire()?;
    if lease.expired {
        return Err(ViewError::Expired { age: lease.age() });
    }
    Ok(ReadGuard(lease))
}

/// An idle expired view must not pin old WAL frames merely because its owner never reads
/// again. Never wait for a currently executing query; the next maintenance pass retries it.
pub(super) fn expire_idle(readers: &ActiveReaders) -> anyhow::Result<()> {
    let mut readers = readers.lock().unwrap_or_else(PoisonError::into_inner);
    readers.retain(|reader| reader.strong_count() > 0);
    for reader in readers.iter().filter_map(Weak::upgrade) {
        if let Ok(mut lease) = reader.try_lock() {
            lease.expire()?;
        }
    }
    Ok(())
}

pub(super) fn recycle(pool: &Mutex<Vec<Connection>>, conn: Connection) {
    let mut pool = pool.lock().unwrap_or_else(PoisonError::into_inner);
    if pool.len() < MAX_IDLE_READERS {
        pool.push(conn);
    }
}
