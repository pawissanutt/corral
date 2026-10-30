use corral_core::{AutoStage, GateResult, Run, Status, Task, Usage};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::Stage;
use crate::hit;
use crate::theme;
use crate::{Action, Hit, HitMap};
use corral_ops::watch::StageHistory;

pub(crate) struct StripState {
    pub history: StageHistory,
    pub landing: Option<(jiff::Timestamp, Option<jiff::Timestamp>)>,
    /// Plan-first under the effective config: the strip leads with the plan cells.
    pub plan: bool,
    /// A submitted plan whose review the owner can start; its "Plan review" cell presses `R`.
    pub plan_review: Option<corral_ops::PlanReviewOffer>,
}

pub(crate) fn load(ctx: &corral_ops::Ctx, task: &Task) -> Option<StripState> {
    let result = (|| -> anyhow::Result<StripState> {
        let history = corral_ops::watch::load(ctx, &task.id)?;
        let mut landing = None;
        if let Some(run) = task.current_run() {
            for event in ctx.store.events()?.iter().filter(|e| {
                e.task.as_deref() == Some(task.id.as_str())
                    && e.detail["run"].as_str() == Some(run.id.as_str())
            }) {
                if event.kind == "autopilot" && event.detail["to"] == "landing" {
                    landing = Some((event.ts, None));
                } else if (event.kind == "approved"
                    || (event.kind == "autopilot" && event.detail["to"] == "done"))
                    && let Some((_, end)) = &mut landing
                {
                    *end = Some(event.ts);
                }
            }
        }
        let plan = crate::plan::plan_first(task, Some(&ctx.store.config));
        Ok(StripState {
            history,
            landing,
            plan,
            plan_review: plan
                .then(|| corral_ops::plan_review_offer(ctx, task))
                .flatten(),
        })
    })();
    match result {
        Ok(state) => Some(state),
        Err(error) => {
            corral_ops::report_message(Some(&task.id), "stage strip", format!("{error:#}"));
            None
        }
    }
}

pub(crate) fn height(task: &Task, saved: Option<&StripState>, width: u16) -> u16 {
    if width < 144 && saved.map_or_else(|| crate::plan::plan_first(task, None), |s| s.plan) {
        7
    } else {
        3
    }
}

pub(super) fn cost(usage: Option<&Usage>) -> String {
    let Some(u) = usage else {
        return "cost unknown".into();
    };
    if let Some(value) = u.reported_usd.or(u.cost_usd) {
        format!("${value:.2}")
    } else if let Some(value) = u.estimated_usd {
        format!("est. ${value:.2}")
    } else {
        "cost unpriced".into()
    }
}

fn state(task: &Task, stage: Stage) -> (&'static str, Option<&Run>) {
    let Some(run) = task.current_run() else {
        return ("pending", None);
    };
    match stage {
        Stage::Agent => (
            if run.ended.is_some() {
                "done"
            } else {
                "running"
            },
            Some(run),
        ),
        Stage::Gate => (
            run.gate.as_ref().map_or("pending", |g| {
                if g.process_lost {
                    "lost"
                } else {
                    match g.result {
                        GateResult::Running => "running",
                        GateResult::Pass => "passed",
                        GateResult::Fail => "failed",
                        GateResult::Stale => "stale",
                    }
                }
            }),
            None,
        ),
        Stage::Review => (
            run.review.as_ref().map_or("pending", |r| {
                if r.process_lost {
                    "lost"
                } else {
                    r.verdict.map_or("running", |v| v.as_str())
                }
            }),
            run.review_runs.last(),
        ),
        Stage::Land => (
            if task.status == Status::Done {
                "landed"
            } else {
                match run.autopilot.as_ref().map(|a| a.stage) {
                    Some(AutoStage::Landing) => "running",
                    Some(AutoStage::Blocked) => "blocked",
                    _ => "pending",
                }
            },
            None,
        ),
    }
}

