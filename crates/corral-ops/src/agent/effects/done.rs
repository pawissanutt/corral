//! Swept worker completion without loading the task's historical containers.
use super::{Receipted, Replayed, admission, failpoint};
use crate::Ctx;
use anyhow::{Context, Result, bail};
use corral_core::agent::{ContainerRef, RequestOrigin, RequestReceipt};

pub(crate) fn swept(
    ctx: &Ctx,
    origin: &RequestOrigin,
    op: impl FnOnce() -> Result<Option<RequestReceipt>>,
) -> Result<Receipted> {
    let container @ ContainerRef::Worker { task, run } = &origin.binding.container else {
        bail!("done requires a worker container");
    };
    let head = ctx.store.head(task)?;
    let stored = head.agent_receipt(container, &origin.request_id)?;
    let (mut receipt, replayed) = if let Some(stored) = stored {
        admission::same_request(&stored, &origin.op, &origin.payload_sha256)?;
        (stored, true)
    } else {
        match op() {
            Ok(Some(receipt)) => (receipt, false),
            Ok(None) => bail!("{} committed no receipt", origin.op),
            Err(error) => match error.downcast::<Replayed>() {
                Ok(Replayed(stored)) => (*stored, true),
                Err(error) => return Err(error),
            },
        }
    };
    failpoint("swept_after_commit");
    let mut done = Vec::new();
    for entry in receipt
        .post_pending
        .iter()
        .filter(|e| e.starts_with("effect:"))
    {
        let effect = (|| -> Result<bool> {
            match entry.as_str() {
                "effect:event" => {
                    let event = receipt
                        .result
                        .get("event")
                        .context("owed event detail missing")?;
                    let kind = event["kind"].as_str().context("owed event kind missing")?;
                    ctx.store
                        .log_event(Some(task), kind, event["detail"].clone())?;
                    Ok(true)
                }
                "effect:done_audit" => {
                    let head = ctx.store.head(task)?;
                    let Some(named) = head.run_head(run)? else {
                        return Ok(false);
                    };
                    if let Some(audit) = &named.completion.audit {
                        crate::review::audit_artifact(ctx, task, run, audit, true)?;
                    }
                    Ok(true)
                }
                other => bail!("unknown owed effect {other}"),
            }
        })();
        match effect {
            Ok(true) => done.push(entry.clone()),
            Ok(false) => {}
            Err(error) => crate::messages::report(
                Some(task),
                "agent effect",
                format!("{entry} of {} stays owed: {error:#}", receipt.request_id),
            ),
        }
    }
    failpoint("swept_after_post");
    if !done.is_empty() {
        ctx.store.update_head(task, |head| {
            head.edit_worker_receipt(run, &receipt.request_id, |stored| {
                stored.post_pending.retain(|entry| !done.contains(entry));
                Ok(())
            })
        })?;
        receipt.post_pending.retain(|entry| !done.contains(entry));
    }
    failpoint("swept_before_ack");
    Ok(Receipted { receipt, replayed })
}
