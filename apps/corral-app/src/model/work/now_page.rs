//! The main-loop Now page for every form: owner moves, moving tasks, Waiting, Closed, the loop
//! band and one side panel driven by one selection.
use corral_protocol::rows::PlanState;
use std::collections::BTreeMap;

use super::{
    Action, Button, ClosedView, GroupKey, Inspector, NowAskPanel, NowAskRow, NowDigest, Press, Row,
    State, TaskRow,
};
use crate::model::board::{Actor, Mark, TaskLine};
use crate::model::daemon::Feed;
use crate::model::glance::{LoopBand, LoopStage, StageRail, Stat, StatIcon};
use crate::model::projects::{FilterScreen, NowGroup, ProjectFilter, project_chip};
use crate::model::{Destination, Form, Hue, Selection};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NowScopeAction {
    pub screen: FilterScreen,
    pub filter: ProjectFilter,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NowTaskRow {
    pub line: TaskLine,
    pub rail: StageRail,
    pub project: Option<String>,
    pub project_chip: Option<String>,
    pub selected: bool,
    pub action: Press,
}

impl NowTaskRow {
    /// The phone explanatory line is the daemon's now-phrase, not another stored copy.
    pub fn explanation(&self) -> &str {
        &self.line.now
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NowRow {
    Ask(NowAskRow),
    Task(NowTaskRow),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NowSection {
    pub key: String,
    pub label: String,
    pub count: usize,
    pub hue: Hue,
    pub hint: String,
    pub rows: Vec<NowRow>,
}

/// One of Waiting's groups (Upcoming, Held, Parked): its rows, then the line showing its hidden
/// rest while the group is collapsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NowWaitingGroup {
    pub key: GroupKey,
    pub label: String,
    /// Every task in the group, the hidden ones included.
    pub count: u32,
    pub hue: Hue,
    pub rows: Vec<NowTaskRow>,
    /// `+ N more`, shown while the group's limit hides rows.
    pub more: Option<Button>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NowWaiting {
    pub label: &'static str,
    pub total: usize,
    pub hint: String,
    pub stats: Vec<Stat>,
    /// The groups show, and the keys walk their rows, only while Waiting is open.
    pub expanded: bool,
    pub toggle: Press,
    pub groups: Vec<NowWaitingGroup>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum NowPanel {
    Digest(NowDigest),
    Ask(NowAskPanel),
    Task(Inspector),
    Closed(Inspector),
}

#[derive(Debug, Clone, PartialEq)]
pub struct NowPage {
    pub title: &'static str,
    /// Loop-stage destinations and Changes navigation use this same resolved scope.
    pub filter: ProjectFilter,
    pub grouping: NowGroup,
    pub stats: Vec<Stat>,
    pub scope_note: Option<String>,
    pub scope_action: Option<NowScopeAction>,
    pub loop_band: LoopBand,
    pub sections: Vec<NowSection>,
    pub waiting: NowWaiting,
    pub closed: Option<ClosedView>,
    /// Landed tasks on the loaded Closed page, or recorded landings since the seen cursor.
    pub closed_landed: Option<Stat>,
    pub panel: NowPanel,
    pub clear_selection: Press,
    pub close_label: &'static str,
    pub status: super::Status,
    pub loading_rows: usize,
    pub notice: Option<String>,
}

impl NowPage {
    /// The list's name for screen readers.
    pub const LIST_LABEL: &'static str = "Who acts next";
    /// What screen readers hear for the placeholder rows while the board loads.
    pub const LOADING: &'static str = "Loading the board";

    /// Opening `to` from this page carries Now's scope into that destination's own filter.
    pub fn scope_for(&self, to: Destination) -> Option<NowScopeAction> {
        FilterScreen::of(to).map(|screen| NowScopeAction {
            screen,
            filter: self.filter.clone(),
        })
    }
}

/// Builds all forms from one project scope. Dispatch `scope_action` through the existing
/// project-filter setter; forward `filter` when opening loop destinations or Changes.
pub fn now_page(
    state: &State,
    feed: &Feed,
    selection: &Selection,
    form: Form,
    filter: &ProjectFilter,
    grouping: NowGroup,
) -> NowPage {
    let filter = filter.resolved(feed);
    let NowList {
        sections,
        waiting_groups,
        verifying,
        workers,
        loading_rows,
        notice,
    } = now_list(state, feed, selection, form, &filter, grouping);
    let needs = crate::model::inbox::attention_counts_filtered(feed, &filter).total;
    let elsewhere = crate::model::inbox::attention_counts_filtered(feed, &ProjectFilter::All)
        .total
        .saturating_sub(needs);
    let stale = scoped_tasks(feed, &filter)
        .filter(|task| task.rail.iter().any(|step| step.mark == Mark::Stale))
        .count();
    let mut stats = Vec::new();
    push_count(
        &mut stats,
        "needs-you",
        StatIcon::NeedsYou,
        needs,
        Hue::You,
        format!("{needs} things wait on you"),
    );
    let moving = verifying + workers;
    push_count(
        &mut stats,
        "moving",
        StatIcon::Moving,
        moving as u64,
        Hue::Corral,
        format!("{moving} tasks are moving: {verifying} in verification, {workers} with a worker"),
    );
    push_count(
        &mut stats,
        "stale",
        StatIcon::Stale,
        stale as u64,
        Hue::Held,
        format!("{stale} tasks have a stale stage result"),
    );
    let digest = super::now_digest::digest(feed, &filter);
    let loop_band = band(feed, &filter, digest.landed.as_ref());
    let mut closed = super::closed_offered(feed)
        .then(|| super::closed_view_filtered(state, feed, selection, form, &filter));
    if state.selected_ask.is_some() || state.digest_only {
        if let Some(closed) = &mut closed {
            for row in &mut closed.rows {
                row.selected = false;
            }
        }
    }
    let closed_landed = closed
        .as_ref()
        .map(|closed| {
            closed
                .rows
                .iter()
                .filter(|row| row.mark == Mark::Done)
                .count()
        })
        .filter(|count| *count > 0)
        .map(|count| {
            Stat::new(
                "closed-landed",
                StatIcon::Landed,
                count.to_string(),
                Hue::Ok,
                format!("{count} landed tasks on this loaded closed page"),
            )
        })
        .or_else(|| {
            digest.landed.clone().map(|landed| Stat {
                key: "closed-landed".into(),
                ..landed
            })
        });
    let selected_ask =
        sections
            .iter()
            .flat_map(|section| &section.rows)
            .find_map(|row| match row {
                NowRow::Ask(ask) if state.selected_ask.as_ref() == Some(&ask.key) => Some(ask),
                _ => None,
            });
    let panel = if let Some(ask) = selected_ask {
        NowPanel::Ask(super::now_asks::panel(feed, ask.clone()))
    // A phone pushes the selected task's page instead; its Now keeps the digest's summary row.
    } else if form != Form::Phone
        && state.selected_ask.is_none()
        && !state.digest_only
        && selection.task.as_ref().is_some_and(|id| {
            scoped_tasks(feed, &filter).any(|task| &task.id == id)
                || (feed.board.task(id).is_none()
                    && (filter == ProjectFilter::All
                        || closed
                            .as_ref()
                            .is_some_and(|closed| closed.rows.iter().any(|row| &row.id == id))))
        })
    {
        let inspector =
            super::daemon_inspector_filtered(state, feed, selection, form, &filter, grouping);
        if super::off_board(selection, feed).is_some() {
            NowPanel::Closed(inspector)
        } else {
            NowPanel::Task(inspector)
        }
    } else {
        NowPanel::Digest(digest)
    };
    let total = waiting_groups
        .iter()
        .map(|group| group.count as usize)
        .sum();
    let mut waiting_stats = Vec::new();
    for group in &waiting_groups {
        let (key, icon, noun, hue) = match group.key {
            GroupKey::Held => ("held", StatIcon::Held, "held", Hue::Held),
            GroupKey::Parked => ("parked", StatIcon::Parked, "parked", Hue::Neutral),
            _ => (
                "queued",
                StatIcon::Queued,
                "queued or upcoming",
                Hue::Neutral,
            ),
        };
        push_count(
            &mut waiting_stats,
            key,
            icon,
            u64::from(group.count),
            hue,
            format!("{} tasks are {noun}", group.count),
        );
    }
    let scope_note = (filter != ProjectFilter::All && elsewhere > 0)
        .then(|| format!("{elsewhere} more wait on you in other projects"));
    let notice = notice
        .filter(|_| sections.is_empty() && total == 0)
        .map(str::to_owned);
    NowPage {
        title: "Now",
        filter,
        grouping,
        stats,
        scope_action: scope_note.as_ref().map(|_| NowScopeAction {
            screen: FilterScreen::Now,
            filter: ProjectFilter::All,
        }),
        scope_note,
        loop_band,
        sections,
        waiting: NowWaiting {
            label: "Waiting",
            total,
            hint: format!("{total} tasks are queued, held or parked"),
            stats: waiting_stats,
            expanded: state.waiting_open,
            toggle: Press::Work(Action::ToggleWaiting),
            groups: waiting_groups,
        },
        closed,
        closed_landed,
        panel,
        clear_selection: Press::ClearSelection,
        close_label: "Close selection",
        status: super::daemon_status(feed),
        loading_rows,
        notice,
    }
}

pub(super) struct NowList {
    pub sections: Vec<NowSection>,
    pub waiting_groups: Vec<NowWaitingGroup>,
    pub verifying: usize,
    pub workers: usize,
    pub loading_rows: usize,
    pub notice: Option<&'static str>,
}

/// Rendering and keyboard traversal share the same row projection and section order.
pub(super) fn now_list(
    state: &State,
    feed: &Feed,
    selection: &Selection,
    form: Form,
    filter: &ProjectFilter,
    grouping: NowGroup,
) -> NowList {
    let mut asks = super::now_asks::asks(state, feed, filter);
    // A phone has no side panel: an owner move's row opens its record, as its button does.
    if form == Form::Phone {
        for ask in &mut asks {
            ask.selected = false;
            if let Some(open) = ask.button.action.clone() {
                ask.select = open;
            }
        }
    }
    let source = super::daemon_list_filtered(state, feed, selection, form, filter, NowGroup::State);
    let mut verifying = Vec::new();
    let mut workers = Vec::new();
    let mut waiting_groups = Vec::new();
    for group in source.groups {
        if matches!(
            group.key,
            GroupKey::Queued | GroupKey::Held | GroupKey::Parked
        ) {
            let mut rows = Vec::new();
            let mut more = None;
            for row in group.rows {
                match row {
                    Row::Task(row) => {
                        rows.extend(task_row(state, feed, selection, form, filter, row))
                    }
                    Row::More(button) => more = Some(button),
                }
            }
            waiting_groups.push(NowWaitingGroup {
                key: group.key,
                label: group.label,
                count: group.count,
                hue: group.hue,
                rows,
                more,
            });
            continue;
        }
        if group.key == GroupKey::You {
            continue;
        }
        let target = if group.key == GroupKey::Corral {
            &mut verifying
        } else {
            &mut workers
        };
        for row in group.rows {
            if let Row::Task(row) = row {
                target.extend(task_row(state, feed, selection, form, filter, row));
            }
        }
    }
    let verifying_count = verifying.len();
    let worker_count = workers.len();
    let sections = if grouping == NowGroup::Project {
        by_project(asks, verifying, workers)
    } else {
        by_actor(asks, verifying, workers)
    };
    NowList {
        sections,
        waiting_groups,
        verifying: verifying_count,
        workers: worker_count,
        loading_rows: source.loading_rows,
        notice: source.notice,
    }
}

/// A board row with its named rail, selected only while no owner move or digest holds the panel.
fn task_row(
    state: &State,
    feed: &Feed,
    selection: &Selection,
    form: Form,
    filter: &ProjectFilter,
    row: TaskRow,
) -> Option<NowTaskRow> {
    let task = feed.board.task(&row.line.id)?;
    Some(NowTaskRow {
        rail: StageRail::daemon(task, row.line.actor),
        project: task.project.clone(),
        project_chip: project_chip(feed, filter, task.project.as_deref()),
        selected: form != Form::Phone
            && state.selected_ask.is_none()
            && !state.digest_only
            && selection.task.as_ref() == Some(&task.id),
        action: row.action,
        line: row.line,
    })
}

pub(super) fn scoped_tasks<'a>(
    feed: &'a Feed,
    filter: &'a ProjectFilter,
) -> impl Iterator<Item = &'a crate::model::daemon::Task> {
    feed.board
        .tasks()
        .iter()
        .filter(move |task| filter.matches(task.project.as_deref()))
}

fn by_actor(
    asks: Vec<NowAskRow>,
    verifying: Vec<NowTaskRow>,
    workers: Vec<NowTaskRow>,
) -> Vec<NowSection> {
    [
        (
            "you",
            "Needs you",
            Hue::You,
            "Owner moves wait on you",
            asks.into_iter().map(NowRow::Ask).collect::<Vec<_>>(),
        ),
        (
            "corral",
            "Corral",
            Hue::Corral,
            "Corral is gating, reviewing or landing these tasks",
            verifying.into_iter().map(NowRow::Task).collect(),
        ),
        (
            "workers",
            "Workers",
            Hue::Worker,
            "Workers are producing these tasks",
            workers.into_iter().map(NowRow::Task).collect(),
        ),
    ]
    .into_iter()
    .filter(|(_, _, _, _, rows)| !rows.is_empty())
    .map(|(key, label, hue, hint, rows)| NowSection {
        key: key.into(),
        label: label.into(),
        count: rows.len(),
        hue,
        hint: format!("{}: {hint}", rows.len()),
        rows,
    })
    .collect()
}

fn by_project(
    asks: Vec<NowAskRow>,
    verifying: Vec<NowTaskRow>,
    workers: Vec<NowTaskRow>,
) -> Vec<NowSection> {
    // The leading bool keeps Unassigned after every real project, even one named Unassigned.
    let mut projects: BTreeMap<(bool, String), Vec<NowRow>> = BTreeMap::new();
    for mut row in asks {
        row.project_chip = None;
        projects
            .entry(project_key(row.project.as_deref()))
            .or_default()
            .push(NowRow::Ask(row));
    }
    for mut row in verifying.into_iter().chain(workers) {
        row.project_chip = None;
        projects
            .entry(project_key(row.project.as_deref()))
            .or_default()
            .push(NowRow::Task(row));
    }
    projects
        .into_iter()
        .map(|((unassigned, label), rows)| NowSection {
            key: if unassigned {
                "unassigned".into()
            } else {
                format!("project-{label}")
            },
            hint: format!("{} owner moves and moving tasks in {label}", rows.len()),
            count: rows.len(),
            label,
            hue: Hue::Neutral,
            rows,
        })
        .collect()
}

fn project_key(project: Option<&str>) -> (bool, String) {
    (project.is_none(), project.unwrap_or("Unassigned").into())
}

fn band(feed: &Feed, filter: &ProjectFilter, landed: Option<&Stat>) -> LoopBand {
    let mut plan_stats = Vec::new();
    for (key, wanted, icon, hue, phrase) in [
        (
            "needs-you",
            PlanState::Awaiting,
            StatIcon::NeedsYou,
            Hue::You,
            "await acceptance",
        ),
        (
            "moving",
            PlanState::Executing,
            StatIcon::Moving,
            Hue::Corral,
            "are executing",
        ),
    ] {
        let count = feed
            .plans
            .values()
            .filter(|plan| match filter {
                ProjectFilter::All => true,
                ProjectFilter::Project(id) => {
                    plan.projects.iter().any(|project| project == id.as_str())
                }
                ProjectFilter::Unassigned => plan.projects.is_empty(),
            })
            .filter(|plan| plan.state == wanted)
            .count();
        push_count(
            &mut plan_stats,
            key,
            icon,
            count as u64,
            hue,
            format!("{count} plans {phrase}"),
        );
    }
    let mut stages = vec![LoopStage {
        key: "plan",
        label: "Plan",
        hint: "Plans awaiting acceptance or executing. Opens Plans in this scope.".into(),
        stats: plan_stats,
        to: Some(Destination::Plans),
    }];
    for (index, key, label, to) in [
        (0, "run", "Run", Destination::Agents),
        (1, "gate", "Gate", Destination::Review),
        (2, "review", "Review", Destination::Review),
    ] {
        let mut counts = [0_u64; 3];
        for task in scoped_tasks(feed, filter) {
            let group = super::served::task_group(feed, task);
            let actor = if group == GroupKey::You {
                Actor::You
            } else {
                crate::model::board::daemon_actor(task.group)
            };
            let rail = StageRail::daemon(task, actor);
            if rail.current != Some(index) {
                continue;
            }
            if actor == Actor::You {
                counts[0] += 1;
            } else if rail.marks[index] == Mark::Stale {
                counts[2] += 1;
            } else if matches!(group, GroupKey::Corral | GroupKey::Worker) {
                counts[1] += 1;
            }
        }
        let mut stats = Vec::new();
        for (index, stat_key, icon, hue, phrase) in [
            (0, "needs-you", StatIcon::NeedsYou, Hue::You, "wait on you"),
            (1, "moving", StatIcon::Moving, Hue::Corral, "are moving"),
            (2, "stale", StatIcon::Stale, Hue::Held, "have stale results"),
        ] {
            push_count(
                &mut stats,
                stat_key,
                icon,
                counts[index],
                hue,
                format!("{} tasks at {key} {phrase}", counts[index]),
            );
        }
        stages.push(LoopStage {
            key,
            label,
            hint: format!("Tasks at {key}. Opens {} in this scope.", to.label()),
            stats,
            to: Some(to),
        });
    }
    stages.push(LoopStage {
        key: "land",
        label: "Land",
        hint: "Recorded landings since your last visit in loaded history.".into(),
        stats: landed.cloned().into_iter().collect(),
        to: None,
    });
    LoopBand { stages }
}

fn push_count(
    stats: &mut Vec<Stat>,
    key: &str,
    icon: StatIcon,
    count: u64,
    hue: Hue,
    hint: String,
) {
    if count > 0 {
        stats.push(Stat::new(key, icon, count.to_string(), hue, hint));
    }
}
