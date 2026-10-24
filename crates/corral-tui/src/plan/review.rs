//! "Run plan review": the owner action behind `corral plan review <id>`, offered on the Plan tab and the stage strip.

use corral_ops::{Ctx, PlanReviewOffer};
use ratatui::crossterm::event::KeyCode;

use crate::{Action, Toast, Tone};

pub(crate) const KEY: KeyCode = KeyCode::Char('R');

pub(crate) fn label(offer: PlanReviewOffer) -> &'static str {
    match offer {
        PlanReviewOffer::Run => "Run plan review",
        PlanReviewOffer::Retry => "Retry plan review",
    }
}

/// Starts the task's routed plan reviewer through the CLI's path. A transient refusal changes nothing, so the
/// same action stays on screen and the toast says to press it again.
pub(crate) fn run(ctx: &Ctx, task: &str) -> Action {
    Action::Toast(
        match corral_ops::plan_review(ctx, task, None) {
            Ok(review) => {
                let pane = review
                    .herdr
                    .as_ref()
                    .map_or("no pane yet", |herdr| herdr.pane_id.as_str());
                Toast::new(
                    Tone::Good,
                    format!("▶ {task} plan review {} started", review.id),
                    format!("reviewer {} · pane {pane}", review.reviewer),
                )
            }
            Err(error) if corral_ops::plan_review_retryable(&error) => Toast::new(
                Tone::Warn,
                format!("{task} plan review not started"),
                format!("{error:#} - press R to retry"),
            ),
            Err(error) => Toast::new(
                Tone::Bad,
                format!("{task} plan review failed"),
                format!("{error:#}"),
            ),
        }
        .about(task),
    )
}
