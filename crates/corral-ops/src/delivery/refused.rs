//! An envelope its bound session refused before admission ([`Refusal`]): that session gets no
//! further attempt, and once another session (or none) is bound its unsettled members return
//! to pending, so the replacement receives them once in a fresh envelope. Never orphaned or
//! failed: the refusal proves nothing was delivered.

use anyhow::Result;
use corral_core::inbox::{Envelope, Inbox, Recipient, Refusal};

use super::{BindOutcome, DeliveryAdapter, DeliveryStatus, blocked};
use crate::Ctx;

/// `Some(status)` while the refusing session is still bound (or the binding is ambiguous);
/// `None` once the envelope was requeued and delivery may publish its members anew.
pub(super) fn settle(
    ctx: &Ctx,
    inbox: &Inbox<'_>,
    adapter: &dyn DeliveryAdapter,
    recipient: &Recipient,
    envelope: &Envelope,
    refusal: &Refusal,
) -> Result<Option<DeliveryStatus>> {
    match adapter.bind(ctx, recipient)? {
        BindOutcome::Bound(binding) if binding == envelope.binding => {
            hold(inbox, refusal).map(Some)
        }
        BindOutcome::Ambiguous { candidates } => {
            blocked(inbox, &format!("session_ambiguous: {candidates:?}")).map(Some)
        }
        BindOutcome::Bound(_) | BindOutcome::Unbound { .. } => {
            inbox.requeue_refused(envelope, refusal)?;
            Ok(None)
        }
    }
}

/// Reported once per reason, without another send, until the session is replaced.
pub(super) fn hold(inbox: &Inbox<'_>, refusal: &Refusal) -> Result<DeliveryStatus> {
    let generation = refusal
        .generation
        .map(|n| format!(" by generation {n}"))
        .unwrap_or_default();
    blocked(
        inbox,
        &format!(
            "host_refused: {}{generation}; held for the replacement session",
            refusal.code
        ),
    )
}
