//! The actual owning-sync resource seam: recover facts, publish core intent, then deliver.
use crate::{Change, Ctx, resources::ResourceState};
use anyhow::Result;
use jiff::Timestamp;

pub(crate) fn reconcile(ctx: &Ctx, state: &ResourceState, now: Timestamp) -> Result<Vec<Change>> {
    crate::resource_escalation::drain_notices(ctx)?;
    let update = crate::resource_escalation::reconcile(ctx, state, now)?;
    crate::resource_escalation::deliver(ctx, update)
}
