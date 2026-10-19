//! The plan-first flow on the board (T-100): a task's plan stage, its card lines, the detail Plan tab and the
//! owner decision sheet. Plan stages come from the task file only (`plan_runs`, `plans`, `children`), so cards,
//! the stage strip and the Plan tab always agree.

pub(crate) mod decision;
pub(crate) mod tab;
#[cfg(test)]
pub(crate) mod tests;
pub(crate) mod tracker;

use corral_core::{Config, PlanRunState, Status, Task, Verdict};
use jiff::Timestamp;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use unicode_width::UnicodeWidthStr;

use crate::theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PlanStage {
    /// Plan-first, but no plan has been started yet.
    Needed,
    /// The planner writes the plan (round 0) or revises it after requested changes.
    Planning {
        round: u32,
    },
    /// The independent reviewer has the plan; `round` is 1-based.
    Review {
        round: u32,
    },
    AwaitingOwner,
    Escalated,
    Failed,
    /// Accepted and attached to this task as its single implementation task.
    Approved,
    Decomposed(usize),
}

impl PlanStage {
    pub(crate) fn label(self) -> String {
        match self {
            Self::Needed => "◇ plan first".into(),
            Self::Planning { round: 0 } => "✎ planning".into(),
            Self::Planning { round } => format!("✎ planning · round {}", round + 1),
            Self::Review { round } => format!("◎ plan review round {round}"),
            Self::AwaitingOwner => "▲ awaiting your approval".into(),
            Self::Escalated => "⛔ plan escalated".into(),
            Self::Failed => "✗ plan failed".into(),
            Self::Approved => "✓ plan approved".into(),
            Self::Decomposed(1) => "⊞ decomposed into 1 task".into(),
            Self::Decomposed(n) => format!("⊞ decomposed into {n} tasks"),
        }
    }

    pub(crate) fn color(self) -> Color {
        match self {
            Self::Needed => theme::DIM,
            Self::Planning { .. } | Self::Review { .. } => theme::BLUE,
            Self::AwaitingOwner => theme::YELLOW,
            Self::Escalated | Self::Failed => theme::RED,
            Self::Approved | Self::Decomposed(_) => theme::GREEN,
        }
    }

    /// The planning flow still owns the task, before any implementation work.
    pub(crate) fn active(self) -> bool {
        matches!(
            self,
            Self::Planning { .. }
                | Self::Review { .. }
                | Self::AwaitingOwner
                | Self::Escalated
                | Self::Failed
        )
    }

    pub(crate) fn decidable(self) -> bool {
        matches!(self, Self::AwaitingOwner | Self::Escalated)
    }
}

/// Plan-first: the task has plan history, or needs a plan before implementation (`design`, `plan_required`).
/// Without `config` only the task's own `plan_required` override counts. A decomposed child carries its parent's
/// approved plan, and implementation runs that started without a plan predate planning.
pub(crate) fn plan_first(t: &Task, config: Option<&Config>) -> bool {
    if t.tracker {
        return false;
    }
    if !t.plan_runs.is_empty() || !t.plans.is_empty() || !t.children.is_empty() {
        return true;
    }
    let required = config.map_or(t.plan_required == Some(true), |c| {
        c.plan_required(t.project.as_deref(), t.plan_required)
    });
    (t.design || required)
        && t.plan.is_none()
        && t.runs.is_empty()
        && !matches!(t.status, Status::Done | Status::Dropped)
}

