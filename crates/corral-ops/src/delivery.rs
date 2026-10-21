//! Inbox delivery policy. Harness adapters live in `adapters`; owner callers in `owner`.

use std::time::Duration;

use anyhow::{Context, Result, ensure};
pub use corral_core::inbox::{
    Envelope, ObserveCursor, Recipient as RecipientRef, SendOutcome, SessionBinding,
};
use corral_core::inbox::{
    Inbox, Receipt, ReceiptLevel, Recipient, SessionCursor, Stage, TerminalState,
};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::Ctx;

pub mod adapters;
mod budget;
pub mod effects;
#[cfg(all(test, unix))]
mod omo_fixture;
pub(crate) mod owner;
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
    let mut open = observe_pass(ctx, &inbox, adapter, fallback, now)?;
    let replacement = inbox
        .state()?
        .replacement_envelope
        .map(|number| inbox.envelope(number))
        .transpose()?;
    let selected = if let Some(envelope) = open.as_ref().or(replacement.as_ref()) {
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
                alert_once(
                    &inbox,
                    "delivery_uncertain",
                    &format!("env{}-a{}", envelope.number, envelope.attempts.len()),
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
    if let Some(envelope) = &open
        && envelope.attempts.is_empty()
    {
        for seq in &envelope.members {
            if inbox.receipt(*seq, ReceiptLevel::Observed)?.is_some()
                || inbox.terminal(*seq)?.is_some()
            {
                // Keep frozen bytes intact, but never first-send an ineligible member.
                // Selection and readiness above still belong to the published adapter.
                inbox.retire_unattempted(envelope)?;
                open = None;
                break;
            }
        }
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
        SendOutcome::Uncertain { reason } => alert_once(
            &inbox,
            "delivery_uncertain",
            &format!("env{}-a{}", envelope.number, envelope.attempts.len()),
            json!({"envelope":envelope.number,"reason":reason}),
        )?,
        SendOutcome::Accepted | SendOutcome::Held { .. } => {}
    }
    clear_blocked(&inbox)?;
    Ok(DeliveryStatus::Sent {
        envelope: envelope.number,
    })
}

/// Collects late evidence for a finished recipient without sending anything: an uncertain
/// attempt is never retried into a session the run no longer owns.
pub fn observe_only(
    ctx: &Ctx,
    recipient: &Recipient,
    adapter: &dyn DeliveryAdapter,
    now: Timestamp,
) -> Result<()> {
    let inbox = Inbox::new(&ctx.store, recipient.clone())?;
    let _deliver = inbox.delivery_lock()?;
    observe_pass(ctx, &inbox, adapter, None, now)?;
    Ok(())
}

/// Caller holds the deliver lock. Returns the open envelope that is still unsettled.
fn observe_pass(
    ctx: &Ctx,
    inbox: &Inbox<'_>,
    adapter: &dyn DeliveryAdapter,
    fallback: Option<&dyn DeliveryAdapter>,
    now: Timestamp,
) -> Result<Option<Envelope>> {
    inbox.recover_publication()?;
    let mut open = inbox
        .state()?
        .open_envelope
        .map(|n| inbox.envelope(n))
        .transpose()?;
    if let Some(envelope) = &mut open
        && envelope
            .attempts
            .last()
            .is_some_and(|attempt| attempt.outcome.is_none())
    {
        inbox.finish_attempt(
            envelope,
            SendOutcome::Uncertain {
                reason: "interrupted dispatching attempt".into(),
            },
        )?;
    }
    // Observation outlives the send gate: cancellation, abandonment and owner
    // resend must not discard complete evidence arriving in the original session.
    let mut observed_sessions = Vec::new();
    for envelope in inbox.envelopes()? {
        let stream = (envelope.adapter.clone(), envelope.binding.clone());
        if envelope.attempts.is_empty() || observed_sessions.contains(&stream) {
            continue;
        }
        let mut unobserved = false;
        for seq in &envelope.members {
            if inbox.receipt(*seq, ReceiptLevel::Observed)?.is_none() {
                unobserved = true;
                break;
            }
        }
        if !unobserved {
            continue;
        }
        let observer = if envelope.adapter == adapter.harness() {
            Some(adapter)
        } else {
            fallback.filter(|a| a.harness() == envelope.adapter)
        };
        let Some(observer) = observer else { continue };
        let active = open.as_ref().filter(|active| {
            !active.attempts.is_empty()
                && active.adapter == envelope.adapter
                && active.binding == envelope.binding
        });
        let blocks_send = match active {
            Some(active) => !inbox.settled(active)?,
            None => false,
        };
        if let Err(error) = observe(ctx, inbox, observer, &envelope.binding, now) {
            if blocks_send {
                return Err(error);
            }
            // An unreadable retired stream remains eligible on later passes,
            // but cannot block independent messages or silently lose its error.
            inbox.event(
                "delivery_observation_failed",
                json!({"adapter":envelope.adapter,"binding":envelope.binding,"reason":format!("{error:#}")}),
            )?;
        }
        observed_sessions.push(stream);
    }
    alert_unobserved(inbox, now)?;
    if let Some(envelope) = &mut open {
        *envelope = inbox.envelope(envelope.number)?;
    }
    if let Some(envelope) = &open
        && inbox.close_settled(envelope)?
    {
        open = None;
    }
    Ok(open)
}

fn blocked(inbox: &Inbox<'_>, reason: &str) -> Result<DeliveryStatus> {
    note_blocked(inbox, reason)?;
    Ok(DeliveryStatus::Blocked {
        reason: reason.into(),
    })
}

fn orphaned(inbox: &Inbox<'_>, envelope: &Envelope, reason: &str) -> Result<DeliveryStatus> {
    alert_once(
        inbox,
        "delivery_orphaned",
        &format!("env{}", envelope.number),
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
    let mut cursor = inbox
        .state()?
        .observe_cursors
        .into_iter()
        .find(|stream| stream.adapter == adapter.harness() && &stream.binding == binding)
        .map(|stream| stream.cursor)
        // A new stream starts from its origin, never another stream's offset.
        .unwrap_or_default();
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
        if state.binding.as_ref() == Some(binding) {
            state.observe_cursor = cursor.clone();
        }
        if let Some(stream) = state
            .observe_cursors
            .iter_mut()
            .find(|stream| stream.adapter == adapter.harness() && &stream.binding == binding)
        {
            stream.cursor = cursor;
        } else {
            state.observe_cursors.push(SessionCursor {
                adapter: adapter.harness().into(),
                binding: binding.clone(),
                cursor,
            });
        }
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
            alert_once(
                inbox,
                "message_unobserved",
                &message.seq.to_string(),
                json!({"id":message.id,"created":message.created}),
            )?;
        }
    }
    Ok(())
}

