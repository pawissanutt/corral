//! Owner-message family for runs launched with `delivery: inbox` (plan phase 2): posting,
//! one delivery kick per logical batch, receipt-driven effects, the run-end rules and the
//! `done` gate. Legacy runs never reach this module.

use anyhow::{Context, Result};
use corral_core::inbox::{
    DeliveryMode, EffectStatus, Inbox, MAX_PAYLOAD, Message, MessageBinding, Post, Receipt,
    ReceiptLevel, Recipient, TerminalState, Urgency,
};
use corral_core::{ReceiptOutcome, ResumeDelivery, ResumeReceipt, Task};
use serde_json::json;

use super::adapters::omo::OmoAdapter;
use super::effects::{self, EffectHandler};
use super::{DeliveryAdapter, DeliveryStatus};
use crate::Ctx;

pub(crate) const OWNER: &str = "owner";

/// Fixed at launch: only harnesses with an enabled adapter get inbox delivery (phase 2: OmO).
pub(crate) fn launch_mode(harness: &str) -> DeliveryMode {
    if harness == super::adapters::omo::HARNESS {
        DeliveryMode::Inbox
    } else {
        DeliveryMode::Legacy
    }
}

/// The task's live coding run, when it was launched with inbox delivery.
pub(crate) fn live_run(task: &Task) -> Option<Recipient> {
    let run = task
        .current_run()
        .filter(|r| r.outcome.is_none() && r.ended.is_none())?;
    (run.delivery == DeliveryMode::Inbox).then(|| Recipient::Run {
        task: task.id.clone(),
        run: run.id.clone(),
    })
}

/// The current run regardless of outcome: request-changes reopens a finished inbox run.
pub(crate) fn current_run(task: &Task) -> Option<Recipient> {
    let run = task.current_run()?;
    (run.delivery == DeliveryMode::Inbox).then(|| Recipient::Run {
        task: task.id.clone(),
        run: run.id.clone(),
    })
}

pub(crate) fn live_planner(task: &Task) -> Option<Recipient> {
    let run = task.plan_runs.last().filter(|r| r.holds_slot())?;
    (run.delivery == DeliveryMode::Inbox).then(|| Recipient::PlanRun {
        task: task.id.clone(),
        plan_run: run.id.clone(),
    })
}

/// The caller's text, without the message header.
pub(crate) fn text(message: &Message) -> &str {
    message.body.split_once('\n').map_or("", |(_, text)| text)
}

/// Size-checked, enqueue-only post; `admit` runs in the same task transaction as the seq.
pub(crate) fn post(
    ctx: &Ctx,
    recipient: &Recipient,
    kind: &str,
    body: &str,
    binding: MessageBinding,
    admit: impl FnOnce(&mut Task, &Message) -> Result<()>,
) -> Result<Message> {
    Inbox::new(&ctx.store, recipient.clone())?.post_with(
        Post {
            kind,
            urgency: Urgency::Wake,
            author: OWNER,
            body,
            binding,
            subject: None,
            max_payload: MAX_PAYLOAD,
        },
        admit,
    )
}

pub(crate) fn adapter() -> OmoAdapter {
    OmoAdapter::from_env()
}

/// One delivery pass plus effects. A failure is recorded and returned, never lost: the
/// message stays in the inbox for the next sync.
pub(crate) fn kick(ctx: &Ctx, recipient: &Recipient) -> Result<DeliveryStatus, String> {
    kick_with(ctx, recipient, &adapter())
}

pub(crate) fn kick_with(
    ctx: &Ctx,
    recipient: &Recipient,
    adapter: &dyn DeliveryAdapter,
) -> Result<DeliveryStatus, String> {
    let result = (|| {
        // An idle session writes its typed entry while the send call returns, and an
        // idempotent resend follows an uncertain result: a few passes settle both.
        let mut status = super::deliver(ctx, recipient, adapter, None)?;
        for _ in 0..2 {
            if !matches!(status, DeliveryStatus::Sent { .. }) {
                break;
            }
            status = super::deliver(ctx, recipient, adapter, None)?;
        }
        effects::reconcile(ctx, recipient, &OwnerEffects)?;
        refresh_view(ctx, recipient)?;
        Ok::<_, anyhow::Error>(status)
    })();
    result.map_err(|error| {
        let reason = format!("{error:#}");
        if let Ok(inbox) = Inbox::new(&ctx.store, recipient.clone()) {
            let _ = super::note_blocked(&inbox, &reason);
        }
        crate::messages::report(recipient.task(), "inbox", format!("delivery: {reason}"));
        reason
    })
}

