//! The Agents tab: the session's child agents (fleet order, then helper calls newest first) and
//! its running commands and latest exit as their own group; the selected agent's or job's detail with what can be
//! done to it; the timeline of when each agent ran and the messages between them. Opening an
//! agent shows its conversation (`conversation`), opening a job follows its output (`jobs`);
//! an agent the list does not hold opens by its id (`AgentLookup`).
//! The rows are read from the fleet corrald last sent with the session's live pages; an agent's
//! brief, edits, steers and report from its `SessionChild` part; the jobs from `SessionJobs`. A
//! cost, a context share or a time the host did not report stays unknown, never zero. What the
//! owner does goes out through `ops` as `session.child` and `session.job`. What the clock moves
//! (running times, the timeline's bars, jobs' elapsed time) is kept apart in `AgentsTab.clock`,
//! so everything the tab's controls are built from stays equal while only the clock ticks.

use std::collections::{BTreeMap, BTreeSet};

use corral_protocol::detail::{CardKind, DetailBody, DetailRef, SessionChildDetail};
use corral_protocol::subscription::{
    SessionAgentMessage, SessionChild, SessionChildVerdict, SessionFleet,
};

use crate::model::Id;
use crate::model::board::Mark;
use crate::model::daemon::{self, Link};
use crate::model::hue::Hue;
use crate::model::text::{Rich, span};
use crate::model::work::when;

use super::status::cost_text;
use super::tabs::load;
use super::{Action, Can, Cx, Host, State, SubView};

#[path = "child_detail.rs"]
mod child;
#[path = "fleet_clock.rs"]
mod clock;
#[path = "conversation.rs"]
mod conversation;
#[path = "jobs.rs"]
mod jobs;
#[path = "fleet_ops.rs"]
mod ops;

pub use child::{AgentDetail, Clipped, EditLine, EditList, ModelChoiceRow, ModelPicker};
pub use clock::AgentsClock;
pub(super) use conversation::opened_agent;
pub use conversation::{Conversation, EndLine};
pub use jobs::{JobDetail, JobList, JobOutputView, JobRowView};
pub(super) use jobs::{short_id, visible_commands};
pub use ops::{Asking, ConfirmView, FleetAction, FleetRequest};
pub(super) use ops::{act, actions, keys, press, settle};

/// The host capability that steers, cancels and re-models children.
const CHILD_CONTROL: &str = "child_control";

/// The Agents tab's state on this device, per session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FleetUi {
    /// The selected background job; a selected agent (`Ui.agent`) takes precedence.
    pub job: Option<String>,
    /// A cancel or stop waiting for its one confirmation.
    pub asking: Option<Asking>,
    /// The agent whose model choices are open.
    pub picking: Option<String>,
    /// Long texts opened in full, by key (`brief:<child>`, `report:<child>`, `edits:<child>`).
    pub opened: BTreeSet<String>,
    /// Requests this page sent, read back under their keys.
    pub pending: BTreeMap<Id, FleetRequest>,
}

impl FleetUi {
    pub const EMPTY: Self = Self {
        job: None,
        asking: None,
        picking: None,
        opened: BTreeSet::new(),
        pending: BTreeMap::new(),
    };
}

