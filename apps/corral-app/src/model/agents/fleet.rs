//! A watched session's child agents and the messages between them, from the fleet corrald
//! last sent with the session's live pages (`session.fleet.v1`). Children are read here, never
//! steered: no command addresses them. A child and a message read as a published build's
//! slices and messages do, through the same words (`builds::slice_state`, `delivery` ...).

use corral_protocol::subscription::{
    SessionAgentMessage, SessionChild, SessionChildVerdict, SessionFleet,
};

use crate::model::Id;
use crate::model::builds::{self, Line};
use crate::model::daemon;
use crate::model::hue::Hue;
use crate::model::text::{Rich, span};
use crate::model::work::when;

use super::live::stale;
use super::{Action, State};

/// What the session page shows of the fleet. Absent for a session with neither children nor
/// messages, and from a daemon that does not serve them: nothing then stands in their place.
#[derive(Debug, Clone, PartialEq)]
pub struct Fleet {
    pub children: Option<FleetSection>,
    pub messages: Option<FleetSection>,
    /// Said while corrald is not answering: the fleet is the last one received.
    pub stale: Option<String>,
    /// The phone's folded line: what the fold holds, then how many children, how many still at
    /// work and how many messages.
    pub fold_title: &'static str,
    pub summary: String,
    /// The phone shows the sections, rather than only the folded line.
    pub open: bool,
    pub toggle: Action,
}

/// One list: the children, or the messages.
#[derive(Debug, Clone, PartialEq)]
pub struct FleetSection {
    pub heading: &'static str,
    /// How many, or "showing 100 of 240 messages" when corrald sent only the newest.
    pub note: Option<String>,
    pub entries: Vec<FleetEntry>,
}

/// A child or a message, drawn as a build's marked entry.
#[derive(Debug, Clone, PartialEq)]
pub struct FleetEntry {
    /// `agent-<session>-child-<id>` or `agent-<session>-msg-<seq>`.
    pub test_id: String,
    pub line: Line,
}

/// Child states still at work: their running time counts up to now.
const LIVE: [&str; 3] = ["queued", "running", "needs"];

/// The fleet of session `id`, as its live page last carried it.
pub(super) fn fleet(state: &State, id: &Id, feed: &daemon::Feed) -> Option<Fleet> {
    let fleet = feed.live.get(id.as_str())?.fleet.as_ref()?;
    let children = children(id, fleet, feed);
    let messages = messages(id, fleet, feed);
    if children.is_none() && messages.is_none() {
        return None;
    }
    let key = fold_key(id);
    let open = state.expanded.contains(&key);
    Some(Fleet {
        children,
        messages,
        stale: stale(feed, &format!("session:{id}")).then(|| {
            format!(
                "Showing the children and messages as of {}; corrald is not answering.",
                feed.synced.as_deref().unwrap_or("the last update")
            )
        }),
        fold_title: "Child agents",
        summary: summary(fleet),
        open,
        toggle: Action::Expand { key, fetch: None },
    })
}

/// The phone fold's key among the opened rows, apart from any transcript row's.
fn fold_key(id: &Id) -> String {
    format!("fleet:{id}")
}

/// `3 children · 1 at work · 1 open question · 12 messages`.
fn summary(fleet: &SessionFleet) -> String {
    let mut parts = Vec::new();
    if !fleet.children.is_empty() {
        let n = fleet.children.len() as u64;
        parts.push(builds::count(n, "child", "children"));
        let working = fleet
            .children
            .iter()
            .filter(|child| LIVE.contains(&child.state.as_str()))
            .count() as u64;
        if working > 0 {
            parts.push(format!("{working} at work"));
        }
        let asking = fleet.children.iter().filter(|child| asks(child)).count() as u64;
        if asking > 0 {
            parts.push(builds::count(asking, "open question", "open questions"));
        }
    }
    if fleet.messages_total > 0 {
        parts.push(builds::count(fleet.messages_total, "message", "messages"));
    }
    parts.join(" · ")
}

fn asks(child: &SessionChild) -> bool {
    child.question == Some(true)
}

