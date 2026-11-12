//! One background update of the board: sync, then the live cost refresh of unfinished runs.

use corral_ops::{Change, Ctx};

pub(crate) fn run_with_attention(
    ctx: &Ctx,
    messages: &std::sync::mpsc::Sender<corral_ops::Message>,
) -> Result<(Vec<Change>, crate::attention::Snapshot, String), String> {
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
        let _timing = corral_core::timing::span("board.attention");
        crate::attention::load_board(ctx, jiff::Timestamp::now())
    };
    Ok(read_back(ctx, changes, attention))
}

/// The pass of a board whose executable was replaced: the installed corral syncs and this
/// outdated process only reads the result back (T-587).
pub(crate) fn run_installed(
    ctx: &Ctx,
    installed: &std::path::Path,
) -> Result<(Vec<Change>, crate::attention::Snapshot, String), String> {
    corral_ops::sync_installed(ctx, installed)
        .map_err(|error| corral_herdr::friendly_error(&error))?;
    let attention = {
        let _timing = corral_core::timing::span("board.attention");
        crate::attention::load_board(ctx, jiff::Timestamp::now())
    };
    Ok(read_back(ctx, Vec::new(), attention))
}

/// A daemon-owned board reads the daemon's result back. Only an explicit refresh (`wait` is
/// set) asks for a pass: a nudge on every periodic tick would start a daemon pass per board
/// tick and defeat the daemon's own pacing.
pub(crate) fn run_daemon(
    ctx: &Ctx,
    wait: Option<std::time::Duration>,
) -> Result<(Vec<Change>, crate::attention::Snapshot, String), String> {
    // Failure to reach the daemon never authorizes a competing in-process pass.
    if let Some(wait) = wait
        && let Ok(Some(mut client)) = corral_ops::daemon_client::Client::connect(&ctx.store)
    {
        let _ = client.nudge("board", Some(wait));
    }
    let attention = {
        let _timing = corral_core::timing::span("board.attention");
        crate::attention::load_board(ctx, jiff::Timestamp::now())
    };
    Ok(read_back(ctx, Vec::new(), attention))
}

fn read_back(
    ctx: &Ctx,
    changes: Vec<Change>,
    attention: Result<corral_ops::attention::AttentionBoard, String>,
) -> (Vec<Change>, crate::attention::Snapshot, String) {
    let workset = attention
        .as_ref()
        .map_err(|error| anyhow::anyhow!(error.clone()))
        .and_then(|board| {
            let _timing = corral_core::timing::span("board.workset");
            corral_ops::pm_workset_with_attention(ctx, board)
        });
    (
        changes,
        attention.and_then(crate::attention::from_board),
        super::operating::readback_result(workset),
    )
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
