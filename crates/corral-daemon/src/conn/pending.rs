//! Operation replies share the connection's writer with reads and subscription pages.

use anyhow::Result;
use corral_protocol::op::OpReply;
use serde_json::Value;

use crate::{ops, state::Shared};

// Bound outstanding RPCs independently of the global limit on actual mutations.
const MAX_PENDING: usize = crate::server::MAX_CONNECTIONS;

#[derive(Default)]
pub(crate) struct Pending(Vec<(Value, ops::Wait)>);

impl Pending {
    pub(crate) fn push(
        &mut self,
        id: Value,
        start: impl FnOnce() -> Result<ops::Wait>,
    ) -> Result<()> {
        if self.0.len() >= MAX_PENDING {
            return Err(ops::WorkerBusy.into());
        }
        self.0.push((id, start()?));
        Ok(())
    }

    pub(crate) fn flush(
        &mut self,
        shared: &Shared,
        mut reply: impl FnMut(Value, Result<OpReply>) -> Result<()>,
    ) -> Result<()> {
        let mut i = 0;
        while i < self.0.len() {
            if let Some(outcome) = self.0[i].1.poll(shared) {
                let (id, _) = self.0.swap_remove(i);
                reply(id, outcome)?;
            } else {
                i += 1;
            }
        }
        Ok(())
    }
}