/// Actionable alerts are logged once per subject, not on every sync pass.
pub(crate) fn alert_once(
    inbox: &Inbox<'_>,
    kind: &str,
    subject: &str,
    detail: Value,
) -> Result<()> {
    let dir = inbox.dir().join("alerts");
    std::fs::create_dir_all(&dir).context("creating inbox alert directory")?;
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dir.join(format!("{kind}-{subject}")))
    {
        Ok(_) => inbox.event(kind, detail),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(e).context("recording inbox alert"),
    }
}

/// A recurring sync pass reports a blocked recipient once per distinct reason.
pub(crate) fn note_blocked(inbox: &Inbox<'_>, reason: &str) -> Result<()> {
    let path = inbox.dir().join("blocked");
    if std::fs::read_to_string(&path).is_ok_and(|last| last == reason) {
        return Ok(());
    }
    inbox.event("delivery_blocked", json!({"reason":reason}))?;
    std::fs::create_dir_all(inbox.dir()).context("creating inbox directory")?;
    std::fs::write(&path, reason).context("recording inbox blocked marker")
}

fn clear_blocked(inbox: &Inbox<'_>) -> Result<()> {
    match std::fs::remove_file(inbox.dir().join("blocked")) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).context("clearing inbox blocked marker"),
    }
}
