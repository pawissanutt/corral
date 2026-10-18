//! Durable terminal intent for ordinary OmO replies, separate from native answers.
use anyhow::{Context, Result, ensure};
use corral_core::Task;
use corral_herdr::{Delivery, TextDeliveryState};
use serde_json::json;

use crate::Ctx;

pub(super) fn deliver(ctx: &Ctx, task: &Task, text: &str, resend: bool) -> Result<Delivery> {
    let target = super::target(ctx, task)?;
    ensure!(
        crate::session_scope::owns_task(ctx, task)?,
        "selected herdr session does not own the ordinary reply pane"
    );
    let mut state = None;
    if resend {
        let mut queued = false;
        for event in ctx
            .store
            .events()?
            .into_iter()
            .rev()
            .filter(|event| event.task.as_deref() == Some(&task.id))
        {
            if event.kind == "reply_terminal_state" {
                ensure!(
                    event.detail["run"] == target.run
                        && event.detail["pane"] == target.pane
                        && event.detail["text"] == text,
                    "terminal reply binding changed; inspect the pending reply manually"
                );
                state = Some(
                    serde_json::from_value::<TextDeliveryState>(event.detail["state"].clone())
                        .context("reading terminal reply journal")?,
                );
                break;
            }
            if event.kind == "reply_queued"
                && event.detail["pane"] == target.pane
                && event.detail["text"] == text
            {
                queued = true;
                break;
            }
            if event.kind == "reply_pending" {
                break;
            }
        }
        // A legacy pending reply may already have reached the model. Without a
        // journal there is no safe reason to paste it again into an empty editor.
        ensure!(
            state.is_some() || queued,
            "pending ordinary reply has no terminal journal; verify delivery and use --mark-delivered, or submit the existing draft manually"
        );
    }
    Ok(ctx
        .herdr
        .pane_send_text_recorded(target.pane, text, state, |state| {
            ctx.store.log_event(
                Some(&task.id),
                "reply_terminal_state",
                json!({
                    "run":target.run, "pane":target.pane, "text":text, "state":state,
                }),
            )
        }))
}
