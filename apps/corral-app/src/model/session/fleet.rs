//! The Agents tab: the session's child agents (fleet order, then helper calls newest first) as a
//! table, the selected one's detail, the timeline of when each ran, and the messages between
//! them. Everything is read from the fleet corrald last sent with the session's live pages
//! (`session.fleet.v1`); a cost, a context share or a time the host did not report stays
//! unknown, never zero.

use corral_protocol::subscription::{
    SessionAgentMessage, SessionChild, SessionChildVerdict, SessionFleet,
};

use crate::model::Id;
use crate::model::board::Mark;
use crate::model::daemon;
use crate::model::hue::Hue;
use crate::model::text::{Rich, span};
use crate::model::work::when;

use super::status::cost_text;
use super::{Action, Cx, Host};

#[derive(Debug, Clone, PartialEq)]
pub struct AgentsTab {
    /// Said while the host is unreachable or not running: the rows are the last ones received.
    pub unbound: Option<String>,
    pub rows: Vec<AgentRowView>,
    pub selected: Option<String>,
    pub detail: Option<AgentDetail>,
    pub timeline: Option<Timeline>,
    /// The messages between the agents, oldest first.
    pub messages: Option<MessageList>,
    /// What an empty tab says.
    pub empty: Option<String>,
}

/// One agent's row.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentRowView {
    /// `agent-<session>-child-<id>`.
    pub test_id: String,
    pub child: String,
    pub mark: Mark,
    pub hue: Hue,
    /// The state as a word beside the mark (`running`, `needs an answer`).
    pub state: &'static str,
    pub title: String,
    /// `slice · gpt-6`, `review · deep`, `→ <pending model>` appended while one is pending.
    pub profile_model: String,
    /// `1m 02s`, `waiting` before it started, `–` when unknown.
    pub time: String,
    pub cost: String,
    pub ctx: String,
    /// Edited files, `!n` when some were outside its own.
    pub files: String,
    pub outside: bool,
    /// What it does now, or how it ended.
    pub now: String,
    pub selected: bool,
    pub action: Action,
}

/// The selected agent's facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentDetail {
    pub child: String,
    pub title: String,
    /// `Owns 3 files · starts after a0`.
    pub owns: Option<String>,
    /// `▲ Asks: …` while it waits on the owner.
    pub asks: bool,
    pub verdict: Option<(Hue, String)>,
    pub pending_model: Option<String>,
    pub outside: Option<String>,
    pub error: Option<String>,
}

