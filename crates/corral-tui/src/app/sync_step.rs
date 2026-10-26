//! One background update of the board: sync, then the live cost refresh of unfinished runs.

use corral_ops::{Change, Ctx};

pub(crate) fn run_with_attention(
    ctx: &Ctx,
    messages: &std::sync::mpsc::Sender<corral_ops::Message>,
) -> Result<(Vec<Change>, crate::attention::Snapshot), String> {
    let (sender, diagnostics) = std::sync::mpsc::channel();
    let synced = {
        let _capture = corral_core::capture_messages(sender);
        let synced = corral_ops::sync_with_attention(ctx);
        corral_ops::refresh_active_usage(&ctx.store);
        synced
    };
    let mut unavailable = None;
    for message in diagnostics.try_iter() {
        if message.file.is_some() {
            unavailable.get_or_insert_with(|| message.text.clone());
        }
        let _ = messages.send(message);
    }
    let (changes, board) = synced.map_err(|error| corral_herdr::friendly_error(&error))?;
    let attention = if let Some(reason) = unavailable {
        Err(reason)
    } else if let Some(board) = board {
        crate::attention::from_board(board)
    } else {
        // A foreign scope or busy reconciliation lease did not classify this pass.
        crate::attention::load(ctx, jiff::Timestamp::now())
    };
    Ok((changes, attention))
}

#[cfg(test)]
pub(crate) fn run(ctx: &Ctx) -> Result<Vec<Change>, String> {
    let synced = corral_ops::sync(ctx).map_err(|e| corral_herdr::friendly_error(&e));
    corral_ops::refresh_active_usage(&ctx.store);
    synced
}

#[cfg(test)]
#[path = "sync_step_tests.rs"]
mod tests;
