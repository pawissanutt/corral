//! Now's Closed section: done and dropped tasks, read from corrald one served page at a time
//! once the owner opens it. The board never carries closed tasks (their number only grows), so
//! nothing here is projected from it; the count is the daemon's total, never the rows on screen.

use std::borrow::Cow;

use corral_protocol::app::CLOSED_V1;
use corral_protocol::detail::{ClosedPage, ClosedTask, DetailBody, DetailGet, DetailRef};

use super::{Button, ButtonKind, Press, State, stamp};
use crate::model::board::Mark;
use crate::model::daemon::{DetailCache, Feed, Link};
use crate::model::effect::Effect;
use crate::model::health::capability_availability;
use crate::model::{Availability, Form, Hue, Id, Live, Selection};

/// The section's heading.
pub const CLOSED_TITLE: &str = "Closed";

/// Whether the Closed section is open, and which served page it shows.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ClosedState {
    /// Collapsed by default: closed history is read on request, never with the board.
    pub expanded: bool,
    /// The served `older` anchor of the page shown; none for the newest page.
    pub before: Option<String>,
}

/// The Closed section's own controls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClosedPress {
    /// Open or fold the section.
    Toggle,
    /// Back to the most recently closed tasks.
    Newest,
    /// The page before this served anchor.
    Older(String),
    /// Ask again for the page whose fetch failed.
    Retry,
}

impl From<ClosedPress> for Press {
    fn from(press: ClosedPress) -> Press {
        Press::Closed(press)
    }
}

impl ClosedState {
    /// The page the section shows: the newest, or the one before the served anchor.
    pub fn target(&self) -> DetailRef {
        DetailRef::Closed {
            before: self.before.clone(),
        }
    }

    /// Moves the section; the shown page itself is fetched by whoever renders it, which also
    /// sees a capability arrive late or a page dropped from the cache, and pins it in the
    /// client's cache only while shown, so folding lets the client drop it.
    pub(super) fn press(&mut self, press: ClosedPress) -> Vec<Effect> {
        match press {
            ClosedPress::Toggle => {
                self.expanded = !self.expanded;
                // Reopening starts from the newest page rather than somewhere in history.
                if !self.expanded {
                    self.before = None;
                }
            }
            ClosedPress::Newest => self.before = None,
            ClosedPress::Older(anchor) => self.before = Some(anchor),
            // A failed page is not refetched on its own, so only the owner's Retry asks again.
            ClosedPress::Retry => {
                return vec![Effect::Detail(DetailGet {
                    target: self.target(),
                    etag: None,
                    page: None,
                })];
            }
        }
        Vec::new()
    }
}

/// One closed task, newest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClosedRow {
    pub id: Id,
    pub title: String,
    /// Done is a pass, dropped not applicable: the two differ by shape as well as colour.
    pub mark: Mark,
    pub hue: Hue,
    /// The canonical status word, then the plan and goal the task served when known.
    pub status: String,
    /// When it last changed (normally when it closed), as a real feed date.
    pub at: String,
    /// The whole row for screen readers.
    pub label: String,
    /// Whether the row is the task shown beside the list.
    pub selected: bool,
    /// Opens the task, read-only, from its own detail.
    pub action: Press,
}

/// The Closed section: its heading toggle and, while open, a page of rows and its pager.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClosedView {
    pub title: &'static str,
    /// The daemon's count of closed tasks, while the section is open and its page said it.
    pub total: Option<u64>,
    pub expanded: bool,
    /// Opens or folds the section; its label names the section with its count.
    pub toggle: Button,
    pub rows: Vec<ClosedRow>,
    /// Capability, connection, loading, error, freshness or empty.
    pub note: Option<String>,
    /// Newest, Older, or Retry after an error.
    pub pager: Vec<Button>,
}

/// Whether Now offers the Closed section under its list: once a complete board arrived for an
/// enrolled device, even an empty one, since closed history is what an empty board has left.
pub fn closed_offered(feed: &Feed) -> bool {
    feed.board.complete && feed.device.board_notice().is_none()
}

