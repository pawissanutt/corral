//! Feed-backed Inbox view builders shared by every data source.

use super::live::{is_question, ordered_filtered, selected};
use super::{
    DecideBar, EmptyState, Inbox, InboxList, InboxPreview, Notice, Sheet, State, live_list,
    live_sheet, phone,
};
use crate::model::Id;
use crate::model::daemon::Feed;
use crate::model::effect::Env;
use crate::model::projects::ProjectFilter;

/// Where Inbox content comes from.
#[derive(Debug, Clone, Copy)]
pub enum Src<'a> {
    /// corrald's served rows and details.
    Live(&'a Feed),
}

/// Whether a handover can be drawn yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Handover {
    /// The question's detail is here.
    Ready,
    /// The question's detail is on its way.
    Loading {
        /// The status the placeholder announces.
        caption: &'static str,
    },
    /// The detail did not load and nothing earlier is cached.
    Failed(Notice),
    /// corrald no longer serves this question, or nothing is selected.
    Gone(EmptyState),
}

/// The question a handover shows: the focus page's key, else the selected row's.
pub fn shown_key_filtered(
    state: &State,
    feed: &Feed,
    key: Option<&Id>,
    filter: &ProjectFilter,
) -> Option<Id> {
    if let Some(key) = key {
        return Some(key.clone());
    }
    let rows = ordered_filtered(feed, filter);
    selected(&state.desk, &rows)
        .filter(|row| is_question(row))
        .map(|row| Id::new(&row.id))
}

/// List rows or cards, the empty state and the changes since the last visit.
pub fn inbox_list_filtered(state: &State, src: Src, env: Env, filter: &ProjectFilter) -> InboxList {
    match src {
        Src::Live(feed) => live_list::list_filtered(state, feed, env, filter),
    }
}

/// The desktop and terminal pane of the selected row.
pub fn inbox_preview_filtered(
    state: &State,
    src: Src,
    env: Env,
    filter: &ProjectFilter,
) -> InboxPreview {
    match src {
        Src::Live(feed) => live_list::preview_filtered(state, feed, env, filter),
    }
}

/// The whole destination.
pub fn inbox_filtered(state: &State, src: Src, env: Env, filter: &ProjectFilter) -> Inbox {
    Inbox {
        title: "Inbox",
        large_title: phone(env),
        list: inbox_list_filtered(state, src, env, filter),
        preview: (!phone(env)).then(|| inbox_preview_filtered(state, src, env, filter)),
    }
}

/// Whether the handover of `key` (or the selected row) can be drawn.
pub fn handover_filtered(
    state: &State,
    src: Src,
    key: Option<&Id>,
    filter: &ProjectFilter,
) -> Handover {
    match src {
        Src::Live(feed) => match shown_key_filtered(state, feed, key, filter) {
            Some(key) => live_sheet::handover(feed, &key),
            None => Handover::Gone(live_sheet::gone()),
        },
    }
}

/// The desktop handover: inline (`with_side` false) or the focus page's centre and context.
pub fn sheet_desk_filtered(
    state: &State,
    src: Src,
    key: Option<&Id>,
    env: Env,
    with_side: bool,
    filter: &ProjectFilter,
) -> Sheet {
    match src {
        Src::Live(feed) => match shown_key_filtered(state, feed, key, filter) {
            Some(shown) => {
                live_sheet::sheet(state, feed, &shown, env, key.is_some(), with_side, filter)
            }
            None => live_sheet::placeholder(),
        },
    }
}

/// The focus page in every form.
pub fn question_filtered(
    state: &State,
    src: Src,
    key: Option<&Id>,
    env: Env,
    filter: &ProjectFilter,
) -> Sheet {
    match src {
        Src::Live(feed) => match shown_key_filtered(state, feed, key, filter) {
            Some(shown) => live_sheet::sheet(state, feed, &shown, env, true, !phone(env), filter),
            None => live_sheet::placeholder(),
        },
    }
}

/// The pinned decide bar.
pub fn decide_filtered(
    state: &State,
    src: Src,
    key: Option<&Id>,
    env: Env,
    filter: &ProjectFilter,
) -> DecideBar {
    match src {
        Src::Live(feed) => match shown_key_filtered(state, feed, key, filter) {
            Some(shown) => live_sheet::decide(state, feed, &shown, env, filter),
            None => live_sheet::placeholder().decide,
        },
    }
}
