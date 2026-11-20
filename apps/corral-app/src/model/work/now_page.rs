//! The main-loop Now page for every form: the tasks that move (needs you, corral, workers),
//! Waiting (queued, backlog, ideas), Closed, the loop band and one side panel driven by one
//! selection.
use corral_types::Status;
use std::collections::BTreeMap;

use super::{
    Action, Button, ClosedView, GroupKey, Inspector, NowDigest, Press, Row, State, TaskRow,
};
use crate::model::board::TaskLine;
use crate::model::daemon::{self, Feed};
use crate::model::glance::{LoopBand, LoopStage, StageRail, Stat, StatIcon};
use crate::model::projects::{NowGroup, ProjectFilter, project_chip};
use crate::model::{Destination, Form, Hue, Selection};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NowScopeAction {
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
    /// The phone explanatory line is the row's now-phrase: a Blocked task's reason, else where
    /// it stands.
    pub fn explanation(&self) -> &str {
        &self.line.now
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NowSection {
    pub key: String,
    pub label: String,
    pub count: usize,
    pub hue: Hue,
    pub hint: String,
    /// Running slots in use of the limit (`3 of 5 slots`), on the section of running work.
    pub slots: Option<String>,
    pub rows: Vec<NowTaskRow>,
}

/// One of Waiting's groups (Queued, Backlog, Ideas): its rows, then the line showing its hidden
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
    /// The task Now opens at: the first that needs you, in list order.
    pub opens_at: Option<crate::model::Id>,
    pub waiting: NowWaiting,
    pub closed: Option<ClosedView>,
    /// Tasks recorded as landed since the seen cursor, in the loaded history: the digest's count.
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
    pub const LIST_LABEL: &'static str = "Who moves next";
    /// What screen readers hear for the placeholder rows while the board loads.
    pub const LOADING: &'static str = "Loading open tasks";
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
        opens_at,
        waiting_groups,
        verifying,
        workers,
        loading_rows,
        notice,
    } = now_list(state, feed, selection, form, &filter, grouping);
    let needs = scoped_tasks(feed, &filter)
        .filter(|task| task.group == daemon::Group::NeedsYou)
        .count();
    let elsewhere = feed
        .board
        .count(daemon::Group::NeedsYou)
        .saturating_sub(needs);
    let mut stats = Vec::new();
    push_count(
        &mut stats,
        "needs-you",
        StatIcon::NeedsYou,
        needs as u64,
        Hue::You,
        format!("{needs} tasks wait on you"),
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
    let digest = super::now_digest::digest(feed, &filter);
    let loop_band = band(feed, &filter, digest.landed.as_ref());
    let mut closed = super::closed_offered(feed)
        .then(|| super::closed_view_filtered(state, feed, selection, form, &filter));
    if state.digest_only
        && let Some(closed) = &mut closed
    {
        for row in &mut closed.rows {
            row.selected = false;
        }
    }
    // The same count whether the section is folded or open: the loaded page's rows are one page
    // of the closed history, not the tasks that landed since the owner's last visit.
    let closed_landed = digest.landed.clone().map(|landed| Stat {
        key: "closed-landed".into(),
        ..landed
    });
    // A phone pushes the selected task's page instead; its Now keeps the digest's summary row.
    let panel = if form != Form::Phone
        && !state.digest_only
        && selection.task.as_ref().is_some_and(|id| {
            scoped_tasks(feed, &filter).any(|task| &task.id == id)
                || (feed.board.task(id).is_none()
                    && (filter == ProjectFilter::All
                        || closed
                            .as_ref()
                            .is_some_and(|closed| closed.rows.iter().any(|row| &row.id == id))))
        }) {
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
        let (key, icon, noun) = match group.key {
            GroupKey::Backlog => ("backlog", StatIcon::Parked, "in the backlog"),
            GroupKey::Ideas => ("ideas", StatIcon::Parked, "ideas"),
            _ => ("queued", StatIcon::Queued, "queued"),
        };
        push_count(
            &mut waiting_stats,
            key,
            icon,
            u64::from(group.count),
            Hue::Neutral,
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
            filter: ProjectFilter::All,
        }),
        scope_note,
        loop_band,
        sections,
        opens_at,
        waiting: NowWaiting {
            label: "Waiting",
            total,
            hint: format!("{total} tasks are queued, in the backlog or ideas"),
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
    pub opens_at: Option<crate::model::Id>,
    pub waiting_groups: Vec<NowWaitingGroup>,
    pub verifying: usize,
    pub workers: usize,
    pub loading_rows: usize,
    pub notice: Option<&'static str>,
}

/// `3 of 5 slots` and its sentence, from the daemon's served slot counts.
fn slot_words(count: &corral_protocol::health::SlotCount, scope: &str) -> (String, String) {
    (
        format!("{} of {} slots", count.used, count.limit),
        format!(
            "{} of {} running slots in use{scope}: a Running or Blocked task holds one",
            count.used, count.limit
        ),
    )
}

/// The running slots of Now's scope: a project with its own limit counts its own, else the
/// workspace's.
fn slots(feed: &Feed, filter: &ProjectFilter) -> Option<(String, String)> {
    let slots = feed.health.as_ref()?.slots.as_ref()?;
    if let ProjectFilter::Project(project) = filter
        && let Some(count) = slots.projects.get(project.as_str())
    {
        return Some(slot_words(count, &format!(" in {project}")));
    }
    Some(slot_words(&slots.global, ""))
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
    let source = super::daemon_list_filtered(state, feed, selection, form, filter, NowGroup::State);
    let mut needs = Vec::new();
    let mut verifying = Vec::new();
    let mut workers = Vec::new();
    let mut waiting_groups = Vec::new();
    for group in source.groups {
        if matches!(
            group.key,
            GroupKey::Queued | GroupKey::Backlog | GroupKey::Ideas
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
        let target = match group.key {
            GroupKey::You => &mut needs,
            GroupKey::Corral => &mut verifying,
            _ => &mut workers,
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
        by_project(feed, needs, verifying, workers)
    } else {
        by_actor(needs, verifying, workers, slots(feed, filter))
    };
    let opens_at = sections
        .iter()
        .flat_map(|section| &section.rows)
        .find(|row| row.line.actor == crate::model::board::Actor::You)
        .map(|row| crate::model::Id::new(&row.line.id));
    NowList {
        sections,
        opens_at,
        waiting_groups,
        verifying: verifying_count,
        workers: worker_count,
        loading_rows: source.loading_rows,
        notice: source.notice,
    }
}

/// A board row with its named rail, selected only while the digest does not hold the panel.
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
        project: task.project().map(str::to_owned),
        project_chip: project_chip(feed, filter, task.project()),
        selected: form != Form::Phone
            && !state.digest_only
            && selection.task.as_ref() == Some(&task.id),
        action: row.action,
        line: row.line,
    })
}

pub(super) fn scoped_tasks<'a>(
    feed: &'a Feed,
    filter: &'a ProjectFilter,
) -> impl Iterator<Item = &'a daemon::Task> {
    feed.board
        .tasks()
        .iter()
        .filter(move |task| filter.matches(task.project()))
}

