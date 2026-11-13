//! One background update of the board: sync, then the live cost refresh of unfinished runs.
//! A daemon-owned board instead reads the daemon's classic frame.

use corral_ops::attention::AttentionBoard;
use corral_ops::classic::ClassicFrame;
use corral_ops::{Change, Ctx};

/// A pass's changes and the attention board the frame is computed with.
pub(crate) type Synced = Result<(Vec<Change>, Result<AttentionBoard, String>), String>;

pub(crate) fn run_with_attention(
    ctx: &Ctx,
    messages: &std::sync::mpsc::Sender<corral_ops::Message>,
) -> Synced {
    let (sender, diagnostics) = std::sync::mpsc::channel();
    let synced = {
        let _capture = corral_core::capture_messages(sender);
        let synced = {
            let _timing = corral_core::timing::span("board.sync");
            corral_ops::sync_with_attention(ctx)
        };
        {
            let _timing = corral_core::timing::span("board.usage");
            corral_ops::refresh_active_usage(&ctx.store);
        }
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
        Ok(board)
    } else {
        // A foreign scope or busy reconciliation lease did not classify this pass.
        local_attention(ctx)
    };
    Ok((changes, attention))
}

/// The pass of a board whose executable was replaced: the installed corral syncs and this
/// outdated process only reads the result back (T-587).
pub(crate) fn run_installed(ctx: &Ctx, installed: &std::path::Path) -> Synced {
    corral_ops::sync_installed(ctx, installed)
        .map_err(|error| corral_herdr::friendly_error(&error))?;
    Ok((Vec::new(), local_attention(ctx)))
}

/// The daemon's classic frame, or None when it is absent, refused (identity mismatch), busy
/// or invalid; the caller then computes the frame locally. Only an explicit refresh (`wait` is
/// set) asks for a pass: a nudge on every periodic tick would start a daemon pass per board
/// tick and defeat the daemon's own pacing. Failure to reach the daemon never authorizes a
/// competing in-process pass.
pub(crate) fn daemon_frame(ctx: &Ctx, wait: Option<std::time::Duration>) -> Option<ClassicFrame> {
    if let Some(wait) = wait
        && let Ok(Some(mut client)) = corral_ops::daemon_client::Client::connect(&ctx.store)
    {
        let _ = client.nudge("board", Some(wait));
    }
    let bytes = {
        let _timing = corral_core::timing::span("board.frame.read");
        corral_ops::daemon_client::served(
            &ctx.store,
            corral_protocol::read::ReadKind::ClassicFrame,
        )?
    };
    let _timing = corral_core::timing::span("board.frame.decode");
    serde_json::from_slice(&bytes).ok()
}

/// The attention board read in this process when no pass or daemon supplied one.
pub(crate) fn local_attention(ctx: &Ctx) -> Result<AttentionBoard, String> {
    let _timing = corral_core::timing::span("board.attention");
    crate::attention::load_board(ctx, jiff::Timestamp::now())
}

#[cfg(test)]
pub(crate) fn run(ctx: &Ctx) -> Result<Vec<Change>, String> {
    ctx.store
        .check_runtime()
        .map_err(|e| corral_herdr::friendly_error(&e))?;
    corral_ops::lesson_rows::prepare_evaluation(ctx)
        .map_err(|e| corral_herdr::friendly_error(&e))?;
    let synced = corral_ops::sync(ctx).map_err(|e| corral_herdr::friendly_error(&e));
    corral_ops::refresh_active_usage(&ctx.store);
    synced
}

#[cfg(test)]
#[path = "sync_step_tests.rs"]
mod tests;
