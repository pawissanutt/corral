//! The session page's one side panel, Details: what the host serves besides the conversation,
//! each part a section that folds on its own. Changes (the files the session changed), Agents
//! (its child agents and background jobs), Plan (the plan ledger's steps), Usage (cost and
//! tokens), Context (what the session works with), and for a build its Board, for a task run its
//! Task and Proof. Each section is the part's body (`PartBody`: its summary, why the host cannot
//! serve it now, where its read stands) with its rows, and opens the part's full view (the
//! Changes list beside its diff, the Agents fleet, the board) in the conversation's place. On a
//! wide page Details is open beside the conversation until the owner closes it; elsewhere it is
//! closed until opened (Alt+B or the head's Details). The choice and the folds are this device's,
//! for every session. This module also routes the Plan ledger and Host events sheets, which a
//! section and the actions menu open.

mod context;
mod files;

use std::collections::BTreeSet;

use corral_protocol::detail::DetailRef;

use crate::model::Id;
use crate::model::daemon::Feed;
use crate::model::effect::Effect;
use crate::model::hue::Hue;
use crate::model::nav::Form;

use super::actions::{ActionGroup, row};
use super::events::{EventsAction, EventsUi};
use super::keys::{KeyAction, KeyBinding, bind};
use super::plan::{PlanAction, PlanTab, PlanUi};
use super::tabs::{PartBody, load, part};
use super::{
    Action, Can, Cx, SessionRole, State, SubView, Tab, board, changes, events, plan, proof, task,
    usage,
};

pub use context::{ContextFact, ContextList, ContextTab, McpRow, SideAgent, SideJob};
pub use files::{FilesTab, SideFile};

/// A section of Details, in the panel's order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Section {
    Task,
    Changes,
    Agents,
    Plan,
    Usage,
    Context,
    Board,
    Proof,
}

impl Section {
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Task => "task",
            Self::Changes => "changes",
            Self::Agents => "agents",
            Self::Plan => "plan",
            Self::Usage => "usage",
            Self::Context => "context",
            Self::Board => "board",
            Self::Proof => "proof",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Task => "Task",
            Self::Changes => "Changes",
            Self::Agents => "Agents",
            Self::Plan => "Plan",
            Self::Usage => "Usage",
            Self::Context => "Context",
            Self::Board => "Board",
            Self::Proof => "Proof",
        }
    }

    /// The host part the section is served from; none for what the engine's records or the
    /// host's state brief say.
    const fn capability(self) -> Option<&'static str> {
        match self {
            Self::Changes => Some("changes"),
            Self::Agents => Some("children"),
            Self::Plan => Some("plan"),
            Self::Context => Some("context"),
            Self::Board => Some("board"),
            Self::Task | Self::Usage | Self::Proof => None,
        }
    }
}

/// Details on this device, for every session page: whether the owner opened or closed it, and
/// the sections they folded.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Panel {
    /// The owner's choice; none follows the page's width.
    pub open: Option<bool>,
    /// The page is wide enough for Details beside the conversation, as the page last measured.
    pub wide: bool,
    pub folded: BTreeSet<Section>,
}

impl Panel {
    /// Whether Details shows: the owner's choice, else open on a wide page.
    pub fn shown(&self) -> bool {
        self.open.unwrap_or(self.wide)
    }

    /// A view opens (`opens`) in the conversation's place: where Details stands in that place
    /// (a narrow page, the phone's sheet), it closes so the view shows.
    pub(super) fn give_way(&mut self, opens: bool) {
        if opens && !self.wide && self.shown() {
            self.open = Some(false);
        }
    }
}

/// The per-session state of the sheets Details and the actions menu open.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SidebarUi {
    pub plan: PlanUi,
    pub events: EventsUi,
}

impl SidebarUi {
    pub const EMPTY: Self = Self {
        plan: PlanUi::EMPTY,
        events: EventsUi::EMPTY,
    };
}

/// What Details and its sheets do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SidebarAction {
    /// Open or close Details.
    Toggle,
    /// Fold or unfold a section.
    Fold(Section),
    /// The page measured whether Details fits beside the conversation.
    Wide(bool),
    /// Open a child agent in the Agents view.
    Agent {
        session: Id,
        child: String,
    },
    /// Open a background job in the Agents view.
    Job {
        session: Id,
        job: String,
    },
    Plan(PlanAction),
    Events(EventsAction),
}

/// One section of Details.
#[derive(Debug, Clone, PartialEq)]
pub struct DetailSection {
    pub section: Section,
    /// `details-<slug>`: the section's header, which folds it.
    pub id: String,
    pub label: &'static str,
    /// Its summary, why the host cannot serve it now and where its read stands.
    pub body: PartBody,
    pub folded: bool,
    pub fold: Action,
    /// `Open` (`btn-details-<slug>`): the part's full view in the conversation's place.
    pub view: Option<Action>,
    /// The section's rows; none while folded and for a section its summary says in full.
    pub content: Option<SectionContent>,
}