pub(crate) fn stage(t: &Task, config: Option<&Config>) -> Option<PlanStage> {
    if !plan_first(t, config) {
        return None;
    }
    if !t.children.is_empty() {
        return Some(PlanStage::Decomposed(t.children.len()));
    }
    let run = t.plan_runs.last();
    let plan = t.plans.last();
    let round = run.map_or(0, |r| r.rounds);
    Some(match run.map(|r| r.state) {
        Some(PlanRunState::Planning | PlanRunState::Changes) => PlanStage::Planning { round },
        _ if plan.is_some_and(|p| p.approval.is_some()) => PlanStage::Approved,
        Some(PlanRunState::Submitted | PlanRunState::Reviewing) => {
            PlanStage::Review { round: round + 1 }
        }
        Some(PlanRunState::AwaitingOwner) => PlanStage::AwaitingOwner,
        Some(PlanRunState::Approved) => PlanStage::Approved,
        Some(PlanRunState::Escalated) => PlanStage::Escalated,
        Some(PlanRunState::Failed | PlanRunState::Cancelled) => PlanStage::Failed,
        None => match plan {
            None => PlanStage::Needed,
            Some(p) => {
                let n = u32::try_from(p.reviews.len()).unwrap_or(u32::MAX);
                match p.reviews.last().map(|r| r.verdict) {
                    None => PlanStage::Review { round: 1 },
                    Some(None) => PlanStage::Review { round: n },
                    Some(Some(Verdict::Approve)) => PlanStage::AwaitingOwner,
                    Some(Some(Verdict::Changes)) => PlanStage::Planning { round: n },
                    Some(Some(Verdict::Escalate)) => PlanStage::Escalated,
                }
            }
        },
    })
}

/// The planning flow owns the task: Enter, the card menu and the review route open its Plan tab.
pub(crate) fn owns(t: &Task) -> bool {
    t.tracker || t.current_run().is_none() && stage(t, None).is_some_and(PlanStage::active)
}

fn first_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(str::to_string)
}

/// Word-wraps `text` to `width` so a stage label is never cut off on a narrow card.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = vec![String::new()];
    for word in text.split(' ') {
        let line = lines.last_mut().map_or(0, |l| l.width());
        if line > 0 && line + 1 + word.width() > width {
            lines.push(word.to_string());
        } else if let Some(l) = lines.last_mut() {
            if line > 0 {
                l.push(' ');
            }
            l.push_str(word);
        }
    }
    lines
}

pub(crate) fn stage_lines(stage: PlanStage, width: usize) -> Vec<Line<'static>> {
    let style = Style::new().fg(stage.color());
    wrap(&stage.label(), width)
        .into_iter()
        .map(|l| Line::styled(l, style))
        .collect()
}

/// Card lines while the planning flow owns the task; they replace the status lines.
pub(crate) fn card_lines(
    t: &Task,
    stage: PlanStage,
    width: usize,
    now: Timestamp,
) -> Vec<Line<'static>> {
    let run = t.plan_runs.last();
    let plan = t.plans.last();
    let since = match stage {
        PlanStage::Planning { .. } => run.map_or(t.updated, |r| r.started),
        _ => t.updated,
    };
    let age = corral_core::age_text(now.as_second() - since.as_second());
    let mut out = stage_lines(stage, width);
    let last = plan.and_then(|p| p.reviews.last());
    let reason = || {
        last.and_then(|r| r.notes.as_deref())
            .or(run.and_then(|r| r.pending_reply.as_deref()))
            .and_then(first_line)
    };
    let italic = Style::new()
        .fg(theme::YELLOW)
        .add_modifier(Modifier::ITALIC);
    match stage {
        PlanStage::Review { .. } => out.push(Line::styled(
            match last.filter(|r| r.verdict.is_none()) {
                Some(r) => format!("reviewer {} · {age}", r.reviewer),
                None => format!("waiting for a reviewer · {age}"),
            },
            theme::dim(),
        )),
        PlanStage::Planning { round } => {
            if let Some(r) = run {
                out.push(Line::from(vec![
                    Span::styled(
                        r.profile.clone(),
                        Style::new().fg(theme::agent_color(&r.profile)),
                    ),
                    Span::styled(format!(" · {age}"), theme::dim()),
                ]));
            }
            if let Some(why) = reason().filter(|_| round > 0) {
                out.push(Line::styled(format!("changes: {why}"), theme::dim()));
            }
        }
        PlanStage::AwaitingOwner => {
            if let (Some(p), Some(r)) = (plan, last) {
                out.push(Line::styled(
                    format!("{} approved by {}", p.id, r.reviewer),
                    theme::dim(),
                ));
            }
            out.push(Line::styled(
                "→ r decide the plan",
                Style::new().fg(theme::FG),
            ));
        }
        PlanStage::Escalated | PlanStage::Failed => {
            if let Some(why) = reason() {
                out.push(Line::styled(format!("\"{why}\""), italic));
            }
        }
        _ => {}
    }
    out
}
