//! Resource samples are refreshed with the board, never during terminal redraws.

use std::collections::HashMap;
use std::sync::Arc;

use corral_core::{AutoStage, Config, GateResult, ResourceState, Status, Task};
use corral_ops::Ctx;
use corral_ops::resources::FixedResourceProbe;
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
        let (state, min_mem, min_disk) = match sample.and_then(|state| {
            Ok((
                state,
                config.resources.min_mem_bytes()?,
                config.resources.min_disk_bytes()?,
            ))
        }) {
            Ok(values) => values,
            Err(error) => {
                return Self {
                    reason: None,
                    gauges: vec![(format!("resources unavailable: {error:#}"), theme::YELLOW)],
                };
            }
        };
        let s = &state.snapshot;
        let load_color = if s.load_per_core > config.resources.max_load_per_core {
            theme::RED
        } else if s.load_per_core >= config.resources.max_load_per_core * 0.8 {
            theme::YELLOW
        } else {
            theme::GREEN
        };
        let low_color = |available: u64, minimum: u64| {
            if available < minimum {
                theme::RED
            } else if available < minimum.saturating_add(minimum / 4) {
                theme::YELLOW
            } else {
                theme::GREEN
            }
        };
        let disk = s.disks.iter().map(|d| d.free_bytes).min();
        let mut gauges = vec![
            (format!("load {:.2}/core", s.load_per_core), load_color),
            (
                format!("mem {}", bytes(s.mem_available)),
                low_color(s.mem_available, min_mem),
            ),
            (
                disk.map_or_else(|| "disk n/a".into(), |free| format!("disk {}", bytes(free))),
                disk.map_or(theme::YELLOW, |free| low_color(free, min_disk)),
            ),
        ];
        if state.held {
            gauges.push((
                if state.severe {
                    "held: needs you"
                } else {
                    "held"
                }
                .into(),
                if state.severe {
                    theme::RED
                } else {
                    theme::YELLOW
                },
            ));
        }
        Self {
            reason: state.reason,
            gauges,
        }
    }

    /// One host observation per board refresh, replayed into the shared dispatch queue so the gauges and every
    /// Ready card's waiting reason (`eligible` or the exact wait) agree. Returns the reasons by task id.
    pub(crate) fn sample_board(ctx: &Ctx, tasks: &[Task]) -> (Self, HashMap<String, String>) {
        let snapshot = match ctx.resource_probe.sample(&ctx.store) {
            Ok(snapshot) => snapshot,
            Err(error) => {
                return (
                    Self::from_sample(Err(error), &ctx.store.config),
                    HashMap::new(),
                );
            }
        };
        let replay = Ctx {
            resource_probe: Arc::new(FixedResourceProbe(snapshot)),
            ..ctx.clone()
        };
        let mut display = Self::sample(&replay);
        let waits = match corral_ops::dispatch_queue(&replay) {
            Ok(entries) => entries
                .into_iter()
                .filter(|e| {
                    tasks
                        .iter()
                        .any(|t| t.id == e.id && t.status == Status::Ready)
                })
                .map(|e| (e.id, e.reason))
                .collect(),
            Err(error) => {
                display
                    .gauges
                    .push((format!("queue unavailable: {error:#}"), theme::RED));
                HashMap::new()
            }
        };
        (display, waits)
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

fn bytes(bytes: u64) -> String {
    if bytes >= 1_073_741_824 {
        format!("{}GiB", bytes / 1_073_741_824)
    } else {
        format!("{}MiB", bytes / 1_048_576)
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

pub(crate) fn append_hold(
    lines: &mut Vec<Line<'static>>,
    task: &Task,
    config: &Config,
    reason: Option<&str>,
    width: usize,
) {
    if reason
        .and_then(|reason| hold(task, config, reason))
        .is_none()
    {
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
