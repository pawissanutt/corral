//! Plan groups and dependency edges over the by-task lanes, from the reply's `tasks`. An older
//! daemon serves no `tasks`; then no lane has a plan or a dependency, so the lanes stay one flat
//! list without edges, as before corrald served them.

use std::collections::BTreeMap;

use corral_protocol::detail::{TimelinePage, TimelineTask};
use corral_types::Status;
use jiff::Timestamp;

use super::PlotLane;
use super::live::Window;
use crate::model::daemon::Feed;
use crate::model::work::when;

/// The key of the group of lanes whose task belongs to no plan.
pub const UNPLANNED: &str = "unplanned";

/// The served tasks by id.
pub type Tasks<'a> = BTreeMap<&'a str, &'a TimelineTask>;

pub fn index(page: &TimelinePage) -> Tasks<'_> {
    page.tasks
        .iter()
        .map(|task| (task.id.as_str(), task))
        .collect()
}

/// A plan's lanes under one header, or the lanes of no plan under "Unplanned".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanGroup {
    /// The plan's id, or [`UNPLANNED`]: what folds it.
    pub key: String,
    /// The plan's id; none for Unplanned.
    pub id: Option<String>,
    pub title: String,
    /// `3/5 landed`, over every child of the plan corrald served (Unplanned: over its lanes).
    pub tally: String,
    pub landed: usize,
    pub total: usize,
    /// The earliest start and latest end of its lanes' segments, in plot minutes.
    pub span: Option<(i64, i64)>,
    /// Its lanes' ids, in the order the lanes are drawn.
    pub lanes: Vec<String>,
}

impl PlanGroup {
    /// The landed share of the plan, 0 to 1.
    pub fn share(&self) -> f32 {
        if self.total == 0 {
            0.0
        } else {
            self.landed as f32 / self.total as f32
        }
    }

    /// What a screen reader hears of the header; a title that is only the id is said once.
    pub fn spoken(&self) -> String {
        match &self.id {
            Some(id) if *id == self.title => format!("Plan {id}, {}", self.tally),
            Some(id) => format!("Plan {id} {}, {}", self.title, self.tally),
            None => format!("{}, {}", self.title, self.tally),
        }
    }
}

/// The plan whose group a lane's task goes in. A task that is itself a plan (a served task
/// names it as parent) heads its own group, so a plan nested in another keeps its children
/// under it; any other task goes under its parent, the nearest plan above it, which heads a
/// group whether corrald served the parent's own row or only its id.
fn plan_of(task: &str, tasks: &Tasks) -> Option<String> {
    if tasks.values().any(|t| t.parent.as_deref() == Some(task)) {
        return Some(task.to_owned());
    }
    tasks.get(task).and_then(|t| t.parent.clone())
}

/// The lanes regrouped under their plans, groups in the order of their first lane (lanes come
/// ordered by first activity), a plan's own lane first in its group, Unplanned last; none and
/// the lanes untouched when no lane's task has a plan. `title` names a plan; a plan it cannot
/// name is headed by its id.
pub fn group(
    lanes: Vec<PlotLane>,
    tasks: &Tasks,
    title: impl Fn(&str) -> String,
) -> (Vec<PlotLane>, Vec<PlanGroup>) {
    let plans: Vec<Option<String>> = lanes.iter().map(|lane| plan_of(&lane.id, tasks)).collect();
    if plans.iter().all(Option::is_none) {
        return (lanes, Vec::new());
    }
    let mut order: Vec<Option<String>> = Vec::new();
    for plan in plans.iter().flatten() {
        if !order.iter().flatten().any(|known| known == plan) {
            order.push(Some(plan.clone()));
        }
    }
    if plans.iter().any(Option::is_none) {
        order.push(None);
    }
    let mut placed: Vec<Option<PlotLane>> = lanes.into_iter().map(Some).collect();
    let mut out = Vec::with_capacity(placed.len());
    let mut groups = Vec::with_capacity(order.len());
    for plan in order {
        let mut members: Vec<PlotLane> = plans
            .iter()
            .zip(placed.iter_mut())
            .filter(|(of, _)| **of == plan)
            .filter_map(|(_, lane)| lane.take())
            .collect();
        // Stable: the plan's own lane leads, its children keep their order.
        members.sort_by_key(|lane| plan.as_deref() != Some(lane.id.as_str()));
        let (landed, total) = match &plan {
            Some(plan) => {
                let children: Vec<&&TimelineTask> = tasks
                    .values()
                    .filter(|t| t.parent.as_deref() == Some(plan.as_str()))
                    .collect();
                let landed = children.iter().filter(|t| t.landed.is_some()).count();
                (landed, children.len())
            }
            None => {
                let landed = members
                    .iter()
                    .filter(|lane| {
                        tasks
                            .get(lane.id.as_str())
                            .is_some_and(|t| t.landed.is_some())
                    })
                    .count();
                (landed, members.len())
            }
        };
        let span = members.iter().flat_map(|lane| &lane.segments).fold(
            None,
            |span: Option<(i64, i64)>, s| {
                Some(span.map_or((s.start, s.end), |(a, b)| (a.min(s.start), b.max(s.end))))
            },
        );
        groups.push(PlanGroup {
            key: plan.clone().unwrap_or_else(|| UNPLANNED.to_owned()),
            title: match &plan {
                Some(plan) => Some(title(plan))
                    .filter(|named| !named.is_empty())
                    .unwrap_or_else(|| plan.clone()),
                None => "Unplanned".to_owned(),
            },
            id: plan,
            tally: format!("{landed}/{total} landed"),
            landed,
            total,
            span,
            lanes: members.iter().map(|lane| lane.id.clone()).collect(),
        });
        out.extend(members);
    }
    (out, groups)
}