pub(crate) fn is_observed(ctx: &Ctx, recipient: &Recipient, seq: u64) -> Result<bool> {
    Ok(Inbox::new(&ctx.store, recipient.clone())?
        .receipt(seq, ReceiptLevel::Observed)?
        .is_some())
}

/// Rederives the pending view after every pass: observation, a terminal state and a stale
/// effect all settle a message without the view depending on which one did.
fn refresh_view(ctx: &Ctx, recipient: &Recipient) -> Result<()> {
    let Some(id) = recipient.task() else {
        return Ok(());
    };
    let inbox = Inbox::new(&ctx.store, recipient.clone())?;
    let _lock = inbox.lock()?;
    let view = effects::pending_view(&inbox)?;
    ctx.store.try_update(id, |task| {
        let pending = match recipient {
            Recipient::Run { run, .. } => task
                .runs
                .iter_mut()
                .find(|r| &r.id == run)
                .map(|r| &mut r.pending_reply),
            Recipient::PlanRun { plan_run, .. } => task
                .plan_runs
                .iter_mut()
                .find(|r| &r.id == plan_run)
                .map(|r| &mut r.pending_reply),
            _ => None,
        };
        Ok(match pending {
            Some(pending) if *pending != view => {
                pending.clone_from(&view);
                true
            }
            _ => false,
        })
    })?;
    Ok(())
}

/// Unsettled owner-kind wake messages; uncertain and orphaned ones count.
pub(crate) fn unobserved_owner(ctx: &Ctx, recipient: &Recipient) -> Result<Vec<String>> {
    let inbox = Inbox::new(&ctx.store, recipient.clone())?;
    let mut ids = Vec::new();
    for message in inbox.messages()? {
        if message.owner_kind()
            && message.urgency == Urgency::Wake
            && inbox
                .receipt(message.seq, ReceiptLevel::Observed)?
                .is_none()
            && inbox.terminal(message.seq)?.is_none()
        {
            ids.push(message.id);
        }
    }
    Ok(ids)
}

/// `corral done` pre-check: a late observation counts; a failed pass is recorded and the gate
/// still decides.
pub(crate) fn done_gate(ctx: &Ctx, task: &Task) -> Result<()> {
    let Some(recipient) = live_run(task) else {
        return Ok(());
    };
    let _ = kick(ctx, &recipient);
    refuse_unobserved(ctx, &recipient)
}

/// Held across the final done transition: owner posts take this lock first, so none can land
/// between the recheck and the run's move to Review.
pub(crate) fn done_admission(
    ctx: &Ctx,
    task: &Task,
) -> Result<Option<corral_core::store::StoreLock>> {
    let Some(recipient) = live_run(task) else {
        return Ok(None);
    };
    #[cfg(all(test, unix))]
    if let Some(hook) = tests::BEFORE_DONE_ADMISSION.with(|hook| hook.borrow_mut().take()) {
        hook();
    }
    let lock = Inbox::new(&ctx.store, recipient.clone())?.lock()?;
    refuse_unobserved(ctx, &recipient)?;
    Ok(Some(lock))
}

fn refuse_unobserved(ctx: &Ctx, recipient: &Recipient) -> Result<()> {
    let waiting = unobserved_owner(ctx, recipient)?;
    anyhow::ensure!(
        waiting.is_empty(),
        "owner_messages_unobserved: {} not yet received by the agent; the owner may settle them with `corral inbox mark <id> --observed|--cancel --reason <text>`",
        waiting.join(", ")
    );
    Ok(())
}

