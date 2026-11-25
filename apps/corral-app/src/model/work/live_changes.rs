//! Activity › Feed: every engine event in the one project scope, newest first, grouped by the
//! local day it happened on, each row opening its task; older pages load on request, and Mark
//! seen moves this device's own seen cursor, which Home's "since your last visit" counts from.
//! The Feed's own filters ([`FeedFilter`]) narrow the loaded rows without touching either.

use corral_types::{Event, EventKind};
use jiff::civil::Date;

use crate::model::daemon::{Feed, Link};
use crate::model::projects::Scope;
use crate::model::{Id, Live};

use super::{
    Alert, Button, ButtonKind, Category, FeedFilter, Press, Status, Who, actor_word,
    changes_loading, changes_notice, daemon_status, day_label, event_detail, event_project, failed,
    local_day,
};

/// The Feed's title and what it lists.
pub const FEED_TITLE: &str = "Feed";
pub const FEED_SUB: &str = "every event, newest first";
/// The search field's label and placeholder.
pub const FEED_SEARCH: &str = "Search task id or event";
/// The reset control's words.
pub const FEED_RESET: &str = "Clear filters";

/// One event: its local time, what happened in words, its task and title, the detail and who
/// did it, with the category and alert a row marks it with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedEvent {
    /// `18:40`, local.
    pub at: String,
    /// What happened, readable: `Moved`, `Gate failed`, `Review started`.
    pub label: String,
    pub category: Category,
    /// Blocked, failed or forced: named beside the label, not only coloured.
    pub alert: Option<Alert>,
    /// The task the event concerns; empty for an event about none.
    pub id: String,
    /// The task's title when the board holds it.
    pub title: Option<String>,
    /// What the event says beside its label: `backlog → ready`.
    pub text: String,
    pub who: &'static str,
    pub actor: Who,
    /// The page the row opens: the task's.
    pub open: Option<Live>,
    /// The row's test id: `event-<seq>`.
    pub test_id: String,
}

impl FeedEvent {
    /// What a screen reader hears: time, alert, what happened, task, title, detail and who.
    pub fn spoken(&self) -> String {
        [
            self.at.as_str(),
            self.alert.map_or("", Alert::label),
            self.label.as_str(),
            self.id.as_str(),
            self.title.as_deref().unwrap_or_default(),
            self.text.as_str(),
            self.who,
        ]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
    }

