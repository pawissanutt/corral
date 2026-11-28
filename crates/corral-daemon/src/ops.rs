//! Owner operations run off the connection and sync threads.
use std::{
    sync::{Arc, atomic::Ordering},
    thread,
    time::Instant,
};

use anyhow::{Context, Result, anyhow};
use corral_engine::Engine;
use corral_protocol::op::{OpReply, OpSubmit};
use corral_types::Op;

use crate::{state::Shared, wake::Waker};

mod late;
use late::Outcome;
pub(crate) use late::{Late, result};

pub(crate) fn submit(shared: &Arc<Shared>, submit: OpSubmit, waker: &Arc<Waker>) -> Result<Wait> {
    {
        let mut state = shared.lock();
        if state.stop || state.op_workers >= crate::server::MAX_CONNECTIONS {
            return Err(WorkerBusy.into());
        }
        state.op_workers += 1;
    }
    let permit = OperationPermit(Arc::clone(shared));
    let outcome = Arc::new(Outcome::default());
    let complete = Complete(Some(Arc::clone(&outcome)));
    thread::Builder::new()
        .name("corrald-op".into())
        .spawn(move || {
            let shared = &permit.0;
            let advance = match &submit.op {
                Op::Done { id, .. } | Op::Verdict { id, .. } | Op::Gate { id } => Some(id.clone()),
                _ => None,
            };
            let (result, engine) = match shared.engine() {
                Ok(engine) => (apply(shared, &engine, submit), Some(engine)),
                Err(error) => (Err(error), None),
            };
            let succeeded = result.is_ok();
            shared.publish();
            shared.nudge(None);
            complete.finish(result);
            if let (true, Some(task), Some(engine)) = (succeeded, advance, engine) {
                if let Err(error) = engine.advance(&task) {
                    eprintln!("corrald advance {task}: {error:#}");
                }
                shared.publish();
            }
        })
        .context("start daemon operation worker")?;
    Ok(Wait::new(shared, outcome, WaitKind::Submit, waker))
}

/// A reply being awaited by a connection. Only that connection's loop polls and sends it.
pub(crate) struct Wait {
    outcome: Arc<Outcome>,
    deadline: Instant,
    kind: WaitKind,
    park_now: bool,
}

enum WaitKind {
    Submit,
    Follow(u64),
}

impl Wait {
    fn new(shared: &Shared, outcome: Arc<Outcome>, kind: WaitKind, waker: &Arc<Waker>) -> Self {
        let wait = shared.late_ops.wait();
        outcome.watch(waker);
        Self {
            outcome,
            deadline: Instant::now() + wait,
            kind,
            // Tests can force the delayed submit path even when the worker is already done.
            park_now: wait.is_zero(),
        }
    }

    pub(crate) fn poll(&self, shared: &Shared) -> Option<Result<OpReply>> {
        let expired = || Instant::now() >= self.deadline;
        match self.kind {
            WaitKind::Submit if self.park_now => {
                Some(Ok(late::park(shared, Arc::clone(&self.outcome))))
            }
            WaitKind::Submit => self
                .outcome
                .take()
                .or_else(|| expired().then(|| Ok(late::park(shared, Arc::clone(&self.outcome))))),
            WaitKind::Follow(n) => self
                .outcome
                .replay()
                .or_else(|| expired().then(|| Ok(late::running(shared, n)))),
        }
    }
}

struct Complete(Option<Arc<Outcome>>);

impl Complete {
    fn finish(mut self, result: Result<OpReply>) {
        if let Some(outcome) = self.0.take() {
            outcome.finish(result);
        }
    }
}

impl Drop for Complete {
    fn drop(&mut self) {
        if let Some(outcome) = self.0.take() {
            outcome.finish(Err(anyhow!("operation ended without an outcome")));
        }
    }
}

fn apply(shared: &Shared, engine: &Engine, submit: OpSubmit) -> Result<OpReply> {
    let applied = engine.apply(submit.op, submit.force)?;
    shared.latest_seq.fetch_max(applied.seq, Ordering::AcqRel);
    Ok(OpReply::Done {
        seq: applied.seq,
        task: applied.task.map(|task| task.id),
    })
}

struct OperationPermit(Arc<Shared>);

impl Drop for OperationPermit {
    fn drop(&mut self) {
        self.0.lock().op_workers -= 1;
        self.0.changed.notify_all();
    }
}

#[derive(Debug)]
pub(crate) struct WorkerBusy;

impl std::fmt::Display for WorkerBusy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("daemon is not accepting another operation")
    }
}

impl std::error::Error for WorkerBusy {}
