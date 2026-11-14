//! Agents' Filed from chat, read from corrald's global filings index one served page at a time.
//! The count is the index's total, never the rows on screen; paging moves between the anchors
//! corrald serves. It needs no assistant runtime and no thread history.

use corral_protocol::app::FILINGS_V1;
use corral_protocol::detail::{DetailBody, DetailRef, FiledIdea};

use super::views::button;
use super::{Action, Button, FiledLine, State, clock};
use crate::model::Availability;
use crate::model::daemon::{Feed, Link};
use crate::model::health::capability_availability;
use crate::model::id::Id;
use crate::model::live::Live;

/// The global filings page the Agents wall shows: the newest, or the one before an anchor.
pub fn filings_target(state: &State) -> DetailRef {
    DetailRef::Filings {
        before: state.live.filings_before,
    }
}

/// The section's rows with what they cover, how fresh they are, and how to page.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GlobalFiled {
    pub rows: Vec<FiledLine>,
    /// The source's total of filings; none while unknown.
    pub total: Option<u64>,
    /// Loading, freshness, error, capability or empty.
    pub note: Option<String>,
    /// Newest, Newer, Older, or Retry after an error.
    pub pager: Vec<Button>,
}

impl GlobalFiled {
    fn note(note: impl Into<String>) -> GlobalFiled {
        GlobalFiled {
            note: Some(note.into()),
            ..GlobalFiled::default()
        }
    }
}

/// Builds Filed from chat from the served page of `filings_target`.
pub fn global_filed(state: &State, feed: &Feed) -> GlobalFiled {
    match capability_availability(feed, FILINGS_V1) {
        Availability::Ready => {}
        Availability::Connecting if feed.link == Link::Down => {
            return GlobalFiled::note(
                "corrald is not reachable; filed ideas load once a session with it is open.",
            );
        }
        Availability::Connecting => return GlobalFiled::note("Loading filed ideas…"),
        Availability::Missing(_) => {
            return GlobalFiled::note(
                "This corrald does not list filed ideas to the app (filings.v1 is not advertised).",
            );
        }
    }
    let before = state.live.filings_before;
    let retry = || button("Retry", Action::FilingsPage(before));
    let cache = feed.detail(&filings_target(state));
    let error = cache.and_then(|cache| cache.error.as_ref());
    let page = cache.and_then(|cache| match cache.body() {
        Some(DetailBody::Filings(page)) => Some(page),
        _ => None,
    });
    let Some(page) = page else {
        return match error {
            Some(error) => GlobalFiled {
                note: Some(format!("Filed ideas unavailable: {}", error.message)),
                pager: vec![retry()],
                ..GlobalFiled::default()
            },
            None => GlobalFiled::note("Loading filed ideas…"),
        };
    };
    let rows: Vec<FiledLine> = page.rows.iter().map(|idea| line(feed, idea)).collect();
    let shown = rows.len() as u64;
    let stale = cache.is_some_and(|cache| cache.stale);
    let note = if let Some(error) = error {
        Some(format!("Updates stopped: {}", error.message))
    } else if stale && feed.link != Link::Up {
        Some("Shown as last received; reconnecting.".to_owned())
    } else if stale {
        Some("Shown as last received; refreshing.".to_owned())
    } else if page.total == 0 {
        Some("Nothing filed from chat yet.".to_owned())
    } else if shown < page.total {
        Some(format!("Showing {shown} of {}.", page.total))
    } else {
        None
    };
    let mut pager = Vec::new();
    if before.is_some() {
        pager.push(button("Newest", Action::FilingsPage(None)));
        pager.push(button("Newer", Action::FilingsPage(page.newer)));
    }
    if let Some(older) = page.older {
        pager.push(button("Older", Action::FilingsPage(Some(older))));
    }
    if error.is_some() {
        pager.push(retry());
    }
    GlobalFiled {
        rows,
        total: Some(page.total),
        note,
        pager,
    }
}

/// One filing: the task it made (or the task it was merged into), its source session, the
/// recorded device and its canonical time.
fn line(feed: &Feed, idea: &FiledIdea) -> FiledLine {
    let (title, opens) = match &idea.decision {
        Some(decision) => match &decision.into {
            Some(into) => (format!("merged into {into} · {}", idea.title), into.clone()),
            None => (
                format!("{} · {}", decision.action, idea.title),
                idea.task.clone(),
            ),
        },
        None => (idea.title.clone(), idea.task.clone()),
    };
    // The served source is the conversation the words came from, not who filed them.
    let mut source = if idea.quick {
        format!("From {} · quick thread", idea.session)
    } else {
        format!("From {}", idea.session)
    };
    if let Some(device) = &idea.device {
        source.push_str(&format!(" · {device}"));
    }
    source.push_str(&format!(" · {}", idea.status));
    FiledLine {
        id: idea.task.clone(),
        title,
        at: clock(&idea.at, &feed.date),
        source,
        action: Action::OpenLive(Live::Task(Id::from(opens.as_str()))),
    }
}
