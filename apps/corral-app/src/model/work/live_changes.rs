//! Changes in the Daemon state: the engine's events, newest first, split by this device's own
//! seen cursor, filtered, paged backwards, with who did each, and the device's Mark seen
//! advancing its cursor through the newest event.

use corral_types::Event;

use crate::model::daemon::{Feed, Link};
use crate::model::nav::Form;
use crate::model::projects::ProjectFilter;

use super::{
    Button, ButtonKind, Changes, FeedRow, Press, State, change_row_filtered, changes_loading,
    changes_notice, daemon_status, feed_filters, stamp,
};

/// Whether an unseen count is canonical or only the events this device has loaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnseenCount {
    pub count: u64,
    pub total: bool,
}

/// The loaded events past this device's seen cursor.
fn unseen_events(feed: &Feed) -> impl Iterator<Item = &Event> {
    let through = feed.seen.as_ref().map_or(0, |seen| seen.through);
    feed.changes
        .range((
            std::ops::Bound::Excluded(through),
            std::ops::Bound::Unbounded,
        ))
        .map(|(_, event)| event)
}

/// Event sequences are cursors, not counts. A server total belongs to its seen cursor.
pub fn unseen_count(feed: &Feed) -> UnseenCount {
    let through = feed.seen.as_ref().map_or(0, |seen| seen.through);
    if let Some((cursor, count)) = feed.changes_unseen
        && cursor == through
    {
        return UnseenCount { count, total: true };
    }
    UnseenCount {
        count: u64::try_from(unseen_events(feed).count()).unwrap_or(u64::MAX),
        total: false,
    }
}

/// Project totals cover loaded changes only; the daemon's unseen total is workspace-wide.
pub fn unseen_count_filtered(feed: &Feed, filter: &ProjectFilter) -> UnseenCount {
    let filter = filter.resolved(feed);
    if filter == ProjectFilter::All {
        return unseen_count(feed);
    }
    let count = unseen_events(feed)
        .filter(|event| change_matches_project(event, feed, &filter))
        .count();
    UnseenCount {
        count: u64::try_from(count).unwrap_or(u64::MAX),
        total: false,
    }
}

/// When this device last saw the feed, in local time, if it ever did.
fn last_visit(feed: &Feed) -> Option<String> {
    feed.seen
        .as_ref()
        .filter(|seen| seen.through != 0)
        .and_then(|seen| seen.at.as_deref())
        .map(|at| stamp(at, feed))
        .filter(|at| !at.is_empty())
}

