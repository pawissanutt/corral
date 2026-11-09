//! General › Coding concurrency: the soft automatic target, the hard slot ceiling and the unfinished-work (WIP)
//! limit, grouped with a heading that states their effective values and how they differ. Number rows step with
//! `−`/`+` (mouse) or `←`/`→` (keyboard); stepping never clamps, so an out-of-range value shows its validation error.

use corral_core::Config;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;

use super::fields::FormLine;
use super::model::{Field, Kind, Row};
use crate::theme;

pub(super) fn rows(cfg: &Config) -> Vec<Row> {
    if cfg.limits.capacity.is_some() {
        return super::capacity::rows(cfg);
    }
    let l = &cfg.limits;
    vec![
        Row::new(
            Field::SlotLimit,
            "Hard slot ceiling",
            Kind::Number { optional: false },
            l.running.to_string(),
        )
        .help("Most runs holding a slot at once (incl. Blocked), all projects.")
        .key("limits.running")
        .stepper(),
        Row::new(
            Field::RunningTarget,
            "Soft coding target",
            Kind::Number { optional: true },
            l.running_target.map(|t| t.to_string()).unwrap_or_default(),
        )
        .help("Automatic coding fills to here; the rest is manual/P0 headroom.")
        .inherits(format!("hard ceiling ({})", l.running))
        .key("limits.running_target")
        .stepper(),
        Row::new(
            Field::WipLimit,
            "Unfinished work",
            Kind::Number { optional: false },
            l.wip.to_string(),
        )
        .help("Coding tasks not landed (Running/Blocked/parked/Review). 0 = off.")
        .key("limits.wip")
        .stepper(),
        super::capacity::mode_row(false),
    ]
}

/// Steps a stepper row by one. `None` for rows that do not step.
pub(super) fn step(cfg: &mut Config, field: &Field, forward: bool) -> Option<()> {
    if matches!(field, Field::Capacity(_)) {
        return super::capacity::step(cfg, field, forward);
    }
    let by = |n: usize| {
        if forward {
            n.saturating_add(1)
        } else {
            n.saturating_sub(1)
        }
    };
    let l = &mut cfg.limits;
    match field {
        Field::SlotLimit => l.running = by(l.running),
        Field::RunningTarget => l.running_target = Some(by(l.running_target.unwrap_or(l.running))),
        Field::WipLimit => l.wip = by(l.wip),
        _ => return None,
    }
    Some(())
}

/// Heading lines drawn above `row` in General: the concurrency group before its first row, and a
/// "Board and PM" heading before the rest.
pub(super) fn heading(cfg: &Config, row: &Row, width: usize) -> Vec<FormLine> {
    if let Some(lines) = super::capacity::heading(cfg, row, width) {
        return lines;
    }
    let bold = Style::new().fg(theme::CYAN).add_modifier(Modifier::BOLD);
    match row.field {
        Field::SlotLimit => {
            let l = &cfg.limits;
            let target = corral_ops::soft_target(l);
            let wip = if l.wip == 0 {
                "off".to_string()
            } else {
                l.wip.to_string()
            };
            let mut text = "Slots count runs active now; WIP counts unfinished coding tasks, \
                incl. blocked, parked and review. WIP may exceed the target so review never \
                stalls coding."
                .to_string();
            if l.wip != 0 && l.wip <= target {
                text.push_str(" WIP ≤ target: unfinished work, not slots, limits new coding.");
            }
            let mut out = vec![FormLine::plain(Line::styled(
                format!(
                    "  Coding concurrency   auto {target} · hard {} · WIP {wip}",
                    l.running
                ),
                bold,
            ))];
            out.extend(
                crate::render::wrap::wrap(
                    &crate::render::wrap::chunks(&[(text, theme::dim())]),
                    width.saturating_sub(4),
                )
                .into_iter()
                .map(|mut spans| {
                    spans.insert(0, ratatui::text::Span::raw("    "));
                    FormLine::plain(Line::from(spans))
                }),
            );
            out
        }
        Field::DefaultProfile => vec![
            FormLine::plain(Line::from("")),
            FormLine::plain(Line::styled("  Board and PM", bold)),
        ],
        _ => Vec::new(),
    }
}

/// Footer hints of a stepper row.
pub(super) fn hints(row: &Row) -> Vec<(&'static str, &'static str)> {
    let mut h = vec![("⏎", "edit"), ("←→", "step")];
    if row.field == Field::RunningTarget && !row.value.is_empty() {
        h.push(("x", "unset"));
    }
    h
}

#[cfg(test)]
#[path = "concurrency_tests.rs"]
mod tests;
