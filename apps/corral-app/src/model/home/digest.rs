//! What happened since this device's last visit, read from the engine's events the client
//! holds (the same ones Activity › Feed lists): the tasks that finished, what the Landed tile
//! counts, and the latest events in scope. Counts cover the loaded events only; nothing is
//! inferred from the board.

use std::collections::BTreeSet;

use corral_protocol::detail::DetailBody;
use corral_types::{Event, EventKind, Status};

use super::HomeRow;
use crate::model::daemon::Feed;
use crate::model::projects::Scope;
use crate::model::status::{status_look, status_word};
use crate::model::work::{event_project, event_text, stamp_at, when};
use crate::model::{Id, Live};

/// At most this many finished tasks are listed; the rest are in Activity › Feed.
const FINISHED_MOST: usize = 8;

/// The since-visit digest of Home in one scope.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Since {
    /// When the last visit was: `09:16` today, `Nov 18 09:16` before; none before this device
    /// has a recorded visit.
    pub at: Option<String>,
    /// Tasks that landed, were dropped or were created, and forced operations, since then.
    pub landed: usize,
    pub dropped: usize,
    pub created: usize,
    pub forced: usize,
    /// Tasks that landed or were dropped since then and are not open again, newest first.
    pub finished: Vec<Id>,
}

/// One engine event as the Latest events panel lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventLine {
    /// `18:40` today, `Nov 12` before.
    pub at: String,
    /// The task the event is about; empty for a workspace event.
    pub id: String,
    pub text: String,
    /// The page the line opens.
    pub live: Option<Live>,
}

/// Whether an event belongs to the scope: its project, else its open task's; an event whose
/// project is unknown shows only with every project in scope.
fn in_scope(event: &Event, feed: &Feed, scope: &Scope) -> bool {
    match (scope, event_project(event, feed)) {
        (Scope::All, _) => true,
        (_, Some(project)) => scope.matches(project),
        (_, None) => false,
    }
}

/// The events after the last visit, newest first.
fn unseen(feed: &Feed) -> impl Iterator<Item = &Event> {
    let through = feed.seen.as_ref().map_or(0, |seen| seen.through);
    feed.changes
        .range((
            std::ops::Bound::Excluded(through),
            std::ops::Bound::Unbounded,
        ))
        .rev()
        .map(|(_, event)| event)
}

/// What happened in scope since the last visit.
pub fn since(feed: &Feed, scope: &Scope) -> Since {
    let at = feed
        .seen
        .as_ref()
        .and_then(|seen| seen.at.as_deref())
        .map(|at| when(at, feed))
        .filter(|at| !at.is_empty());
    let mut landed = BTreeSet::new();
    let mut dropped = BTreeSet::new();
    let mut created = BTreeSet::new();
    let mut forced = 0;
    let mut finished: Vec<Id> = Vec::new();
    for event in unseen(feed).filter(|event| in_scope(event, feed, scope)) {
        let Some(task) = event.task.as_deref() else {
            continue;
        };
        match event.kind {
            EventKind::Landed => _ = landed.insert(task),
            EventKind::Dropped => _ = dropped.insert(task),
            EventKind::Created => _ = created.insert(task),
            EventKind::Forced => forced += 1,
            _ => {}
        }
        // A task reopened since is open again: it is listed where it stands now.
        if matches!(event.kind, EventKind::Landed | EventKind::Dropped)
            && feed.board.task(task).is_none()
            && finished.len() < FINISHED_MOST
            && !finished.iter().any(|id| id.as_str() == task)
        {
            finished.push(Id::new(task));
        }
    }
    Since {
        at,
        landed: landed.len(),
        dropped: dropped.len(),
        created: created.len(),
        forced,
        finished,
    }
}

/// The newest `most` events in scope, seen or not.
pub fn latest(feed: &Feed, scope: &Scope, most: usize) -> Vec<EventLine> {
    feed.changes
        .values()
        .rev()
        .filter(|event| in_scope(event, feed, scope))
        .take(most)
        .map(|event| EventLine {
            at: stamp_at(event.ts, feed),
            id: event.task.clone().unwrap_or_default(),
            text: event_text(event),
            live: event.task.as_deref().map(|task| Live::Task(Id::new(task))),
        })
        .collect()
}

/// A closed task's title from a loaded page of closed tasks; the open-task list does not hold
/// it, and the event names none.
fn closed_title(feed: &Feed, id: &str) -> Option<String> {
    feed.details.values().find_map(|cache| match cache.body() {
        Some(DetailBody::Closed(page)) => page
            .rows
            .iter()
            .find(|task| task.id == id)
            .map(|task| task.title.clone()),
        _ => None,
    })
}

/// The row of a task that finished: its status from the event that closed it, what that event
/// says and when.
pub(super) fn finished_row(feed: &Feed, id: &Id) -> Option<HomeRow> {
    let event = feed.changes.values().rev().find(|event| {
        event.task.as_deref() == Some(id.as_str())
            && matches!(event.kind, EventKind::Landed | EventKind::Dropped)
    })?;
    let status = if event.kind == EventKind::Landed {
        Status::Done
    } else {
        Status::Dropped
    };
    let at = stamp_at(event.ts, feed);
    Some(HomeRow {
        id: id.clone(),
        live: Live::Task(id.clone()),
        look: status_look(status),
        word: status_word(status),
        title: closed_title(feed, id).unwrap_or_default(),
        now: [event_text(event), at]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" · "),
        next: None,
        in_place: false,
    })
}

#[cfg(test)]
mod tests {
    use corral_protocol::app::SeenCursor;
    use corral_types::Actor;

    use super::*;
    use crate::model::daemon::{Task, bare_task, task_row};

    fn event(seq: u64, task: &str, project: &str, kind: EventKind) -> Event {
        Event {
            seq,
            ts: jiff::Timestamp::UNIX_EPOCH,
            task: Some(task.into()),
            run: None,
            project: Some(project.into()),
            kind,
            actor: Actor::Auto,
            detail: serde_json::json!({}),
        }
    }

    /// Just finished listed what landed before the last visit, and a task reopened since it
    /// landed twice: once finished and once where it stands now.
    #[test]
    fn just_finished_is_what_closed_since_the_visit_and_is_not_open_again() {
        let mut feed = Feed::default();
        for (seq, task, kind) in [
            (1, "T-1", EventKind::Landed),
            (2, "T-2", EventKind::Landed),
            (3, "T-3", EventKind::Dropped),
            (4, "T-4", EventKind::Landed),
            (5, "T-4", EventKind::Reopened),
            (6, "T-5", EventKind::Landed),
        ] {
            let project = if task == "T-5" { "web" } else { "app" };
            feed.changes.insert(seq, event(seq, task, project, kind));
        }
        feed.seen = Some(SeenCursor {
            through: 1,
            ..SeenCursor::default()
        });
        let reopened = bare_task("T-4", "T-4", Status::Backlog, jiff::Timestamp::UNIX_EPOCH);
        feed.board
            .apply_batch(1, vec![Task::new(task_row(reopened))], &[]);
        let app = Scope::Project(Id::from("app"));
        let scoped = since(&feed, &app);
        assert_eq!(scoped.finished, [Id::from("T-3"), Id::from("T-2")]);
        assert_eq!((scoped.landed, scoped.dropped), (2, 1));
        assert_eq!(since(&feed, &Scope::All).finished.len(), 3);
        let row = finished_row(&feed, &Id::from("T-3")).expect("a dropped task's row");
        assert_eq!(row.word, "Dropped");
    }
}
