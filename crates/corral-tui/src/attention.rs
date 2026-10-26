//! Refresh-time attention reads and shared, read-only presentation for board and detail.
use std::collections::BTreeMap;
use std::sync::Arc;

use corral_core::attention::{Assessment, Condition, Responsible, WaitClass};
use corral_core::{Config, ResourceSnapshot, Store, Task};
use corral_ops::Ctx;
use corral_ops::resources::ResourceProbe;
use jiff::Timestamp;
use ratatui::style::Style;
use ratatui::text::Line;

use crate::artifacts::ui;
use crate::overlays::truncate;
use crate::theme;

pub(crate) type Snapshot = Result<BTreeMap<String, Assessment>, String>;

#[derive(Debug)]
struct RefreshProbe(Result<ResourceSnapshot, String>);

impl ResourceProbe for RefreshProbe {
    fn sample(&self, _: &Store) -> anyhow::Result<ResourceSnapshot> {
        self.0.clone().map_err(anyhow::Error::msg)
    }
}

/// Gauges and attention share one observation, including an unavailable sample.
/// Keep this context local to the refresh; the next refresh must sample again.
pub(crate) fn refresh_context(ctx: &Ctx) -> Ctx {
    let observation = ctx
        .resource_probe
        .sample(&ctx.store)
        .map_err(|error| format!("{error:#}"));
    Ctx {
        resource_probe: Arc::new(RefreshProbe(observation)),
        ..ctx.clone()
    }
}

/// Called by refresh/load, never by draw. A failed read is not an empty assessment.
pub(crate) fn load(ctx: &Ctx, now: Timestamp) -> Snapshot {
    // Store listings intentionally skip malformed tasks. Do not turn such a partial
    // listing into a successful attention read (and a falsely reassuring count).
    let (sender, messages) = std::sync::mpsc::channel();
    let result = {
        let _capture = corral_core::capture_messages(sender);
        corral_ops::attention::board(ctx, now)
    };
    let mut unavailable = None;
    for message in messages.try_iter() {
        if message.file.is_some() {
            unavailable.get_or_insert(message.text);
        } else {
            corral_core::report_message(message.task.as_deref(), message.source, message.text);
        }
    }
    if let Some(error) = unavailable {
        return Err(error);
    }
    result
        .map(|board| {
            board
                .tasks
                .into_iter()
                .map(|task| (task.task, task.assessment))
                .collect()
        })
        .map_err(|error| format!("{error:#}"))
}

fn age(condition: &Condition, now: Timestamp) -> Option<i64> {
    condition
        .since
        .map(|since| now.duration_since(since).as_secs().max(0))
}

fn age_text(condition: &Condition, now: Timestamp) -> String {
    age(condition, now).map_or_else(|| "unknown".into(), corral_core::age_text)
}

/// Outstanding keyed conditions are due at their responsible party's configured bound.
/// Handling pauses the ladder until next_check; a notification never resolves a condition.
pub(crate) fn overdue(condition: &Condition, task: &Task, config: &Config, now: Timestamp) -> bool {
    let Some(key) = &condition.key else {
        return false;
    };
    if task
        .attention
        .records
        .get(key)
        .and_then(|record| record.handling.as_ref())
        .is_some_and(|handling| now < handling.next_check)
    {
        return false;
    }
    let bound = match condition.responsible {
        Responsible::Pm => config.pm.attention_secs,
        Responsible::Owner => config.pm.attention_owner_secs,
        Responsible::Executor | Responsible::External => return false,
    };
    age(condition, now).is_some_and(|age| u64::try_from(age).is_ok_and(|age| age >= bound))
}

pub(crate) fn header(
    snapshot: &Snapshot,
    tasks: &[Task],
    config: &Config,
    now: Timestamp,
) -> Option<String> {
    match snapshot {
        Ok(assessments) => {
            let count = tasks
                .iter()
                .filter_map(|task| {
                    assessments
                        .get(&task.id)
                        .map(|assessment| (task, assessment))
                })
                .map(|(task, assessment)| {
                    assessment
                        .conditions
                        .iter()
                        .filter(|condition| overdue(condition, task, config, now))
                        .count()
                })
                .sum::<usize>();
            (count > 0).then(|| format!("overdue {count}"))
        }
        Err(error) => Some(format!("attention unavailable: {error}")),
    }
}

