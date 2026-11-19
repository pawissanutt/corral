//! Brief-first Review: readiness, expandable changed files and the task's request.

use crate::model::daemon::Feed;
use crate::model::effect::Env;

use super::brief_side::{BriefSide, side};
use super::live::Shown;
use super::readiness::{ReadyLink, readiness_chain};
use super::rounds::{Rounds, rounds};
use super::view::effective_layout;
use super::{Action, FileView, State};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedFile {
    pub basename: String,
    /// The literal path before the last slash, not an invented description.
    pub directory: String,
    pub added: String,
    pub deleted: String,
    pub hint: String,
    pub expanded: bool,
    pub toggle: Action,
    /// The file's patch, present only when expanded.
    pub body: Option<FileView>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Brief {
    pub readiness_heading: &'static str,
    pub readiness: Vec<ReadyLink>,
    /// Earlier rounds and the reviewer that stays across them; absent in a first round without
    /// a live reviewer.
    pub rounds: Option<Rounds>,
    pub changes_heading: &'static str,
    pub files: Vec<ChangedFile>,
    pub files_empty: Option<String>,
    pub side: BriefSide,
}

pub(super) fn brief(state: &State, feed: &Feed, shown: &Shown, env: Env, width: f32) -> Brief {
    let (files, files_empty) = match shown.review {
        Some(review) => {
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
                .map(|(file, body)| {
                    let (directory, basename) = file
                        .path
                        .rsplit_once('/')
                        .unwrap_or(("", file.path.as_str()));
                    let expanded = body.collapse.is_some();
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
                        body: expanded.then_some(body),
                    }
                })
                .collect();
            (files, diff.empty)
        }
        None => (
            Vec::new(),
            Some("The changed files arrive with the review detail.".into()),
        ),
    };
    Brief {
        readiness_heading: "Ready to land?",
        readiness: readiness_chain(feed, shown),
        rounds: rounds(feed, shown),
        changes_heading: "What changed",
        files,
        files_empty,
        side: side(feed, shown),
    }
}
