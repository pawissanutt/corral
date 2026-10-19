//! The header and editor project the same read-only workset as CLI status and PM brief.
use corral_core::OperatingMode;
use corral_ops::{Ctx, PmAction, PmActionKind, PmWorkset};

pub(crate) fn reason(action: &PmAction) -> &str {
    match action.kind {
        PmActionKind::Owner | PmActionKind::Wait => action.evidence.first().map(String::as_str),
        PmActionKind::Question => action.evidence.last().map(String::as_str),
        _ => None,
    }
    .unwrap_or(&action.why)
}

pub(crate) fn summary(workset: &PmWorkset) -> String {
    // The editor retains the complete facts; an arbitrary multi-line question
    // must not consume the board's whole viewport.
    let concise = |text: &str| {
        crate::overlays::truncate(&text.split_whitespace().collect::<Vec<_>>().join(" "), 120)
    };
    let mode = match workset.mode {
        OperatingMode::Assist => "Assist",
        OperatingMode::Autopilot => "Autopilot",
    };
    let progress = &workset.progress;
    let mut text = format!(
        "{mode}{} | {} | scope {}/{} accepted; {} running | O mode",
        if workset.paused { " (paused)" } else { "" },
        workset.status.as_str(),
        progress.accepted,
        progress.total,
        progress.running,
    );
    if workset.paused {
        text.push_str(
            "\nNo new automatic actions; Resume required; running receipts still collected.",
        );
    } else if let Some(decision) = workset.decisions.first() {
        text.push_str(&format!(
            "\n{}: {}; {} decision(s); independent work {}",
            decision.task,
            concise(reason(&decision.action)),
            workset.decisions.len(),
            if workset.independent_work_continues {
                "continues"
            } else {
                "waiting"
            },
        ));
    } else if let Some(wait) = workset.waits.first() {
        text.push_str(&format!(
            "\n{}: {}",
            wait.task.as_deref().unwrap_or("workspace"),
            concise(reason(wait))
        ));
    } else if let Some(blocker) = workset.blockers.first() {
        text.push_str(&format!("\n{}", concise(blocker)));
    }
    text
}

pub(crate) fn readback(ctx: &Ctx) -> String {
    match corral_ops::pm_workset(ctx) {
        Ok(workset) => summary(&workset),
        Err(error) => format!("Assist | policy unavailable; no automatic authority: {error:#}"),
    }
}
