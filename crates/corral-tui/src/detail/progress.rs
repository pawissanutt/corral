//! Execution progress and the latest portable handoff for the selected run.

use corral_core::{Run, StepState};
use ratatui::style::Style;
use ratatui::text::Line;
use unicode_width::UnicodeWidthChar;

use crate::theme;

pub(super) fn lines(run: &Run, width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if let Some(progress) = &run.progress {
        lines.push(Line::default());
        push(
            &mut lines,
            &format!("Progress r{} · {}", progress.revision, progress.summary()),
            theme::bold(),
            width,
        );
        for phase in &progress.phases {
            let done = phase
                .steps
                .iter()
                .filter(|s| s.status == StepState::Done)
                .count();
            push(
                &mut lines,
                &format!(
                    "{} [{}] · {done}/{} done",
                    phase.label,
                    phase.id,
                    phase.steps.len()
                ),
                theme::bold(),
                width,
            );
            for step in &phase.steps {
                let color = match &step.status {
                    StepState::Pending => theme::DIM,
                    StepState::InProgress => theme::BLUE,
                    StepState::Done => theme::GREEN,
                    StepState::Dropped { .. } => theme::YELLOW,
                };
                push(
                    &mut lines,
                    &format!("  [{}] {}: {}", step.status.label(), step.id, step.label),
                    Style::new().fg(color),
                    width,
                );
                if let StepState::Dropped { reason } = &step.status {
                    push(
                        &mut lines,
                        &format!("    reason: {reason}"),
                        Style::new().fg(theme::YELLOW),
                        width,
                    );
                }
                if !step.criteria.is_empty() {
                    push(
                        &mut lines,
                        &format!("    criteria: {}", step.criteria.join(", ")),
                        theme::dim(),
                        width,
                    );
                }
            }
        }
    }
    // Older checkpoints are history, not the current set of outstanding obligations.
    if let Some(record) = run.checkpoints.last() {
        let checkpoint = &record.checkpoint;
        let input = &checkpoint.input;
        lines.push(Line::default());
        push(
            &mut lines,
            &format!(
                "Checkpoint r{} · progress r{} · {}",
                checkpoint.revision, input.progress_revision, record.artifact
            ),
            theme::bold(),
            width,
        );
        push(
            &mut lines,
            &format!("Next action: {}", input.next_action),
            Style::new().fg(theme::CYAN),
            width,
        );
        for metadata in [
            format!("Captured: {}", checkpoint.captured),
            format!("Spec: {}", input.spec_revision),
            format!("HEAD: {} · base: {}", input.head, input.base),
            format!("Tree: {}", input.tree),
            format!(
                "Completed: {} · current: {}",
                checkpoint.completed_steps.join(", "),
                checkpoint.current_step.as_deref().unwrap_or("-")
            ),
            format!("Evidence: {}", input.evidence_refs.join(", ")),
        ] {
            push(&mut lines, &metadata, theme::dim(), width);
        }
        if !input.criterion_ids.is_empty() {
            push(
                &mut lines,
                &format!("Checkpoint criteria: {}", input.criterion_ids.join(", ")),
                theme::dim(),
                width,
            );
        }
        for decision in &input.decisions {
            push(
                &mut lines,
                &format!("Unresolved decision: {decision}"),
                Style::new().fg(theme::YELLOW),
                width,
            );
        }
        for wait in &input.waits {
            if let Some(resolution) = &wait.resolution {
                push(
                    &mut lines,
                    &format!("Resolved wait [{}]: {resolution}", wait.id),
                    theme::dim(),
                    width,
                );
                continue;
            }
            push(
                &mut lines,
                &format!(
                    "Wait [{}] · owner {} · handle {}",
                    wait.id, wait.owner, wait.handle
                ),
                Style::new().fg(theme::BLUE),
                width,
            );
            push(
                &mut lines,
                &format!("  condition: {}", wait.condition),
                theme::dim(),
                width,
            );
        }
        for resource in &input.resources {
            if let Some(resolution) = &resource.resolution {
                push(
                    &mut lines,
                    &format!("Released resource [{}]: {resolution}", resource.id),
                    theme::dim(),
                    width,
                );
                continue;
            }
            push(
                &mut lines,
                &format!(
                    "Resource [{}] · owner {}: {}",
                    resource.id, resource.owner, resource.description
                ),
                Style::new().fg(theme::YELLOW),
                width,
            );
            push(
                &mut lines,
                &format!("  cleanup: {}", resource.cleanup),
                theme::dim(),
                width,
            );
        }
    }
    lines
}

/// Pre-wrap only the new detail rows; the existing Runs table keeps its column layout.
fn push(lines: &mut Vec<Line<'static>>, text: &str, style: Style, width: usize) {
    let width = width.max(1);
    for paragraph in text.split('\n') {
        let mut row = String::new();
        let mut used = 0;
        for ch in paragraph.chars() {
            let cells = ch.width().unwrap_or(0);
            if used + cells > width && !row.is_empty() {
                lines.push(Line::styled(std::mem::take(&mut row), style));
                used = 0;
            }
            row.push(ch);
            used += cells;
        }
        lines.push(Line::styled(row, style));
    }
}
