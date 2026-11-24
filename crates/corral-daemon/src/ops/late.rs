//! Operations that outlasted their `op.submit` reply. Each outcome is kept until one
//! `op.result` takes it, so a client follows the original submission to its end instead of
//! submitting again or guessing from what changed. Retention is bounded: the oldest outcome
//! is dropped first (its error logged), and a dropped one reads as not found.

use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::Duration,
};

use anyhow::{Result, anyhow};
use corral_protocol::op::{OpReply, OpResult};

use crate::{state::Shared, views::NotFound};

const REPLY_WAIT_MS: u64 = 5_000;
/// Outcomes kept for `op.result`; the oldest is dropped beyond this.
const LATE_MAX: usize = 256;
const GONE: NotFound = NotFound("operation not found or its outcome was already returned");

pub(super) type Outcome = mpsc::Receiver<Result<OpReply>>;

pub(crate) struct Late {
    wait_ms: AtomicU64,
    next: AtomicU64,
    waiting: Mutex<BTreeMap<u64, Arc<Mutex<Outcome>>>>,
}

impl Default for Late {
    fn default() -> Self {
        Self {
            wait_ms: AtomicU64::new(REPLY_WAIT_MS),
            next: AtomicU64::new(0),
            waiting: Mutex::default(),
        }
    }
}

impl Late {
    /// How long a reply waits for an operation's outcome.
    pub(super) fn wait(&self) -> Duration {
        Duration::from_millis(self.wait_ms.load(Ordering::Relaxed))
    }

    #[cfg(test)]
    pub(crate) fn set_wait(&self, wait: Duration) {
        let ms = u64::try_from(wait.as_millis()).unwrap_or(u64::MAX);
        self.wait_ms.store(ms, Ordering::Relaxed);
    }

    /// Keeps `outcome` under a new number.
    fn park(&self, outcome: Outcome) -> u64 {
        let n = self.next.fetch_add(1, Ordering::Relaxed);
        let mut waiting = self.waiting.lock().unwrap_or_else(PoisonError::into_inner);
        waiting.insert(n, Arc::new(Mutex::new(outcome)));
        while waiting.len() > LATE_MAX {
            let Some((_, dropped)) = waiting.pop_first() else {
                break;
            };
            let dropped = dropped.lock().unwrap_or_else(PoisonError::into_inner);
            if let Ok(Err(error)) = dropped.try_recv() {
                eprintln!("corrald late operation: {error:#}");
            }
        }
        n
    }

    /// The outcome of operation `n` within `wait`, once; `None` while it is still running.
    fn take(&self, n: u64, wait: Duration) -> Result<Option<Result<OpReply>>> {
        let slot = self
            .waiting
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&n)
            .cloned()
            .ok_or(GONE)?;
        let received = slot
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .recv_timeout(wait);
        let outcome = match received {
            Ok(outcome) => outcome,
            Err(mpsc::RecvTimeoutError::Timeout) => return Ok(None),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                Err(anyhow!("operation ended without an outcome"))
            }
        };
        self.waiting
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&n);
        Ok(Some(outcome))
    }
}

/// An id names its daemon too, so a client of an earlier daemon cannot take another's outcome.
fn operation_id(shared: &Shared, n: u64) -> String {
    format!("{:x}-{n}", shared.epoch())
}

fn operation_number(shared: &Shared, id: &str) -> Option<u64> {
    let (epoch, n) = id.split_once('-')?;
    (u64::from_str_radix(epoch, 16).ok()? == shared.epoch())
        .then(|| n.parse().ok())
        .flatten()
}

fn running(shared: &Shared, n: u64) -> OpReply {
    OpReply::Running {
        seq: shared.latest_seq.load(Ordering::Acquire),
        operation: Some(operation_id(shared, n)),
    }
}

/// Parks an outcome the reply did not wait for and answers `running` with its id.
pub(super) fn park(shared: &Shared, outcome: Outcome) -> OpReply {
    let n = shared.late_ops.park(outcome);
    running(shared, n)
}

/// `op.result`: the parked outcome, or `running` again after the reply wait.
pub(crate) fn result(shared: &Shared, request: OpResult) -> Result<OpReply> {
    let n = operation_number(shared, &request.operation).ok_or(GONE)?;
    match shared.late_ops.take(n, shared.late_ops.wait())? {
        Some(outcome) => outcome,
        None => Ok(running(shared, n)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An operation that outlasts its reply must still deliver its outcome (the task it
    /// filed) to the one follow-up that asks, and only once.
    #[test]
    fn a_late_outcome_is_returned_once_after_it_arrives() {
        let late = Late::default();
        let (send, receive) = mpsc::sync_channel(1);
        let n = late.park(receive);
        assert!(late.take(n, Duration::ZERO).unwrap().is_none());
        let done = OpReply::Done {
            seq: 3,
            task: Some("T-7".into()),
        };
        send.send(Ok(done.clone())).unwrap();
        let outcome = late.take(n, Duration::ZERO).unwrap().unwrap().unwrap();
        assert_eq!(outcome, done);
        assert!(late.take(n, Duration::ZERO).is_err());
    }
}