fn by_actor(
    needs: Vec<NowTaskRow>,
    verifying: Vec<NowTaskRow>,
    workers: Vec<NowTaskRow>,
    slots: Option<(String, String)>,
) -> Vec<NowSection> {
    [
        (
            "you",
            "Needs you",
            Hue::You,
            "Blocked, or reviewed and ready to land: you move next",
            needs,
            None,
        ),
        (
            "corral",
            "Corral",
            Hue::Corral,
            "Corral is gating or reviewing these tasks",
            verifying,
            None,
        ),
        (
            "workers",
            "Workers",
            Hue::Worker,
            "Workers are producing these tasks",
            workers,
            slots,
        ),
    ]
    .into_iter()
    .filter(|(_, _, _, _, rows, _)| !rows.is_empty())
    .map(|(key, label, hue, hint, rows, slots)| NowSection {
        key: key.into(),
        label: label.into(),
        count: rows.len(),
        hue,
        hint: match &slots {
            Some((_, sentence)) => format!("{}: {hint}. {sentence}", rows.len()),
            None => format!("{}: {hint}", rows.len()),
        },
        slots: slots.map(|(words, _)| words),
        rows,
    })
    .collect()
}

fn by_project(
    feed: &Feed,
    needs: Vec<NowTaskRow>,
    verifying: Vec<NowTaskRow>,
    workers: Vec<NowTaskRow>,
) -> Vec<NowSection> {
    // The leading bool keeps Unassigned after every real project, even one named Unassigned.
    let mut projects: BTreeMap<(bool, String), Vec<NowTaskRow>> = BTreeMap::new();
    for mut row in needs.into_iter().chain(verifying).chain(workers) {
        row.project_chip = None;
        projects
            .entry(project_key(row.project.as_deref()))
            .or_default()
            .push(row);
    }
    let limited = |project: &str| {
        let count = feed
            .health
            .as_ref()?
            .slots
            .as_ref()?
            .projects
            .get(project)?;
        Some(slot_words(count, &format!(" in {project}")))
    };
    projects
        .into_iter()
        .map(|((unassigned, label), rows)| {
            let slots = (!unassigned).then(|| limited(&label)).flatten();
            NowSection {
                key: if unassigned {
                    "unassigned".into()
                } else {
                    format!("project-{label}")
                },
                hint: match &slots {
                    Some((_, sentence)) => {
                        format!("{} moving tasks in {label}. {sentence}", rows.len())
                    }
                    None => format!("{} moving tasks in {label}", rows.len()),
                },
                slots: slots.map(|(words, _)| words),
                count: rows.len(),
                label,
                hue: Hue::Neutral,
                rows,
            }
        })
        .collect()
}