/// Run end: never-attempted messages are cancelled for reposting; attempted but unobserved
/// ones stay as they are and are surfaced as orphaned.
pub(crate) fn retire(ctx: &Ctx, recipient: &Recipient, reason: &str) -> Result<()> {
    let inbox = Inbox::new(&ctx.store, recipient.clone())?;
    let _deliver = inbox.delivery_lock()?;
    let envelopes = inbox.envelopes()?;
    for message in inbox.messages()? {
        if inbox
            .receipt(message.seq, ReceiptLevel::Observed)?
            .is_some()
            || inbox.terminal(message.seq)?.is_some()
        {
            continue;
        }
        let attempted = envelopes
            .iter()
            .find(|e| e.members.contains(&message.seq) && !e.attempts.is_empty());
        match attempted {
            Some(envelope) => super::alert_once(
                &inbox,
                "delivery_orphaned",
                &format!("env{}", envelope.number),
                json!({"envelope": envelope.number, "reason": reason}),
            )?,
            // A native answer leaves no envelope attempt; it may have reached the agent.
            None if message.binding.question_request.is_some() => super::alert_once(
                &inbox,
                "delivery_orphaned",
                &format!("seq{}", message.seq),
                json!({"id": message.id, "reason": reason, "via": "omo_question"}),
            )?,
            None => {
                let why = if message.urgency == Urgency::Info {
                    "moot: run ended".to_string()
                } else {
                    reason.to_string()
                };
                inbox.mark_terminal(message.seq, TerminalState::Cancelled, &why, "run_end")?;
            }
        }
    }
    Ok(())
}

fn ended(task: &Task, recipient: &Recipient) -> bool {
    match recipient {
        Recipient::Run { run, .. } => task
            .current_run()
            .is_none_or(|r| &r.id != run || r.outcome.is_some() || r.ended.is_some()),
        Recipient::PlanRun { plan_run, .. } => task
            .plan_runs
            .last()
            .is_none_or(|r| &r.id != plan_run || !r.holds_slot()),
        _ => true,
    }
}

/// What a sync pass owes one recipient.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Work {
    /// A message is neither observed nor terminal, or an envelope is still open.
    pub unsettled: bool,
    /// An observed message's effect never finished (missing or Started).
    pub effects: bool,
    /// An attempted envelope (including an abandoned or cancelled one) lacks evidence.
    pub late_evidence: bool,
    /// The task's pending_reply differs from the view derived from message state (for
    /// example after `inbox mark --cancel` or abandoning an unattempted publication).
    pub view: bool,
}

pub(crate) fn work(ctx: &Ctx, recipient: &Recipient) -> Result<Work> {
    let inbox = Inbox::new(&ctx.store, recipient.clone())?;
    let mut work = Work::default();
    if !inbox.dir().is_dir() {
        return Ok(work);
    }
    let applied = inbox.state()?.effects_applied;
    for message in inbox.messages()? {
        let entry = inbox.entry(message.seq)?;
        if entry.observed.is_some() {
            work.effects |= !applied.contains(&message.seq)
                && entry
                    .effect
                    .is_none_or(|effect| effect.status == EffectStatus::Started);
        } else if entry.terminal.is_none() {
            work.unsettled = true;
        }
    }
    for envelope in inbox.envelopes()? {
        if envelope.attempts.is_empty() {
            continue;
        }
        for seq in &envelope.members {
            work.late_evidence |= inbox.receipt(*seq, ReceiptLevel::Observed)?.is_none();
        }
    }
    work.unsettled |= inbox.state()?.open_envelope.is_some();
    let task = ctx.store.get(
        recipient
            .task()
            .context("inbox work requires a task recipient")?,
    )?;
    let shown = match recipient {
        Recipient::Run { run, .. } => task
            .runs
            .iter()
            .find(|r| &r.id == run)
            .map(|r| &r.pending_reply),
        Recipient::PlanRun { plan_run, .. } => task
            .plan_runs
            .iter()
            .find(|r| &r.id == plan_run)
            .map(|r| &r.pending_reply),
        _ => None,
    };
    let view = effects::pending_view(&inbox)?;
    work.view = shown.is_some_and(|shown| *shown != view);
    Ok(work)
}

