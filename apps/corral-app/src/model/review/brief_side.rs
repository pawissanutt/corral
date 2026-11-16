//! The side column only repeats task/run/session facts actually carried by the feed.

use corral_protocol::detail::{ReviewDetail, TaskDetail};
use corral_protocol::rows::AgentStepState;

use crate::model::board::Mark;
use crate::model::daemon::Feed;

use super::view::{ButtonStyle, button};
use super::{Action, Button};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AskedFor {
    pub heading: &'static str,
    pub count: String,
    /// Empty for aggregate-only progress or an explicitly empty checklist.
    pub items: Vec<(Mark, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewFact {
    pub label: &'static str,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BriefSide {
    pub asked_for: Option<AskedFor>,
    pub facts_heading: &'static str,
    pub facts: Vec<ReviewFact>,
    pub open_session: Option<Button>,
}

pub(super) fn side(feed: &Feed, review: &ReviewDetail, task: Option<&TaskDetail>) -> BriefSide {
    let run = task.and_then(|task| task.run.as_ref());
    let matches = |row: &&corral_protocol::rows::AgentRow| {
        row.task.as_deref() == Some(review.task.as_str())
            && row.run.as_deref() == run.map(|run| run.id.as_str())
            && row.steps.is_some()
    };
    let preferred = run
        .and_then(|run| run.session.as_deref())
        .and_then(|session| feed.agents.get(session))
        .filter(matches);
    let session = preferred.or_else(|| run.and_then(|_| feed.agents.values().find(matches)));
    let asked_for = session
        .and_then(|row| row.steps.as_ref())
        .map(|steps| {
            let completed = steps
                .iter()
                .filter(|step| step.state == AgentStepState::Done)
                .count();
            AskedFor {
                heading: "Asked for",
                count: format!("{completed}/{}", steps.len()),
                items: steps
                    .iter()
                    .map(|step| {
                        (
                            match step.state {
                                AgentStepState::Done => Mark::Done,
                                AgentStepState::InProgress => Mark::Active,
                                AgentStepState::Dropped => Mark::Na,
                                AgentStepState::Pending | AgentStepState::Other => Mark::Next,
                            },
                            step.title.clone(),
                        )
                    })
                    .collect(),
            }
        })
        .or_else(|| {
            task.and_then(|task| task.progress.as_ref())
                .map(|progress| AskedFor {
                    heading: "Asked for",
                    count: format!("{}/{}", progress.completed, progress.total),
                    items: Vec::new(),
                })
        });
    let mut facts = Vec::new();
    if let Some(base) = review
        .base
        .as_deref()
        .or_else(|| run.and_then(|run| run.base.as_deref()))
    {
        facts.push(ReviewFact {
            label: "Base",
            value: base.into(),
        });
    }
    if let Some(run) = run {
        facts.push(ReviewFact {
            label: "Run",
            value: run.id.clone(),
        });
        if let Some(session) = &run.session {
            facts.push(ReviewFact {
                label: "Session",
                value: session.clone(),
            });
        }
    }
    BriefSide {
        asked_for,
        facts_heading: "Facts",
        facts,
        open_session: run
            .and_then(|run| run.session.as_ref())
            .map(|_| button("Open session", Action::OpenSession, ButtonStyle::Quiet)),
    }
}
