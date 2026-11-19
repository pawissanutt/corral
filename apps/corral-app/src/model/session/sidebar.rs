//! The sidebar beside the main view (a rail when the page is wide, else a summary row above the
//! composer that opens a drawer): Context (the agents and jobs at work and what the session
//! works with), Files (what changed) and Plan (the plan ledger). Until the owner picks a tab, it
//! shows Context while an agent or job is live, else Plan when the plan has steps, else Context,
//! so it moves to what just became relevant; once the owner picks a tab, it never moves again in
//! this session. Alt+B hides the rail, F6 shows the next tab. This module holds the summary row
//! from the host's state, the shown tab's body (its summary, its unavailable, reading and failed
//! states, and its rows) and the sheets the sidebar opens: the Plan ledger and the Host events.

mod context;
mod files;

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
use super::{Action, Can, Cx, SessionRole, State, changes, events, plan};

pub use context::{ContextFact, ContextList, ContextTab, McpRow, SideAgent, SideJob};
pub use files::{FilesTab, SideFile};

/// A sidebar tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SideTab {
    #[default]
    Context,
    Files,
    Plan,
}

impl SideTab {
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Context => "context",
            Self::Files => "files",
            Self::Plan => "plan",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Context => "Context",
            Self::Files => "Files",
            Self::Plan => "Plan",
        }
    }

    /// The host part the tab reads.
    const fn capability(self) -> &'static str {
        match self {
            Self::Context => "context",
            Self::Files => "changes",
            Self::Plan => "plan",
        }
    }
}

/// The sidebar's state on this device, per session.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SidebarUi {
    /// The owner's pick; none follows the automatic rule. Once set it stays for the session.
    pub picked: Option<SideTab>,
    /// Alt+B hid the rail; the summary row stands for it.
    pub hidden: bool,
    pub plan: PlanUi,
    pub events: EventsUi,
}

impl SidebarUi {
    pub const EMPTY: Self = Self {
        picked: None,
        hidden: false,
        plan: PlanUi::EMPTY,
        events: EventsUi::EMPTY,
    };
}

/// What the sidebar does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SidebarAction {
    /// Show a tab; the pick stays.
    Pick {
        session: Id,
        tab: SideTab,
    },
    /// Hide or show the rail.
    Toggle(Id),
    /// Open a child agent in the Agents tab.
    Agent {
        session: Id,
        child: String,
    },
    /// Open a background job in the Agents tab.
    Job {
        session: Id,
        job: String,
    },
    Plan(PlanAction),
    Events(EventsAction),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Sidebar {
    pub tab: SideTab,
    /// The owner picked the tab.
    pub picked: bool,
    /// The rail is hidden: the summary row stands for it.
    pub hidden: bool,
    /// `side-<tab>`, the tabs the host serves.
    pub tabs: Vec<SideTabItem>,
    /// The shown tab's frame: its summary and where its read stands.
    pub body: PartBody,
    /// The shown tab's rows.
    pub content: SideContent,
    pub summary: SidebarSummary,
    /// Alt+B.
    pub toggle: Action,
}

/// The rows of the shown tab.
#[derive(Debug, Clone, PartialEq)]
pub enum SideContent {
    Context(ContextTab),
    Files(FilesTab),
    Plan(PlanTab),
}

#[derive(Debug, Clone, PartialEq)]
pub struct SideTabItem {
    pub tab: SideTab,
    /// `side-<slug>`.
    pub id: String,
    pub label: &'static str,
    pub selected: bool,
    pub action: Action,
}

