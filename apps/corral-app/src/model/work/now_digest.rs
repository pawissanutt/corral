//! A bounded, task-grouped view of the same engine events as the Changes feed, since this
//! device's last visit.
use corral_protocol::detail::DetailBody;
use corral_types::{Event, EventKind};
use std::collections::{BTreeMap, BTreeSet};

use super::{Action, Button, ButtonKind, Notice, Press, UnseenCount};
use crate::model::board::Mark;
use crate::model::daemon::Feed;
use crate::model::glance::{Stat, StatIcon};
use crate::model::projects::ProjectFilter;
use crate::model::{Destination, Hue, Id, Live};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NowDigestRow {
    pub mark: Mark,
    pub hue: Hue,
    pub task: Option<Id>,
    pub phrase: &'static str,
    pub time: String,
    pub hint: String,
    pub events: usize,
    pub action: Option<Press>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NowDigest {
    pub since: String,
    pub stats: Vec<Stat>,
    pub rows: Vec<NowDigestRow>,
    pub more: Button,
    pub events: UnseenCount,
    /// Counts are from loaded evidence, never a prediction from current board state.
    pub coverage: String,
    pub notice: Option<Notice>,
    pub landed: Option<Stat>,
}

pub(super) fn digest(feed: &Feed, filter: &ProjectFilter) -> NowDigest {
    let since = feed
        .seen
        .as_ref()
        .and_then(|seen| seen.at.as_deref())
        .map(|at| super::when(at, feed))
        .filter(|at| !at.is_empty())
        .map_or_else(
            || "Since your last visit".into(),
            |at| format!("Since {at}"),
        );
    let through = feed.seen.as_ref().map_or(0, |seen| seen.through);
    let mut grouped: BTreeMap<Option<&str>, (&Event, usize)> = BTreeMap::new();
    let mut failed = BTreeSet::new();
    let mut landed = BTreeSet::new();
    let mut loaded = 0_u64;
    for (_, event) in feed
        .changes
        .range((
            std::ops::Bound::Excluded(through),
            std::ops::Bound::Unbounded,
        ))
        .rev()
    {
        if !matches_scope(event, feed, filter) {
            continue;
        }
        loaded += 1;
        if let Some(task) = event.task.as_deref() {
            if super::live_context::failed(event) {
                failed.insert(task);
            }
            if event.kind == EventKind::Landed {
                landed.insert(task);
            }
        }
        let entry = grouped.entry(event.task.as_deref()).or_insert((event, 0));
        entry.1 += 1;
        // A failure must survive a later routine update.
        if priority(event) < priority(entry.0) {
            entry.0 = event;
        }
    }
    let mut groups: Vec<_> = grouped.into_values().collect();
    groups.sort_by(|(a, _), (b, _)| {
        priority(a)
            .cmp(&priority(b))
            .then_with(|| b.seq.cmp(&a.seq))
    });
    let rows: Vec<NowDigestRow> = groups
        .into_iter()
        .take(7)
        .map(|(event, events)| {
            let (phrase, mark, hue) = summary(event);
            let text = super::event_text(event);
            NowDigestRow {
                mark,
                hue,
                phrase,
                events,
                task: event.task.as_deref().map(Id::new),
                time: super::stamp_at(event.ts, feed),
                hint: if events == 1 {
                    text
                } else {
                    format!("{text} ({events} events)")
                },
                action: event
                    .task
                    .as_deref()
                    .map(|id| Press::Open(Live::Task(id.into()))),
            }
        })
        .collect();
    let events = if *filter == ProjectFilter::All {
        super::unseen_count(feed)
    } else {
        UnseenCount {
            count: loaded,
            total: false,
        }
    };
    // Keys carry the digest's prefix: the page head and the Closed heading count landed tasks on
    // the same page, and each `stat-<key>` test id must name one count.
    let landed = (!landed.is_empty()).then(|| {
        Stat::new(
            "since-landed",
            StatIcon::Landed,
            landed.len().to_string(),
            Hue::Ok,
            format!(
                "{} tasks landed since your last visit in loaded history",
                landed.len()
            ),
        )
    });
    let mut stats = Vec::new();
    stats.extend(landed.clone());
    if !failed.is_empty() {
        stats.push(Stat::new(
            "since-failed",
            StatIcon::Failed,
            failed.len().to_string(),
            Hue::Fail,
            format!(
                "{} tasks recorded a failure since your last visit in loaded history",
                failed.len()
            ),
        ));
    }
    if events.count > 0 {
        stats.push(Stat::new(
            "since-events",
            StatIcon::Events,
            events.count.to_string(),
            Hue::Neutral,
            if events.total {
                format!("{} events since your last visit", events.count)
            } else {
                format!("{} loaded events since your last visit", events.count)
            },
        ));
    }
    let notice = super::live_context::changes_notice(feed).or_else(|| {
        rows.is_empty().then_some(Notice::Quiet(
            "Nothing new in this scope in loaded history.",
        ))
    });
    NowDigest {
        since,
        stats,
        rows,
        events,
        landed,
        coverage: "Task outcomes cover the events loaded on this device only; project scopes omit events without a known project.".into(),
        more: Button::new(
            if events.total {
                format!("All {} events", events.count)
            } else {
                format!("{} loaded events", events.count)
            },
            Action::Go(Destination::Feed),
            ButtonKind::Quiet,
        ),
        notice,
    }
}

/// Lower comes first. A failure must survive a later routine update, but a landing or a drop ends
/// the task: a task's earlier "ready to land" or failure must not stand for it once it is closed.
fn priority(event: &Event) -> u8 {
    if super::live_context::failed(event)
        || matches!(event.kind, EventKind::Landed | EventKind::Dropped)
    {
        0
    } else if matches!(event.kind, EventKind::Blocked | EventKind::Ready) {
        1
    } else {
        2
    }
}

/// An event with a task in scope: the project the event names, else the open task's, or a
/// loaded closed page's.
fn matches_scope(event: &Event, feed: &Feed, filter: &ProjectFilter) -> bool {
    if *filter == ProjectFilter::All {
        return true;
    }
    if let Some(project) = event.project.as_deref() {
        return filter.matches(Some(project));
    }
    let Some(id) = event.task.as_deref() else {
        return false;
    };
    if let Some(task) = feed.board.task(id) {
        return filter.matches(task.project());
    }
    for cache in feed.details.values() {
        if let Some(DetailBody::Closed(page)) = cache.body()
            && let Some(task) = page.rows.iter().find(|task| task.id == id)
        {
            return filter.matches(task.project.as_deref());
        }
    }
    false
}

fn summary(event: &Event) -> (&'static str, Mark, Hue) {
    if super::live_context::failed(event) {
        return (
            match event.kind {
                EventKind::Gate => "gate failed",
                EventKind::Verdict => "changes requested",
                _ => "failed",
            },
            Mark::Fail,
            Hue::Fail,
        );
    }
    match event.kind {
        EventKind::Landed => ("landed", Mark::Done, Hue::Ok),
        EventKind::Blocked => ("blocked", Mark::Active, Hue::You),
        EventKind::Ready => ("ready to land", Mark::Active, Hue::You),
        EventKind::Gate => ("gate passed", Mark::Done, Hue::Corral),
        EventKind::ReviewStarted => ("review started", Mark::Active, Hue::Corral),
        EventKind::Verdict => ("approved", Mark::Done, Hue::Corral),
        EventKind::SentBack => ("sent back", Mark::Active, Hue::Worker),
        EventKind::Dispatched | EventKind::Resumed => ("worker running", Mark::Active, Hue::Worker),
        EventKind::Done => ("worker done", Mark::Done, Hue::Worker),
        EventKind::Stopped => ("stopped", Mark::Na, Hue::Held),
        EventKind::Forced => ("forced", Mark::Na, Hue::Held),
        EventKind::Dropped => ("dropped", Mark::Na, Hue::Neutral),
        EventKind::Reopened => ("reopened", Mark::Next, Hue::Neutral),
        EventKind::Created => ("created", Mark::Next, Hue::Neutral),
        EventKind::Moved => ("moved", Mark::Next, Hue::Neutral),
        EventKind::Commented | EventKind::Replied => ("comment added", Mark::Na, Hue::Neutral),
        EventKind::Artifact => ("artifact added", Mark::Na, Hue::Neutral),
        EventKind::Edited => ("edited", Mark::Na, Hue::Neutral),
        EventKind::Error => ("failed", Mark::Fail, Hue::Fail),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use corral_types::Actor;

    /// A landed task kept reading "ready to land" (and a forced landing "gate failed"): an
    /// earlier prompt outranked the landing that closed it.
    #[test]
    fn a_landed_task_reads_landed_not_its_earlier_prompt_or_failure() {
        let event = |seq: u64, kind: EventKind, detail: serde_json::Value| Event {
            seq,
            ts: jiff::Timestamp::UNIX_EPOCH,
            task: Some("T-1".into()),
            run: None,
            project: None,
            kind,
            actor: Actor::Auto,
            detail,
        };
        let mut feed = Feed::default();
        for (seq, kind, detail) in [
            (1, EventKind::Gate, serde_json::json!({"result": "fail"})),
            (2, EventKind::Ready, serde_json::json!({})),
            (3, EventKind::Landed, serde_json::json!({})),
        ] {
            feed.changes.insert(seq, event(seq, kind, detail));
        }
        let rows = digest(&feed, &ProjectFilter::All).rows;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].phrase, "landed");
    }
}