impl Default for FleetUi {
    fn default() -> Self {
        Self::EMPTY
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentsTab {
    /// Said while the host is unreachable or not running: the rows are the last ones received.
    pub unbound: Option<String>,
    pub rows: Vec<AgentRowView>,
    /// The visible commands; none when the session has none and serves none.
    pub jobs: Option<JobList>,
    pub selected: Option<String>,
    pub detail: Option<AgentDetail>,
    /// The selected job's detail, when a job rather than an agent is selected.
    pub job: Option<JobDetail>,
    /// An agent's conversation or a job's output, shown in place of the list.
    pub opened: Option<Opened>,
    /// A cancel or stop waiting for its confirmation.
    pub confirm: Option<ConfirmView>,
    /// An agent's model choices, while open.
    pub picker: Option<ModelPicker>,
    pub timeline: Option<Timeline>,
    /// The messages between the agents, oldest first.
    pub messages: Option<MessageList>,
    /// What an empty tab says.
    pub empty: Option<String>,
    /// The field that opens an agent by its id, listed or not.
    pub lookup: AgentLookup,
    /// What the clock moves; everything else is equal across a tick.
    pub clock: AgentsClock,
}

/// The field that opens an agent of the session by its id: one the live list leaves out (it
/// holds only the newest) or one of a host that stopped is read from what corrald recorded. The
/// owner finds the id in the parent's transcript, where the parent started the agent.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentLookup {
    pub session: Id,
    /// The field's test id, which `FleetAction::Lookup` gives the keys to.
    pub field: &'static str,
    pub label: &'static str,
    pub placeholder: &'static str,
    pub note: &'static str,
}

impl AgentLookup {
    fn of(session: &Id) -> Self {
        Self {
            session: session.clone(),
            field: ops::LOOKUP_FIELD,
            label: "Open an agent by id",
            placeholder: "Agent id, as the parent's transcript names it",
            note: "Opens what corrald recorded for an agent of this session, also one the list \
                   no longer shows.",
        }
    }

    /// The conversation `typed` opens; `Err` says why it opens none, or nothing while nothing
    /// is typed.
    pub fn check(&self, typed: &str) -> Result<Action, Option<String>> {
        ops::lookup(&self.session, typed)
    }
}

/// What replaces the list.
#[derive(Debug, Clone, PartialEq)]
pub enum Opened {
    Conversation(Box<Conversation>),
    Job(JobOutputView),
}

/// One agent's row.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentRowView {
    /// `agent-<session>-child-<id>`.
    pub test_id: String,
    pub child: String,
    /// The id as the row shows it: a helper call's shortened to its first eight characters.
    pub name: String,
    pub mark: Mark,
    pub hue: Hue,
    /// The state as a word beside the mark (`running`, `asks you`).
    pub state: &'static str,
    pub title: String,
    /// `slice · gpt-6`, `review · deep`, `→ <pending model>` appended while one is pending.
    pub profile_model: String,
    pub cost: String,
    pub ctx: String,
    /// `4 turns`, `–` before its first.
    pub turns: String,
    /// Edited files, `!n` when some were outside its own.
    pub files: String,
    pub outside: bool,
    /// What it does now, or how it ended; none when there is nothing to say.
    pub now: Option<String>,
    /// It waits on the owner's answer.
    pub asks: bool,
    /// The card its question waits in, once the host posted it.
    pub card: Option<String>,
    pub selected: bool,
    /// Selects the agent; on the selected row, opens its conversation.
    pub action: Action,
}

/// A control on an agent or a job. One the host cannot run now stays, muted with its reason,
/// and does nothing.
#[derive(Debug, Clone, PartialEq)]
pub struct FleetButton {
    /// `btn-child-<verb>`, `btn-job-<verb>`.
    pub id: String,
    pub label: String,
    /// The key cap that runs it where keys show (`Alt+S`).
    pub key: Option<&'static str>,
    pub unavailable: Option<String>,
    /// Asks once before it goes out.
    pub danger: bool,
    pub action: Action,
}

impl FleetButton {
    fn new(id: &str, label: impl Into<String>, key: Option<&'static str>, action: Action) -> Self {
        Self {
            id: format!("btn-{id}"),
            label: label.into(),
            key,
            unavailable: None,
            danger: false,
            action,
        }
    }

    /// Muted with the first reason given.
    fn unless(mut self, why: Option<String>) -> Self {
        if self.unavailable.is_none() {
            self.unavailable = why;
        }
        self
    }

    fn danger(mut self) -> Self {
        self.danger = true;
        self
    }
}

