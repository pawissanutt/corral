//! Builds: the records interactive build sessions publish once they implemented, verified,
//! reviewed and landed (or kept, or abandoned) work themselves. A build is not a task: it has
//! no board row, rail or task commands, and nothing here offers any. The list comes from
//! `Topic::Builds`, the detail from `DetailRef::Build`; both need `builds.v1`, so a daemon
//! without it shows Builds as unavailable rather than empty.
//!
//! The selected build belongs to the root selection, so Builds keeps no state of its own: its
//! presses and keys are pure functions of the feed and the selection.

mod detail;
mod list;
mod wording;

#[cfg(test)]
mod tests;

pub use detail::{Block, BuildView, Detail, Line, Section, detail_of};
pub use list::{BuildList, ListNotice, ListRow, ListSection, current_build, live_list};
/// A child agent's state, verdict, message delivery and figures read the same in a live
/// session's fleet as in a published build.
pub(crate) use wording::{cost, count, delivery, note, slice_state, tokens, verdict};

use corral_protocol::detail::{BuildDetail, DetailBody, DetailGet, DetailRef};

use crate::model::daemon::Feed;
use crate::model::effect::Effect;
use crate::model::{Id, Live, Selection};

/// Captions shared by every form's loading notices.
pub const LIST_LOADING: &str = "Loading builds…";
pub const DETAIL_LOADING: &str = "Loading the build…";
/// What the detail says while there is no build to show.
pub const NONE_SELECTED: &str = "Choose a build to see how it was built.";

/// Presses on corrald's builds, by owned id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Select a build in place, or push its page on the phone.
    Open(Id),
    /// Request the build's detail again after an error.
    Retry(Id),
}

/// The effects of one press.
pub fn reduce(action: Action) -> Vec<Effect> {
    match action {
        Action::Open(id) => vec![Effect::OpenLive(Live::Build(id))],
        Action::Retry(id) => vec![Effect::Detail(DetailGet {
            target: build_ref(&id),
            etag: None,
            page: None,
        })],
    }
}

/// j/k (and the arrows) move the selection through the list; other keys are not Builds'.
pub fn key_live(key: &str, selection: &Selection, feed: &Feed) -> Option<Vec<Effect>> {
    if !matches!(key, "j" | "down" | "k" | "up") {
        return None;
    }
    let order = list::order(feed);
    if order.is_empty() {
        return Some(vec![]);
    }
    let index =
        current_build(selection, feed).and_then(|id| order.iter().position(|row| *row == id));
    let next = if matches!(key, "j" | "down") {
        index.map_or(0, |at| (at + 1).min(order.len() - 1))
    } else {
        index.unwrap_or(0).saturating_sub(1)
    };
    Some(vec![Effect::Select(Live::Build(order[next].clone()))])
}

/// The detail reference of a build.
pub fn build_ref(id: &str) -> DetailRef {
    DetailRef::Build { id: id.to_owned() }
}

/// The build's served detail body, if one has arrived.
pub fn build_detail<'a>(feed: &'a Feed, id: &str) -> Option<&'a BuildDetail> {
    match feed.detail(&build_ref(id))?.body()? {
        DetailBody::Build(detail) => Some(detail),
        _ => None,
    }
}

/// The pushed phone page's title: the build's own, else its id.
pub fn page_title(feed: &Feed, id: &Id) -> String {
    feed.builds
        .get(id.as_str())
        .map(|row| row.title.clone())
        .or_else(|| build_detail(feed, id).map(|detail| detail.title.clone()))
        .unwrap_or_else(|| id.to_string())
}
