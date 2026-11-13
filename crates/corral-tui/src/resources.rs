//! Resource samples are refreshed with the board, never during terminal redraws.

use corral_core::{AutoStage, Config, GateResult, ResourceState, Status, Task};
use corral_ops::Ctx;
use corral_ops::classic::{GaugeLevel, ResourceGauges};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

use crate::{overlays::truncate, theme};

#[derive(Clone)]
pub(crate) struct ResourceDisplay {
    pub(crate) reason: Option<String>,
    gauges: Vec<(String, Color)>,
}

impl ResourceDisplay {
    pub(crate) fn sample(ctx: &Ctx) -> Self {
        Self::from_sample(corral_ops::resources::status(ctx), &ctx.store.config)
    }

    pub(crate) fn from_sample(sample: anyhow::Result<ResourceState>, config: &Config) -> Self {
        Self::from_gauges(&corral_ops::classic::gauges(sample, config))
    }

    /// The gauges of a classic frame, drawn in frame order.
    pub(crate) fn from_gauges(gauges: &ResourceGauges) -> Self {
        Self {
            reason: gauges.reason.clone(),
            gauges: gauges
                .gauges
                .iter()
                .map(|gauge| {
                    let color = match gauge.level {
                        GaugeLevel::Good => theme::GREEN,
                        GaugeLevel::Warn => theme::YELLOW,
                        GaugeLevel::Bad => theme::RED,
                    };
                    (gauge.text.clone(), color)
                })
                .collect(),
        }
    }

    pub(crate) fn line(&self) -> Line<'static> {
        let mut spans = vec![Span::raw(" ")];
        for (i, (text, color)) in self.gauges.iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled("  ", theme::dim()));
            }
            spans.push(Span::styled(text.clone(), Style::new().fg(*color)));
        }
        Line::from(spans)
    }
}

/// The board-wide load numbers live in the header; a card only says that it waits.
pub(crate) const MACHINE_BUSY: &str = "waiting: machine busy";

/// A review can be held before its first autopilot stage is persisted.
pub(crate) fn waiting_review(task: &Task) -> bool {
    if task.status != Status::Review {
        return false;
    }
    let run = task.current_run();
    if run.is_some_and(|run| {
        run.gate
            .as_ref()
            .is_some_and(|gate| gate.result == GateResult::Running)
            || run
                .review
                .as_ref()
                .is_some_and(|review| review.holds_slot())
            || run.review_runs.iter().any(|review| review.ended.is_none())
    }) {
        return false;
    }
    if task.review_wait.is_some() {
        return true;
    }
    match run.and_then(|r| r.autopilot.as_ref()).map(|a| a.stage) {
        None => true,
        Some(AutoStage::Gating) => !run
            .and_then(|r| r.gate.as_ref())
            .is_some_and(|g| g.result == GateResult::Running),
        Some(AutoStage::Reviewing) => !run.is_some_and(|r| {
            r.review.as_ref().is_some_and(|review| review.holds_slot())
                || r.review_runs.iter().any(|review| review.ended.is_none())
        }),
        Some(
            AutoStage::Landing
            | AutoStage::Reviewed
            | AutoStage::Changes
            | AutoStage::Blocked
            | AutoStage::Escalated
            | AutoStage::Done,
        ) => false,
    }
}

pub(crate) fn waiting_review_head(task: &corral_core::TaskHead) -> bool {
    if task.status != Status::Review {
        return false;
    }
    let run = task.current_run.as_ref();
    if run.is_some_and(|r| {
        r.gate
            .as_ref()
            .is_some_and(|g| g.result == GateResult::Running)
            || r.review.as_ref().is_some_and(|r| r.holds_slot())
            || r.facts.open_children > 0
    }) {
        return false;
    }
    if task.review_wait.is_some() {
        return true;
    }
    match run.and_then(|r| r.autopilot.as_ref()).map(|a| a.stage) {
        None => true,
        Some(AutoStage::Gating) => !run
            .and_then(|r| r.gate.as_ref())
            .is_some_and(|g| g.result == GateResult::Running),
        Some(AutoStage::Reviewing) => !run.is_some_and(|r| {
            r.review.as_ref().is_some_and(|r| r.holds_slot()) || r.facts.open_children > 0
        }),
        Some(
            AutoStage::Landing
            | AutoStage::Reviewed
            | AutoStage::Changes
            | AutoStage::Blocked
            | AutoStage::Escalated
            | AutoStage::Done,
        ) => false,
    }
}

/// Resource hold text shared by the board and textual autopilot status. Ready work is admitted for every
/// configured project unless `[dispatch] ready_projects` names a restriction.
pub fn hold(task: &Task, config: &Config, reason: &str) -> Option<String> {
    let authorized_ready = task.status == Status::Ready
        && !task.dispatch_hold
        && config.ready_project_allowed(task.project.as_deref());
    // Only Ready is the queue: a stale `queued` flag on a Backlog card never reads as waiting.
    let enqueued_ready = task.status == Status::Ready && task.queued;
    (enqueued_ready || authorized_ready || waiting_review(task))
        .then(|| format!("waiting: machine busy ({reason})"))
}

pub(crate) fn append_hold_head(
    lines: &mut Vec<Line<'static>>,
    task: &Task,
    config: &Config,
    reason: Option<&str>,
    width: usize,
    head: Option<&corral_core::TaskHead>,
) {
    let held = match head {
        None => reason
            .and_then(|reason| hold(task, config, reason))
            .is_some(),
        Some(head) => {
            reason.is_some()
                && (head.status == Status::Ready
                    && (head.queued
                        || !head.dispatch_hold
                            && config.ready_project_allowed(head.project.as_deref()))
                    || waiting_review_head(head))
        }
    };
    if !held {
        return;
    }
    let room = width;
    let parts = if MACHINE_BUSY.len() <= room {
        vec![MACHINE_BUSY]
    } else {
        vec!["waiting:", "machine busy"]
    };
    lines.extend(parts.into_iter().map(|part| {
        Line::from(vec![
            Span::raw("  "),
            Span::styled(truncate(part, room), Style::new().fg(theme::YELLOW)),
        ])
    }));
}

#[cfg(test)]
mod tests;