/// Shared navigation strip. Each whole cell is a shortcut, including its status and cost.
pub(crate) fn draw(
    f: &mut Frame,
    area: Rect,
    task: &Task,
    selected: Option<Stage>,
    saved: Option<&StripState>,
    hits: &mut HitMap,
) {
    let plan = saved.map_or_else(|| crate::plan::plan_first(task, None), |s| s.plan);
    let all = if plan && area.width < 144 {
        let [planning, _, implementation] = Layout::vertical([
            Constraint::Length(3),
            Constraint::Length(1),
            Constraint::Length(3),
        ])
        .areas(area);
        let row = |area| {
            Layout::horizontal([Constraint::Fill(1); 4])
                .spacing(1)
                .split(area)
        };
        let mut cells = row(planning).to_vec();
        cells.extend_from_slice(&row(implementation));
        cells
    } else {
        Layout::horizontal(vec![Constraint::Fill(1); if plan { 8 } else { 4 }])
            .spacing(1)
            .split(area)
            .to_vec()
    };
    let cells = if plan {
        super::plan_cells::draw(f, &all[..4], task, saved.and_then(|s| s.plan_review), hits);
        &all[4..]
    } else {
        &all[..]
    };
    for (i, stage) in Stage::ALL.into_iter().enumerate() {
        let cell = cells[i];
        let (status, run) = state(task, stage);
        let status = match stage {
            Stage::Gate => saved
                .and_then(|s| {
                    s.history
                        .gates
                        .iter()
                        .rev()
                        .find(|g| task.current_run().is_some_and(|r| r.id == g.run))
                })
                .map_or(status, |g| super::gate::state(g.state).0),
            Stage::Review => saved
                .and_then(|s| {
                    s.history
                        .reviews
                        .iter()
                        .rev()
                        .find(|r| task.current_run().is_some_and(|parent| parent.id == r.run))
                })
                .map_or(status, |r| {
                    r.record
                        .as_ref()
                        .and_then(|r| r.verdict)
                        .map_or_else(|| super::gate::state(r.state).0, |v| v.as_str())
                }),
            Stage::Agent | Stage::Land => status,
        };
        let times = match stage {
            Stage::Agent | Stage::Review => run.map(|r| (r.started, r.ended)),
            Stage::Gate => saved.and_then(|s| {
                s.history
                    .gates
                    .iter()
                    .rev()
                    .find(|g| task.current_run().is_some_and(|r| r.id == g.run))
                    .map(|g| (g.started, g.ended))
            }),
            Stage::Land => saved.and_then(|s| s.landing).or_else(|| {
                task.current_run()
                    .and_then(|r| r.autopilot.as_ref())
                    .filter(|a| a.stage == AutoStage::Landing)
                    .map(|a| (a.at, None))
            }),
        };
        let elapsed = super::gate::duration(times.map(|t| t.0), times.and_then(|t| t.1));
        let price = match stage {
            Stage::Agent | Stage::Review => cost(run.and_then(|r| r.usage.as_ref())),
            Stage::Gate | Stage::Land => "$0.00".into(),
        };
        let color = match status {
            "running" => theme::BLUE,
            "failed" | "lost" | "changes" | "escalate" => theme::RED,
            "done" | "passed" | "approve" | "landed" => theme::GREEN,
            "blocked" => theme::YELLOW,
            _ => theme::DIM,
        };
        let title_style = if selected == Some(stage) {
            theme::bold().bg(theme::SELECTED_BG)
        } else {
            Style::new().fg(theme::CYAN)
        };
        let lines = vec![
            Line::from(Span::styled(
                format!(
                    "F{} {}{}",
                    i + 1,
                    stage.label(),
                    if i < 3 { " >" } else { "" }
                ),
                title_style,
            )),
            Line::from(Span::styled(
                format!("{status} {elapsed}"),
                Style::new().fg(color),
            )),
            Line::from(Span::styled(price, theme::dim())),
        ];
        f.render_widget(Paragraph::new(lines), cell);
        hits.push(cell, Hit::Key(hit::press(KeyCode::F(stage.number()))));
    }
}

pub(crate) fn action(key: &KeyEvent, task: &str) -> Option<Action> {
    if key.code == KeyCode::F(5) {
        return Some(Action::OpenPlan(task.into()));
    }
    Stage::from_key(key).map(|stage| Action::OpenStage(task.into(), stage))
}