/// Where a dependency stands for its dependent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Landing {
    /// Landed inside the window, at this instant.
    InWindow(Timestamp),
    /// Landed before the window starts.
    Before,
    /// Done without a landing corrald recorded (closed before landings were recorded, or
    /// done without landing): nothing to have started early for.
    DoneUnrecorded,
    /// Dropped: it will not land, so waiting on it means nothing.
    Dropped,
    /// Not landed yet, or reopened since.
    NotLanded,
}

impl Landing {
    pub fn of(task: &TimelineTask, window: &Window) -> Self {
        match (task.landed, task.status) {
            (Some(at), _) if at >= window.from => Self::InWindow(at),
            (Some(_), _) => Self::Before,
            (None, Status::Done) => Self::DoneUnrecorded,
            (None, Status::Dropped) => Self::Dropped,
            (None, _) => Self::NotLanded,
        }
    }

    /// The words of a dependency with no landing instant; none for one that landed.
    fn unlanded(self) -> Option<&'static str> {
        match self {
            Self::InWindow(_) | Self::Before => None,
            Self::DoneUnrecorded => Some("done, landing not recorded"),
            Self::Dropped => Some("dropped"),
            Self::NotLanded => Some("not landed"),
        }
    }

    /// `landed 14:20`, `landed before this window`, `dropped`, `not landed`.
    pub fn words(self, feed: &Feed) -> String {
        match self {
            Self::InWindow(at) => format!("landed {}", when(&at.to_string(), feed)),
            Self::Before => "landed before this window".to_owned(),
            other => other.unlanded().unwrap_or_default().to_owned(),
        }
    }
}

/// A dependent that started before its dependency landed, or while it has not landed, is out
/// of order: its work may rest on what was not there yet. A dependency dropped, or done without
/// a recorded landing, has no landing to be early for, so it never makes its dependent so.
pub fn out_of_order(started: Timestamp, dependency: &TimelineTask) -> bool {
    match dependency.landed {
        Some(landed) => started < landed,
        None => !matches!(dependency.status, Status::Done | Status::Dropped),
    }
}

/// What a lane carries when it started out of order: `started before T-852 landed`.
pub fn early_hint(dependency: &str) -> String {
    format!("started before {dependency} landed")
}

/// One dependency edge: from the dependency's landing to its dependent's first run in the
/// window, in plot minutes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edge {
    /// The dependency.
    pub from: String,
    /// The dependent.
    pub to: String,
    pub from_at: i64,
    pub to_at: i64,
    pub landing: Landing,
    pub out_of_order: bool,
    /// What a screen reader and the hover hear.
    pub label: String,
}

/// The edge of every dependency of a lane's task that ran in the window. The edge starts at the
/// dependency's landing (the chart's left edge when it landed before the window; its lane's
/// last segment end when it has not landed) and ends where the dependent's first run in the
/// window started. A dependency corrald did not serve has no edge.
pub fn edges(
    page: &TimelinePage,
    window: &Window,
    lanes: &[PlotLane],
    tasks: &Tasks,
    (start, now): (i64, i64),
) -> Vec<Edge> {
    let clamp = |m: i64| m.clamp(start, now);
    let mut out = Vec::new();
    for lane in lanes {
        let Some(task) = tasks.get(lane.id.as_str()) else {
            continue;
        };
        // A dependent that started before the window is classified by its first run in the window.
        let Some(first) = page
            .runs
            .iter()
            .filter(|run| run.task == lane.id)
            .map(|run| run.started)
            .min()
        else {
            continue;
        };
        for dependency in &task.depends_on {
            let Some(dep) = tasks.get(dependency.as_str()) else {
                continue;
            };
            let landing = Landing::of(dep, window);
            let from_at = match landing {
                Landing::InWindow(at) => clamp(window.minutes(at)),
                Landing::Before => start,
                Landing::DoneUnrecorded | Landing::Dropped | Landing::NotLanded => lanes
                    .iter()
                    .find(|l| l.id == *dependency)
                    .and_then(|l| l.segments.iter().map(|s| s.end).max())
                    .map_or(start, clamp),
            };
            let early = out_of_order(first, dep);
            let label = match landing.unlanded() {
                _ if early => format!("{} {}", lane.id, early_hint(dependency)),
                Some(words) => format!("{} depends on {dependency}, {words}", lane.id),
                None => format!("{} started after {dependency} landed", lane.id),
            };
            out.push(Edge {
                from: dependency.clone(),
                to: lane.id.clone(),
                from_at,
                to_at: clamp(window.minutes(first)),
                landing,
                out_of_order: early,
                label,
            });
        }
    }
    out
}

/// Each out-of-order edge's hint on its dependent's lane.
pub fn mark_early(lanes: &mut [PlotLane], edges: &[Edge]) {
    for lane in lanes {
        let hints: Vec<String> = edges
            .iter()
            .filter(|edge| edge.out_of_order && edge.to == lane.id)
            .map(|edge| early_hint(&edge.from))
            .collect();
        lane.warning = (!hints.is_empty()).then(|| hints.join(" · "));
    }
}

/// Whether an edge is drawn: an out-of-order one always, any other only while one of its ends
/// is the selected lane, so the chart is not crossed by every dependency at once.
pub fn edge_shown(edge: &Edge, selected: Option<&str>) -> bool {
    edge.out_of_order || selected.is_some_and(|lane| lane == edge.from || lane == edge.to)
}

#[cfg(test)]
#[path = "plans_tests.rs"]
mod tests;
