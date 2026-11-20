//! Activity › Feed: every engine event in the one project scope, newest first, grouped by the
//! local day it happened on, each row opening its task; older pages load on request, and Mark
//! seen moves this device's own seen cursor, which Home's "since your last visit" counts from.

use corral_types::{Event, EventKind};
use jiff::civil::Date;

use crate::model::daemon::{Feed, Link};
use crate::model::projects::Scope;
use crate::model::{Id, Live};

use super::{
    Button, ButtonKind, Press, Status, actor_word, changes_loading, changes_notice, daemon_status,
    day_label, event_detail, event_project, local_day,
};

/// The Feed's title and what it lists.
pub const FEED_TITLE: &str = "Feed";
pub const FEED_SUB: &str = "every event, newest first";

/// One event: its local time, the engine's word for its kind, its task, what it says and who
/// did it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedEvent {
    /// `18:40`, local.
    pub at: String,
    /// The engine's kind, `review_started`.
    pub kind: &'static str,
    /// A forced operation: its kind reads in the failure colour, so it is not read past.
    pub forced: bool,
    /// The task the event concerns; empty for an event about none.
    pub id: String,
    pub text: String,
    pub who: &'static str,
    /// The page the row opens: the task's.
    pub open: Option<Live>,
    /// The row's test id: `event-<seq>`.
    pub test_id: String,
}

impl FeedEvent {
    /// What a screen reader hears: time, kind, task, event and attribution.
    pub fn spoken(&self) -> String {
        [
            self.at.as_str(),
            self.kind,
            self.id.as_str(),
            self.text.as_str(),
            self.who,
        ]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
    }
}

/// The events of one local day.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedDay {
    /// `Today`, `Yesterday`, `Wed 18 Nov`.
    pub label: String,
    pub events: Vec<FeedEvent>,
}

/// The whole Feed page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedPage {
    pub days: Vec<FeedDay>,
    /// Move this device's seen cursor through the newest event, while corrald serves changes.
    pub mark_seen: Option<Button>,
    /// Fetch the page before the oldest row shown, while corrald has more.
    pub older: Option<Button>,
    /// One line instead of the rows: not served, loading, failed or none in scope.
    pub notice: Option<String>,
    pub status: Status,
}

/// The project an event is filed under (see [`event_project`]); an event with none known, a
/// workspace event or one of a task nothing loaded names, stays in every scope rather than
/// silently dropping from the retained history.
fn in_scope(event: &Event, feed: &Feed, scope: &Scope) -> bool {
    event_project(event, feed).is_none_or(|project| scope.matches(project))
}

fn event_row(event: &Event, at: String) -> FeedEvent {
    FeedEvent {
        at,
        kind: event.kind.as_str(),
        forced: event.kind == EventKind::Forced,
        id: event.task.clone().unwrap_or_default(),
        text: event_detail(event),
        who: actor_word(event.actor),
        open: event.task.as_deref().map(|task| Live::Task(Id::new(task))),
        test_id: format!("event-{}", event.seq),
    }
}