/// When each agent ran, on one axis from the earliest start to now.
#[derive(Debug, Clone, PartialEq)]
pub struct Timeline {
    /// `Timeline · up to 3 at once · 3 messages`.
    pub heading: String,
    /// The bars' extents and the axis are `AgentsClock`'s.
    pub rows: Vec<TimelineRow>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TimelineRow {
    pub child: String,
    /// The id as the row shows it: a helper call's shortened.
    pub label: String,
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
        "needs" => (Mark::Active, Hue::You, "asks you"),
        "done" => (Mark::Done, Hue::Ok, "done"),
        "failed" => (Mark::Fail, Hue::Fail, "failed"),
        "cancelled" => (Mark::Na, Hue::Neutral, "cancelled"),
        _ => (Mark::Na, Hue::Neutral, "state not known to this app"),
    }
}

/// The mark, hue and word of `child`, a waiting question winning over its state.
fn shown_state(child: &SessionChild) -> (Mark, Hue, &'static str) {
    let (mark, hue, word) = child_state(&child.state);
    if asks(child) {
        (mark, Hue::You, "asks you")
    } else {
        (mark, hue, word)
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

fn live(child: &SessionChild) -> bool {
    LIVE.contains(&child.state.as_str())
}

/// `1m 02s` run, `waiting` before it started; `–` without a start. Times are milliseconds.
fn time(child: &SessionChild, now: u64) -> String {
    let Some(start) = child.started_at.map(|ms| ms / 1000) else {
        return if live(child) {
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

/// What the child does now, or how it ended; none when there is nothing to say.
fn now_words(child: &SessionChild) -> Option<String> {
    if let Some(error) = &child.error {
        return Some(error.clone());
    }
    if asks(child) {
        return Some("waiting for your answer".into());
    }
    Some(match (child.state.as_str(), child.verdict) {
        ("done", Some(SessionChildVerdict::Approve)) => "approved".into(),
        ("done", Some(SessionChildVerdict::Changes)) => "changes requested".into(),
        ("cancelled", _) => "cancelled · its edits stay in the worktree".into(),
        ("queued", _) if !child.after.is_empty() => {
            format!("waits for {}", child.after.join(", "))
        }
        ("queued", _) => "waits for a free slot".into(),
        ("running", _) => child.activity.clone().unwrap_or_else(|| "working".into()),
        (_, _) if child.files > 0 => format!("{} files", child.files),
        _ => return None,
    })
}

/// The id as rows show it: a helper call's (a long request id) shortened.
fn shown_id(child: &SessionChild) -> String {
    if helper(child) {
        jobs::short_id(&child.id)
    } else {
        child.id.clone()
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

fn ctx(child: &SessionChild) -> String {
    child
        .context_percent
        .filter(|value| value.is_finite())
        .map_or_else(|| "–".into(), |value| format!("{}%", value.round() as u32))
}

fn turns(n: u64) -> String {
    match n {
        0 => "–".into(),
        1 => "1 turn".into(),
        n => format!("{n} turns"),
    }
}

/// The scope of a child's own transcript.
fn scope(session: &Id, child: &str) -> Id {
    Id::from(format!("{session}/child/{child}"))
}

fn child_target(session: &Id, child: &str) -> DetailRef {
    DetailRef::SessionChild {
        id: session.to_string(),
        child: child.to_owned(),
    }
}

/// The child's detail as last read.
fn child_detail<'f>(
    feed: &'f daemon::Feed,
    session: &Id,
    child: &str,
) -> Option<&'f SessionChildDetail> {
    match feed.detail(&child_target(session, child))?.body()? {
        DetailBody::SessionChild(detail) => Some(detail),
        _ => None,
    }
}

/// The card a child's question waits in, as the host's state briefs it.
fn card_of(cx: &Cx<'_>, child: &str) -> Option<String> {
    cx.state?
        .cards
        .iter()
        .find(|card| card.kind == CardKind::ChildQuestion && card.child.as_deref() == Some(child))
        .map(|card| card.id.clone())
}

/// Why the host cannot run what `capability` covers now.
fn control(cx: &Cx<'_>, capability: &str) -> Option<String> {
    if cx.feed.link != Link::Up {
        return Some("corrald is unreachable".into());
    }
    match cx.can(capability) {
        Can::Absent => {
            return Some(match capability {
                "job_control" => "this session cannot control its jobs".into(),
                "jobs" => "this session does not serve its jobs".into(),
                CHILD_CONTROL => "this session cannot control its agents".into(),
                _ => "this session does not serve its agents' details".into(),
            });
        }
        Can::Unavailable(why) => return Some(why),
        Can::Yes | Can::Unknown => {}
    }
    match cx.host {
        Host::NotRunning | Host::Ended => Some("the host is not running".into()),
        Host::Unreachable => Some("corrald cannot reach the host".into()),
        Host::Running | Host::Unknown => None,
    }
}

/// The children in the tab's order: fleet order, then helper calls newest first.
fn ordered(fleet: &SessionFleet) -> Vec<&SessionChild> {
    let mut children: Vec<&SessionChild> = fleet
        .children
        .iter()
        .filter(|child| !helper(child))
        .collect();
    let mut helpers: Vec<&SessionChild> = fleet.children.iter().filter(|c| helper(c)).collect();
    helpers.sort_by(|a, b| b.started_at.cmp(&a.started_at));
    children.extend(helpers);
    children
}

fn fleet<'a>(cx: &Cx<'a>) -> Option<&'a SessionFleet> {
    cx.live.and_then(|live| live.fleet.as_ref())
}

/// The selected agent, when it is still in the fleet.
fn selected_child<'a>(cx: &Cx<'a>) -> Option<&'a SessionChild> {
    let id = cx.ui.agent.as_deref()?;
    fleet(cx)?.children.iter().find(|child| child.id == id)
}

fn row(cx: &Cx<'_>, child: &SessionChild, selected: bool) -> AgentRowView {
    let (mark, hue, state) = shown_state(child);
    let session = cx.id.clone();
    AgentRowView {
        test_id: format!("agent-{session}-child-{}", child.id),
        child: child.id.clone(),
        name: shown_id(child),
        mark,
        hue,
        state,
        title: child.title.clone(),
        profile_model: profile_model(child),
        cost: cost_text(child.cost_usd, false),
        ctx: ctx(child),
        turns: turns(child.turns),
        files: match (child.files, child.outside) {
            (0, 0) => "–".into(),
            (files, 0) => files.to_string(),
            (files, outside) => format!("{files} !{outside}"),
        },
        outside: child.outside > 0,
        now: now_words(child),
        asks: asks(child),
        card: asks(child).then(|| card_of(cx, &child.id)).flatten(),
        selected,
        action: if selected {
            Action::Fleet(FleetAction::Open {
                session,
                child: child.id.clone(),
            })
        } else {
            Action::SelectAgent {
                session,
                child: Some(child.id.clone()),
            }
        },
    }
}

fn timeline(session: &Id, fleet: &SessionFleet) -> Option<Timeline> {
    fleet.children.iter().find_map(|child| child.started_at)?;
    let rows = fleet
        .children
        .iter()
        .filter_map(|child| {
            child.started_at?;
            let (_, hue, _) = shown_state(child);
            Some(TimelineRow {
                child: child.id.clone(),
                label: shown_id(child),
                hue,
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

/// The opened conversation or job output, from the tab's sub-view.
fn opened(cx: &Cx<'_>, state: &State, fleet: Option<&SessionFleet>) -> Option<Opened> {
    match cx.ui.sub.as_ref()? {
        SubView::Agent(child) => {
            let row = fleet.and_then(|fleet| fleet.children.iter().find(|c| c.id == *child));
            let ui = state.ui(scope(cx.id, child).as_str());
            Some(Opened::Conversation(Box::new(conversation::conversation(
                cx, ui, child, row,
            ))))
        }
        SubView::Job(job) => Some(Opened::Job(jobs::output(cx, job))),
        SubView::Usage => None,
    }
}

/// The Agents tab of the session `cx` reads; `state` holds the opened conversation's own UI
/// state (what its rows have open).
pub fn agents_tab(cx: &Cx<'_>, state: &State) -> AgentsTab {
    let fleet = fleet(cx);
    let unbound = match cx.host {
        _ if cx.feed.link != Link::Up => {
            Some("! corrald unreachable · these observations may be stale".to_owned())
        }
        Host::Unreachable => Some("! Host unreachable · these observations may be stale".into()),
        Host::NotRunning | Host::Ended if fleet.is_some() => {
            Some("The host is not running · this is its last fleet".into())
        }
        _ => None,
    };
    let children = fleet.map(ordered).unwrap_or_default();
    let selected = selected_child(cx).map(|child| child.id.clone());
    let job = selected
        .is_none()
        .then(|| cx.ui.fleet.job.clone())
        .flatten();
    let job_list = jobs::list(cx, job.as_deref());
    let job = job.filter(|job| {
        job_list
            .as_ref()
            .is_some_and(|list| list.rows.iter().any(|row| row.job == *job))
    });
    let messages_total = fleet.map_or(0, |fleet| fleet.messages_total);
    let empty = (children.is_empty() && messages_total == 0 && job_list.is_none()).then(|| {
        "No agents yet. Agents and commands appear here when the session starts them.".to_owned()
    });
    AgentsTab {
        unbound,
        rows: children
            .iter()
            .map(|child| row(cx, child, selected.as_deref() == Some(child.id.as_str())))
            .collect(),
        detail: selected_child(cx).map(|child| child::detail(cx, child)),
        job: job.as_deref().and_then(|job| jobs::detail(cx, job)),
        jobs: job_list,
        selected,
        opened: opened(cx, state, fleet),
        confirm: ops::confirm_view(cx),
        picker: child::picker(cx),
        timeline: fleet.and_then(|fleet| timeline(cx.id, fleet)),
        messages: fleet.and_then(|fleet| messages(cx.id, fleet, cx.feed)),
        empty,
        lookup: AgentLookup::of(cx.id),
        clock: clock::clock(cx),
    }
}

/// What the tab reads while it shows: the jobs; the selected, opened or re-modelled agent's
/// detail; an opened agent's transcript and the complete words behind its previews; an opened
/// job's output.
pub fn fetches(cx: &Cx<'_>, state: &State) -> Vec<Action> {
    let mut wanted: Vec<Action> = jobs::fetches(cx);
    let opened = match &cx.ui.sub {
        Some(SubView::Agent(child)) => Some(child.as_str()),
        _ => None,
    };
    let mut details: Vec<&str> = [
        opened,
        cx.ui.agent.as_deref(),
        cx.ui.fleet.picking.as_deref(),
    ]
    .into_iter()
    .flatten()
    .collect();
    details.sort_unstable();
    details.dedup();
    for child in details {
        // Child details, like transcripts, can be served from the stopped host's archive.
        wanted.extend(load(cx, Ok(child_target(cx.id, child)), None));
    }
    if let Some(child) = opened {
        let scope = scope(cx.id, child);
        wanted.extend(load(
            cx,
            Ok(DetailRef::Session {
                id: scope.to_string(),
            }),
            None,
        ));
        wanted.extend(
            super::transcript::fetches(cx.feed, &scope, state.ui(&scope))
                .into_iter()
                .map(|token| Action::Fetch {
                    session: scope.clone(),
                    token,
                }),
        );
    }
    wanted
}

#[cfg(test)]
#[path = "fleet_tests.rs"]
mod tests;
