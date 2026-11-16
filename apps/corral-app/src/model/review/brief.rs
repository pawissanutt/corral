//! Brief-first Review: readiness, expandable changed files and the task's recorded request.

use corral_protocol::detail::ReviewDetail;

use crate::model::daemon::{Feed, Task};
use crate::model::effect::Env;
use crate::model::glance::{Stat, StatIcon};
use crate::model::hue::Hue;

use super::brief_side::{BriefSide, side};
use super::live::Shown;
use super::readiness::{ReadyLink, readiness_chain};
use super::view::effective_layout;
use super::{Action, FileView, State};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedFile {
    pub basename: String,
    /// The literal path before the last slash, not an invented report description.
    pub directory: String,
    pub added: String,
    pub deleted: String,
    pub hint: String,
    pub expanded: bool,
    pub toggle: Action,
    pub stats: Vec<Stat>,
    /// Exactly the existing patch/discussion projection, present only when expanded.
    pub body: Option<FileView>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Brief {
    pub readiness_heading: &'static str,
    pub readiness: Vec<ReadyLink>,
    pub changes_heading: &'static str,
    pub files: Vec<ChangedFile>,
    pub files_empty: Option<String>,
    pub side: BriefSide,
}

/// None means the review detail has not arrived; it never means a fabricated empty review.
/// Expanded patches use the existing parser and annotation-placement path unchanged.
pub fn candidate_brief(
    state: &State,
    feed: &Feed,
    task: &Task,
    env: Env,
    width: f32,
) -> Option<Brief> {
    let shown = Shown::of(feed, task);
    let review = shown.review?;
    Some(brief(state, feed, &shown, review, env, width))
}

pub(super) fn brief(
    state: &State,
    feed: &Feed,
    shown: &Shown,
    review: &ReviewDetail,
    env: Env,
    width: f32,
) -> Brief {
    let diff = super::live_files::diff(
        state,
        feed,
        shown,
        review,
        effective_layout(state, env, width),
        env,
    );
    let files = review
        .files
        .iter()
        .zip(diff.files)
        .enumerate()
        .map(|(at, (file, body))| {
            let (directory, basename) = file
                .path
                .rsplit_once('/')
                .unwrap_or(("", file.path.as_str()));
            let expanded = body.collapse.is_some();
            let threads = review
                .annotations
                .iter()
                .filter(|thread| thread.path == file.path)
                .count();
            ChangedFile {
                basename: basename.into(),
                directory: directory.into(),
                added: format!("+{}", file.added),
                deleted: format!("−{}", file.deleted),
                hint: file.path.clone(),
                expanded,
                toggle: if expanded {
                    Action::Collapse(file.path.clone())
                } else {
                    Action::Expand(file.path.clone())
                },
                stats: if threads == 0 {
                    Vec::new()
                } else {
                    vec![Stat::new(
                        format!("threads-{at}"),
                        StatIcon::Events,
                        threads.to_string(),
                        Hue::Neutral,
                        format!(
                            "{threads} annotation thread{} on {}",
                            if threads == 1 { "" } else { "s" },
                            file.path
                        ),
                    )]
                },
                body: expanded.then_some(body),
            }
        })
        .collect();
    Brief {
        readiness_heading: "Ready to land?",
        readiness: readiness_chain(feed, shown.task, review),
        changes_heading: "What changed",
        files,
        files_empty: diff.empty,
        side: side(feed, review, shown.detail),
    }
}
