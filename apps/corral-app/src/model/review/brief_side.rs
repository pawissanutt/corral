//! The side column repeats only task, run and session facts the feed carries.

use crate::model::daemon::Feed;

use super::live::{Shown, sha7};
use super::view::{ButtonStyle, button};
use super::{Action, Button};

/// What the task asked for: its body as written, Markdown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AskedFor {
    pub heading: &'static str,
    pub text: String,
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

pub(super) fn side(feed: &Feed, shown: &Shown) -> BriefSide {
    let body = shown.task.task().body.trim();
    let mut facts = Vec::new();
    let mut fact = |label, value: String| facts.push(ReviewFact { label, value });
    if let Some(run) = shown.run() {
        fact("Base", format!("{} @ {}", run.base, sha7(&run.base_sha)));
        fact("Branch", run.branch.clone());
        fact("Run", run.id.clone());
        fact(
            "Worker",
            match &run.model {
                Some(model) => format!("{} · {model}", run.profile),
                None => run.profile.clone(),
            },
        );
    }
    let session = shown
        .session()
        .filter(|session| feed.agents.contains_key(session.as_str()));
    if let Some(session) = &session {
        fact("Session", session.clone());
    }
    BriefSide {
        asked_for: (!body.is_empty()).then(|| AskedFor {
            heading: "Asked for",
            text: body.to_owned(),
        }),
        facts_heading: "Facts",
        facts,
        open_session: session
            .map(|_| button("Open session", Action::OpenSession, ButtonStyle::Quiet)),
    }
}