/// The tasks the open section lists, newest first: the Now list's keys walk them after the
/// board's rows, and never while the section is folded or shows no rows.
pub(super) fn closed_ids(closed: &ClosedState, feed: &Feed) -> Vec<Id> {
    if !closed.expanded || !closed_offered(feed) || !feed.capable(CLOSED_V1) {
        return Vec::new();
    }
    served(feed.detail(&closed.target())).map_or_else(Vec::new, |page| {
        page.rows
            .iter()
            .map(|task| Id::from(task.id.as_str()))
            .collect()
    })
}

fn served(cache: Option<&DetailCache>) -> Option<&ClosedPage> {
    match cache?.body() {
        Some(DetailBody::Closed(page)) => Some(page),
        _ => None,
    }
}

/// Builds the Closed section from the served page of `ClosedState::target`. As with the
/// board's rows, a phone marks no row selected: a press there pushes the task's own page.
pub fn closed_view(state: &State, feed: &Feed, selection: &Selection, form: Form) -> ClosedView {
    let closed = &state.closed;
    let cache = feed.detail(&closed.target());
    let page = served(cache);
    // Folding lets the client drop the page, so a folded heading never carries a count: it reads
    // the same whether or not a page happens to still be cached.
    let total = page.and_then(|page| page.total).filter(|_| closed.expanded);
    let label = match total {
        Some(total) => format!("{CLOSED_TITLE} tasks, {total}"),
        None => format!("{CLOSED_TITLE} tasks"),
    };
    let mut view = ClosedView {
        title: CLOSED_TITLE,
        total,
        expanded: closed.expanded,
        toggle: Button::new(label, ClosedPress::Toggle, ButtonKind::Quiet),
        rows: Vec::new(),
        note: None,
        pager: Vec::new(),
    };
    if !closed.expanded {
        return view;
    }
    match capability_availability(feed, CLOSED_V1) {
        Availability::Ready => {}
        Availability::Connecting if feed.link == Link::Down => {
            view.note = Some(
                "corrald is not reachable; closed tasks load once a session with it is open."
                    .into(),
            );
            return view;
        }
        Availability::Connecting => {
            view.note = Some("Loading closed tasks…".into());
            return view;
        }
        Availability::Missing(_) => {
            view.note = Some(
                "This corrald does not list closed tasks to the app (closed.v1 is not advertised)."
                    .into(),
            );
            return view;
        }
    }
    let error = cache.and_then(|cache| cache.error.as_ref());
    let retry = || Button::new("Retry", ClosedPress::Retry, ButtonKind::Ghost);
    let Some(page) = page else {
        match error {
            Some(error) => {
                view.note = Some(format!("Closed tasks unavailable: {}", error.message));
                view.pager.push(retry());
            }
            None => view.note = Some("Loading closed tasks…".into()),
        }
        return view;
    };
    let shown = selection.task.as_ref().filter(|_| form != Form::Phone);
    view.rows = page
        .rows
        .iter()
        .map(|task| row(feed, task, shown))
        .collect();
    let stale = cache.is_some_and(|cache| cache.stale);
    // A live link refetches the page on every board change; saying so each time would flicker.
    view.note = if let Some(error) = error {
        Some(format!("Updates stopped: {}", error.message))
    } else if stale && feed.link != Link::Up {
        Some("Shown as last received; reconnecting.".into())
    } else if page.rows.is_empty() && closed.before.is_none() {
        Some("No closed tasks.".into())
    } else if page.rows.is_empty() {
        Some("No older closed tasks.".into())
    } else {
        None
    };
    if closed.before.is_some() {
        view.pager.push(Button::new(
            "Newest",
            ClosedPress::Newest,
            ButtonKind::Quiet,
        ));
    }
    if let Some(older) = &page.older {
        view.pager.push(Button::new(
            "Older",
            ClosedPress::Older(older.clone()),
            ButtonKind::Quiet,
        ));
    }
    if error.is_some() {
        view.pager.push(retry());
    }
    view
}

