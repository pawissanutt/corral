//! The sample's Timeline tasks (`TimelinePage.tasks`), shaped as corrald serves them: every
//! task the window's runs and events name, their plans, those plans' other children and the
//! tasks any of these depend on, each with its title, status, plan, dependencies and last
//! landing (from the whole log, undone by a later reopen).

use std::collections::BTreeSet;

use corral_protocol::detail::{TaskDetail, TimelineTask};
use corral_types::{Event, EventKind, Outcome, Run};
use jiff::Timestamp;

use crate::model::daemon::Feed;

/// Dependencies the Timeline sample serves beside the records' own, so the fixture draws an
/// edge from a landing inside the window (T-2 into T-14), one from before it (T-5 into T-14)
/// and a dependent that started before its dependency landed (T-8 on T-2). They stay off the
/// task records so the sample engine's dispatch checks do not change.
const SHOWN_DEPENDENCIES: &[(&str, &[&str])] = &[("T-8", &["T-2"]), ("T-14", &["T-2", "T-5"])];

/// The tasks a Timeline reply of `runs` and `events` serves.
pub(super) fn tasks(
    feed: &Feed,
    records: &[TaskDetail],
    runs: &[Run],
    events: &[Event],
) -> Vec<TimelineTask> {
    let find = |id: &str| records.iter().find(|record| record.task.id == id);
    let mut wanted: BTreeSet<String> = runs
        .iter()
        .map(|run| run.task.clone())
        .chain(events.iter().filter_map(|event| event.task.clone()))
        .collect();
    let plans: BTreeSet<String> = wanted
        .iter()
        .filter_map(|id| find(id)?.task.parent.clone())
        .collect();
    wanted.extend(plans.iter().cloned());
    wanted.extend(
        records
            .iter()
            .filter(|record| {
                record
                    .task
                    .parent
                    .as_ref()
                    .is_some_and(|p| plans.contains(p))
            })
            .map(|record| record.task.id.clone()),
    );
    let dependencies: Vec<String> = wanted
        .iter()
        .flat_map(|id| depends_on(find(id), id))
        .collect();
    wanted.extend(dependencies);
    wanted
        .iter()
        .filter_map(|id| {
            let record = find(id)?;
            Some(TimelineTask {
                id: id.clone(),
                title: record.task.title.clone(),
                status: record.task.status,
                parent: record.task.parent.clone(),
                depends_on: depends_on(Some(record), id),
                landed: landed(feed, record),
            })
        })
        .collect()
}

fn depends_on(record: Option<&TaskDetail>, id: &str) -> Vec<String> {
    let mut out: Vec<String> = record
        .map(|record| record.task.depends_on.clone())
        .unwrap_or_default();
    for (task, more) in SHOWN_DEPENDENCIES {
        if *task == id {
            out.extend(more.iter().map(|d| (*d).to_owned()));
        }
    }
    out.dedup();
    out
}

/// The task's last landing in the whole log unless a later reopen undid it; a sample task whose
/// landing the log does not hold lands when its landed run ended.
fn landed(feed: &Feed, record: &TaskDetail) -> Option<Timestamp> {
    let mut log: Vec<&Event> = feed
        .changes
        .values()
        .filter(|event| event.task.as_deref() == Some(record.task.id.as_str()))
        .filter(|event| matches!(event.kind, EventKind::Landed | EventKind::Reopened))
        .collect();
    log.sort_by_key(|event| (event.ts, event.seq));
    match log.last() {
        Some(event) if event.kind == EventKind::Landed => Some(event.ts),
        Some(_) => None,
        None => record
            .runs
            .iter()
            .filter(|run| run.outcome == Some(Outcome::Landed))
            .filter_map(|run| run.ended)
            .max(),
    }
}
