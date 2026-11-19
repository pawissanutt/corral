//! The session's plan ledger as the host serves it (`DetailRef::SessionPlan`, host method
//! `plan`): the sidebar's Plan tab (heading, progress, the steps with their state and what each
//! waits on, the auto-continue record) and the Plan ledger sheet it opens (every step with its
//! criteria or drop reason, the counts by state, the auto-continue record and the earlier plans
//! the host kept). A ledger the host could not confirm is named stale, never shown as empty. The
//! host's history arrives over several pages; the client follows `next` and merges them, so the
//! sheet holds every earlier plan the host kept.

use corral_protocol::detail::{DetailBody, DetailGet, DetailRef, PlanStep, SessionPlan as Served};

use crate::model::Id;
use crate::model::board::Mark;
use crate::model::daemon::Feed;
use crate::model::effect::Effect;
use crate::model::hue::Hue;

use super::actions::{ActionGroup, row};
use super::read_state::ReadState;
use super::sidebar::SidebarAction;
use super::{Action, Can, Cx, Host, SessionRole, State};

/// The sidebar folds finished steps into one row above this many steps.
const FOLD_FROM: usize = 8;
/// The muted rows a selected step shows in the sidebar.
const STEP_DETAIL_ROWS: usize = 3;

/// The plan's state on this device, per session.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PlanUi {
    /// The step last opened; the sidebar shows its detail and the sheet marks it.
    pub step: Option<String>,
    /// The Plan ledger sheet is open.
    pub ledger: bool,
}

impl PlanUi {
    pub const EMPTY: Self = Self {
        step: None,
        ledger: false,
    };
}

/// What the plan surfaces do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanAction {
    /// Open the Plan ledger sheet, at a step when one is named.
    Open {
        session: Id,
        step: Option<String>,
    },
    Close(Id),
}

/// A step's state, as the host's ledger names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepState {
    Done,
    Working,
    Pending,
    Dropped,
    /// A state this app does not know; its word is shown as served.
    Other,
}

impl StepState {
    fn of(state: &str) -> Self {
        match state {
            "done" => Self::Done,
            "in_progress" => Self::Working,
            "pending" => Self::Pending,
            "dropped" => Self::Dropped,
            _ => Self::Other,
        }
    }

    pub const fn mark(self) -> (Mark, Hue) {
        match self {
            Self::Done => (Mark::Done, Hue::Ok),
            Self::Working => (Mark::Active, Hue::Worker),
            Self::Pending => (Mark::Next, Hue::Neutral),
            Self::Dropped | Self::Other => (Mark::Na, Hue::Neutral),
        }
    }

    fn word(self, served: &str) -> String {
        match self {
            Self::Done => "done".into(),
            Self::Working => "in progress".into(),
            Self::Pending => "not started".into(),
            Self::Dropped => "dropped".into(),
            Self::Other => served.replace('_', " "),
        }
    }
}

/// The sidebar's Plan tab.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanTab {
    /// `Plan <title> · r<n>`; none until a plan is served.
    pub heading: Option<String>,
    /// `3/7 done`.
    pub progress: Option<String>,
    /// The host could not confirm the ledger: what is shown is its last copy, or nothing.
    pub stale: Option<String>,
    /// Finished steps folded into one row: `✓ 4 done`.
    pub folded: Option<String>,
    pub steps: Vec<StepRow>,
    /// `Continued: <plan> · attempt 2`.
    pub continued: Option<(Hue, String)>,
    /// `no plan yet`, said only once the host answered that it has none.
    pub empty: Option<String>,
    /// The heading opens the ledger sheet.
    pub open: Action,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StepRow {
    /// `side-step-<n>`.
    pub test_id: String,
    pub id: String,
    pub text: String,
    pub state: StepState,
    /// What the step waits on, as the host names it.
    pub wait: Option<String>,
    pub selected: bool,
    /// Up to three muted rows under the selected step: its id and criteria, what it waits on,
    /// why it was dropped.
    pub detail: Vec<String>,
    /// Opens the ledger at this step.
    pub action: Action,
}

