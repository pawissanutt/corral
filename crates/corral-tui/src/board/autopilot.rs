//! Review autopilot state on board cards (docs/DESIGN.md §5): the change round on Running cards, the escalation
//! reason on Needs me cards, the stage badge and blocked reason on Review cards, auto-landed Done cards.

use corral_core::{AutoStage, Run, Task};
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use super::cards::wrap2;
use crate::theme;

fn stage(run: Option<&Run>) -> Option<AutoStage> {
    run.and_then(|r| r.autopilot.as_ref()).map(|a| a.stage)
}

pub(super) fn changes_span(t: &Task, run: &Run) -> Option<Span<'static>> {
    (stage(Some(run)) == Some(AutoStage::Changes)).then(|| {
        Span::styled(
            format!("↺ changes r{}", t.rounds),
            Style::new().fg(theme::YELLOW),
        )
    })
}

pub(super) fn escalation(t: &Task) -> Option<String> {
    let state = t
        .current_run()?
        .autopilot
        .as_ref()
        .filter(|_| t.escalated())?;
    Some(format!(
        "autopilot: {}",
        state.reason.as_deref().unwrap_or("escalated")
    ))
}

/// Lines under a Review card's meta line: the stage badge on its own line (the meta line is full at normal
/// widths), then a blocked run's reason wrapped to two lines.
pub(super) fn review_lines(run: Option<&Run>, width: usize) -> Vec<Line<'static>> {
    let Some(state) = run.and_then(|r| r.autopilot.as_ref()) else {
        return Vec::new();
    };
    let (text, color) = match state.stage {
        AutoStage::Gating => ("⟳ gate", theme::BLUE),
        AutoStage::Reviewing => ("⟳ reviewing", theme::BLUE),
        AutoStage::Landing => ("⟳ landing", theme::BLUE),
        AutoStage::Reviewed => ("reviewed ✓", theme::GREEN),
        AutoStage::Blocked => ("⏸ blocked", theme::YELLOW),
        AutoStage::Changes | AutoStage::Escalated | AutoStage::Done => return Vec::new(),
    };
    let mut out = vec![Line::styled(text, Style::new().fg(color))];
    if state.stage == AutoStage::Blocked
        && let Some(reason) = state.reason.as_deref()
    {
        let style = Style::new().fg(theme::YELLOW);
        out.extend(
            wrap2(reason, width)
                .into_iter()
                .map(|l| Line::styled(l, style)),
        );
    }
    out
}

pub(super) fn done_span(run: Option<&Run>) -> Option<Span<'static>> {
    (stage(run) == Some(AutoStage::Done))
        .then(|| Span::styled(" landed by autopilot", theme::dim()))
}