fn project_key(project: Option<&str>) -> (bool, String) {
    (project.is_none(), project.unwrap_or("Unassigned").into())
}

/// How many of a plan's children are in `statuses`.
fn children_in(plan: &corral_protocol::rows::PlanRow, statuses: &[Status]) -> u64 {
    plan.child_counts
        .iter()
        .filter(|count| statuses.contains(&count.status))
        .map(|count| count.count)
        .sum()
}

fn band(feed: &Feed, filter: &ProjectFilter, landed: Option<&Stat>) -> LoopBand {
    let mut plan_stats = Vec::new();
    for (key, wanted, icon, hue, phrase) in [
        (
            "needs-you",
            &[Status::Blocked][..],
            StatIcon::NeedsYou,
            Hue::You,
            "have a blocked child",
        ),
        (
            "moving",
            &[Status::Running, Status::Review][..],
            StatIcon::Moving,
            Hue::Corral,
            "have children moving",
        ),
    ] {
        let count = feed
            .plans
            .values()
            .filter(|plan| filter.matches(plan.parent.project.as_deref()))
            .filter(|plan| children_in(plan, wanted) > 0)
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
        hint: "Plans with blocked or moving children. Opens Plans in this scope.".into(),
        stats: plan_stats,
        to: Some(Destination::Tasks),
    }];
    for (index, key, label, to) in [
        (0, "run", "Run", Destination::Sessions),
        (1, "gate", "Gate", Destination::Tasks),
        (2, "review", "Review", Destination::Tasks),
    ] {
        let mut counts = [0_u64; 2];
        for task in scoped_tasks(feed, filter) {
            let actor = crate::model::board::daemon_actor(task.group);
            let rail = StageRail::daemon(task, actor);
            if rail.current != Some(index) {
                continue;
            }
            match task.group {
                daemon::Group::NeedsYou => counts[0] += 1,
                daemon::Group::Verifying | daemon::Group::Producing => counts[1] += 1,
                _ => {}
            }
        }
        let mut stats = Vec::new();
        for (index, stat_key, icon, hue, phrase) in [
            (0, "needs-you", StatIcon::NeedsYou, Hue::You, "wait on you"),
            (1, "moving", StatIcon::Moving, Hue::Corral, "are moving"),
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
        hint: "Landings since your last visit in loaded history.".into(),
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