/// The collapsed row: `Context 2 running · 1 waiting   Files 12   Plan 3/7   ▴`.
#[derive(Debug, Clone, PartialEq)]
pub struct SidebarSummary {
    /// The spinner before Context while anything works.
    pub working: bool,
    pub segments: Vec<SummarySegment>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SummarySegment {
    pub tab: SideTab,
    pub label: String,
    pub hue: Hue,
    /// Opens the drawer on the segment's tab.
    pub action: Action,
}

fn target(cx: &Cx<'_>, tab: SideTab) -> DetailRef {
    match tab {
        SideTab::Context => DetailRef::SessionContext {
            id: cx.id.to_string(),
        },
        SideTab::Files => changes::target(cx),
        SideTab::Plan => plan::target(cx),
    }
}

/// The tabs the host serves; none for a child's conversation.
fn tabs(cx: &Cx<'_>) -> Vec<SideTab> {
    if *cx.role == SessionRole::Child || cx.log_only() {
        return Vec::new();
    }
    [SideTab::Context, SideTab::Files, SideTab::Plan]
        .into_iter()
        .filter(|tab| matches!(cx.can(tab.capability()), Can::Yes | Can::Unavailable(_)))
        .collect()
}

fn working(cx: &Cx<'_>) -> bool {
    cx.state
        .is_some_and(|state| state.children.live.saturating_add(state.jobs.live) > 0)
}

/// The automatic tab: Context while anything works, else Plan when it has steps, else Context.
fn auto(cx: &Cx<'_>, offered: &[SideTab]) -> SideTab {
    let planned = cx
        .state
        .and_then(|state| state.plan.as_ref())
        .is_some_and(|plan| plan.total > 0);
    let wanted = if !working(cx) && planned {
        SideTab::Plan
    } else {
        SideTab::Context
    };
    if offered.contains(&wanted) {
        wanted
    } else {
        offered.first().copied().unwrap_or_default()
    }
}

/// The tab shown: the owner's pick while the host serves it, else the automatic one.
fn shown(cx: &Cx<'_>, offered: &[SideTab]) -> SideTab {
    cx.ui
        .sidebar
        .picked
        .filter(|picked| offered.contains(picked))
        .unwrap_or_else(|| auto(cx, offered))
}

fn summary_of(cx: &Cx<'_>, tab: SideTab) -> (String, Hue) {
    let state = cx.state;
    match tab {
        SideTab::Context => {
            let Some(state) = state else {
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
        SideTab::Files => match state.and_then(|state| state.changes) {
            Some(0) => ("no changes yet".into(), Hue::Neutral),
            Some(n) => (n.to_string(), Hue::Neutral),
            None => ("–".into(), Hue::Neutral),
        },
        SideTab::Plan => match state.and_then(|state| state.plan.as_ref()) {
            Some(plan) if plan.stale => {
                (format!("{}/{} · stale", plan.done, plan.total), Hue::Held)
            }
            Some(plan) if plan.total > 0 => (format!("{}/{}", plan.done, plan.total), Hue::Neutral),
            Some(_) => ("no plan yet".into(), Hue::Neutral),
            None => ("–".into(), Hue::Neutral),
        },
    }
}

fn pick(cx: &Cx<'_>, tab: SideTab) -> Action {
    Action::Sidebar(SidebarAction::Pick {
        session: cx.id.clone(),
        tab,
    })
}

/// The sidebar of the session `cx` reads; none where the host serves none of its tabs.
pub fn sidebar(cx: &Cx<'_>) -> Option<Sidebar> {
    let offered = tabs(cx);
    if offered.is_empty() {
        return None;
    }
    let ui = &cx.ui.sidebar;
    let tab = shown(cx, &offered);
    let (summary, hue) = summary_of(cx, tab);
    let read = target(cx, tab);
    let body = part(cx, Ok(read.clone()), Some(tab.capability()), summary, hue);
    let content = match tab {
        SideTab::Context => SideContent::Context(context::context_tab(cx, &read)),
        SideTab::Files => SideContent::Files(files::files_tab(cx, &read)),
        SideTab::Plan => SideContent::Plan(plan::plan_tab(cx)),
    };
    Some(Sidebar {
        tab,
        picked: ui.picked.is_some(),
        hidden: ui.hidden,
        tabs: offered
            .iter()
            .map(|side| SideTabItem {
                tab: *side,
                id: format!("side-{}", side.slug()),
                label: side.label(),
                selected: *side == tab,
                action: pick(cx, *side),
            })
            .collect(),
        body,
        content,
        summary: SidebarSummary {
            working: working(cx),
            segments: offered
                .iter()
                .map(|side| {
                    let (words, hue) = summary_of(cx, *side);
                    SummarySegment {
                        tab: *side,
                        label: format!("{} {words}", side.label()),
                        hue,
                        action: pick(cx, *side),
                    }
                })
                .collect(),
        },
        toggle: Action::Sidebar(SidebarAction::Toggle(cx.id.clone())),
    })
}

/// What the shown tab reads: its part, and on Context the jobs while the host lists one.
pub fn fetches(cx: &Cx<'_>) -> Vec<Action> {
    let offered = tabs(cx);
    if offered.is_empty() {
        return Vec::new();
    }
    let tab = shown(cx, &offered);
    let mut reads: Vec<Action> = load(cx, Ok(target(cx, tab)), Some(tab.capability()))
        .into_iter()
        .collect();
    if tab == SideTab::Context && cx.state.is_some_and(|state| state.jobs.total > 0) {
        reads.extend(load(cx, Ok(context::jobs_target(cx)), Some("jobs")));
    }
    reads
}

pub(super) fn act(state: &mut State, action: SidebarAction, feed: &Feed) -> Vec<Effect> {
    let opened = match action {
        SidebarAction::Pick { session, tab } => {
            state.ui_mut(&session).sidebar.picked = Some(tab);
            return Vec::new();
        }
        SidebarAction::Toggle(session) => {
            let sidebar = &mut state.ui_mut(&session).sidebar;
            sidebar.hidden = !sidebar.hidden;
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

/// `Hide sidebar` / `Show sidebar` among the display rows.
pub fn actions(cx: &Cx<'_>) -> Option<ActionGroup> {
    if tabs(cx).is_empty() || cx.env.form == Form::Phone {
        return None;
    }
    Some(ActionGroup {
        title: "Display",
        rows: vec![row(
            "sidebar",
            if cx.ui.sidebar.hidden {
                "Show sidebar"
            } else {
                "Hide sidebar"
            },
            Some("Alt+B"),
            Action::Sidebar(SidebarAction::Toggle(cx.id.clone())),
        )],
    })
}

pub fn keys() -> Vec<KeyBinding> {
    vec![
        bind(
            "alt+b",
            "Alt+B",
            "hide or show the sidebar",
            KeyAction::Part,
        ),
        bind("f6", "F6", "next sidebar tab", KeyAction::Part),
    ]
}

pub fn press(key: &str, cx: &Cx<'_>) -> Option<Action> {
    let offered = tabs(cx);
    if offered.is_empty() {
        return None;
    }
    match key {
        "alt+b" => Some(Action::Sidebar(SidebarAction::Toggle(cx.id.clone()))),
        "f6" => {
            let at = offered
                .iter()
                .position(|tab| *tab == shown(cx, &offered))
                .map_or(0, |at| (at + 1) % offered.len());
            offered.get(at).map(|tab| pick(cx, *tab))
        }
        _ => None,
    }
}