/// Sync pass for every owned inbox recipient. A live recipient gets a full delivery pass; a
/// finished one only collects late evidence and finishes effects, never another send.
pub fn reconcile_all(ctx: &Ctx) -> Result<()> {
    reconcile_all_with(ctx, &adapter())
}

pub(crate) fn reconcile_all_with(ctx: &Ctx, adapter: &dyn DeliveryAdapter) -> Result<()> {
    for task in ctx.store.list()? {
        if !crate::session_scope::owns_task(ctx, &task)? {
            continue;
        }
        let mut recipients = Vec::new();
        for run in task
            .runs
            .iter()
            .filter(|r| r.delivery == DeliveryMode::Inbox)
        {
            recipients.push(Recipient::Run {
                task: task.id.clone(),
                run: run.id.clone(),
            });
        }
        for run in task
            .plan_runs
            .iter()
            .filter(|r| r.delivery == DeliveryMode::Inbox)
        {
            recipients.push(Recipient::PlanRun {
                task: task.id.clone(),
                plan_run: run.id.clone(),
            });
        }
        for recipient in recipients {
            let work = work(ctx, &recipient)?;
            if work == Work::default() {
                continue;
            }
            if !ended(&task, &recipient) && work.unsettled {
                let _ = kick_with(ctx, &recipient, adapter);
                continue;
            }
            if !(work.unsettled || work.effects || work.late_evidence) {
                // Settlement without a delivery pass: only the derived view is stale.
                if let Err(error) = refresh_view(ctx, &recipient) {
                    crate::messages::report(recipient.task(), "inbox", format!("{error:#}"));
                }
                continue;
            }
            if work.unsettled {
                retire(ctx, &recipient, "run ended")?;
            }
            let _ = settle_with(ctx, &recipient, adapter);
        }
    }
    Ok(())
}

/// Observation and effects only; the recorded failure is returned, never lost.
fn settle_with(
    ctx: &Ctx,
    recipient: &Recipient,
    adapter: &dyn DeliveryAdapter,
) -> Result<(), String> {
    let result = (|| {
        super::observe_only(ctx, recipient, adapter, jiff::Timestamp::now())?;
        effects::reconcile(ctx, recipient, &OwnerEffects)?;
        refresh_view(ctx, recipient)
    })();
    result.map_err(|error| {
        let reason = format!("{error:#}");
        if let Ok(inbox) = Inbox::new(&ctx.store, recipient.clone()) {
            let _ = super::note_blocked(&inbox, &reason);
        }
        crate::messages::report(recipient.task(), "inbox", format!("settle: {reason}"));
        reason
    })
}

/// Owner messages answer a pending decision through the journaled receipt.
pub(crate) struct OwnerEffects;

impl EffectHandler for OwnerEffects {
    fn prepare_decision(
        &self,
        message: &Message,
        receipt: &Receipt,
    ) -> Result<Option<ResumeReceipt>> {
        let (Some(decision), Recipient::Run { run, .. }) =
            (&message.binding.decision, &message.recipient)
        else {
            return Ok(None);
        };
        Ok(Some(ResumeReceipt {
            version: 1,
            id: decision.clone(),
            run: run.clone(),
            outcome: ReceiptOutcome::Answered,
            delivery: ResumeDelivery::Acknowledged,
            observed_at: receipt.observed_at,
            answer: Some(text(message).to_string()),
            data: json!({"via": "corral_reply", "inbox_message": message.id}),
        }))
    }

    fn apply_locked(
        &self,
        ctx: &Ctx,
        message: &Message,
        _receipt: &Receipt,
        decision_receipt: Option<&ResumeReceipt>,
    ) -> Result<()> {
        if let Some(receipt) = decision_receipt {
            let id = message.recipient.task().context("decision without task")?;
            crate::obligations::answer_reply_with(ctx, id, receipt)?;
        }
        Ok(())
    }

    fn apply_task(&self, _task: &mut Task, _message: &Message, _receipt: &Receipt) -> Result<()> {
        Ok(())
    }
}

#[cfg(all(test, unix))]
#[path = "owner_tests.rs"]
mod tests;