/// Changes for the authenticated device's own cursor; the shell owns back navigation.
/// Resolve projects only from open tasks. Workspace events and events whose task has closed
/// remain visible: missing current metadata is not proof of another project.
pub fn live_changes_filtered(
    state: &State,
    feed: &Feed,
    device: Option<&str>,
    form: Form,
    filter: &ProjectFilter,
) -> Changes {
    let filter = filter.resolved(feed);
    let phone = form == Form::Phone;
    let through = feed.seen.as_ref().map_or(0, |seen| seen.through);
    let visit = last_visit(feed);
    let mut recent: Vec<FeedRow> = Vec::new();
    let mut history: Vec<FeedRow> = Vec::new();
    for event in feed
        .changes
        .values()
        .rev()
        .filter(|event| state.filter.admits(event.kind))
        .filter(|event| change_matches_project(event, feed, &filter))
    {
        if event.seq > through {
            recent.push(change_row_filtered(event, feed, true, &filter));
        } else {
            history.push(change_row_filtered(event, feed, false, &filter));
        }
    }
    let unseen = unseen_count_filtered(feed, &filter);
    let device_words = device.map_or_else(|| "this device".to_owned(), |name| name.to_owned());
    let seen_line = match (&visit, phone) {
        (Some(at), true) => format!("last visit {at}"),
        (Some(at), false) => format!("your last visit · {at} · seen-cursor for {device_words}"),
        (None, true) => "nothing marked seen yet".to_owned(),
        (None, false) => format!("nothing marked seen on {device_words} yet"),
    };
    let subtitle = (!phone).then(|| {
        let events = format!(
            "{} {}event{}",
            unseen.count,
            if unseen.total {
                "new "
            } else {
                "loaded unseen "
            },
            if unseen.count == 1 { "" } else { "s" }
        );
        let cursor =
            "the seen-cursor is per device: marking seen here leaves your other devices' marks";
        match &visit {
            Some(at) => format!("since this device's last visit {at} · {events} · {cursor}"),
            None => format!("{events} · {cursor}"),
        }
    });
    let up = feed.link == Link::Up;
    let loading = changes_loading(feed);
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
            (recent.is_empty() && history.is_empty())
                .then(|| format!("No {} events loaded.", state.filter.label().to_lowercase()))
        });
    Changes {
        title: "Changes",
        subtitle,
        filters: feed_filters(state),
        recent,
        seen_line,
        history,
        illustrative: None,
        // Each device keeps its own cursor, so the phone can move its own too.
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

/// Historical and workspace-wide events have no reliable current project without an open task,
/// so keep them rather than silently dropping events from the retained history.
pub(crate) fn change_matches_project(event: &Event, feed: &Feed, filter: &ProjectFilter) -> bool {
    event
        .task
        .as_deref()
        .and_then(|id| feed.board.task(id))
        .is_none_or(|task| filter.matches(task.project()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::board::{ChangeKind, Mark};
    use crate::model::work::ChangeFilter;
    use corral_protocol::app::{CHANGES_V1, SeenCursor};
    use corral_types::{Actor, EventKind};
    use serde_json::json;

    fn event(seq: u64, kind: EventKind, detail: serde_json::Value) -> Event {
        Event {
            seq,
            ts: "2026-11-14T10:00:00Z".parse().unwrap(),
            task: Some(format!("T-{seq}")),
            run: None,
            kind,
            actor: Actor::Owner,
            detail,
        }
    }

    /// The seen cursor splits new from seen; a forced line names every rule it skipped in plain
    /// words, and Forced narrows the feed to the forced lines, each opening its task.
    #[test]
    fn the_seen_cursor_splits_new_from_seen_and_a_forced_line_names_what_it_skipped() {
        let mut feed = Feed::default();
        feed.capabilities.insert(CHANGES_V1.into());
        feed.changes.extend([
            (1, event(1, EventKind::Gate, json!({"result": "fail"}))),
            (
                2,
                event(2, EventKind::Blocked, json!({"reason": "gate failed"})),
            ),
            (
                3,
                event(
                    3,
                    EventKind::Forced,
                    json!({"operation": "dispatch", "skipped": ["dependencies", "limits.running"]}),
                ),
            ),
            (
                4,
                event(
                    4,
                    EventKind::Landed,
                    json!({"sha": "abcdef123", "base": "main"}),
                ),
            ),
        ]);
        feed.seen = Some(SeenCursor {
            through: 2,
            at: Some("2026-11-14T09:00:00Z".into()),
        });
        let mut state = State::default();
        let all = ProjectFilter::All;
        let changes = live_changes_filtered(&state, &feed, Some("phone"), Form::Desktop, &all);
        let ids = |rows: &[FeedRow]| rows.iter().map(|row| row.id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(&changes.recent), ["T-4", "T-3"]);
        assert_eq!(ids(&changes.history), ["T-2", "T-1"]);
        assert!(
            changes.recent[1]
                .text
                .ends_with("skipped unfinished dependencies and the running-slot limit"),
            "{}",
            changes.recent[1].text
        );
        assert_eq!(changes.history[1].kind, ChangeKind::Mark(Mark::Fail));
        assert_eq!(changes.recent[0].at, "Nov 14");
        assert_eq!(
            changes.mark_seen.and_then(|button| button.action),
            Some(Press::Seen(4))
        );
        state.filter = ChangeFilter::Verdicts;
        let changes = live_changes_filtered(&state, &feed, None, Form::Desktop, &all);
        assert_eq!(ids(&changes.recent), ["T-4"]);
        assert_eq!(ids(&changes.history), ["T-1"]);
        state.filter = ChangeFilter::Forced;
        let changes = live_changes_filtered(&state, &feed, None, Form::Desktop, &all);
        assert_eq!(ids(&changes.recent), ["T-3"]);
        assert!(changes.history.is_empty());
        assert_eq!(
            changes.recent[0].action,
            Some(Press::Open(crate::model::Live::Task("T-3".into())))
        );
    }
}
