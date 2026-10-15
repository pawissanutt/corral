//! The configured card fields as one line of spans, shared by the board cards and the settings preview.

use corral_core::{CardCfg, CardField, RunProgress, StepState, Task, TaskCost};
use jiff::Timestamp;
use ratatui::style::Style;
use ratatui::text::Span;
use unicode_width::UnicodeWidthStr;

use crate::overlays::truncate;
use crate::theme;

fn field_style(field: CardField, t: &Task, cfg: &CardCfg) -> Style {
    match field {
        CardField::Cost => {
            let cost = TaskCost::of(t);
            match cfg.cost_warn {
                Some(warn) if cost.total() >= warn => Style::new().fg(theme::YELLOW),
                _ if cost.reported.is_some() && cost.estimated.is_none() => {
                    Style::new().fg(theme::GREEN)
                }
                _ => theme::dim(),
            }
        }
        CardField::Profile => Style::new().fg(theme::agent_color(
            t.current_run().map_or("", |r| r.agent.as_str()),
        )),
        CardField::Stage => Style::new().fg(theme::BLUE),
        CardField::NeedsMe => Style::new().fg(theme::YELLOW),
        CardField::Lines => Style::new().fg(theme::GREEN),
        CardField::Pinned => Style::new().fg(theme::YELLOW),
        _ => theme::dim(),
    }
}

fn line_width(parts: &[(CardField, String)]) -> usize {
    parts.iter().map(|(_, s)| s.width()).sum::<usize>() + 3 * parts.len().saturating_sub(1)
}

/// The fields `cfg` selects for `t`'s column that have a value, in priority order, dropping the lowest-priority
/// (last) ones until the line fits `width`. A leading cost that still does not fit switches to its short form
/// (`~$2.25`) before anything is cut off. `None` when nothing is left.
pub(crate) fn field_spans(
    t: &Task,
    all: &[Task],
    cfg: &CardCfg,
    width: usize,
    now: Timestamp,
    pinned: bool,
) -> Option<Vec<Span<'static>>> {
    let mut parts: Vec<(CardField, String)> = cfg
        .fields_for(t.status)
        .into_iter()
        .filter_map(|f| f.text(t, all, now, pinned).map(|text| (f, text)))
        .collect();
    parts = CardField::fit(parts, width);
    if line_width(&parts) > width
        && let Some((CardField::Cost, text)) = parts.first_mut()
    {
        *text = TaskCost::of(t).short();
    }
    let mut spans = Vec::new();
    for (i, (field, text)) in parts.into_iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" · ", theme::dim()));
        }
        let room = width.saturating_sub(spans.iter().map(|s| s.width()).sum());
        spans.push(Span::styled(
            truncate(&text, room),
            field_style(field, t, cfg),
        ));
    }
    (!spans.is_empty()).then_some(spans)
}

/// `▰▰▱▱ 2/4` for rich density, at most `width` cells wide; `None` without checklist steps.
pub(crate) fn progress_bar(p: &RunProgress, width: usize) -> Option<String> {
    let total = p.steps().count();
    if total == 0 {
        return None;
    }
    let done = p.steps().filter(|s| s.status == StepState::Done).count();
    let label = format!(" {done}/{total}");
    let cells = total.min(width.saturating_sub(label.width())).min(12);
    if cells == 0 {
        return None;
    }
    let filled = (done * cells).div_ceil(total).min(cells);
    Some(format!(
        "{}{}{label}",
        "▰".repeat(filled),
        "▱".repeat(cells - filled)
    ))
}
