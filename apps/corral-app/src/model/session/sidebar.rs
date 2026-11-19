//! The sidebar beside the main view (a rail when the page is wide, else a summary row above the
//! composer that opens a drawer): Context (agents and jobs at work), Files (what changed) and
//! Plan (the plan ledger). Until the owner picks a tab, it shows Context while an agent or job
//! is live, else Plan when the plan has steps, else Context; a pick stays. Alt+B hides the rail.
//! This module holds the summary row from the host's state, each tab's summary and its
//! unavailable, reading and failed states; the tabs' rows are this module's to add, with their
//! state in `SidebarUi` and their actions in `SidebarAction`.

use corral_protocol::detail::{ChangesComparison, DetailRef};

use crate::model::Id;
use crate::model::daemon::Feed;
use crate::model::effect::Effect;
use crate::model::hue::Hue;
use crate::model::nav::Form;

use super::actions::{ActionGroup, row};
use super::keys::{KeyAction, KeyBinding, bind};
use super::tabs::{PartBody, load, part};
use super::{Action, Can, Cx, SessionRole, State};

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
    /// The owner's pick; none follows the automatic rule.
    pub picked: Option<SideTab>,
    /// Alt+B hid the rail; the summary row stands for it.
    pub hidden: bool,
}

impl SidebarUi {
    pub const EMPTY: Self = Self {
        picked: None,
        hidden: false,
    };
}

/// What the sidebar does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SidebarAction {
    /// Show a tab; the pick stays.
    Pick { session: Id, tab: SideTab },
    /// Hide or show the rail.
    Toggle(Id),
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
    /// The shown tab's body.
    pub body: PartBody,
    pub summary: SidebarSummary,
    /// Alt+B.
    pub toggle: Action,
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
    let id = cx.id.to_string();
    match tab {
        SideTab::Context => DetailRef::SessionContext { id },
        SideTab::Files => DetailRef::SessionChanges {
            id,
            comparison: ChangesComparison::Session,
            scope: None,
        },
        SideTab::Plan => DetailRef::SessionPlan { id },
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

/// The automatic tab: Context while anything works, else Plan when it has steps, else Context.
fn auto(cx: &Cx<'_>, offered: &[SideTab]) -> SideTab {
    let state = cx.state;
    let working =
        state.is_some_and(|state| state.children.live.saturating_add(state.jobs.live) > 0);
    let planned = state
        .and_then(|state| state.plan.as_ref())
        .is_some_and(|plan| plan.total > 0);
    let wanted = if !working && planned {
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
    let tab = ui
        .picked
        .filter(|picked| offered.contains(picked))
        .unwrap_or_else(|| auto(cx, &offered));
    let (summary, hue) = summary_of(cx, tab);
    let body = part(
        cx,
        Ok(target(cx, tab)),
        Some(tab.capability()),
        summary,
        hue,
    );
    let working = cx
        .state
        .is_some_and(|state| state.children.live.saturating_add(state.jobs.live) > 0);
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
        summary: SidebarSummary {
            working,
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

/// What the shown tab reads.
pub fn fetches(cx: &Cx<'_>) -> Vec<Action> {
    let Some(sidebar) = sidebar(cx) else {
        return Vec::new();
    };
    load(
        cx,
        Ok(target(cx, sidebar.tab)),
        Some(sidebar.tab.capability()),
    )
    .into_iter()
    .collect()
}

pub(super) fn act(state: &mut State, action: SidebarAction, _: &Feed) -> Vec<Effect> {
    match action {
        SidebarAction::Pick { session, tab } => {
            state.ui_mut(&session).sidebar.picked = Some(tab);
        }
        SidebarAction::Toggle(session) => {
            let sidebar = &mut state.ui_mut(&session).sidebar;
            sidebar.hidden = !sidebar.hidden;
        }
    }
    Vec::new()
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
    vec![bind(
        "alt+b",
        "Alt+B",
        "hide or show the sidebar",
        KeyAction::Part,
    )]
}

pub fn press(key: &str, cx: &Cx<'_>) -> Option<Action> {
    (key == "alt+b" && !tabs(cx).is_empty())
        .then(|| Action::Sidebar(SidebarAction::Toggle(cx.id.clone())))
}