/// The Feed in `scope`: every loaded event, newest first, by local day.
pub fn feed_page(feed: &Feed, scope: &Scope) -> FeedPage {
    let scope = scope.resolved(feed);
    let mut days: Vec<(Date, Vec<FeedEvent>)> = Vec::new();
    for event in feed
        .changes
        .values()
        .rev()
        .filter(|event| in_scope(event, feed, &scope))
    {
        let (day, at) = local_day(event.ts, feed);
        match days.last_mut() {
            Some((last, events)) if *last == day => events.push(event_row(event, at)),
            _ => days.push((day, vec![event_row(event, at)])),
        }
    }
    let up = feed.link == Link::Up;
    let loading = changes_loading(feed);
    let through = feed.seen.as_ref().map_or(0, |seen| seen.through);
    let head = feed
        .changes
        .keys()
        .next_back()
        .copied()
        .unwrap_or(0)
        .max(feed.changes_head);
    let notice = changes_notice(feed)
        .map(|notice| notice.text().to_owned())
        .or_else(|| {
            days.is_empty()
                .then(|| "No events in this scope yet.".to_owned())
        });
    FeedPage {
        days: days
            .into_iter()
            .map(|(day, events)| FeedDay {
                label: day_label(day, feed),
                events,
            })
            .collect(),
        // Each device keeps its own cursor: marking seen here leaves the others' marks.
        mark_seen: feed.capable(corral_protocol::app::CHANGES_V1).then(|| {
            if head > through {
                Button::new("Mark seen", Press::Seen(head), ButtonKind::Ghost).enabled(up)
            } else {
                Button::new("All seen", Press::Seen(head), ButtonKind::Ghost).enabled(false)
            }
        }),
        older: feed.changes_older.map(|before| {
            Button::new(
                if loading {
                    "Loading older…"
                } else {
                    "Load older"
                },
                Press::Older(before),
                ButtonKind::Quiet,
            )
            .enabled(up && !loading)
        }),
        notice,
        status: daemon_status(feed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use corral_protocol::app::CHANGES_V1;
    use corral_types::Actor;
    use serde_json::json;

    fn event(seq: u64, at: &str, kind: EventKind, project: Option<&str>) -> Event {
        Event {
            seq,
            ts: at.parse().unwrap(),
            task: Some(format!("T-{seq}")),
            run: None,
            project: project.map(str::to_owned),
            kind,
            actor: Actor::Owner,
            detail: json!({}),
        }
    }

    /// The Feed lists newest first under each local day, a forced operation names what it
    /// skipped, and a project's scope keeps its own events (named by the event even for a task
    /// no longer on the board) and those whose project nothing names, never another project's.
    #[test]
    fn the_feed_groups_by_day_newest_first_and_keeps_the_scopes_events() {
        let mut feed = Feed {
            now: "2026-11-15T12:00:00Z"
                .parse::<jiff::Timestamp>()
                .unwrap()
                .as_second() as u64,
            time_zone: Some(jiff::tz::TimeZone::UTC),
            ..Feed::default()
        };
        feed.capabilities.insert(CHANGES_V1.into());
        feed.capabilities
            .insert(corral_protocol::PROJECTS_V1.into());
        for id in ["corral", "site"] {
            feed.projects.insert(
                Id::from(id),
                corral_protocol::rows::ProjectRow {
                    id: id.into(),
                    config: Default::default(),
                    open: 0,
                    open_by_status: None,
                    effective: None,
                    local_branches: None,
                },
            );
        }
        for event in [
            event(1, "2026-11-14T09:00:00Z", EventKind::Created, Some("site")),
            event(2, "2026-11-15T08:00:00Z", EventKind::Landed, Some("corral")),
            Event {
                detail: json!({"operation": "dispatch", "skipped": ["dependencies"]}),
                ..event(3, "2026-11-15T09:30:00Z", EventKind::Forced, None)
            },
        ] {
            feed.changes.insert(event.seq, event);
        }
        let page = feed_page(&feed, &Scope::All);
        let days: Vec<(&str, Vec<&str>)> = page
            .days
            .iter()
            .map(|day| {
                let ids = day.events.iter().map(|event| event.id.as_str()).collect();
                (day.label.as_str(), ids)
            })
            .collect();
        assert_eq!(
            days,
            [("Today", vec!["T-3", "T-2"]), ("Yesterday", vec!["T-1"])]
        );
        let forced = &page.days[0].events[0];
        assert!(forced.forced);
        assert!(
            forced.text.ends_with("skipped unfinished dependencies"),
            "{}",
            forced.text
        );
        assert_eq!(
            page.days[0].events[0].open,
            Some(Live::Task(Id::new("T-3")))
        );
        let site = feed_page(&feed, &Scope::Project(Id::new("site")));
        let ids: Vec<&str> = site
            .days
            .iter()
            .flat_map(|day| day.events.iter().map(|event| event.id.as_str()))
            .collect();
        assert_eq!(ids, ["T-3", "T-1"]);
    }
}
