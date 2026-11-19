//! The one-line candidate outline, scoped counts and selected-candidate identity.

use corral_types::{CheckKind, CheckResult};

use crate::model::board::rail_label;
use crate::model::daemon::{Feed, Group, Task};
use crate::model::effect::Effect;
use crate::model::glance::{Stat, StatIcon};
use crate::model::hue::Hue;
use crate::model::id::Id;
use crate::model::live::{Live, Selection};
use crate::model::projects::{ProjectFilter, project_chip};

use super::live::{Shown, candidates_filtered, selected_filtered, sha7};
use super::view::chip;
use super::{Action, Chip, State, Tab, TabView};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutlineRow {
    pub test_id: String,
    pub id: String,
    pub title: String,
    pub project_chip: Option<String>,
    pub selected: bool,
    pub action: Action,
    pub status: String,
    /// The status's first word, where a terminal row runs out of room (`ready`, `gate`).
    pub status_word: String,
    pub hue: Hue,
    pub hint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutlineGroup {
    pub label: &'static str,
    pub count: u32,
    pub hue: Hue,
    pub rows: Vec<OutlineRow>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateHeader {
    pub state: Chip,
    pub task: String,
    pub title: String,
    /// `round 2 of 3`; the limit once the review detail tells it.
    pub round: Option<String>,
    pub sha7: Option<String>,
    pub project_chip: Option<String>,
    /// Unknown until the review detail arrives, rather than a fabricated zero.
    pub files: Option<Stat>,
    pub added: Option<String>,
    pub deleted: Option<String>,
}

pub(super) fn one_line(text: &str) -> String {
    let mut line = String::with_capacity(text.len());
    for word in text.split_whitespace() {
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    line
}

/// Where a candidate stands, from its row's checks and `ready`.
fn status(task: &Task) -> (&'static str, Hue) {
    let failed = |check: Option<&corral_types::Check>| {
        check.is_some_and(|check| check.result == CheckResult::Fail)
    };
    if task.row.ready {
        ("ready to land", Hue::You)
    } else if failed(task.row.gate.as_ref()) {
        ("gate failed", Hue::Fail)
    } else if task.row.gate.is_none() {
        ("gate running", Hue::Corral)
    } else if failed(task.row.review.as_ref()) {
        ("review asks changes", Hue::Held)
    } else if task.row.review.is_none() {
        ("review running", Hue::Corral)
    } else {
        ("reviewed", Hue::Corral)
    }
}

/// Who moves a candidate next: the owner once corrald marked it ready, else verification.
fn yours(task: &Task) -> bool {
    task.group == Group::NeedsYou
}

const YOUR_VERDICT: &str = "Your verdict";

/// The scoped candidates in their two groups, the owner's verdict first; an empty group is
/// left out.
fn grouped<'a>(feed: &'a Feed, filter: &ProjectFilter) -> Vec<(&'static str, Hue, Vec<&'a Task>)> {
    let (mine, verifying): (Vec<&Task>, Vec<&Task>) = candidates_filtered(feed, filter)
        .into_iter()
        .partition(|task| yours(task));
    [
        (YOUR_VERDICT, Hue::You, mine),
        ("In verification", Hue::Corral, verifying),
    ]
    .into_iter()
    .filter(|(_, _, tasks)| !tasks.is_empty())
    .collect()
}

/// The rail as a sentence, after the candidate it describes.
fn row_hint(task: &Task) -> String {
    let rail = rail_label(task.rail.each_ref().map(|step| step.mark));
    match task.rail[0].sha7.as_deref() {
        Some(sha) => format!("Candidate {sha}: {rail}"),
        None => rail,
    }
}

/// One line per candidate, grouped and project-chipped as the scope needs.
/// `focus` is a pushed candidate; the outline itself always remains project-scoped.
pub fn outline_filtered(
    feed: &Feed,
    selection: &Selection,
    focus: Option<&Id>,
    filter: &ProjectFilter,
) -> Vec<OutlineGroup> {
    let chosen = focus.or_else(|| selected_filtered(selection, feed, filter).map(|task| &task.id));
    grouped(feed, filter)
        .into_iter()
        .map(|(label, hue, tasks)| OutlineGroup {
            label,
            count: u32::try_from(tasks.len()).unwrap_or(u32::MAX),
            hue,
            rows: tasks
                .into_iter()
                .map(|task| {
                    let (status, hue) = status(task);
                    OutlineRow {
                        test_id: format!("task-{}", task.id),
                        id: task.id.to_string(),
                        title: one_line(task.title()),
                        project_chip: project_chip(feed, filter, task.project()),
                        selected: chosen == Some(&task.id),
                        action: Action::Pick(task.id.clone()),
                        status: status.into(),
                        status_word: status.split(' ').next().unwrap_or(status).into(),
                        hue,
                        hint: row_hint(task),
                    }
                })
                .collect(),
        })
        .collect()
}

pub(super) fn outline_stats(groups: &[OutlineGroup]) -> Vec<Stat> {
    let count = |mine: bool| {
        groups
            .iter()
            .filter(|group| (group.label == YOUR_VERDICT) == mine)
            .map(|group| group.count)
            .sum::<u32>()
    };
    let (verdict, verifying) = (count(true), count(false));
    let mut stats = Vec::new();
    if verdict > 0 {
        stats.push(Stat::new(
            "verdict",
            StatIcon::NeedsYou,
            verdict.to_string(),
            Hue::You,
            format!(
                "{verdict} candidate{} on your verdict",
                if verdict == 1 { " waits" } else { "s wait" }
            ),
        ));
    }
    if verifying > 0 {
        stats.push(Stat::new(
            "verification",
            StatIcon::Moving,
            verifying.to_string(),
            Hue::Corral,
            format!(
                "{verifying} candidate{} in verification",
                if verifying == 1 { " is" } else { "s are" }
            ),
        ));
    }
    stats
}

pub fn candidate_header(feed: &Feed, task: &Task, filter: &ProjectFilter) -> CandidateHeader {
    let shown = Shown::of(feed, task);
    let (status, hue) = status(task);
    let round = task.task().round;
    let (files, added, deleted) = match shown.review.filter(|review| review.run.is_some()) {
        Some(review) => {
            let count = review.files.len();
            let (added, deleted) = review.files.iter().fold((0u64, 0u64), |(a, d), file| {
                (a + u64::from(file.added), d + u64::from(file.deleted))
            });
            (
                Some(Stat::new(
                    "files",
                    StatIcon::Files,
                    count.to_string(),
                    Hue::Neutral,
                    format!("{count} file{} changed", if count == 1 { "" } else { "s" }),
                )),
                Some(format!("+{added}")),
                Some(format!("−{deleted}")),
            )
        }
        None => (None, None, None),
    };
    CandidateHeader {
        state: chip(status, hue),
        task: task.id.to_string(),
        title: one_line(task.title()),
        round: (round > 0).then(|| {
            let max = task
                .row
                .max_rounds
                .or_else(|| shown.review.map(|review| review.max_rounds));
            match max {
                Some(max) => format!("round {round} of {max}"),
                None => format!("round {round}"),
            }
        }),
        sha7: shown.head().map(sha7),
        project_chip: project_chip(feed, filter, task.project()),
        files,
        added,
        deleted,
    }
}

fn has_summary(shown: &Shown) -> bool {
    shown
        .run()
        .and_then(|run| run.summary.as_deref())
        .is_some_and(|text| !text.trim().is_empty())
}

fn has_artifacts(shown: &Shown) -> bool {
    shown
        .detail
        .is_some_and(|detail| !detail.artifacts.is_empty())
}

fn has_log(shown: &Shown) -> bool {
    shown
        .check(CheckKind::Gate)
        .is_some_and(|gate| gate.log.is_some())
}

pub(super) fn selected_tab(state: &State, shown: &Shown) -> Tab {
    match state.tab {
        Tab::Report if !has_summary(shown) => Tab::Brief,
        Tab::Shot if !has_artifacts(shown) => Tab::Brief,
        Tab::Log if !has_log(shown) => Tab::Brief,
        tab => tab,
    }
}

/// The tabs this candidate has content for. When optional content disappears, Brief is
/// selected without destroying the remembered choice.
pub(super) fn candidate_tabs(state: &State, shown: &Shown) -> Vec<TabView> {
    let selected = selected_tab(state, shown);
    let mut specs = vec![(Tab::Brief, "Brief"), (Tab::Diff, "Diff")];
    if has_log(shown) {
        specs.push((Tab::Log, "Gate log"));
    }
    if has_summary(shown) {
        specs.push((Tab::Report, "Summary"));
    }
    if has_artifacts(shown) {
        specs.push((Tab::Shot, "Artifacts"));
    }
    specs
        .into_iter()
        .map(|(tab, label)| TabView {
            tab,
            label,
            selected: selected == tab,
            action: Action::Tab(tab),
        })
        .collect()
}

pub(super) fn navigation(action: &Action, shown: &Shown) -> Option<Vec<Effect>> {
    match action {
        Action::AskCandidate => Some(vec![Effect::AskAbout {
            context: Some(Live::Review(shown.id().clone()).context(None)),
            text: None,
        }]),
        Action::OpenSession => Some(
            shown
                .session()
                .map(|session| vec![Effect::OpenLive(Live::Session(Id::new(&session)))])
                .unwrap_or_default(),
        ),
        Action::OpenReviewer => Some(
            shown
                .task
                .row
                .reviewer
                .as_ref()
                .map(|run| {
                    let session = format!("{}_{}", shown.id(), run.id);
                    vec![Effect::OpenLive(Live::Session(Id::new(&session)))]
                })
                .unwrap_or_default(),
        ),
        _ => None,
    }
}
