//! Question placement from the canonical task and lazily loaded plan graph.
use super::Background;
use crate::model::daemon::Feed;
use crate::model::text::{Rich, span};
use crate::model::{Id, Live};
use corral_protocol::detail::QuestionDetail;
pub(super) fn background(detail: &QuestionDetail, feed: &Feed) -> Background {
    let task = detail.task.as_deref();
    let board = task.and_then(|task| feed.board.task(task));
    let mut goal: Rich = Vec::new();
    if let Some(task) = task {
        let name = match board {
            Some(row) => format!("{task} {}", row.title),
            None => task.to_owned(),
        };
        goal.push(span(name).bold().live(Live::Task(Id::new(task))));
    }
    let text = detail
        .place
        .goal
        .clone()
        .or_else(|| board.and_then(|row| row.goal.clone()));
    if let Some(text) = text {
        if !goal.is_empty() {
            goal.push(span(" · "));
        }
        goal.push(span(text));
    }
    let mut plan_line: Rich = Vec::new();
    if let Some(position) = &detail.place.position {
        plan_line.extend([
            span("Plan "),
            span(position.plan.clone())
                .mono()
                .live(Live::Plan(Id::new(&position.plan))),
            span(format!(
                " · layer {} of {} · {} of {} landed",
                position.layer, position.layers, position.landed, position.total
            )),
        ]);
    } else if let Some(plan) = &detail.plan {
        plan_line.extend([
            span("Plan "),
            span(plan.clone()).mono().live(Live::Plan(Id::new(plan))),
        ]);
    }
    if let Some(plan) = super::context::plan_id(detail) {
        if let Some(plan_detail) = crate::model::plans::plan_detail(feed, plan) {
            return Background::Plan {
                goal,
                plan_line,
                graph: crate::model::plans::graph_input(feed, &Id::new(plan), plan_detail),
                selected: detail.task.as_deref().map(Id::new),
            };
        }
        let target = corral_protocol::detail::DetailRef::Plan { id: plan.into() };
        let note = match feed.detail(&target) {
            Some(cache) if cache.error.is_some() => {
                let message = cache
                    .error
                    .as_ref()
                    .map(|error| error.message.as_str())
                    .unwrap_or_default();
                format!(" · Graph unavailable: {message}")
            }
            Some(cache) if cache.body().is_some() => {
                " · Plan graph is unavailable in this daemon's response.".into()
            }
            _ => " · Loading plan graph…".into(),
        };
        plan_line.push(span(note).dim());
    }
    if goal.is_empty() && plan_line.is_empty() {
        return Background::Absent {
            note: "No task or plan is recorded for this question.",
        };
    }
    Background::Ready { goal, plan_line }
}
