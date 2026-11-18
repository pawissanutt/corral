//! The served candidate's Diff tab: each file of the stored diff, and its patch once expanded
//! (fetched as `ReviewFile`, never read from a host path).

use corral_protocol::detail::{DetailBody, ReviewDetail, ReviewFile};

use crate::model::daemon::Feed;
use crate::model::effect::Env;
use crate::model::hue::Hue;
use crate::model::nav::Form;
use crate::model::text::span;

use super::diff::parse_patch;
use super::live::{Shown, file_ref};
use super::view::{ButtonStyle, DiffView, FileBody, FileView, HunkView, button, hunk_content};
use super::{Action, Layout, State};

fn file_view(state: &State, feed: &Feed, shown: &Shown, file: &ReviewFile, layout: Layout) -> FileView {
    let id = shown.id();
    let heading = vec![
        span(file.path.clone()).mono(),
        span(format!(" +{}", file.added)).hue(Hue::Ok),
        span(format!(" −{}", file.deleted)).hue(Hue::Fail),
    ];
    let expanded = state.expanded.contains(&(id.clone(), file.path.clone()));
    let mut view = FileView {
        path: file.path.clone(),
        heading,
        ask: None,
        collapse: None,
        body: FileBody::Collapsed(button(
            format!("{} changed lines · expand", file.added + file.deleted),
            Action::Expand(file.path.clone()),
            ButtonStyle::Ghost,
        )),
    };
    if !expanded {
        return view;
    }
    view.collapse = Some(button(
        "Collapse",
        Action::Collapse(file.path.clone()),
        ButtonStyle::Quiet,
    ));
    let cache = feed.detail(&file_ref(id, &file.path));
    let patch = cache.and_then(|cache| match cache.body() {
        Some(DetailBody::ReviewFile(page)) => Some(page),
        _ => None,
    });
    let Some(patch) = patch else {
        view.body = match cache.and_then(|cache| cache.error.as_ref()) {
            Some(error) => FileBody::Unavailable(format!(
                "Patch unavailable: {}. Collapse and expand to try again.",
                error.message
            )),
            None => FileBody::Loading("Loading this file's patch…".into()),
        };
        return view;
    };
    let parsed = parse_patch(&patch.patch, true);
    view.ask = Some(button(
        "Ask about this file",
        Action::AskAbout {
            path: file.path.clone(),
            hunk: parsed.hunks.first().map(|hunk| hunk.header.clone()),
        },
        ButtonStyle::Quiet,
    ));
    let mut notes = parsed.notes;
    if parsed.hunks.is_empty() && notes.is_empty() {
        notes.push("No textual changes are served for this file.".into());
    }
    view.body = FileBody::Hunks {
        hunks: parsed
            .hunks
            .into_iter()
            .map(|hunk| HunkView {
                header: hunk.header,
                content: hunk_content(hunk.lines, layout),
            })
            .collect(),
        notes,
        tail: patch.truncated.then(|| {
            "corrald stops this patch at its size bound; the rest of the file's changes are not served to the app.".into()
        }),
    };
    view
}

/// The Diff tab of a served candidate.
pub(super) fn diff(
    state: &State,
    feed: &Feed,
    shown: &Shown,
    review: &ReviewDetail,
    layout: Layout,
    env: Env,
) -> DiffView {
    DiffView {
        layout,
        notice: (env.form == Form::Phone)
            .then_some("Unified on phones; side by side needs ≥ 1100 pt of content."),
        files: review
            .files
            .iter()
            .map(|file| file_view(state, feed, shown, file, layout))
            .collect(),
        empty: review.files.is_empty().then(|| {
            if review.run.is_none() {
                "No worker run has finished for this task, so corrald stores no diff yet.".into()
            } else {
                "corrald reports no changed files for this candidate.".into()
            }
        }),
    }
}