/// When each agent ran, on one axis from the earliest start to now.
#[derive(Debug, Clone, PartialEq)]
pub struct Timeline {
    /// `Timeline · up to 3 at once · 3 messages`.
    pub heading: String,
    pub rows: Vec<TimelineRow>,
    /// `0s` … `4m 12s`.
    pub axis_end: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TimelineRow {
    pub child: String,
    /// Fractions of the axis, 0 to 1.
    pub start: f32,
    pub end: f32,
    pub hue: Hue,
    /// `✓ ✗ !` at its end, none while it runs.
    pub glyph: Option<&'static str>,
    pub action: Action,
}

/// The messages between the agents.
#[derive(Debug, Clone, PartialEq)]
pub struct MessageList {
    pub heading: &'static str,
    /// `showing 100 of 240 messages` when corrald sent only the newest.
    pub note: Option<String>,
    pub entries: Vec<MessageLine>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MessageLine {
    /// `agent-<session>-msg-<seq>`.
    pub test_id: String,
    pub mark: Mark,
    pub hue: Hue,
    /// `parent → review · 14:02`.
    pub head: Rich,
    pub body: Vec<Rich>,
}

/// Child states still at work: their running time counts up to now.
const LIVE: [&str; 3] = ["queued", "running", "needs"];

/// A child's state as a mark, its hue and its word.
fn child_state(state: &str) -> (Mark, Hue, &'static str) {
    match state {
        "queued" => (Mark::Next, Hue::Neutral, "queued"),
        "running" => (Mark::Active, Hue::Worker, "running"),
        "needs" => (Mark::Active, Hue::You, "needs an answer"),
        "done" => (Mark::Done, Hue::Neutral, "done"),
        "failed" => (Mark::Fail, Hue::Fail, "failed"),
        "cancelled" => (Mark::Na, Hue::Neutral, "cancelled"),
        _ => (Mark::Na, Hue::Neutral, "state not known to this app"),
    }
}

/// How a message reached its recipient: the mark, and the words when it did not simply arrive.
fn delivery(delivery: &str) -> (Mark, Hue, Option<&'static str>) {
    match delivery {
        "delivered" => (Mark::Done, Hue::Neutral, None),
        "queued" => (Mark::Next, Hue::Held, Some("queued, not yet delivered")),
        "undeliverable" => (Mark::Fail, Hue::Fail, Some("undeliverable")),
        "answered_from_report" => (Mark::Na, Hue::Neutral, Some("answered from its report")),
        _ => (
            Mark::Na,
            Hue::Neutral,
            Some("delivery not known to this app"),
        ),
    }
}

fn asks(child: &SessionChild) -> bool {
    child.question == Some(true) || child.state == "needs"
}

fn helper(child: &SessionChild) -> bool {
    child.kind.starts_with("helper:")
}

/// `1m 02s` run, `waiting` before it started; `–` without a start. Times are milliseconds.
fn time(child: &SessionChild, now: u64) -> String {
    let Some(start) = child.started_at.map(|ms| ms / 1000) else {
        return if LIVE.contains(&child.state.as_str()) {
            "waiting".into()
        } else {
            "–".into()
        };
    };
    let end = match child.ended_at {
        Some(end) => end / 1000,
        None if now > 0 => now,
        None => return "–".into(),
    };
    super::activity::duration(end.saturating_sub(start))
}

/// What the child does now, or how it ended.
fn now_words(child: &SessionChild) -> String {
    if let Some(error) = &child.error {
        return error.clone();
    }
    if asks(child) {
        return "waiting for your answer".into();
    }
    match (child.state.as_str(), child.verdict) {
        ("done", Some(SessionChildVerdict::Approve)) => "approved".into(),
        ("done", Some(SessionChildVerdict::Changes)) => "changes requested".into(),
        ("cancelled", _) => "cancelled · its edits stay in the worktree".into(),
        ("queued", _) if !child.after.is_empty() => {
            format!("waits for {}", child.after.join(", "))
        }
        ("queued", _) => "waits for a free slot".into(),
        ("running", _) => child.activity.clone().unwrap_or_else(|| "working".into()),
        (_, _) if child.files > 0 => format!("{} files", child.files),
        _ => "–".into(),
    }
}

fn profile_model(child: &SessionChild) -> String {
    let lead = if child.kind == "slice" {
        child.profile.clone()
    } else {
        Some(
            child
                .kind
                .strip_prefix("helper:")
                .unwrap_or(&child.kind)
                .to_owned(),
        )
    };
    let mut words = match lead {
        Some(lead) => format!("{lead} · {}", child.model),
        None => child.model.clone(),
    };
    if let Some(pending) = &child.pending_model {
        words.push_str(&format!(" → {pending}"));
    }
    words
}

fn row(session: &Id, child: &SessionChild, selected: bool, now: u64) -> AgentRowView {
    let (mark, hue, state) = child_state(&child.state);
    let hue = if asks(child) { Hue::You } else { hue };
    AgentRowView {
        test_id: format!("agent-{session}-child-{}", child.id),
        child: child.id.clone(),
        mark,
        hue,
        state,
        title: child.title.clone(),
        profile_model: profile_model(child),
        time: time(child, now),
        cost: cost_text(child.cost_usd, false),
        ctx: child
            .context_percent
            .filter(|value| value.is_finite())
            .map_or_else(|| "–".into(), |value| format!("{}%", value.round() as u32)),
        files: match (child.files, child.outside) {
            (0, 0) => "–".into(),
            (files, 0) => files.to_string(),
            (files, outside) => format!("{files} !{outside}"),
        },
        outside: child.outside > 0,
        now: now_words(child),
        selected,
        action: Action::SelectAgent {
            session: session.clone(),
            child: Some(child.id.clone()),
        },
    }
}

fn detail(child: &SessionChild) -> AgentDetail {
    let mut owns = Vec::new();
    if child.owns > 0 {
        owns.push(format!(
            "Owns {} {}",
            child.owns,
            if child.owns == 1 { "file" } else { "files" }
        ));
    }
    if !child.after.is_empty() {
        owns.push(format!("starts after {}", child.after.join(", ")));
    }
    AgentDetail {
        child: child.id.clone(),
        title: child.title.clone(),
        owns: (!owns.is_empty()).then(|| owns.join(" · ")),
        asks: asks(child),
        verdict: child.verdict.map(|verdict| match verdict {
            SessionChildVerdict::Approve => (Hue::Ok, "Verdict: approved".to_owned()),
            SessionChildVerdict::Changes => (Hue::Held, "Verdict: changes requested".to_owned()),
        }),
        pending_model: child.pending_model.clone(),
        outside: (child.outside > 0).then(|| {
            format!(
                "{} {} outside its own files",
                child.outside,
                if child.outside == 1 { "edit" } else { "edits" }
            )
        }),
        error: child.error.clone(),
    }
}

fn timeline(session: &Id, fleet: &SessionFleet, now: u64) -> Option<Timeline> {
    let starts: Vec<u64> = fleet
        .children
        .iter()
        .filter_map(|child| child.started_at)
        .collect();
    let first = *starts.iter().min()?;
    let last = fleet
        .children
        .iter()
        .filter_map(|child| child.ended_at)
        .chain((now > 0).then_some(now.saturating_mul(1000)))
        .max()
        .unwrap_or(first)
        .max(first + 1);
    let span = (last - first) as f32;
    let at = |ms: u64| (ms.saturating_sub(first)) as f32 / span;
    let rows = fleet
        .children
        .iter()
        .filter_map(|child| {
            let start = child.started_at?;
            let end = child.ended_at.unwrap_or(last);
            let (_, hue, _) = child_state(&child.state);
            Some(TimelineRow {
                child: child.id.clone(),
                start: at(start),
                end: at(end).max(at(start)),
                hue: if asks(child) { Hue::You } else { hue },
                glyph: match child.state.as_str() {
                    "done" => Some("✓"),
                    "failed" => Some("✗"),
                    "cancelled" => Some("!"),
                    _ => None,
                },
                action: Action::SelectAgent {
                    session: session.clone(),
                    child: Some(child.id.clone()),
                },
            })
        })
        .collect();
    let mut heading = vec!["Timeline".to_owned()];
    if fleet.max_parallel > 0 {
        heading.push(format!("up to {} at once", fleet.max_parallel));
    }
    if fleet.messages_total > 0 {
        heading.push(format!(
            "{} {}",
            fleet.messages_total,
            if fleet.messages_total == 1 {
                "message"
            } else {
                "messages"
            }
        ));
    }
    Some(Timeline {
        heading: heading.join(" · "),
        rows,
        axis_end: super::activity::duration((last - first) / 1000),
    })
}

fn messages(session: &Id, fleet: &SessionFleet, feed: &daemon::Feed) -> Option<MessageList> {
    if fleet.messages.is_empty() && fleet.messages_total == 0 {
        return None;
    }
    let mut shown: Vec<&SessionAgentMessage> = fleet.messages.iter().collect();
    shown.sort_by_key(|message| (message.at, message.seq));
    let len = u64::try_from(shown.len()).unwrap_or(u64::MAX);
    Some(MessageList {
        heading: "Agent messages",
        note: Some(if fleet.messages_total > len {
            format!("showing {len} of {} messages", fleet.messages_total)
        } else {
            fleet.messages_total.max(len).to_string()
        }),
        entries: shown
            .into_iter()
            .map(|message| message_line(session, message, feed))
            .collect(),
    })
}

/// `parent → review · 14:02`, the words, then how it was delivered when it did not simply
/// arrive.
fn message_line(session: &Id, message: &SessionAgentMessage, feed: &daemon::Feed) -> MessageLine {
    let (mark, hue, words) = delivery(&message.delivery);
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
    MessageLine {
        test_id: format!("agent-{session}-msg-{}", message.seq),
        mark,
        hue,
        head,
        body,
    }
}

/// The Agents tab of the session `cx` reads.
pub fn agents_tab(cx: &Cx<'_>) -> AgentsTab {
    let fleet = cx.live.and_then(|live| live.fleet.as_ref());
    let unbound = match cx.host {
        _ if cx.feed.link != daemon::Link::Up => {
            Some("! corrald unreachable · these observations may be stale".to_owned())
        }
        Host::Unreachable => Some("! Host unreachable · these observations may be stale".into()),
        Host::NotRunning | Host::Ended if fleet.is_some() => {
            Some("The host is not running · this is its last fleet".into())
        }
        _ => None,
    };
    let Some(fleet) = fleet.filter(|fleet| !fleet.children.is_empty() || fleet.messages_total > 0)
    else {
        return AgentsTab {
            unbound,
            rows: Vec::new(),
            selected: None,
            detail: None,
            timeline: None,
            messages: None,
            empty: Some(
                "No agents yet. Agents and background jobs appear here when the session starts them."
                    .into(),
            ),
        };
    };
    let mut children: Vec<&SessionChild> = fleet
        .children
        .iter()
        .filter(|child| !helper(child))
        .collect();
    let mut helpers: Vec<&SessionChild> = fleet.children.iter().filter(|c| helper(c)).collect();
    helpers.sort_by(|a, b| b.started_at.cmp(&a.started_at));
    children.extend(helpers);
    let selected = cx
        .ui
        .agent
        .clone()
        .filter(|id| children.iter().any(|child| child.id == *id));
    AgentsTab {
        unbound,
        rows: children
            .iter()
            .map(|child| {
                row(
                    cx.id,
                    child,
                    selected.as_deref() == Some(child.id.as_str()),
                    cx.feed.now,
                )
            })
            .collect(),
        detail: selected
            .as_deref()
            .and_then(|id| children.iter().find(|child| child.id == id))
            .map(|child| detail(child)),
        selected,
        timeline: timeline(cx.id, fleet, cx.feed.now),
        messages: messages(cx.id, fleet, cx.feed),
        empty: None,
    }
}

#[cfg(test)]
#[path = "fleet_tests.rs"]
mod tests;