pub(crate) fn card_line(
    assessment: &Assessment,
    task: &Task,
    config: &Config,
    width: usize,
    now: Timestamp,
) -> Option<Line<'static>> {
    let visible = |condition: &&Condition| {
        condition.kind == "unexpected_stall"
            || condition.class == WaitClass::OwnerDecision
            || overdue(condition, task, config, now)
    };
    let condition = assessment
        .primary
        .as_ref()
        .filter(visible)
        .or_else(|| assessment.conditions.iter().find(visible))?;
    let class = if condition.kind == "unexpected_stall" {
        "Stall"
    } else {
        condition
            .class
            .label()
            .split_whitespace()
            .next()
            .unwrap_or_default()
    };
    Some(Line::styled(
        truncate(
            &format!(
                "{class} {}: {}",
                age_text(condition, now),
                condition
                    .reason
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
            width,
        ),
        Style::new().fg(theme::YELLOW),
    ))
}

pub(crate) fn waiting(task: &Task, snapshot: &Snapshot, now: Timestamp) -> Vec<Line<'static>> {
    let mut lines = vec![Line::styled("Waiting", theme::bold())];
    let assessment = match snapshot {
        Err(error) => {
            lines.extend(ui::text_lines(&format!("attention unavailable: {error}")));
            return lines;
        }
        Ok(assessments) => match assessments.get(&task.id) {
            Some(assessment) => assessment,
            None => {
                lines.extend(ui::text_lines(
                    "attention unavailable: task missing from projection",
                ));
                return lines;
            }
        },
    };
    if assessment.conditions.is_empty() {
        lines.push(Line::styled("No waits.", theme::dim()));
        return lines;
    }
    let primary = assessment
        .primary
        .as_ref()
        .and_then(|primary| assessment.conditions.iter().position(|row| row == primary));
    let rows = primary
        .into_iter()
        .chain((0..assessment.conditions.len()).filter(|index| Some(*index) != primary));
    for index in rows {
        let condition = &assessment.conditions[index];
        lines.push(Line::styled(
            format!(
                "{} - {}",
                condition.class.label(),
                condition.responsible.label()
            ),
            Style::new().fg(theme::YELLOW),
        ));
        lines.extend(ui::text_lines(&condition.reason));
        let since = condition.since.map_or_else(
            || "unknown".into(),
            |at| ui::local_date(at, "%Y-%m-%d %H:%M:%S %Z"),
        );
        lines.push(Line::styled(
            format!(
                "Since {since} ({}, age {})",
                condition.since_source.label(),
                age_text(condition, now),
            ),
            theme::dim(),
        ));
        for check in &condition.resume {
            let truth = match check.satisfied {
                Some(true) => "true",
                Some(false) => "false",
                None => "unknown",
            };
            lines.extend(ui::text_lines(&format!(
                "Resume [{truth}]: {}{}",
                check.description,
                if check.machine_checkable {
                    ""
                } else {
                    " (manual check)"
                },
            )));
        }
        let next = &condition.next_action;
        lines.extend(ui::text_lines(&format!(
            "Next{}: {}{}{}",
            if next.requires_owner {
                " (owner only)"
            } else {
                ""
            },
            next.command
                .as_deref()
                .map_or(String::new(), |command| format!("{command} - ")),
            next.description,
            next.recipient
                .as_deref()
                .map_or(String::new(), |recipient| format!(
                    " [recipient {recipient}]"
                )),
        )));
        if let Some(handling) = condition
            .key
            .as_ref()
            .and_then(|key| task.attention.records.get(key))
            .and_then(|record| record.handling.as_ref())
        {
            lines.extend(ui::text_lines(&format!(
                "Handling ({}): {} | next_check {}",
                handling.by.label(),
                handling.note,
                ui::local_date(handling.next_check, "%Y-%m-%d %H:%M:%S %Z"),
            )));
        }
        lines.push(Line::default());
    }
    lines
}
