//! Review autopilot state on board cards (docs/DESIGN.md §5): the change round on Running cards, the escalation
//! reason on Blocked cards, the sub-state badge on Review cards (`⧗ gating`, `👁 reviewing`, `⟳ landing`,
//! `↺ changes 1/2`, `⛔ blocked: <reason>`, `✓ reviewed`), auto-landed Done cards.

use corral_core::{AutoStage, Config, Run, Task};
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use super::cards::wrap2;
use crate::theme;

pub(super) fn active_lines(text: &str, width: usize) -> Vec<Line<'static>> {
    let compact;
    let text = if width < "reviewing:".len()
        && let Some(model) = text.strip_prefix("reviewing: ")
    {
        compact = format!("review: {model}");
        &compact
    } else {
        text
    };
    wrap2(text, width)
        .into_iter()
        .map(|line| Line::styled(line, Style::new().fg(theme::BLUE)))
        .collect()
}

pub(super) fn active_stages(
    ctx: &corral_ops::Ctx,
    tasks: &[Task],
) -> std::collections::HashMap<String, String> {
    use corral_ops::watch::ProgressState;
    tasks
        .iter()
        .filter_map(|task| {
            let run = task.current_run()?;
            if let Some(review) = &run.review
                && review.holds_slot()
            {
                let model = run
                    .review_runs
                    .last()
                    .and_then(|r| r.model.as_deref())
                    .unwrap_or(&review.reviewer);
                return Some((task.id.clone(), format!("reviewing: {model}")));
            }
            if run.gate.as_ref().is_some_and(|g| g.holds_slot()) {
                let history = match corral_ops::watch::load(ctx, &task.id) {
                    Ok(history) => history,
                    Err(error) => {
                        corral_ops::report_message(
                            Some(&task.id),
                            "stage progress",
                            format!("{error:#}"),
                        );
                        return Some((task.id.clone(), "gating: progress unavailable".into()));
                    }
                };
                if let Some(gate) = history.gates.last() {
                    let active = gate
                        .commands
                        .iter()
                        .enumerate()
                        .find(|(_, s)| s.state == ProgressState::Running);
                    let text = if let Some((i, command)) = active {
                        format!(
                            "gating {}/{}: {}",
                            i + 1,
                            gate.commands.len(),
                            command.command
                        )
                    } else {
                        format!("gating: {}", crate::stages::gate_state(gate.state))
                    };
                    return Some((task.id.clone(), text));
                }
            }
            None
        })
        .collect()
}

fn stage(run: Option<&Run>) -> Option<AutoStage> {
    run.and_then(|r| r.autopilot.as_ref()).map(|a| a.stage)
}

fn changes_text(t: &Task, config: &Config) -> String {
    let max = config
        .review_policy(t.project.as_deref(), t.review)
        .max_rounds;
    format!("↺ changes {}/{max}", t.rounds)
}

/// A Running card's change-round badge, on its own line (the meta line is full at normal widths).
pub(super) fn changes_line(t: &Task, run: &Run, config: &Config) -> Option<Line<'static>> {
    (stage(Some(run)) == Some(AutoStage::Changes))
        .then(|| Line::styled(changes_text(t, config), Style::new().fg(theme::YELLOW)))
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

/// Lines under a Review card's meta line: the sub-state badge on its own line (the meta line is full at normal
/// widths); a blocked run's badge carries its reason, wrapped to two lines.
pub(super) fn review_lines(t: &Task, config: &Config, width: usize) -> Vec<Line<'static>> {
    let Some(state) = t.current_run().and_then(|r| r.autopilot.as_ref()) else {
        return Vec::new();
    };
    let (text, color) = match state.stage {
        AutoStage::Gating => ("⧗ gating".to_string(), theme::BLUE),
        AutoStage::Reviewing => ("👁 reviewing".to_string(), theme::BLUE),
        AutoStage::Landing => ("⟳ landing".to_string(), theme::BLUE),
        AutoStage::Reviewed => ("✓ reviewed".to_string(), theme::GREEN),
        AutoStage::Changes => (changes_text(t, config), theme::YELLOW),
        AutoStage::Blocked => {
            let reason = state
                .reason
                .as_deref()
                .and_then(|r| r.lines().map(str::trim).find(|l| !l.is_empty()))
                .unwrap_or("waiting to land");
            let style = Style::new().fg(theme::RED);
            return wrap2(&format!("⛔ blocked: {reason}"), width)
                .into_iter()
                .map(|l| Line::styled(l, style))
                .collect();
        }
        AutoStage::Escalated | AutoStage::Done => return Vec::new(),
    };
    vec![Line::styled(text, Style::new().fg(color))]
}

pub(super) fn done_span(run: Option<&Run>) -> Option<Span<'static>> {
    (stage(run) == Some(AutoStage::Done))
        .then(|| Span::styled(" landed by autopilot", theme::dim()))
}