/// The Plan ledger sheet.
#[derive(Debug, Clone, PartialEq)]
pub struct Ledger {
    /// `Plan <title> · revision <n>`.
    pub title: String,
    /// `3 done · 1 in progress · 2 not started · 1 dropped`, or where the read stands.
    pub sub: String,
    pub stale: Option<String>,
    pub steps: Vec<LedgerStep>,
    /// What the empty ledger says instead of steps.
    pub empty: Option<String>,
    /// The auto-continue record, or `none recorded`.
    pub continued: (Hue, String),
    /// Earlier plans the host kept, newest first.
    pub history: Vec<HistoryLine>,
    pub history_empty: Option<String>,
    pub read: ReadState,
    /// Reread the plan; none while a read runs or the host cannot serve it.
    pub reread: Option<Action>,
    pub unavailable: Option<String>,
    pub close: Action,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LedgerStep {
    /// `ledger-step-<n>`.
    pub test_id: String,
    pub id: String,
    pub text: String,
    pub state: StepState,
    pub state_word: String,
    /// The criteria, or the reason it was dropped; `no criteria recorded`.
    pub detail: String,
    pub wait: Option<String>,
    pub selected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryLine {
    /// When it closed, as the app writes times.
    pub at: String,
    /// `r4`.
    pub revision: String,
    pub summary: String,
}

pub fn target(cx: &Cx<'_>) -> DetailRef {
    DetailRef::SessionPlan {
        id: cx.id.to_string(),
    }
}

/// The ledger the host served last.
fn served<'a>(cx: &Cx<'a>) -> Option<&'a Served> {
    match cx.feed.detail(&target(cx))?.body()? {
        DetailBody::SessionPlan(plan) => Some(plan),
        _ => None,
    }
}

/// Whether the host reports the ledger stale: in the ledger it served, or in its live state.
fn stale(cx: &Cx<'_>, plan: Option<&Served>) -> bool {
    plan.is_some_and(|plan| plan.stale)
        || cx
            .state
            .and_then(|state| state.plan.as_ref())
            .is_some_and(|brief| brief.stale)
}

fn stale_words(has_plan: bool) -> String {
    if has_plan {
        "! stale · the host could not confirm this is the current plan; it is its last copy".into()
    } else {
        "! stale · the host could not read its plan ledger, so nothing is shown as planned".into()
    }
}

/// `plan · attempt 2 · no_progress` as the owner reads it.
fn continued_words(continued: &str) -> (Hue, String) {
    if continued.contains("no_progress") {
        (
            Hue::Held,
            format!(
                "! auto-continue stopped: no progress · {}",
                continued.replace(" · no_progress", "")
            ),
        )
    } else if continued.contains(" · limit") {
        (
            Hue::Held,
            format!(
                "! auto-continue stopped at its limit · {}",
                continued.replace(" · limit", "")
            ),
        )
    } else {
        (Hue::Neutral, format!("Continued: {continued}"))
    }
}

fn open(cx: &Cx<'_>, step: Option<String>) -> Action {
    Action::Sidebar(SidebarAction::Plan(PlanAction::Open {
        session: cx.id.clone(),
        step,
    }))
}

fn detail_rows(step: &PlanStep, state: StepState) -> Vec<String> {
    let mut rows = Vec::new();
    match (state, step.detail.as_deref()) {
        (StepState::Dropped, Some(reason)) => rows.push(format!("{} · dropped: {reason}", step.id)),
        (_, Some(criteria)) => rows.push(format!("{} · {criteria}", step.id)),
        (_, None) => rows.push(format!("{} · no criteria recorded", step.id)),
    }
    if let Some(wait) = &step.wait {
        rows.push(format!("waits for: {wait}"));
    }
    rows.truncate(STEP_DETAIL_ROWS);
    rows
}

