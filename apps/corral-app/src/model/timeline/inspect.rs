//! The lane inspector: the selected lane's task as the Timeline reply knows it (status and
//! plan from `tasks`, who ran it and for how long from its runs, what it waits on and
//! unblocks, its gaps and last events in the window). Every fact is the reply's; a fact an
//! older daemon does not serve is left out, not guessed.

use std::collections::BTreeSet;

use corral_protocol::detail::TimelinePage;
use corral_types::{Event, Status};

use super::live::{Window, duration, lane_gaps, moved, task_title};
use super::plans::{self, Landing};
use super::{Gap, Opens, Plot};
use crate::model::daemon::Feed;
use crate::model::status::status_word;
use crate::model::work::when;

/// How many of the lane's latest events the inspector lists.
const EVENT_LIMIT: usize = 4;

/// A task the selected one waits on or unblocks, with where it stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub id: String,
    pub title: String,
    /// `landed 14:20`, `landed before this window`, `not landed`; a dependent's status.
    pub words: String,
    /// Not landed: the selected task ran ahead of it.
    pub danger: bool,
}

/// What the inspector shows of the selected lane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inspector {
    pub lane: String,
    pub task: String,
    pub title: String,
    pub status: Option<Status>,
    /// `! started before T-852 landed`.
    pub warning: Option<String>,
    /// Label and value: plan, who ran it, first start, elapsed against running.
    pub facts: Vec<(&'static str, String)>,
    pub waits_title: String,
    pub waits: Vec<Link>,
    pub unblocks_title: String,
    pub unblocks: Vec<Link>,
    pub gaps_title: &'static str,
    pub gaps: Vec<Gap>,
    pub events_title: &'static str,
    /// `14:20 landed`, newest first.
    pub events: Vec<String>,
    pub open: Opens,
}

/// A lane's task: the lane itself by task, the run's task by run (`T-1/R2`).
fn task_of(lane: &str) -> &str {
    lane.split_once('/').map_or(lane, |(task, _)| task)
}

/// The inspector of lane `lane`; none when the plot has no such lane.
pub fn inspect(
    page: &TimelinePage,
    window: &Window,
    plot: &Plot,
    lane: &str,
    feed: &Feed,
) -> Option<Inspector> {
    let placed = plot.lanes.iter().find(|l| l.id == lane)?;
    let task = task_of(lane);
    let tasks = plans::index(page);
    let served = tasks.get(task);
    let runs: Vec<_> = page
        .runs
        .iter()
        .filter(|run| run.task == task && (task == lane || lane.ends_with(&format!("/{}", run.id))))
        .collect();
    let mut facts = Vec::new();
    if let Some(plan) = served.and_then(|t| t.parent.as_deref()) {
        let title = task_title(plan, &tasks, feed);
        facts.push(("Plan", format!("{plan} {title}").trim_end().to_owned()));
    }
    let profiles: BTreeSet<&str> = runs.iter().map(|run| run.profile.as_str()).collect();
    if !profiles.is_empty() {
        let who: Vec<&str> = profiles.into_iter().collect();
        facts.push(("Ran by", who.join(", ")));
    }
    let now = window.now;
    if let Some(first) = runs.iter().map(|run| run.started).min() {
        facts.push(("First start", when(&first.to_string(), feed)));
        let last = runs
            .iter()
            .map(|run| run.ended.unwrap_or(now).min(now))
            .max()
            .unwrap_or(now);
        let seconds =
            |from: jiff::Timestamp, to: jiff::Timestamp| (to.as_second() - from.as_second()).max(0);
        let running: i64 = runs
            .iter()
            .map(|run| seconds(run.started, run.ended.unwrap_or(now).min(now)))
            .sum();
        let elapsed = seconds(first, last);
        facts.push((
            "Elapsed",
            format!(
                "{} · {} running",
                duration(elapsed / 60),
                duration(running / 60)
            ),
        ));
    }
    let dependencies: &[String] = served.map_or(&[], |t| t.depends_on.as_slice());
    // The same start the lane's edges classify by, so the danger here matches the lane's `!`.
    let task_start = page
        .runs
        .iter()
        .filter(|run| run.task == task)
        .map(|run| run.started)
        .min();
    let waits: Vec<Link> = dependencies
        .iter()
        .map(|id| match tasks.get(id.as_str()) {
            Some(dep) => Link {
                id: id.clone(),
                title: dep.title.clone(),
                words: Landing::of(dep, window).words(feed),
                danger: task_start.is_some_and(|started| plans::out_of_order(started, dep)),
            },
            None => Link {
                id: id.clone(),
                title: String::new(),
                words: "not served by corrald".to_owned(),
                danger: false,
            },
        })
        .collect();
    let unblocks: Vec<Link> = page
        .tasks
        .iter()
        .filter(|t| t.depends_on.iter().any(|d| d == task))
        .map(|t| Link {
            id: t.id.clone(),
            title: t.title.clone(),
            words: status_word(t.status).to_lowercase(),
            danger: false,
        })
        .collect();
    let mut events: Vec<&Event> = page
        .events
        .iter()
        .filter(|event| event.task.as_deref() == Some(task))
        .collect();
    events.sort_by_key(|event| std::cmp::Reverse((event.ts, event.seq)));
    let events = events
        .into_iter()
        .take(EVENT_LIMIT)
        .map(|event| {
            format!(
                "{} {}",
                when(&event.ts.to_string(), feed),
                moved(event.kind)
            )
        })
        .collect();
    Some(Inspector {
        lane: lane.to_owned(),
        task: task.to_owned(),
        title: task_title(task, &tasks, feed),
        status: served.map(|t| t.status),
        warning: placed.warning.as_deref().map(super::words::flagged),
        facts,
        waits_title: format!("Waits on {}", waits.len()),
        waits,
        // corrald serves only the dependents among this reply's tasks, not every task waiting
        // on this one.
        unblocks_title: format!("Unblocks {} in this view", unblocks.len()),
        unblocks,
        gaps_title: "Gaps on this lane",
        gaps: lane_gaps(plot, window, feed, lane),
        events_title: "Latest events",
        events,
        // What the lane's second press opens: by run, the run's session rather than its task.
        open: placed.opens.clone(),
    })
}
