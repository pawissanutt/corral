use anyhow::{Context, Result, ensure};
use corral_core::inbox::{Inbox, MessageBinding, Post, ReceiptLevel, Recipient, Urgency};
use serde_json::{Value, json};

use super::Target;
use crate::Ctx;
use crate::delivery::owner;

/// Validate before lifting a fence or reserving a message. The same check guards native
/// task replies; framing counts against the host's existing input cap too.
pub(crate) fn validate_text(ctx: &Ctx, recipient: &Recipient, text: &str) -> Result<()> {
    let limit = crate::agent::delivery::INPUT_MAX_BYTES;
    ensure!(
        !text.trim().is_empty(),
        "invalid_params: session input must not be blank"
    );
    ensure!(
        text.len() <= limit,
        "invalid_params: session input exceeds {limit} bytes"
    );
    Inbox::new(&ctx.store, recipient.clone())?.check(&Post {
        kind: "owner_reply", urgency: Urgency::Wake, author: owner::OWNER,
        body: text, binding: MessageBinding::default(), subject: None, max_payload: limit,
    }).map_err(|error| anyhow::anyhow!(
        "invalid_params: session input must fit the {limit}-byte host limit including inbox framing: {error}"
    ))
}

pub(crate) fn send(ctx: &Ctx, target: &Target, text: &str, policy: &str) -> Result<Value> {
    let policy = match policy {
        "steer" => "steer",
        "follow-up" => "follow_up",
        _ => anyhow::bail!("invalid_params: policy must be steer or follow-up"),
    };
    target.require_task_family()?;
    ensure!(!target.ended, "ended: session {} has ended", target.id);
    ensure!(
        target.bridge,
        "no_bridge: session {} takes session keys, not send",
        target.id
    );
    let recipient = target
        .recipient()
        .context("no_bridge: session has no inbox recipient")?;
    let (message, lifted) = if matches!(recipient, Recipient::Run { .. }) {
        crate::dispatch::inbox_session_reply(ctx, &recipient, text, policy)?
    } else {
        validate_text(ctx, &recipient, text)?;
        let inbox = Inbox::new(&ctx.store, recipient.clone())?;
        let delivery = inbox
            .delivery_lock()
            .context("locking owner session input")?;
        let current = super::resolve(ctx, &target.id)?;
        current.check_generation(target.generation)?;
        ensure!(!current.ended, "ended: session {} has ended", current.id);
        ensure!(
            current.bridge,
            "no_bridge: session {} has no host bridge",
            current.id
        );
        let lifted = super::pause::recipient_paused(ctx, &recipient)?;
        super::pause::lift_recipient_owner(ctx, &recipient)?;
        let message = owner::post_with_policy(
            ctx,
            &recipient,
            "owner_reply",
            text,
            MessageBinding::default(),
            Some(policy),
            |task, _| {
                if let Some(container) = current.container.as_ref() {
                    ensure!(
                        corral_core::agent::container_closure(task, container).is_none(),
                        "ended: session {} has ended",
                        current.id
                    );
                }
                Ok(())
            },
        )
        .context("posting owner session input")?;
        drop(delivery);
        // Transport failure leaves the post durable for the ordinary recipient retry pass.
        let _ = owner::kick(ctx, &recipient);
        (message, lifted)
    };
    let inbox = Inbox::new(&ctx.store, recipient)?;
    let disposition = if let Some(receipt) = inbox.receipt(message.seq, ReceiptLevel::Admitted)? {
        receipt
            .detail
            .get("disposition")
            .and_then(Value::as_str)
            .unwrap_or("pending")
            .to_owned()
    } else if inbox
        .receipt(message.seq, ReceiptLevel::Observed)?
        .is_some()
    {
        "delivered".into()
    } else if inbox.terminal(message.seq)?.is_some() {
        "failed".into()
    } else {
        "pending".into()
    };
    Ok(json!({
        "id": target.id, "message": message.id,
        "delivery": disposition, "pause_lifted": lifted,
    }))
}

#[cfg(all(test, target_os = "linux"))]
#[path = "send_keys_tests.rs"]
mod tests;
