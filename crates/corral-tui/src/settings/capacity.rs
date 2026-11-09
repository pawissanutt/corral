//! Four independent-pool controls, staged through the existing validated Settings save path.
use corral_core::{CapacityLimits, Config};
use ratatui::{style::Style, text::Line};

use super::{
    fields::FormLine,
    model::{Field, Kind, Row},
};
use crate::theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CapacityField {
    Unfinished,
    Agents,
    Gates,
    Reserve,
}

pub(super) fn error_text<'a>(row: &Row, text: &'a str) -> &'a str {
    if matches!(row.field, Field::Capacity(_)) {
        text.strip_prefix(&row.key).unwrap_or(text).trim_start()
    } else {
        text
    }
}

pub(super) fn mode_row(enabled: bool) -> Row {
    Row::new(
        Field::CapacityMode,
        "Independent pools",
        Kind::Toggle,
        if enabled { "on" } else { "off" }.into(),
    )
    .help(
        "Stages four draft limits. Saving requires compatible Corral versions; no work is killed.",
    )
    .key("limits.capacity")
}

pub(super) fn rows(cfg: &Config) -> Vec<Row> {
    let Some(limits) = &cfg.limits.capacity else {
        return vec![mode_row(false)];
    };
    let mut rows = [
        (CapacityField::Unfinished, "Unfinished tasks", limits.unfinished, "unfinished",
            "Started tasks, including paused and review work. Tracker parents do not count."),
        (CapacityField::Agents, "Concurrent agents", limits.agents, "agents",
            "Worker, planner and reviewer leases. A waiting worker counts until its lease is released."),
        (CapacityField::Gates, "Concurrent gates", limits.gates, "gates",
            "Corral gate runs, including aggregate verification and checkpoints; not every shell command."),
        (CapacityField::Reserve, "Ad-hoc reserve", limits.adhoc_reserve, "adhoc_reserve",
            "Reserved in both agents and unfinished tasks; explicit ad-hoc designation is not authority."),
    ].into_iter().map(|(field, label, value, key, help)| {
        Row::new(Field::Capacity(field), label, Kind::Number { optional: false }, value.to_string())
            .help(help).key(format!("limits.capacity.{key}")).stepper()
    }).collect::<Vec<_>>();
    rows.push(mode_row(true));
    rows
}

pub(super) fn set_value(cfg: &mut Config, field: &Field, text: &str) -> Result<(), String> {
    if *field == Field::CapacityMode {
        if text == "on" {
            if cfg.limits.capacity.is_none() {
                // A visible draft, not an automatic migration. The old soft target is not mapped.
                let unfinished = if cfg.limits.wip == 0 {
                    6
                } else {
                    cfg.limits.wip
                };
                let agents = cfg.limits.running;
                cfg.limits.capacity = Some(CapacityLimits {
                    unfinished,
                    agents,
                    gates: cfg.limits.review,
                    adhoc_reserve: usize::from(unfinished > 1 && agents > 1),
                });
            }
        } else {
            cfg.limits.capacity = None;
        }
        return Ok(());
    }
    let Field::Capacity(field) = field else {
        return Err("not a capacity field".into());
    };
    let value = super::model::number(text)?;
    *value_mut(cfg, *field)? = value;
    Ok(())
}

fn value_mut(cfg: &mut Config, field: CapacityField) -> Result<&mut usize, String> {
    let limits = cfg
        .limits
        .capacity
        .as_mut()
        .ok_or("stage independent pools first")?;
    Ok(match field {
        CapacityField::Unfinished => &mut limits.unfinished,
        CapacityField::Agents => &mut limits.agents,
        CapacityField::Gates => &mut limits.gates,
        CapacityField::Reserve => &mut limits.adhoc_reserve,
    })
}

pub(super) fn step(cfg: &mut Config, field: &Field, forward: bool) -> Option<()> {
    let Field::Capacity(field) = field else {
        return None;
    };
    let value = value_mut(cfg, *field).ok()?;
    *value = if forward {
        value.saturating_add(1)
    } else {
        value.saturating_sub(1)
    };
    Some(())
}

pub(super) fn heading(cfg: &Config, row: &Row, width: usize) -> Option<Vec<FormLine>> {
    if row.field != Field::Capacity(CapacityField::Unfinished) {
        return None;
    }
    let limits = cfg.limits.capacity.as_ref()?;
    let mut lines = vec![FormLine::plain(Line::styled(
        "  Independent capacity",
        Style::default().fg(theme::CYAN),
    ))];
    let text = format!(
        "Normal / total limits: agents {} / {}, unfinished {} / {}. Gates {} independently; ad-hoc reserve {}. Review these draft values before saving; the legacy soft target is not reused. Independent scheduling requires store fence 5; returning to legacy does not lower it.",
        limits.normal_agents(),
        limits.agents,
        limits.normal_unfinished(),
        limits.unfinished,
        limits.gates,
        limits.adhoc_reserve,
    );
    lines.extend(
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
    Some(lines)
}
