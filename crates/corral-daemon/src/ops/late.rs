//! Operations that outlasted their `op.submit` reply. Each is followed by `op.result` until
//! it finishes; its outcome is then kept and given again on every later `op.result`, so a
//! reply lost with the connection can be asked for again instead of submitting again or
//! guessing from what changed. Retention is bounded: the oldest operation is dropped first
//! (an outcome nobody read is logged when it was an error), and a dropped one reads as not
//! found. One reader receives at a time; others wait for the outcome on a condition variable.

use std::{
    collections::BTreeMap,
    sync::{
        Condvar, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use anyhow::{Result, anyhow};
use corral_protocol::op::{OpReply, OpResult};
use corral_types::Refusal;

use crate::{state::Shared, views::NotFound};

const REPLY_WAIT_MS: u64 = 5_000;
/// Operations kept for `op.result`; the oldest is dropped beyond this.
const LATE_MAX: usize = 256;
const GONE: NotFound = NotFound("operation not found or no longer kept");

pub(super) type Outcome = mpsc::Receiver<Result<OpReply>>;

/// A finished operation's outcome in a form that can be given more than once.
#[derive(Clone)]
enum Kept {
    Reply(OpReply),
    Refused(Refusal),
    Failed(String),
}

impl Kept {
    fn of(outcome: Result<OpReply>) -> Self {
        match outcome {
            Ok(reply) => Self::Reply(reply),
            Err(error) => match error.downcast::<Refusal>() {
                Ok(refusal) => Self::Refused(refusal),
                Err(error) => Self::Failed(format!("{error:#}")),
            },
        }
    }

    /// The outcome as `op.submit` would have answered it; a refusal keeps its type.
    fn replay(&self) -> Result<OpReply> {
        match self {
            Self::Reply(reply) => Ok(reply.clone()),
            Self::Refused(refusal) => Err(refusal.clone().into()),
            Self::Failed(message) => Err(anyhow!("{message}")),
        }
    }
}

enum Slot {
    /// Running; nobody is receiving its outcome.
    Waiting(Outcome),
    /// Running; one `op.result` is receiving its outcome.
    Receiving,
    Finished(Kept),
}

pub(crate) struct Late {
    wait_ms: AtomicU64,
    next: AtomicU64,
    slots: Mutex<BTreeMap<u64, Slot>>,
    changed: Condvar,
}

impl Default for Late {
    fn default() -> Self {
        Self {
            wait_ms: AtomicU64::new(REPLY_WAIT_MS),
            next: AtomicU64::new(0),
            slots: Mutex::default(),
            changed: Condvar::new(),
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

    fn slots(&self) -> MutexGuard<'_, BTreeMap<u64, Slot>> {
        self.slots.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Keeps `outcome` under a new number.
    fn park(&self, outcome: Outcome) -> u64 {
        let n = self.next.fetch_add(1, Ordering::Relaxed);
        let mut slots = self.slots();
        slots.insert(n, Slot::Waiting(outcome));
        while slots.len() > LATE_MAX {
            if let Some((_, Slot::Waiting(dropped))) = slots.pop_first()
                && let Ok(Err(error)) = dropped.try_recv()
            {
                eprintln!("corrald late operation: {error:#}");
            }
        }
        n
    }

    /// Operation `n`'s outcome, waiting up to `wait`; `None` while it is still running.
    fn take(&self, n: u64, wait: Duration) -> Result<Option<Result<OpReply>>> {
        let deadline = Instant::now() + wait;
        let mut slots = self.slots();
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match slots.get_mut(&n) {
                None => return Err(GONE.into()),
                Some(Slot::Finished(kept)) => return Ok(Some(kept.replay())),
                Some(Slot::Receiving) => {
                    if left.is_zero() {
                        return Ok(None);
                    }
                    slots = self
                        .changed
                        .wait_timeout(slots, left)
                        .unwrap_or_else(PoisonError::into_inner)
                        .0;
                }
                Some(slot) => {
                    let Slot::Waiting(outcome) = std::mem::replace(slot, Slot::Receiving) else {
                        return Ok(None);
                    };
                    drop(slots);
                    let received = outcome.recv_timeout(left);
                    slots = self.slots();
                    let next = match received {
                        Ok(outcome) => Slot::Finished(Kept::of(outcome)),
                        Err(mpsc::RecvTimeoutError::Timeout) => Slot::Waiting(outcome),
                        Err(mpsc::RecvTimeoutError::Disconnected) => Slot::Finished(Kept::Failed(
                            "operation ended without an outcome".into(),
                        )),
                    };
                    // Dropped while it was received: the outcome still goes to this reader.
                    let kept = match &next {
                        Slot::Finished(kept) => Some(kept.replay()),
                        _ => None,
                    };
                    if let Some(slot) = slots.get_mut(&n) {
                        *slot = next;
                    }
                    self.changed.notify_all();
                    return Ok(kept);
                }
            }
        }
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

/// `op.result`: the operation's outcome, or `running` again after the reply wait.
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
    /// filed), and again to a follow-up whose first reply was lost with the connection.
    #[test]
    fn a_late_outcome_arrives_and_is_given_again() {
        let late = Late::default();
        let (send, receive) = mpsc::sync_channel(1);
        let n = late.park(receive);
        assert!(late.take(n, Duration::ZERO).unwrap().is_none());
        let done = OpReply::Done {
            seq: 3,
            task: Some("T-7".into()),
        };
        send.send(Ok(done.clone())).unwrap();
        for _ in 0..2 {
            let outcome = late.take(n, Duration::ZERO).unwrap().unwrap().unwrap();
            assert_eq!(outcome, done);
        }
    }

    /// A refusal that arrives late must still read as a refusal, with its next steps.
    #[test]
    fn a_late_refusal_keeps_its_type() {
        let late = Late::default();
        let (send, receive) = mpsc::sync_channel(1);
        let n = late.park(receive);
        let refusal = Refusal {
            reason: "no".into(),
            next: Vec::new(),
            problems: None,
        };
        send.send(Err(refusal.clone().into())).unwrap();
        let error = late.take(n, Duration::ZERO).unwrap().unwrap().unwrap_err();
        assert_eq!(error.downcast::<Refusal>().unwrap(), refusal);
    }
}
