//! Operation replies share the connection's writer with reads and subscription pages.

use std::collections::VecDeque;

use anyhow::Result;
use corral_protocol::op::OpReply;
use serde_json::Value;

use crate::{ops, state::Shared, wake::Waker};

// Bound outstanding RPCs independently of the global limit on actual mutations.
const MAX_PENDING: usize = crate::server::MAX_CONNECTIONS;

enum Submit {
    Queued(Box<dyn FnOnce() -> Result<ops::Wait> + Send>),
    Awaiting(ops::Wait),
}

#[derive(Default)]
pub(crate) struct Pending {
    submits: VecDeque<(Value, Submit)>,
    results: Vec<(Value, ops::Wait)>,
}

impl Pending {
    fn check_capacity(&self) -> Result<()> {
        if self.submits.len() + self.results.len() >= MAX_PENDING {
            return Err(ops::WorkerBusy.into());
        }
        Ok(())
    }

    pub(crate) fn submit(
        &mut self,
        id: Value,
        start: impl FnOnce() -> Result<ops::Wait> + Send + 'static,
    ) -> Result<()> {
        self.check_capacity()?;
        self.submits
            .push_back((id, Submit::Queued(Box::new(start))));
        Ok(())
    }

    pub(crate) fn follow(
        &mut self,
        id: Value,
        start: impl FnOnce() -> Result<ops::Wait>,
    ) -> Result<()> {
        self.check_capacity()?;
        self.results.push((id, start()?));
        Ok(())
    }

    pub(crate) fn flush(
        &mut self,
        shared: &Shared,
        waker: &Waker,
        mut reply: impl FnMut(Value, Result<OpReply>) -> Result<()>,
    ) -> Result<()> {
        let mut i = 0;
        while i < self.results.len() {
            if let Some(outcome) = self.results[i].1.poll(shared) {
                let (id, _) = self.results.swap_remove(i);
                reply(id, outcome)?;
            } else {
                i += 1;
            }
        }

        // Only the head can start a worker or spend its reply wait. Poll just after
        // starting too: a worker already finished when watched won't send a wake.
        let Some((id, submit)) = self.submits.pop_front() else {
            return Ok(());
        };
        let started = match submit {
            Submit::Queued(start) => start(),
            Submit::Awaiting(wait) => Ok(wait),
        };
        let outcome = match started {
            Ok(wait) => {
                let Some(outcome) = wait.poll(shared) else {
                    self.submits.push_front((id, Submit::Awaiting(wait)));
                    return Ok(());
                };
                outcome
            }
            Err(error) => Err(error),
        };
        reply(id, outcome)?;
        // Advance only after the reply (including Running or an admission error) is
        // written. Yield to other flush work and don't make the next head idle-poll.
        if !self.submits.is_empty() {
            waker.wake();
        }
        Ok(())
    }
}