/// The sidebar's Plan tab.
pub fn plan_tab(cx: &Cx<'_>) -> PlanTab {
    let served_plan = served(cx);
    let plan = served_plan.and_then(|served| served.plan.as_ref());
    let stale = stale(cx, served_plan);
    let selected = cx.ui.sidebar.plan.step.as_deref();
    let steps = plan.map(|plan| plan.steps.as_slice()).unwrap_or_default();
    let done = steps
        .iter()
        .filter(|step| StepState::of(&step.state) == StepState::Done)
        .count();
    let fold = steps.len() > FOLD_FROM && done > 0;
    let rows = steps
        .iter()
        .enumerate()
        .filter_map(|(at, step)| {
            let state = StepState::of(&step.state);
            if fold && state == StepState::Done && selected != Some(step.id.as_str()) {
                return None;
            }
            let chosen = selected == Some(step.id.as_str());
            Some(StepRow {
                test_id: format!("side-step-{at}"),
                id: step.id.clone(),
                text: step.text.clone(),
                state,
                wait: step.wait.clone(),
                selected: chosen,
                detail: if chosen {
                    detail_rows(step, state)
                } else {
                    Vec::new()
                },
                action: open(cx, Some(step.id.clone())),
            })
        })
        .collect();
    let brief = cx.state.and_then(|state| state.plan.as_ref());
    let progress = match (plan, brief) {
        (Some(plan), _) => Some(format!("{done}/{} done", plan.steps.len())),
        (None, Some(brief)) if brief.total > 0 => {
            Some(format!("{}/{} done", brief.done, brief.total))
        }
        _ => None,
    };
    PlanTab {
        heading: plan.map(|plan| format!("Plan {} · r{}", plan.title, plan.revision)),
        progress,
        stale: stale.then(|| stale_words(plan.is_some())),
        folded: fold.then(|| format!("{done} done")),
        steps: rows,
        continued: served_plan
            .and_then(|served| served.continued.as_deref())
            .map(continued_words),
        empty: (served_plan.is_some() && plan.is_none() && !stale).then(|| "no plan yet".into()),
        open: open(cx, None),
    }
}

/// Why the host cannot serve the plan now.
fn unavailable(cx: &Cx<'_>) -> Option<String> {
    match cx.can("plan") {
        Can::Unavailable(why) => Some(why),
        _ => match cx.host {
            Host::NotRunning | Host::Ended => Some("the host is not running".into()),
            Host::Unreachable => Some("corrald cannot reach the host".into()),
            Host::Running | Host::Unknown => None,
        },
    }
}

fn counts(steps: &[PlanStep]) -> String {
    let mut by = [0usize; 5];
    for step in steps {
        let at = match StepState::of(&step.state) {
            StepState::Done => 0,
            StepState::Working => 1,
            StepState::Pending => 2,
            StepState::Dropped => 3,
            StepState::Other => 4,
        };
        by[at] += 1;
    }
    let words = [
        "done",
        "in progress",
        "not started",
        "dropped",
        "in another state",
    ];
    let said: Vec<String> = by
        .iter()
        .zip(words)
        .filter(|(n, _)| **n > 0)
        .map(|(n, word)| format!("{n} {word}"))
        .collect();
    if said.is_empty() {
        "no steps".into()
    } else {
        said.join(" · ")
    }
}