/// The rows of an unfolded section.
#[derive(Debug, Clone, PartialEq)]
pub enum SectionContent {
    Files(FilesTab),
    /// The agents and jobs of the context read.
    Agents(ContextTab),
    Plan(PlanTab),
    /// The facts, lists and servers of the context read.
    Context(ContextTab),
    /// The run's task: `T-12 · Running`, opening the task's page.
    Task(String, Action),
}

/// The Details panel of a session page.
#[derive(Debug, Clone, PartialEq)]
pub struct Details {
    /// Shown now: the owner's choice, else the page's width.
    pub shown: bool,
    pub sections: Vec<DetailSection>,
    /// Alt+B, the head's Details and the panel's close.
    pub toggle: Action,
}

/// The sections the session has: a task run's Task first and Proof last, a build's Board; a
/// section whose part the host declared it does not serve is left out, as is one it declared
/// nothing about (child agents excepted, which corrald's fleet reports for every host).
fn offered(cx: &Cx<'_>) -> Vec<Section> {
    if *cx.role == SessionRole::Child || cx.log_only() {
        return Vec::new();
    }
    let task_bound = cx.role.task_bound() && cx.task.is_some();
    let mut sections = Vec::new();
    if task_bound {
        sections.push(Section::Task);
    }
    sections.extend([
        Section::Changes,
        Section::Agents,
        Section::Plan,
        Section::Usage,
        Section::Context,
    ]);
    if *cx.role == SessionRole::Build {
        sections.push(Section::Board);
    }
    if task_bound {
        sections.push(Section::Proof);
    }
    sections
        .into_iter()
        .filter(|section| match section.capability().map(|id| cx.can(id)) {
            None | Some(Can::Yes | Can::Unavailable(_)) => true,
            Some(Can::Unknown) => *section == Section::Agents,
            Some(Can::Absent) => false,
        })
        .collect()
}

fn context_target(cx: &Cx<'_>) -> DetailRef {
    DetailRef::SessionContext {
        id: cx.id.to_string(),
    }
}

/// The read a section's body follows, `Err` when it reads none.
fn target(cx: &Cx<'_>, section: Section) -> Result<DetailRef, String> {
    match section {
        Section::Task => task::target(cx),
        Section::Changes => Ok(changes::target(cx)),
        Section::Agents if cx.state.is_some_and(|state| state.jobs.total > 0) => {
            Ok(context::jobs_target(cx))
        }
        Section::Plan => Ok(plan::target(cx)),
        Section::Context => Ok(context_target(cx)),
        Section::Board => Ok(board::target(cx)),
        Section::Proof => proof::target(cx),
        Section::Agents | Section::Usage => Err(String::new()),
    }
}

/// `2 running · 1 waiting`: the agents and jobs at work, as the host's state counts them.
fn at_work(cx: &Cx<'_>) -> (String, Hue) {
    let Some(state) = cx.state else {
        return ("not reported yet".into(), Hue::Neutral);
    };
    let running = state.children.live.saturating_add(state.jobs.live);
    let mut parts = Vec::new();
    if running > 0 {
        parts.push(format!("{running} running"));
    }
    if state.children.waiting > 0 {
        parts.push(format!("{} waiting", state.children.waiting));
    }
    if parts.is_empty() {
        return ("no agents or jobs at work".into(), Hue::Neutral);
    }
    let hue = if state.children.waiting > 0 {
        Hue::You
    } else {
        Hue::Worker
    };
    (parts.join(" · "), hue)
}

/// `3/7`, `3/7 · stale`, as the host's plan brief counts the steps.
fn plan_summary(cx: &Cx<'_>) -> (String, Hue) {
    match cx.state.and_then(|state| state.plan.as_ref()) {
        Some(plan) if plan.stale => (format!("{}/{} · stale", plan.done, plan.total), Hue::Held),
        Some(plan) if plan.total > 0 => (format!("{}/{} done", plan.done, plan.total), Hue::Neutral),
        Some(_) => ("no plan yet".into(), Hue::Neutral),
        None => ("not reported yet".into(), Hue::Neutral),
    }
}

/// `38% of the window`, as the host's usage brief reports it.
fn context_summary(cx: &Cx<'_>) -> (String, Hue) {
    let meter = super::status::meter(cx.id, cx.state.map(|state| &state.usage));
    match meter.percent {
        Some(percent) => (format!("{percent}% of the model's window"), meter.hue),
        None => ("window not reported".into(), Hue::Neutral),
    }
}

/// A section's summary line.
fn summary(cx: &Cx<'_>, section: Section) -> (String, Hue) {
    match section {
        Section::Task => (task::summary(cx), Hue::Neutral),
        Section::Changes => (changes::summary(cx), Hue::Neutral),
        Section::Agents => at_work(cx),
        Section::Plan => plan_summary(cx),
        Section::Usage => (usage::summary(cx), Hue::Neutral),
        Section::Context => context_summary(cx),
        Section::Board => board::summary(cx),
        Section::Proof => (proof::summary(cx), Hue::Neutral),
    }
}

