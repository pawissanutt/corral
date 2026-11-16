//! The one-line candidate outline, scoped counts and selected-candidate identity.

use corral_protocol::detail::ReviewDetail;

use crate::model::board::{Mark, rail_label};
use crate::model::daemon::{Feed, Task};
use crate::model::effect::{Effect, Env};
use crate::model::glance::{Stat, StatIcon};
use crate::model::hue::Hue;
use crate::model::id::Id;
use crate::model::live::{Live, Selection};
use crate::model::projects::{ProjectFilter, project_chip};

use super::decide::{candidate_refusal, device_refusal};
use super::live::{Shown, Stage, candidates_filtered, selected_filtered, stage};
use super::view::{ButtonStyle, button, chip};
use super::{Action, Chip, DecideBar, State, Tab, TabView};

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
    pub round: Option<String>,
    pub sha7: Option<String>,
    pub project_chip: Option<String>,
    /// Unknown until the review detail arrives, rather than a fabricated zero.
    pub files: Option<Stat>,
    pub added: Option<String>,
    pub deleted: Option<String>,
}

pub(super) fn sha7(sha: &str) -> String {
    sha.chars().take(7).collect()
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
fn status(task: &Task) -> (&'static str, Hue) {
    match (task.rail[1].mark, task.rail[2].mark) {
        (Mark::Stale, _) => ("gate stale", Hue::Held),
        (Mark::Fail, _) => ("gate failed", Hue::Fail),
        (Mark::Active, _) => ("gate running", Hue::Corral),
        (_, Mark::Fail) => ("review asks changes", Hue::Held),
        (_, Mark::Stale) => ("review stale", Hue::Held),
        (_, Mark::Active) => ("review running", Hue::Corral),
        (Mark::Done | Mark::Na, Mark::Done | Mark::Na) => ("ready for verdict", Hue::You),
        _ => ("gating", Hue::Corral),
    }
}

/// The scoped candidates in their two groups, the owner's verdict first; an empty group is
/// left out.
fn grouped<'a>(feed: &'a Feed, filter: &ProjectFilter) -> Vec<(&'static str, Hue, Vec<&'a Task>)> {
    let all = candidates_filtered(feed, filter);
    [
        ("Your verdict", Hue::You, Stage::YourVerdict),
        ("In verification", Hue::Corral, Stage::Verifying),
    ]
    .into_iter()
    .filter_map(|(label, hue, wanted)| {
        let tasks: Vec<&Task> = all
            .iter()
            .copied()
            .filter(|task| stage(feed, task) == Some(wanted))
            .collect();
        (!tasks.is_empty()).then_some((label, hue, tasks))
    })
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
                        title: one_line(&task.title),
                        project_chip: project_chip(feed, filter, task.project.as_deref()),
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

/// Counts use the same scoped grouping as the outline; only the selected candidate's
/// decide bar can explain canonical approval eligibility.
pub fn page_stats(feed: &Feed, filter: &ProjectFilter) -> Vec<Stat> {
    let mut verdict = 0;
    let mut verifying = 0;
    for (_, hue, tasks) in grouped(feed, filter) {
        let count = u32::try_from(tasks.len()).unwrap_or(u32::MAX);
        if hue == Hue::You {
            verdict = count;
        } else {
            verifying = count;
        }
    }
    count_stats(verdict, verifying)
}

pub(super) fn outline_stats(groups: &[OutlineGroup]) -> Vec<Stat> {
    let mut verdict = 0;
    let mut verifying = 0;
    for group in groups {
        if group.label == "Your verdict" {
            verdict = group.count;
        } else {
            verifying = group.count;
        }
    }
    count_stats(verdict, verifying)
}

fn count_stats(verdict: u32, verifying: u32) -> Vec<Stat> {
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
    let round = shown
        .run()
        .filter(|run| {
            run.strip_prefix('R').is_some_and(|digits| {
                !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
            })
        })
        .map(str::to_owned);
    let (files, added, deleted) = match shown.review {
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
        title: one_line(&task.title),
        round,
        sha7: shown.sha().map(sha7),
        project_chip: project_chip(feed, filter, task.project.as_deref()),
        files,
        added,
        deleted,
    }
}

pub(super) fn selected_tab(state: &State, review: Option<&ReviewDetail>) -> Tab {
    match state.tab {
        Tab::Report if !has_report(review) => Tab::Brief,
        Tab::Shot if !review.is_some_and(|review| !review.artifacts.is_empty()) => Tab::Brief,
        tab => tab,
    }
}

fn has_report(review: Option<&ReviewDetail>) -> bool {
    review.is_some_and(|review| {
        review
            .report
            .as_deref()
            .is_some_and(|text| !text.trim().is_empty())
            || review.acceptance.iter().any(|item| !item.trim().is_empty())
    })
}

/// Old remembered Diff/Report/Shot/Log values still identify their original content. When
/// optional content disappears, Brief is selected without destroying the remembered choice.
pub fn candidate_tabs(state: &State, review: Option<&ReviewDetail>) -> Vec<TabView> {
    let selected = selected_tab(state, review);
    let mut specs = vec![
        (Tab::Brief, "Brief"),
        (Tab::Diff, "Diff"),
        (Tab::Log, "Gate log"),
    ];
    if has_report(review) {
        specs.push((Tab::Report, "Report"));
    }
    if review.is_some_and(|review| !review.artifacts.is_empty()) {
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

/// Shared with the existing approval guard: this changes its explanation, never eligibility.
pub(super) fn approval_unavailable(shown: &Shown) -> String {
    if matches!(shown.task.rail[1].mark, Mark::Active | Mark::Next) {
        "Still verifying: the gate has not passed this candidate".into()
    } else {
        format!(
            "Approval is unavailable in state {}.",
            shown
                .review
                .map_or("unknown", |review| review.state.as_str())
                .replace('_', " ")
        )
    }
}

/// The existing candidate-bound decision path, with terse labels, real disabled states and Ask.
pub fn candidate_decide_bar(
    state: &State,
    feed: &Feed,
    task: Option<&Task>,
    env: Env,
) -> DecideBar {
    let shown = task.map(|task| Shown::of(feed, task));
    let mut bar = super::decide::decide_bar(state, feed, shown.as_ref(), env);
    let Some(shown) = shown else {
        return bar;
    };
    if bar
        .buttons
        .first()
        .is_some_and(|control| matches!(control.action, Action::RequestChanges))
    {
        bar.buttons.reverse();
    }
    for control in &mut bar.buttons {
        match control.action {
            Action::Approve => {
                control.label = shown.sha().map_or_else(
                    || "Approve unknown candidate".into(),
                    |sha| format!("Approve {}", sha7(sha)),
                );
            }
            Action::RequestChanges => {
                control.label = "Request changes".into();
                control.short = Some("Request");
                control.refusal = device_refusal(feed, env)
                    .or_else(|| candidate_refusal(&shown))
                    .or_else(|| {
                        shown.identity().is_none().then(|| {
                            "Waiting for the candidate's identity; no composer can be opened."
                                .into()
                        })
                    });
            }
            _ => {}
        }
        control.enabled = control.refusal.is_none();
    }
    bar.buttons
        .push(button("Ask", Action::AskCandidate, ButtonStyle::Quiet));
    bar
}

pub(super) fn navigation(action: &Action, shown: &Shown) -> Option<Vec<Effect>> {
    match action {
        Action::AskCandidate => Some(vec![Effect::AskAbout {
            context: Some(Live::Review(shown.id().clone()).context(None)),
            text: None,
            target: None,
        }]),
        Action::OpenSession => Some(
            shown
                .detail
                .and_then(|detail| detail.run.as_ref())
                .and_then(|run| run.session.as_deref())
                .map(|session| vec![Effect::OpenLive(Live::Session(Id::new(session)))])
                .unwrap_or_default(),
        ),
        _ => None,
    }
}
