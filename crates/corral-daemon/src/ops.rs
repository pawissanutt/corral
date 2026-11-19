//! Owner operations run off the connection and sync threads.
use std::{
    sync::{Arc, atomic::Ordering, mpsc},
    thread,
    time::Duration,
};

use anyhow::{Context, Result};
use corral_engine::Engine;
use corral_protocol::op::{OpReply, OpSubmit};
use corral_types::Op;

use crate::state::Shared;

const REPLY_WAIT: Duration = Duration::from_secs(5);

pub(crate) fn submit(shared: &Arc<Shared>, submit: OpSubmit) -> Result<OpReply> {
    {
        let mut state = shared.lock();
        if state.stop || state.op_workers >= crate::server::MAX_CONNECTIONS {
            return Err(WorkerBusy.into());
        }
        state.op_workers += 1;
    }
    let permit = OperationPermit(Arc::clone(shared));
    // A rendezvous makes a timeout exact: the worker can tell whether its error was delivered.
    let (send, receive) = mpsc::sync_channel(0);
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
            if let Err(mpsc::SendError(Err(error))) = send.send(result) {
                eprintln!("corrald late operation: {error:#}");
            }
            if let (true, Some(task), Some(engine)) = (succeeded, advance, engine) {
                if let Err(error) = engine.advance(&task) {
                    eprintln!("corrald advance {task}: {error:#}");
                }
                shared.publish();
            }
        })
        .context("start daemon operation worker")?;
    match receive.recv_timeout(REPLY_WAIT) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => Ok(OpReply::Running {
            seq: shared.latest_seq.load(Ordering::Acquire),
        }),
        Err(error) => Err(error).context("receive daemon operation outcome"),
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
