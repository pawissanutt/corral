//! Pre-acceptance planning states do not have a tracker or owner decision bar.

use corral_protocol::detail::PlanDetail;
use corral_protocol::rows::{PlanRow, PlanState};

use super::live::{project_of, state_chip, title, tracker_actions};
use super::view::pipeline_marks;
use super::{Header, PlanningView};
use crate::model::Id;
use crate::model::board::Mark;
use crate::model::text::span;

pub(super) fn view(
    id: &Id,
    row: Option<&PlanRow>,
    detail: &PlanDetail,
    stale: Option<String>,
    phone: bool,
) -> PlanningView {
    let (heading, explanation, mark) = match detail.state {
        PlanState::Reviewing => (
            "Plan in review",
            "The submitted document is being reviewed or processed. It is not awaiting your acceptance.",
            Mark::Active,
        ),
        PlanState::PlanFailed => (
            "Planning failed",
            "Planning or its review failed or escalated. This is not a request to accept the plan.",
            Mark::Fail,
        ),
        _ => (
            "Drafting the plan",
            "Planning or requested changes are in progress. No submitted revision is awaiting your acceptance.",
            Mark::Active,
        ),
    };
    let reasons = detail.accept_blockers.join(" · ");
    let body = if reasons.is_empty() {
        explanation.to_owned()
    } else {
        format!("{explanation}\n{reasons}")
    };
    PlanningView {
        header: Header {
            plan: Some(id.clone()),
            chips: vec![state_chip(detail.state)],
            id: match &detail.revision {
                Some(revision) => format!("{id} {revision}"),
                None => id.to_string(),
            },
            project: project_of(row),
            title: title(id, row),
            meta: detail
                .reviews
                .last()
                .map(|review| vec![span(format!("{} · {}", review.revision, review.verdict)).dim()])
                .unwrap_or_default(),
            illustrative: None,
            actions: tracker_actions(id, row, detail, phone),
            stale,
        },
        heading: heading.into(),
        body,
        pipeline: pipeline_marks([
            ("Design".into(), mark),
            ("Members".into(), Mark::Next),
            ("Aggregate verify".into(), Mark::Next),
            ("Final accept".into(), Mark::Next),
        ]),
    }
}

#[cfg(test)]
#[path = "planning_tests.rs"]
mod tests;