fn row(feed: &Feed, task: &ClosedTask, shown: Option<&Id>) -> ClosedRow {
    let id = Id::from(task.id.as_str());
    let (mark, hue) = if task.status == "done" {
        (Mark::Done, Hue::Ok)
    } else {
        (Mark::Na, Hue::Neutral)
    };
    let mut status: Cow<'_, str> = Cow::Borrowed(task.status.as_str());
    if let Some(plan) = &task.plan {
        status = Cow::Owned(format!("{status} · plan {plan}"));
    }
    if let Some(goal) = &task.goal {
        status = Cow::Owned(format!("{status} · {goal}"));
    }
    let at = stamp(&task.updated, feed);
    ClosedRow {
        label: format!("{} {}, {status}, {at}", task.id, task.title),
        selected: shown == Some(&id),
        action: Press::Open(Live::Task(id.clone())),
        id,
        title: task.title.clone(),
        mark,
        hue,
        status: status.into_owned(),
        at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::daemon::DetailCache;
    use corral_protocol::detail::ClosedPage;

    fn serve(feed: &mut Feed, before: Option<&str>, older: Option<&str>) {
        let target = DetailRef::Closed {
            before: before.map(str::to_owned),
        };
        feed.details.insert(
            crate::model::daemon::detail_key(&target),
            DetailCache {
                target,
                pages: Vec::new(),
                loading: false,
                stale: false,
                error: None,
                merged: Some(DetailBody::Closed(ClosedPage {
                    total: Some(2),
                    rows: Vec::new(),
                    older: older.map(str::to_owned),
                })),
            },
        );
    }

    fn pages(state: &State, feed: &Feed) -> Vec<Option<Press>> {
        closed_view(state, feed, &Selection::default(), Form::Desktop)
            .pager
            .into_iter()
            .map(|button| button.action)
            .collect()
    }

    #[test]
    fn closed_heading_counts_only_while_open() {
        let mut feed = Feed::default();
        feed.capabilities.insert(CLOSED_V1.into());
        serve(&mut feed, None, None);
        let mut state = State::default();
        let heading = |state: &State| {
            let view = closed_view(state, &feed, &Selection::default(), Form::Desktop);
            (view.total, view.toggle.label.to_string())
        };
        // A page still cached under a folded section does not leak its count.
        assert_eq!(heading(&state), (None, "Closed tasks".into()));
        state.closed.press(ClosedPress::Toggle);
        assert_eq!(heading(&state), (Some(2), "Closed tasks, 2".into()));
        state.closed.press(ClosedPress::Toggle);
        assert_eq!(heading(&state), (None, "Closed tasks".into()));
    }

    #[test]
    fn closed_pager_follows_the_served_anchor_and_newest_forgets_it() {
        let mut feed = Feed::default();
        feed.capabilities.insert(CLOSED_V1.into());
        serve(&mut feed, None, Some("anchor-1"));
        serve(&mut feed, Some("anchor-1"), None);
        let mut state = State::default();
        assert!(state.closed.press(ClosedPress::Toggle).is_empty());
        let older = ClosedPress::Older("anchor-1".into());
        assert_eq!(pages(&state, &feed), [Some(Press::from(older.clone()))]);

        state.closed.press(older);
        assert_eq!(
            state.closed.target(),
            DetailRef::Closed {
                before: Some("anchor-1".into())
            }
        );
        assert_eq!(
            pages(&state, &feed),
            [Some(Press::from(ClosedPress::Newest))]
        );
        assert!(matches!(
            state.closed.press(ClosedPress::Retry).as_slice(),
            [Effect::Detail(get)] if get.target == state.closed.target()
        ));

        state.closed.press(ClosedPress::Newest);
        assert_eq!(state.closed.target(), DetailRef::Closed { before: None });
    }
}
