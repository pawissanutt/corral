//! The host's event log (`DetailRef::SessionEvents`, which corrald answers from the host's
//! `events_since`): the Host events sheet `⋯ › Host events` opens, a read-only pager of what the
//! host recorded, newest first, each line its time, its kind and the event as the host wrote it.
//! The client follows the host's pages and merges them; the sheet shows them a page at a time,
//! and Show earlier adds the page before. It is read again each time it opens.

use corral_protocol::detail::{DetailBody, DetailGet, DetailRef, HostEvent, SessionEventsPage};

use crate::model::Id;
use crate::model::daemon::Feed;
use crate::model::effect::Effect;
use crate::model::hue::Hue;
use crate::model::text::span;

use super::actions::{ActionGroup, row};
use super::read_state::ReadState;
use super::sidebar::SidebarAction;
use super::transcript::{Line, LineClass};
use super::{Action, Cx, Host, SessionRole, State};

/// The events one page of the sheet shows.
const PAGE: usize = 200;

/// The sheet's state on this device, per session.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EventsUi {
    pub open: bool,
    /// Pages shown beyond the newest.
    pub earlier: usize,
}

impl EventsUi {
    pub const EMPTY: Self = Self {
        open: false,
        earlier: 0,
    };
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventsAction {
    Open(Id),
    Close(Id),
    /// Show the page before the oldest shown.
    Earlier(Id),
}

/// The Host events sheet.
#[derive(Debug, Clone, PartialEq)]
pub struct HostEvents {
    pub title: String,
    /// `412 events · newest first · read 14:02`.
    pub sub: String,
    pub read: ReadState,
    /// The host cannot be read now, and why.
    pub unavailable: Option<String>,
    /// One line per event: time, kind, what the host wrote.
    pub lines: Vec<Line>,
    /// What the sheet says instead of lines.
    pub empty: Option<String>,
    /// `Show 200 earlier` while older events are held.
    pub earlier: Option<(String, Action)>,
    pub reread: Option<Action>,
    pub close: Action,
}

pub fn target(id: &Id) -> DetailRef {
    DetailRef::SessionEvents { id: id.to_string() }
}

/// Why the host's events cannot be read now; none while it runs.
fn unavailable(cx: &Cx<'_>) -> Option<String> {
    if cx.log_only() {
        return Some("this session has no Pi host, so it keeps no host events".into());
    }
    match cx.host {
        Host::NotRunning | Host::Ended => Some("the host is not running".into()),
        Host::Unreachable => Some("corrald cannot reach the host".into()),
        Host::Running | Host::Unknown => None,
    }
}

fn served<'a>(cx: &Cx<'a>) -> Option<&'a SessionEventsPage> {
    match cx.feed.detail(&target(cx.id))?.body()? {
        DetailBody::SessionEvents(page) => Some(page),
        _ => None,
    }
}

/// An event kind's tone: failures and refusals in the failing hue, warnings held.
fn hue(kind: &str) -> Hue {
    let kind = kind.to_ascii_lowercase();
    if kind.contains("error") || kind.contains("fail") || kind.contains("refus") {
        Hue::Fail
    } else if kind.contains("warn") || kind.contains("stall") || kind.contains("retry") {
        Hue::Held
    } else {
        Hue::Neutral
    }
}

fn line(cx: &Cx<'_>, event: &HostEvent) -> Line {
    let at =
        crate::model::agents::clock(Some(&event.at), cx.feed).unwrap_or_else(|| event.at.clone());
    Line {
        text: vec![
            span(format!("{at:>8}  ")).dim(),
            span(format!("{}  ", event.kind))
                .bold()
                .hue(hue(&event.kind)),
            span(event.text.clone()),
        ],
        class: LineClass::Plain,
    }
}

/// The Host events sheet while it is open.
pub fn host_events(cx: &Cx<'_>) -> Option<HostEvents> {
    let ui = &cx.ui.sidebar.events;
    if !ui.open {
        return None;
    }
    let target = target(cx.id);
    let cache = cx.feed.detail(&target);
    let read = ReadState::of(cache, cx.feed);
    let unavailable = unavailable(cx);
    let events: &[HostEvent] = served(cx).map_or(&[], |page| page.events.as_slice());
    let shown = PAGE.saturating_mul(ui.earlier.saturating_add(1));
    let lines: Vec<Line> = events
        .iter()
        .rev()
        .take(shown)
        .map(|event| line(cx, event))
        .collect();
    let held = events.len().saturating_sub(lines.len());
    let empty = if !lines.is_empty() {
        None
    } else if read.has_rows() {
        Some("The host has recorded no events for this session yet.".to_owned())
    } else {
        Some(match (&read, &unavailable) {
            (ReadState::NotRead, Some(why)) => format!("Not read: {why}."),
            _ => read.words(),
        })
    };
    let count = match events.len() {
        0 if !read.has_rows() => String::new(),
        1 => "1 event · ".to_owned(),
        n => format!("{n} events · newest first · "),
    };
    Some(HostEvents {
        title: "Host events".into(),
        sub: format!("{count}{}", read.words()),
        unavailable,
        lines,
        empty,
        earlier: (held > 0).then(|| {
            (
                format!("Show {} earlier", held.min(PAGE)),
                Action::Sidebar(SidebarAction::Events(EventsAction::Earlier(cx.id.clone()))),
            )
        }),
        reread: (!cache.is_some_and(|cache| cache.loading)).then(|| Action::Refresh(target)),
        read,
        close: Action::Sidebar(SidebarAction::Events(EventsAction::Close(cx.id.clone()))),
    })
}

pub(super) fn act(state: &mut State, action: EventsAction, feed: &Feed) -> Vec<Effect> {
    match action {
        EventsAction::Open(session) => {
            let events = &mut state.ui_mut(&session).sidebar.events;
            events.open = true;
            events.earlier = 0;
            let target = target(&session);
            if feed.detail(&target).is_some_and(|cache| cache.loading) {
                return Vec::new();
            }
            // The log only grows: read it whole again rather than revalidating the first page.
            vec![Effect::Detail(DetailGet {
                target,
                etag: None,
                page: None,
            })]
        }
        EventsAction::Close(session) => {
            state.ui_mut(&session).sidebar.events.open = false;
            Vec::new()
        }
        EventsAction::Earlier(session) => {
            let events = &mut state.ui_mut(&session).sidebar.events;
            events.earlier = events.earlier.saturating_add(1);
            Vec::new()
        }
    }
}

/// `Host events` among the Inspect rows; a child's conversation has no host of its own.
pub fn actions(cx: &Cx<'_>) -> Option<ActionGroup> {
    if *cx.role == SessionRole::Child {
        return None;
    }
    let mut open = row(
        "host-events",
        "Host events",
        None,
        Action::Sidebar(SidebarAction::Events(EventsAction::Open(cx.id.clone()))),
    );
    open.unavailable = unavailable(cx);
    Some(ActionGroup {
        title: "Inspect",
        rows: vec![open],
    })
}
