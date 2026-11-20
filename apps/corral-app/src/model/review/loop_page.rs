//! A review candidate's page: the selected candidate's Brief or evidence with the decide bar.

use crate::model::daemon::{Feed, Link};
use crate::model::effect::{Conn, Env};
use crate::model::id::Id;
use crate::model::live::Selection;
use crate::model::projects::ProjectFilter;

use super::brief::{Brief, brief};
use super::decide::decide_bar;
use super::live::{Shown, log, notice, report, selected_filtered};
use super::outline::{candidate_tabs, selected_tab};
use super::view::{effective_layout, layouts};
use super::{Body, DecideBar, LayoutChoice, Notice, State, Tab, TabView};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoopContent {
    Brief(Brief),
    /// Diff, Gate log, Summary and Artifacts keep their own typed content builders.
    Evidence(Body),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopCandidate {
    /// The candidate's task.
    pub task: Id,
    pub notice: Option<Notice>,
    pub tabs: Vec<TabView>,
    pub layouts: Vec<LayoutChoice>,
    pub content: LoopContent,
    pub decide: DecideBar,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopPage {
    pub detail: Option<LoopCandidate>,
    pub empty: Option<String>,
    pub stale: bool,
    pub conn: Conn,
}

/// `focus` is a pushed candidate: its page bypasses the project filter for its detail and
/// actions, never for its outline or page-level counts.
pub fn loop_page_filtered(
    state: &State,
    feed: &Feed,
    selection: &Selection,
    focus: Option<&Id>,
    env: Env,
    width: f32,
    filter: &ProjectFilter,
) -> LoopPage {
    let task = match focus {
        Some(id) => feed.board.task(id),
        None => selected_filtered(selection, feed, filter),
    };
    let shown = task.map(|task| Shown::of(feed, task));
    let stale = env.conn.refuses()
        || feed.link != Link::Up
        || shown
            .as_ref()
            .and_then(|shown| shown.review_cache)
            .is_some_and(|cache| cache.stale);
    let detail = shown.as_ref().map(|shown| {
        let content = match (shown.review, selected_tab(state, shown)) {
            (_, Tab::Brief) => LoopContent::Brief(brief(state, feed, shown, env, width)),
            (Some(review), Tab::Diff) => {
                LoopContent::Evidence(Body::Diff(super::live_files::diff(
                    state,
                    feed,
                    shown,
                    review,
                    effective_layout(state, env, width),
                    env,
                )))
            }
            (_, Tab::Log) => LoopContent::Evidence(log(feed, shown)),
            (_, Tab::Report) => LoopContent::Evidence(report(shown)),
            (_, Tab::Shot) => {
                LoopContent::Evidence(super::evidence::artifacts(state, feed, shown, env))
            }
            (None, Tab::Diff) => LoopContent::Evidence(Body::Notice(notice(shown).map_or_else(
                || "Loading the candidate's review detail…".into(),
                |notice| notice.text,
            ))),
        };
        LoopCandidate {
            task: shown.task.id.clone(),
            notice: notice(shown),
            tabs: candidate_tabs(state, shown),
            layouts: layouts(state, env),
            content,
            decide: decide_bar(state, feed, shown, env),
        }
    });
    LoopPage {
        empty: if detail.is_some() {
            None
        } else if let Some(id) = focus {
            Some(format!("{id} is no longer on the board."))
        } else if !feed.board.complete {
            Some("Waiting for the board…".into())
        } else {
            Some("No task is in Review in this scope.".into())
        },
        detail,
        stale,
        conn: env.conn,
    }
}
