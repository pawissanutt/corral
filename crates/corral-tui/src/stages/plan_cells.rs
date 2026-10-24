//! The plan-first prefix of the stage strip: Plan > Plan review > Your approval > Tasks. Every cell presses F5,
//! which opens the task's Plan tab, except a "Plan review" cell with a review to start, which presses `R`.

use corral_core::Task;
use corral_ops::PlanReviewOffer;
use ratatui::Frame;
use ratatui::crossterm::event::KeyCode;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::artifacts::ui;
use crate::plan::PlanStage;
use crate::theme::{self, BLUE, DIM, GREEN, RED, YELLOW};
use crate::{Hit, HitMap, hit};

const TITLES: [&str; 4] = ["F5 Plan >", "Plan review >", "Your approval >", "Tasks >"];

fn cells(
    task: &Task,
    stage: PlanStage,
    offer: Option<PlanReviewOffer>,
) -> [(String, Color, String); 4] {
    use PlanStage as S;
    let run = task.plan_runs.last();
    let plan = task.plans.last();
    let review = plan.and_then(|p| p.reviews.iter().rev().find(|r| r.reviewer != "owner"));
    let elapsed = run.map_or(String::new(), |r| {
        format!(" {}", super::gate::duration(Some(r.started), r.ended))
    });
    let planned = match stage {
        S::Needed => ("pending".to_string(), DIM),
        S::Planning { round: 0 } => (format!("running{elapsed}"), BLUE),
        S::Planning { .. } => (format!("revising{elapsed}"), BLUE),
        S::Failed if plan.is_none() => ("failed".into(), RED),
        _ => (format!("submitted{elapsed}"), GREEN),
    };
    let reviewed = match stage {
        S::Needed | S::Planning { round: 0 } => ("pending", DIM),
        S::Planning { .. } => ("changes", YELLOW),
        S::Review { .. } => ("running", BLUE),
        S::AwaitingOwner | S::Approved | S::Decomposed(_) => ("approve", GREEN),
        S::Escalated => ("escalate", RED),
        S::Failed => ("failed", RED),
    };
    let reviewed = match offer {
        Some(PlanReviewOffer::Run) => ("R run review", YELLOW),
        Some(PlanReviewOffer::Retry) => ("R retry review", YELLOW),
        None => reviewed,
    };
    let approval = match stage {
        S::AwaitingOwner => ("waiting on you", YELLOW),
        S::Escalated => ("your decision", YELLOW),
        S::Approved | S::Decomposed(_) => ("approved", GREEN),
        _ => ("pending", DIM),
    };
    let tasks = match stage {
        S::Decomposed(1) => ("1 task".to_string(), GREEN),
        S::Decomposed(n) => (format!("{n} tasks"), GREEN),
        S::Approved => ("single task".into(), GREEN),
        _ => ("pending".into(), DIM),
    };
    let round = run.map_or(1, |r| r.rounds + 1);
    [
        (
            planned.0,
            planned.1,
            run.map_or("—".into(), |r| r.profile.clone()),
        ),
        (
            reviewed.0.into(),
            reviewed.1,
            review.map_or("—".into(), |r| format!("round {round} · {}", r.reviewer)),
        ),
        (
            approval.0.into(),
            approval.1,
            plan.and_then(|p| p.approval.as_ref())
                .map_or("owner".into(), |a| {
                    format!("{} {}", a.by, ui::local_date(a.at, "%b %d"))
                }),
        ),
        (tasks.0, tasks.1, task.children.join(" ")),
    ]
}

pub(super) fn draw(
    f: &mut Frame,
    areas: &[Rect],
    task: &Task,
    offer: Option<PlanReviewOffer>,
    hits: &mut HitMap,
) {
    let stage = crate::plan::stage(task, None).unwrap_or(PlanStage::Needed);
    for (i, ((area, title), (status, color, detail))) in areas
        .iter()
        .zip(TITLES)
        .zip(cells(task, stage, offer))
        .enumerate()
    {
        let width = usize::from(area.width);
        let lines = vec![
            Line::from(Span::styled(
                crate::artifacts::list::truncate(title, width),
                Style::new().fg(theme::CYAN),
            )),
            Line::from(Span::styled(
                crate::artifacts::list::truncate(&status, width),
                Style::new().fg(color),
            )),
            Line::from(Span::styled(
                crate::artifacts::list::truncate(&detail, width),
                theme::dim(),
            )),
        ];
        f.render_widget(Paragraph::new(lines), *area);
        let key = match (i, offer) {
            (1, Some(_)) => crate::plan::review::KEY,
            _ => KeyCode::F(5),
        };
        hits.push(*area, Hit::Key(hit::press(key)));
    }
}