    /// Whether the Feed's filters keep this row; search reads the words the row shows.
    fn kept(&self, filter: &FeedFilter, kind: &str) -> bool {
        let words = [
            self.id.as_str(),
            self.label.as_str(),
            kind,
            self.title.as_deref().unwrap_or_default(),
            self.text.as_str(),
            self.who,
            self.alert.map_or("", Alert::label),
        ];
        filter.matches(self.category, self.actor, &words)
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
    /// Events are loaded in scope but the filters keep none of them.
    pub no_match: Option<String>,
    /// `12 of 80 loaded events` while a filter is set.
    pub counted: Option<String>,
    pub status: Status,
}

/// The project an event is filed under (see [`event_project`]); an event with none known, a
/// workspace event or one of a task nothing loaded names, stays in every scope rather than
/// silently dropping from the retained history.
fn in_scope(event: &Event, feed: &Feed, scope: &Scope) -> bool {
    event_project(event, feed).is_none_or(|project| scope.matches(project))
}

/// What happened, in the words a row leads with; a gate or verdict says its outcome.
fn headline(event: &Event) -> String {
    let words = match event.kind {
        EventKind::Gate if failed(event) => "gate failed",
        EventKind::Gate => "gate passed",
        EventKind::Verdict if failed(event) => "changes requested",
        EventKind::Verdict => "approved",
        EventKind::Done => "worker done",
        EventKind::Ready => "ready to land",
        EventKind::Artifact => "artifact added",
        kind => kind.as_str(),
    }
    .replace('_', " ");
    let mut letters = words.chars();
    letters
        .next()
        .map(|first| first.to_uppercase().chain(letters).collect())
        .unwrap_or_default()
}

fn event_row(event: &Event, at: String, feed: &Feed) -> FeedEvent {
    let text = match event.kind {
        // The headline says the outcome already.
        EventKind::Gate | EventKind::Verdict => String::new(),
        _ => event_detail(event),
    };
    let alert = Alert::of(event);
    FeedEvent {
        at,
        label: headline(event),
        // A failed gate is a problem first: Problems keeps every alert.
        category: if alert.is_some() {
            Category::Problem
        } else {
            Category::of(event.kind)
        },
        alert,
        id: event.task.clone().unwrap_or_default(),
        title: event
            .task
            .as_deref()
            .and_then(|id| feed.board.task(id))
            .map(|task| task.title().to_owned())
            .filter(|title| !title.is_empty()),
        text,
        who: actor_word(event.actor),
        actor: Who::of(event.actor),
        open: event.task.as_deref().map(|task| Live::Task(Id::new(task))),
        test_id: format!("event-{}", event.seq),
    }
}

/// The Feed in `scope` through `filter`: every loaded event that passes, newest first, by
/// local day.
pub fn feed_page(feed: &Feed, scope: &Scope, filter: &FeedFilter) -> FeedPage {
    let scope = scope.resolved(feed);
    let mut days: Vec<(Date, Vec<FeedEvent>)> = Vec::new();
    let mut loaded = 0usize;
    let mut shown = 0usize;
    for event in feed
        .changes
        .values()
        .rev()
        .filter(|event| in_scope(event, feed, &scope))
    {
        loaded += 1;
        let (day, at) = local_day(event.ts, feed);
        let row = event_row(event, at, feed);
        if !row.kept(filter, event.kind.as_str()) {
            continue;
        }
        shown += 1;
        match days.last_mut() {
            Some((last, events)) if *last == day => events.push(row),
            _ => days.push((day, vec![row])),
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
        .or_else(|| (loaded == 0).then(|| "No events in this scope yet.".to_owned()));
    let no_match = (notice.is_none() && shown == 0).then(|| {
        if feed.changes_older.is_some() {
            "No loaded events match these filters. Load older to look further back.".to_owned()
        } else {
            "No events match these filters.".to_owned()
        }
    });
    let counted = (filter.active() && loaded > 0).then(|| {
        let noun = if loaded == 1 { "event" } else { "events" };
        format!("{shown} of {loaded} loaded {noun}")
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
        no_match,
        counted,
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

    fn feed_at_noon() -> Feed {
        let mut feed = Feed {
            now: "2026-11-15T12:00:00Z"
                .parse::<jiff::Timestamp>()
                .unwrap()
                .as_second() as u64,
            time_zone: Some(jiff::tz::TimeZone::UTC),
            ..Feed::default()
        };
        feed.capabilities.insert(CHANGES_V1.into());
        feed
    }

    fn ids(page: &FeedPage) -> Vec<&str> {
        page.days
            .iter()
            .flat_map(|day| day.events.iter().map(|event| event.id.as_str()))
            .collect()
    }

    /// The Feed lists newest first under each local day, a forced operation names what it
    /// skipped, and a project's scope keeps its own events (named by the event even for a task
    /// no longer on the board) and those whose project nothing names, never another project's.
    #[test]
    fn the_feed_groups_by_day_newest_first_and_keeps_the_scopes_events() {
        let mut feed = feed_at_noon();
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
        let page = feed_page(&feed, &Scope::All, &FeedFilter::default());
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
        assert_eq!(forced.alert, Some(Alert::Forced));
        assert!(
            forced.text.ends_with("skipped unfinished dependencies"),
            "{}",
            forced.text
        );
        assert_eq!(
            page.days[0].events[0].open,
            Some(Live::Task(Id::new("T-3")))
        );
        let site = feed_page(
            &feed,
            &Scope::Project(Id::new("site")),
            &FeedFilter::default(),
        );
        assert_eq!(ids(&site), ["T-3", "T-1"]);
    }

    /// Category, actor and search combine; a filter that keeps nothing says so apart from an
    /// empty feed and leaves Load older offered, and a page loaded later is filtered the same.
    #[test]
    fn feed_filters_combine_and_tell_no_match_from_an_empty_feed() {
        let mut feed = feed_at_noon();
        feed.changes_older = Some(1);
        for event in [
            Event {
                detail: json!({"reason": "needs a key"}),
                actor: Actor::Agent,
                ..event(5, "2026-11-15T09:00:00Z", EventKind::Blocked, None)
            },
            Event {
                actor: Actor::Auto,
                detail: json!({"result": "fail"}),
                ..event(6, "2026-11-15T10:00:00Z", EventKind::Gate, None)
            },
            event(7, "2026-11-15T11:00:00Z", EventKind::Commented, None),
        ] {
            feed.changes.insert(event.seq, event);
        }
        let problems = FeedFilter {
            category: Some(Category::Problem),
            ..FeedFilter::default()
        };
        let page = feed_page(&feed, &Scope::All, &problems);
        assert_eq!(ids(&page), ["T-6", "T-5"]);
        assert_eq!(page.days[0].events[1].alert, Some(Alert::Blocked));

        let combined = FeedFilter {
            who: Some(Who::Automation),
            search: "GATE t-6".into(),
            ..FeedFilter::default()
        };
        let page = feed_page(&feed, &Scope::All, &combined);
        assert_eq!(ids(&page), ["T-6"]);
        assert_eq!(page.days[0].events[0].alert, Some(Alert::Failed));

        let none = FeedFilter {
            who: Some(Who::Owner),
            category: Some(Category::Problem),
            ..FeedFilter::default()
        };
        let page = feed_page(&feed, &Scope::All, &none);
        assert!(page.days.is_empty() && page.notice.is_none());
        assert!(page.no_match.is_some() && page.older.is_some());

        // An older page with an owner's error now matches the same filter.
        feed.changes
            .insert(4, event(4, "2026-11-14T08:00:00Z", EventKind::Error, None));
        let page = feed_page(&feed, &Scope::All, &none);
        assert_eq!(ids(&page), ["T-4"]);
        assert!(page.no_match.is_none());

        let empty = feed_page(&feed_at_noon(), &Scope::All, &none);
        assert!(empty.notice.is_some() && empty.no_match.is_none());
    }
}
