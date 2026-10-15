//! The configured card fields as one line of spans, shared by the board cards and the settings preview.

use corral_core::{CardCfg, CardField, RunProgress, StepState, Task, cost_value};
use jiff::Timestamp;
use ratatui::style::Style;
use ratatui::text::Span;
use unicode_width::UnicodeWidthStr;

use crate::overlays::truncate;
use crate::theme;

fn field_style(field: CardField, t: &Task, cfg: &CardCfg) -> Style {
    match field {
        CardField::Cost => match (cost_value(t), cfg.cost_warn) {
            (Some((v, _)), Some(warn)) if v >= warn => Style::new().fg(theme::YELLOW),
            (Some((_, true)), _) => Style::new().fg(theme::GREEN),
            _ => theme::dim(),
        },
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

/// The fields `cfg` selects for `t`'s column that have a value, in priority order, dropping the lowest-priority
/// (last) ones until the line fits `width`. `None` when nothing is left.
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