/// The Plan ledger sheet while it is open.
pub fn ledger(cx: &Cx<'_>) -> Option<Ledger> {
    if !cx.ui.sidebar.plan.ledger {
        return None;
    }
    let served_plan = served(cx);
    let plan = served_plan.and_then(|served| served.plan.as_ref());
    let stale = stale(cx, served_plan);
    let target = target(cx);
    let cache = cx.feed.detail(&target);
    let read = ReadState::of(cache, cx.feed);
    let unavailable = unavailable(cx);
    let selected = cx.ui.sidebar.plan.step.as_deref();
    let steps: Vec<LedgerStep> = plan
        .map(|plan| {
            plan.steps
                .iter()
                .enumerate()
                .map(|(at, step)| {
                    let state = StepState::of(&step.state);
                    LedgerStep {
                        test_id: format!("ledger-step-{at}"),
                        id: step.id.clone(),
                        text: step.text.clone(),
                        state,
                        state_word: state.word(&step.state),
                        detail: match (state, step.detail.as_deref()) {
                            (StepState::Dropped, Some(reason)) => format!("dropped: {reason}"),
                            (_, Some(criteria)) => format!("criteria: {criteria}"),
                            (_, None) => "no criteria recorded".into(),
                        },
                        wait: step.wait.clone(),
                        selected: selected == Some(step.id.as_str()),
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    let empty = match (served_plan, plan) {
        (_, Some(plan)) if plan.steps.is_empty() => Some("This plan has no steps yet.".into()),
        (_, Some(_)) => None,
        (Some(_), None) if stale => None,
        (Some(_), None) => Some("No plan yet: the agent has not written one.".into()),
        (None, None) => Some(read.words()),
    };
    let history: Vec<HistoryLine> = served_plan
        .map(|served| {
            served
                .history
                .iter()
                .map(|change| HistoryLine {
                    at: crate::model::agents::clock(Some(&change.at), cx.feed)
                        .unwrap_or_else(|| change.at.clone()),
                    revision: format!("r{}", change.revision),
                    summary: change.summary.clone(),
                })
                .collect()
        })
        .unwrap_or_default();
    Some(Ledger {
        title: plan.map_or_else(
            || "Plan ledger".into(),
            |plan| format!("Plan {} · revision {}", plan.title, plan.revision),
        ),
        sub: match plan {
            Some(plan) => format!("{} · {}", counts(&plan.steps), read.words()),
            None => read.words(),
        },
        stale: stale.then(|| stale_words(plan.is_some())),
        steps,
        empty,
        continued: served_plan
            .and_then(|served| served.continued.as_deref())
            .map_or_else(
                || (Hue::Neutral, "Auto-continue: none recorded".into()),
                continued_words,
            ),
        history_empty: (served_plan.is_some() && history.is_empty())
            .then(|| "No earlier plans kept by the host.".into()),
        history,
        reread: (unavailable.is_none() && !cache.is_some_and(|cache| cache.loading))
            .then(|| Action::Refresh(target)),
        read,
        unavailable,
        close: Action::Sidebar(SidebarAction::Plan(PlanAction::Close(cx.id.clone()))),
    })
}

/// Revalidate the ledger with the etag of the copy shown.
fn reread(feed: &Feed, session: &Id) -> Effect {
    let target = DetailRef::SessionPlan {
        id: session.to_string(),
    };
    Effect::Detail(DetailGet {
        etag: feed
            .detail(&target)
            .and_then(|cache| cache.pages.first())
            .map(|page| page.etag.clone()),
        target,
        page: None,
    })
}

pub(super) fn act(state: &mut State, action: PlanAction, feed: &Feed) -> Vec<Effect> {
    match action {
        PlanAction::Open { session, step } => {
            let plan = &mut state.ui_mut(&session).sidebar.plan;
            plan.ledger = true;
            if step.is_some() {
                plan.step = step;
            }
            let loading = feed
                .detail(&DetailRef::SessionPlan {
                    id: session.to_string(),
                })
                .is_some_and(|cache| cache.loading);
            if loading {
                Vec::new()
            } else {
                vec![reread(feed, &session)]
            }
        }
        PlanAction::Close(session) => {
            state.ui_mut(&session).sidebar.plan.ledger = false;
            Vec::new()
        }
    }
}

/// `Plan ledger` and `Reread plan` among the Inspect rows, while the host serves a plan.
pub fn actions(cx: &Cx<'_>) -> Option<ActionGroup> {
    if matches!(cx.can("plan"), Can::Absent) || cx.log_only() || *cx.role == SessionRole::Child {
        return None;
    }
    let why = unavailable(cx);
    let mut ledger = row("plan-ledger", "Plan ledger", None, open(cx, None));
    ledger.command = Some("/todo");
    let mut reread = row(
        "reread-plan",
        "Reread plan",
        None,
        Action::Refresh(target(cx)),
    );
    reread.command = Some("/todo refresh");
    reread.unavailable = why;
    Some(ActionGroup {
        title: "Inspect",
        rows: vec![ledger, reread],
    })
}