/// The full view a section opens in the conversation's place.
fn view(cx: &Cx<'_>, section: Section) -> Option<Action> {
    let session = cx.id.clone();
    let tab = |tab| Action::Tab {
        session: session.clone(),
        tab,
    };
    Some(match section {
        Section::Task => tab(Tab::Task),
        Section::Changes => tab(Tab::Changes),
        Section::Agents => tab(Tab::Agents),
        Section::Board => tab(Tab::Board),
        Section::Proof => tab(Tab::Proof),
        Section::Usage => Action::Sub {
            session,
            sub: Some(SubView::Usage),
        },
        Section::Plan | Section::Context => return None,
    })
}

fn content(cx: &Cx<'_>, section: Section) -> Option<SectionContent> {
    Some(match section {
        Section::Changes => SectionContent::Files(files::files_tab(cx, &changes::target(cx))),
        Section::Agents => SectionContent::Agents(context::context_tab(cx, &context_target(cx))),
        Section::Plan => SectionContent::Plan(plan::plan_tab(cx)),
        Section::Context => {
            SectionContent::Context(context::context_tab(cx, &context_target(cx)))
        }
        Section::Task => {
            let task = cx.task?;
            let status = cx
                .feed
                .board
                .task(task)
                .map(|row| crate::model::status::status_word(row.status()));
            let words = match status {
                Some(status) => format!("{task} · {status}"),
                None => task.to_string(),
            };
            SectionContent::Task(
                words,
                Action::OpenLive(crate::model::Live::Task(task.clone())),
            )
        }
        Section::Usage | Section::Board | Section::Proof => return None,
    })
}

/// The Details panel of the session `cx` reads, as `panel` stands; none for a child's
/// conversation and a log, which have no parts.
pub fn details(cx: &Cx<'_>, panel: &Panel) -> Option<Details> {
    let offered = offered(cx);
    if offered.is_empty() {
        return None;
    }
    let sections = offered
        .into_iter()
        .map(|section| {
            let (words, hue) = summary(cx, section);
            let folded = panel.folded.contains(&section);
            DetailSection {
                section,
                id: format!("details-{}", section.slug()),
                label: section.label(),
                body: part(cx, target(cx, section), section.capability(), words, hue),
                folded,
                fold: Action::Sidebar(SidebarAction::Fold(section)),
                view: view(cx, section),
                content: (!folded).then(|| content(cx, section)).flatten(),
            }
        })
        .collect();
    Some(Details {
        shown: panel.shown(),
        sections,
        toggle: Action::Sidebar(SidebarAction::Toggle),
    })
}

/// Each section's name and summary, for the Sessions inspector: what the host's state says,
/// without reading a part.
pub fn brief(cx: &Cx<'_>) -> Vec<(String, String)> {
    offered(cx)
        .into_iter()
        .map(|section| (section.label().to_owned(), summary(cx, section).0))
        .collect()
}

/// What the shown sections read: each unfolded section's part, while Details shows.
pub fn fetches(cx: &Cx<'_>, panel: &Panel) -> Vec<Action> {
    if !panel.shown() {
        return Vec::new();
    }
    offered(cx)
        .into_iter()
        .filter(|section| !panel.folded.contains(section))
        .filter(|section| !matches!(section, Section::Task | Section::Board | Section::Proof))
        .filter_map(|section| {
            let capability = match section {
                Section::Agents => Some("jobs"),
                other => other.capability(),
            };
            load(cx, target(cx, section), capability)
        })
        .collect()
}

pub(super) fn act(state: &mut State, action: SidebarAction, feed: &Feed) -> Vec<Effect> {
    let opened = match action {
        SidebarAction::Toggle => {
            state.panel.open = Some(!state.panel.shown());
            return Vec::new();
        }
        SidebarAction::Fold(section) => {
            if !state.panel.folded.remove(&section) {
                state.panel.folded.insert(section);
            }
            return Vec::new();
        }
        SidebarAction::Wide(wide) => {
            state.panel.wide = wide;
            return Vec::new();
        }
        SidebarAction::Plan(action) => return plan::act(state, action, feed),
        SidebarAction::Events(action) => return events::act(state, action, feed),
        SidebarAction::Agent { session, child } => context::open_agent(&session, child),
        SidebarAction::Job { session, job } => context::open_job(&session, job),
    };
    opened
        .into_iter()
        .flat_map(|action| state.act(action, feed))
        .collect()
}

/// `Details` among the display rows.
pub fn actions(cx: &Cx<'_>) -> Option<ActionGroup> {
    if offered(cx).is_empty() || cx.env.form == Form::Phone {
        return None;
    }
    Some(ActionGroup {
        title: "Display",
        rows: vec![row(
            "details",
            "Open or close Details",
            Some("Alt+B"),
            Action::Sidebar(SidebarAction::Toggle),
        )],
    })
}

pub fn keys() -> Vec<KeyBinding> {
    vec![bind("alt+b", "Alt+B", "Details", KeyAction::Part)]
}

pub fn press(key: &str, cx: &Cx<'_>) -> Option<Action> {
    (key == "alt+b" && !offered(cx).is_empty()).then_some(Action::Sidebar(SidebarAction::Toggle))
}