fn children(id: &Id, fleet: &SessionFleet, feed: &daemon::Feed) -> Option<FleetSection> {
    if fleet.children.is_empty() {
        return None;
    }
    let mut note = fleet.children.len().to_string();
    if fleet.max_parallel > 0 {
        note.push_str(&format!(" · up to {} at once", fleet.max_parallel));
    }
    Some(FleetSection {
        heading: "Children",
        note: Some(note),
        entries: fleet
            .children
            .iter()
            .map(|child| FleetEntry {
                test_id: format!("agent-{id}-child-{}", child.id),
                line: child_line(child, feed),
            })
            .collect(),
    })
}

/// A child: its title, its kind unless it is a slice, a review's verdict and an open question
/// as chips; then its model, state, current tool, running time, tokens and cost. A cost the host
/// did not report is unknown, never zero.
fn child_line(child: &SessionChild, feed: &daemon::Feed) -> Line {
    let (mark, hue, state) = builds::slice_state(&child.state);
    let mut chips = Vec::new();
    if child.kind != "slice" {
        chips.push((child.kind.clone(), Hue::Neutral));
    }
    if let Some(verdict) = child.verdict {
        let (_, hue, words) = builds::verdict(match verdict {
            SessionChildVerdict::Approve => "approve",
            SessionChildVerdict::Changes => "changes",
        });
        chips.push((words.to_owned(), hue));
    }
    if asks(child) {
        chips.push(("open question".to_owned(), Hue::You));
    }
    let live = LIVE.contains(&child.state.as_str());
    let mut facts: Rich = vec![
        span(format!("{} · ", child.model)).dim(),
        span(state).hue(hue),
    ];
    if let Some(tool) = child.activity.as_deref().filter(|_| live) {
        facts.extend([span(" · using ").dim(), span(tool).mono()]);
    }
    if let Some(time) = running_time(child, live, feed.now) {
        facts.push(span(format!(" · {time}")).dim());
    }
    facts.push(
        span(format!(
            " · {} · {}",
            builds::tokens(child.tokens),
            builds::cost(child.cost_usd)
        ))
        .dim(),
    );
    Line {
        mark,
        hue,
        chips,
        head: vec![span(child.title.as_str()).bold()],
        body: vec![facts],
    }
}

/// How long the child has run (`4m so far`) or ran (`ran 12m`); nothing without a start, or for
/// an ended child without an end. `now` is in seconds, the host's times in milliseconds.
fn running_time(child: &SessionChild, live: bool, now: u64) -> Option<String> {
    let start = child.started_at? / 1000;
    match child.ended_at {
        Some(end) => Some(format!("ran {}", daemon::age(Some(start), end / 1000))),
        None if live && now > 0 => Some(format!("{} so far", daemon::age(Some(start), now))),
        None => None,
    }
}

fn messages(id: &Id, fleet: &SessionFleet, feed: &daemon::Feed) -> Option<FleetSection> {
    if fleet.messages.is_empty() && fleet.messages_total == 0 {
        return None;
    }
    let mut shown: Vec<&SessionAgentMessage> = fleet.messages.iter().collect();
    shown.sort_by_key(|message| (message.at, message.seq));
    Some(FleetSection {
        heading: "Agent messages",
        note: builds::note(shown.len(), fleet.messages_total, "messages"),
        entries: shown
            .into_iter()
            .map(|message| FleetEntry {
                test_id: format!("agent-{id}-msg-{}", message.seq),
                line: message_line(message, feed),
            })
            .collect(),
    })
}

/// `parent → review · 14:02`, the words, then how it was delivered when it did not simply
/// arrive.
fn message_line(message: &SessionAgentMessage, feed: &daemon::Feed) -> Line {
    let (mark, hue, words) = builds::delivery(&message.delivery);
    let mut head = vec![
        span(message.from.as_str()).mono(),
        span(" → "),
        span(message.to.as_str()).mono(),
    ];
    let at = i64::try_from(message.at)
        .ok()
        .and_then(|ms| jiff::Timestamp::from_millisecond(ms).ok())
        .map(|at| when(&at.to_string(), feed))
        .unwrap_or_default();
    if !at.is_empty() {
        head.push(span(format!(" · {at}")).dim());
    }
    let mut body = vec![vec![span(message.text.as_str())]];
    if let Some(words) = words {
        body.push(vec![span(words).hue(hue)]);
    }
    Line {
        mark,
        hue,
        chips: Vec::new(),
        head,
        body,
    }
}

#[cfg(test)]
#[path = "fleet_tests.rs"]
mod tests;
