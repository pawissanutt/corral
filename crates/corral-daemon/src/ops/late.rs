//! An operation has one engine outcome. If its submit reply timed out, keep that outcome
//! under a daemon-scoped id so a lost follow-up reply can be asked for again, not retried.

use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex, MutexGuard, PoisonError, Weak,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use anyhow::{Result, anyhow};
use corral_protocol::op::{OpReply, OpResult};
use corral_types::Refusal;

use crate::{state::Shared, views::NotFound, wake::Waker};

use super::{Wait, WaitKind};

const REPLY_WAIT_MS: u64 = 5_000;
/// Operations kept for `op.result`; the oldest is dropped beyond this.
const LATE_MAX: usize = 256;
const GONE: NotFound = NotFound("operation not found or no longer kept");

/// A finished operation's outcome in a form that can be given more than once.
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

    fn replay(&self) -> Result<OpReply> {
        match self {
            Self::Reply(reply) => Ok(reply.clone()),
            Self::Refused(refusal) => Err(refusal.clone().into()),
            Self::Failed(message) => Err(anyhow!("{message}")),
        }
    }
}

#[derive(Default)]
struct State {
    // A weak reference doesn't keep a disconnected connection's descriptors open.
    waiters: Vec<Weak<Waker>>,
    ready: Option<Result<OpReply>>,
    kept: Option<Kept>,
}

#[derive(Default)]
pub(super) struct Outcome(Mutex<State>);

impl Outcome {
    fn state(&self) -> MutexGuard<'_, State> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(super) fn watch(&self, waker: &Arc<Waker>) {
        let mut state = self.state();
        if state.ready.is_none() && state.kept.is_none() {
            let waker = Arc::downgrade(waker);
            state.waiters.retain(|weak| weak.strong_count() > 0);
            if !state.waiters.iter().any(|weak| weak.ptr_eq(&waker)) {
                state.waiters.push(waker);
            }
        }
    }

    pub(super) fn finish(&self, result: Result<OpReply>) {
        let waiters = {
            let mut state = self.state();
            state.ready = Some(result);
            std::mem::take(&mut state.waiters)
        };
        for waker in waiters.into_iter().filter_map(|weak| weak.upgrade()) {
            waker.wake();
        }
    }

    pub(super) fn take(&self) -> Option<Result<OpReply>> {
        self.state().ready.take()
    }

    pub(super) fn replay(&self) -> Option<Result<OpReply>> {
        let mut state = self.state();
        if let Some(outcome) = state.ready.take() {
            state.kept = Some(Kept::of(outcome));
        }
        state.kept.as_ref().map(Kept::replay)
    }
}

impl Drop for Outcome {
    fn drop(&mut self) {
        let state = self.0.get_mut().unwrap_or_else(PoisonError::into_inner);
        if let Some(Err(error)) = state.ready.take() {
            eprintln!("corrald late operation: {error:#}");
        }
    }
}

pub(crate) struct Late {
    wait_ms: AtomicU64,
    next: AtomicU64,
    slots: Mutex<BTreeMap<u64, Arc<Outcome>>>,
}

impl Default for Late {
    fn default() -> Self {
        Self {
            wait_ms: AtomicU64::new(REPLY_WAIT_MS),
            next: AtomicU64::new(0),
            slots: Mutex::default(),
        }
    }
}

impl Late {
    pub(super) fn wait(&self) -> Duration {
        Duration::from_millis(self.wait_ms.load(Ordering::Relaxed))
    }

    #[cfg(test)]
    pub(crate) fn set_wait(&self, wait: Duration) {
        let ms = u64::try_from(wait.as_millis()).unwrap_or(u64::MAX);
        self.wait_ms.store(ms, Ordering::Relaxed);
    }

    fn slots(&self) -> MutexGuard<'_, BTreeMap<u64, Arc<Outcome>>> {
        self.slots.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn park(&self, outcome: Arc<Outcome>) -> u64 {
        let n = self.next.fetch_add(1, Ordering::Relaxed);
        let dropped = {
            let mut slots = self.slots();
            slots.insert(n, outcome);
            (slots.len() > LATE_MAX)
                .then(|| slots.pop_first())
                .flatten()
        };
        // A final reference may log an unread error; don't hold the index lock for that.
        drop(dropped);
        n
    }
}

fn operation_id(shared: &Shared, n: u64) -> String {
    format!("{:x}-{n}", shared.epoch())
}

fn operation_number(shared: &Shared, id: &str) -> Option<u64> {
    let (epoch, n) = id.split_once('-')?;
    (u64::from_str_radix(epoch, 16).ok()? == shared.epoch())
        .then(|| n.parse().ok())
        .flatten()
}

pub(super) fn running(shared: &Shared, n: u64) -> OpReply {
    OpReply::Running {
        seq: shared.latest_seq.load(Ordering::Acquire),
        operation: Some(operation_id(shared, n)),
    }
}

pub(super) fn park(shared: &Shared, outcome: Arc<Outcome>) -> OpReply {
    running(shared, shared.late_ops.park(outcome))
}

pub(crate) fn result(shared: &Shared, request: OpResult, waker: &Arc<Waker>) -> Result<Wait> {
    let n = operation_number(shared, &request.operation).ok_or(GONE)?;
    let outcome = shared.late_ops.slots().get(&n).cloned().ok_or(GONE)?;
    Ok(Wait::new(shared, outcome, WaitKind::Follow(n), waker))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An operation that outlasts its reply must still deliver its outcome (the task it
    /// filed), and again to a follow-up whose first reply was lost with the connection.
    #[test]
    fn a_late_outcome_arrives_and_is_given_again() {
        let outcome = Outcome::default();
        assert!(outcome.replay().is_none());
        let done = OpReply::Done {
            seq: 3,
            task: Some("T-7".into()),
        };
        outcome.finish(Ok(done.clone()));
        for _ in 0..2 {
            assert_eq!(outcome.replay().unwrap().unwrap(), done);
        }
    }

    /// A refusal that arrives late must still read as a refusal, with its next steps.
    #[test]
    fn a_late_refusal_keeps_its_type() {
        let outcome = Outcome::default();
        let refusal = Refusal {
            reason: "no".into(),
            next: Vec::new(),
            problems: None,
        };
        outcome.finish(Err(refusal.clone().into()));
        let error = outcome.replay().unwrap().unwrap_err();
        assert_eq!(error.downcast::<Refusal>().unwrap(), refusal);
    }
}
