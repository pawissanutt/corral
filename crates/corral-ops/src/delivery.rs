//! Inbox delivery policy, deliberately disconnected from production callers in phase 1.

use std::time::Duration;

use anyhow::{Context, Result, ensure};
pub use corral_core::inbox::{
    Envelope, ObserveCursor, Recipient as RecipientRef, SendOutcome, SessionBinding,
};
use corral_core::inbox::{Inbox, Receipt, ReceiptLevel, Recipient, Stage, TerminalState};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::Ctx;

mod budget;
pub mod effects;
#[cfg(test)]
mod tests;

#[derive(Debug, Clone)]
pub struct Capabilities {
    pub idempotent_retry: bool,
    pub admission: bool,
    pub non_waking: bool,
    pub max_payload: usize,
    pub send_budget: Option<SendBudget>,
    pub transcript_reader: bool,
}

#[derive(Debug, Clone)]
pub struct SendBudget {
    pub per_recipient_interval: Duration,
    pub per_minute_total: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockReason {
    PolicyRefused,
    InboundHold,
    ProtocolUnverified,
    ComposerBusy,
    UnverifiedDraft,
    SessionUnbound,
    SessionAmbiguous,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Readiness {
    Native,
    Unavailable { reason: String },
    Blocked { reason: BlockReason },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindOutcome {
    Bound(SessionBinding),
    Unbound { reason: String },
    Ambiguous { candidates: Vec<String> },
}

#[derive(Debug, Clone)]
pub enum Observation {
    Delivered {
        envelope: u64,
        members: Vec<u64>,
        detail: Value,
    },
    Admitted {
        envelope: u64,
        detail: Value,
    },
    Uncertain {
        envelope: u64,
        reason: String,
    },
}

pub trait DeliveryAdapter {
    fn harness(&self) -> &'static str;
    fn capabilities(&self) -> Capabilities;
    fn readiness(&self, ctx: &Ctx, recipient: &RecipientRef) -> Result<Readiness>;
    fn bind(&self, ctx: &Ctx, recipient: &RecipientRef) -> Result<BindOutcome>;
    fn send(&self, ctx: &Ctx, binding: &SessionBinding, envelope: &Envelope) -> SendOutcome;
    fn observe(
        &self,
        ctx: &Ctx,
        binding: &SessionBinding,
        cursor: &mut ObserveCursor,
    ) -> Result<Vec<Observation>>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeliveryStatus {
    Idle,
    Waiting,
    Deferred,
    Blocked { reason: String },
    Uncertain,
    Orphaned,
    Sent { envelope: u64 },
}

/// No fallback is considered after publication; the saved adapter and binding are immutable.
pub fn deliver(
    ctx: &Ctx,
    recipient: &Recipient,
    adapter: &dyn DeliveryAdapter,
    fallback: Option<&dyn DeliveryAdapter>,
) -> Result<DeliveryStatus> {
    deliver_at(ctx, recipient, adapter, fallback, Timestamp::now())
}

/// Explicit time makes rate and alert policy deterministic in tests.
pub fn deliver_at(
    ctx: &Ctx,
    recipient: &Recipient,
    adapter: &dyn DeliveryAdapter,
    fallback: Option<&dyn DeliveryAdapter>,
    now: Timestamp,
) -> Result<DeliveryStatus> {
    let inbox = Inbox::new(&ctx.store, recipient.clone())?;
    let _deliver = inbox.delivery_lock()?;
    inbox.recover_publication()?;
    alert_unobserved(&inbox, now)?;
    let mut open = inbox
        .state()?
        .open_envelope
        .map(|n| inbox.envelope(n))
        .transpose()?;
    let selected = if let Some(envelope) = &open {
        if envelope.adapter == adapter.harness() {
            adapter
        } else if let Some(fallback) = fallback.filter(|a| a.harness() == envelope.adapter) {
            fallback
        } else {
            return blocked(&inbox, "published adapter unavailable");
        }
    } else {
        match adapter.readiness(ctx, recipient)? {
            Readiness::Native => adapter,
            Readiness::Blocked { reason } => return blocked(&inbox, &format!("{reason:?}")),
            Readiness::Unavailable { reason } => {
                let Some(fallback) = fallback.filter(|_| adapter.capabilities().transcript_reader)
                else {
                    return blocked(&inbox, &reason);
                };
                match fallback.readiness(ctx, recipient)? {
                    Readiness::Native => fallback,
                    Readiness::Unavailable { reason } => return blocked(&inbox, &reason),
                    Readiness::Blocked { reason } => {
                        return blocked(&inbox, &format!("{reason:?}"));
                    }
                }
            }
        }
    };
    if let Some(envelope) = &mut open {
        if envelope
            .attempts
            .last()
            .is_some_and(|a| a.outcome.is_none())
        {
            inbox.finish_attempt(
                envelope,
                SendOutcome::Uncertain {
                    reason: "interrupted dispatching attempt".into(),
                },
            )?;
        }
        observe(ctx, &inbox, selected, &envelope.binding, now)?;
        *envelope = inbox.envelope(envelope.number)?;
        if inbox.close_settled(envelope)? {
            // Select readiness afresh on the next pass; never carry a former fallback
            // selection into a new publication without checking the native adapter.
            return Ok(DeliveryStatus::Idle);
        }
    }
    let caps = selected.capabilities();
    if let Some(envelope) = &open
        && let Some(SendOutcome::Rejected { reason, .. }) = &envelope.outcome
    {
        fail(&inbox, envelope, reason)?;
        return Ok(DeliveryStatus::Waiting);
    }
    // Bind is revalidated even for admitted or held messages; old session evidence is retained.
    let binding = match selected.bind(ctx, recipient)? {
        BindOutcome::Bound(binding) => binding,
        BindOutcome::Unbound { reason } => {
            if open.as_ref().is_some_and(|e| !e.attempts.is_empty()) {
                return orphaned(&inbox, open.as_ref().context("open envelope")?, &reason);
            }
            return blocked(&inbox, &format!("session_unbound: {reason}"));
        }
        BindOutcome::Ambiguous { candidates } => {
            return blocked(&inbox, &format!("session_ambiguous: {candidates:?}"));
        }
    };
    if let Some(envelope) = &open
        && envelope.binding != binding
    {
        if !envelope.attempts.is_empty() {
            return orphaned(&inbox, envelope, "bound session replaced");
        }
        // Even an unattempted publication is immutable: retire it, then bind its members anew.
        inbox.retire_unattempted(envelope)?;
        open = None;
    }
    if let Some(envelope) = &open {
        match &envelope.outcome {
            Some(
                SendOutcome::Admitted { .. } | SendOutcome::Accepted | SendOutcome::Held { .. },
            ) => return Ok(DeliveryStatus::Waiting),
            Some(SendOutcome::Uncertain { reason }) if !caps.idempotent_retry => {
                inbox.event(
                    "delivery_uncertain",
                    json!({"envelope":envelope.number,"reason":reason}),
                )?;
                return Ok(DeliveryStatus::Uncertain);
            }
            Some(SendOutcome::Rejected { reason, .. }) => {
                fail(&inbox, envelope, reason)?;
                return Ok(DeliveryStatus::Waiting);
            }
            Some(SendOutcome::Uncertain { .. }) | None => {}
        }
    }
    match selected.readiness(ctx, recipient)? {
        Readiness::Native => {}
        Readiness::Unavailable { reason } => return blocked(&inbox, &reason),
        Readiness::Blocked { reason } => return blocked(&inbox, &format!("{reason:?}")),
    }
    if open.is_none() && !inbox.has_pending_delivery(inbox.flush_requested())? {
        return Ok(DeliveryStatus::Idle);
    }
    if !budget::reserve(
        ctx,
        &inbox,
        selected.harness(),
        caps.send_budget.as_ref(),
        now,
    )? {
        return Ok(DeliveryStatus::Deferred);
    }
    let mut envelope = match open {
        Some(envelope) => envelope,
        None => {
            let Some(envelope) = inbox.publish(
                &binding,
                selected.harness(),
                caps.max_payload,
                inbox.flush_requested(),
            )?
            else {
                return Ok(DeliveryStatus::Idle);
            };
            inbox.consume_flush()?;
            envelope
        }
    };
    // Verify bytes before journaling the external side effect. Adapters read this same file.
    inbox.payload(&envelope)?;
    inbox.begin_attempt(&mut envelope, now)?;
    let outcome = selected.send(ctx, &binding, &envelope);
    inbox.finish_attempt(&mut envelope, outcome.clone())?;
    match outcome {
        SendOutcome::Admitted { detail } => admitted(&inbox, &envelope, detail, now)?,
        SendOutcome::Rejected { reason, .. } => fail(&inbox, &envelope, &reason)?,
        SendOutcome::Uncertain { reason } => inbox.event(
            "delivery_uncertain",
            json!({"envelope":envelope.number,"reason":reason}),
        )?,
        SendOutcome::Accepted | SendOutcome::Held { .. } => {}
    }
    Ok(DeliveryStatus::Sent {
        envelope: envelope.number,
    })
}

fn blocked(inbox: &Inbox<'_>, reason: &str) -> Result<DeliveryStatus> {
    inbox.event("delivery_blocked", json!({"reason":reason}))?;
    Ok(DeliveryStatus::Blocked {
        reason: reason.into(),
    })
}

fn orphaned(inbox: &Inbox<'_>, envelope: &Envelope, reason: &str) -> Result<DeliveryStatus> {
    inbox.event(
        "delivery_orphaned",
        json!({"envelope":envelope.number,"reason":reason}),
    )?;
    Ok(DeliveryStatus::Orphaned)
}

fn fail(inbox: &Inbox<'_>, envelope: &Envelope, reason: &str) -> Result<()> {
    for seq in &envelope.members {
        inbox.mark_terminal(*seq, TerminalState::Failed, reason, &envelope.adapter)?;
    }
    inbox.close_settled(envelope)?;
    Ok(())
}

fn admitted(inbox: &Inbox<'_>, envelope: &Envelope, detail: Value, now: Timestamp) -> Result<()> {
    for seq in &envelope.members {
        inbox.record_receipt(
            *seq,
            ReceiptLevel::Admitted,
            &Receipt {
                envelope: Some(envelope.number),
                binding: Some(envelope.binding.clone()),
                observed_at: now,
                via: envelope.adapter.clone(),
                detail: detail.clone(),
            },
        )?;
    }
    Ok(())
}

fn observe(
    ctx: &Ctx,
    inbox: &Inbox<'_>,
    adapter: &dyn DeliveryAdapter,
    binding: &SessionBinding,
    now: Timestamp,
) -> Result<()> {
    let mut cursor = inbox.state()?.observe_cursor;
    for observation in adapter.observe(ctx, binding, &mut cursor)? {
        let number = match &observation {
            Observation::Delivered { envelope, .. }
            | Observation::Admitted { envelope, .. }
            | Observation::Uncertain { envelope, .. } => *envelope,
        };
        let mut envelope = inbox.envelope(number)?;
        ensure!(
            &envelope.binding == binding && envelope.adapter == adapter.harness(),
            "observation binding mismatch"
        );
        match observation {
            Observation::Delivered {
                members, detail, ..
            } => {
                ensure!(
                    members == envelope.members,
                    "observation must cover complete envelope"
                );
                for seq in members {
                    inbox.record_receipt(
                        seq,
                        ReceiptLevel::Observed,
                        &Receipt {
                            envelope: Some(number),
                            binding: Some(binding.clone()),
                            observed_at: now,
                            via: adapter.harness().into(),
                            detail: detail.clone(),
                        },
                    )?;
                }
                envelope.stages.push(Stage::Observed);
            }
            Observation::Admitted { detail, .. } => {
                admitted(inbox, &envelope, detail.clone(), now)?;
                if !inbox.settled(&envelope)? {
                    envelope.outcome = Some(SendOutcome::Admitted { detail });
                }
            }
            Observation::Uncertain { reason, .. } => {
                if !inbox.settled(&envelope)? {
                    envelope.outcome = Some(SendOutcome::Uncertain { reason });
                }
            }
        }
        inbox.save_envelope(&envelope)?;
    }
    let _inbox = inbox.lock()?;
    inbox.update_state(|state| {
        state.observe_cursor = cursor;
        Ok(())
    })
}

fn alert_unobserved(inbox: &Inbox<'_>, now: Timestamp) -> Result<()> {
    for message in inbox.messages()? {
        if message.urgency == corral_core::inbox::Urgency::Wake
            && now.as_nanosecond() - message.created.as_nanosecond() >= 600_000_000_000
            && inbox
                .receipt(message.seq, ReceiptLevel::Observed)?
                .is_none()
            && inbox.terminal(message.seq)?.is_none()
        {
            inbox.event(
                "message_unobserved",
                json!({"id":message.id,"created":message.created}),
            )?;
        }
    }
    Ok(())
}
