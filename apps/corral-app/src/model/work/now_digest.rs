//! A bounded, task-grouped view of the same owner-relevant changes as the Since feed.
use corral_protocol::app::{ChangeKind, ChangeLine};
use corral_protocol::detail::DetailBody;
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
    let mut grouped: BTreeMap<Option<&str>, (&ChangeLine, usize)> = BTreeMap::new();
    let mut stale = BTreeSet::new();
    let mut landed = BTreeSet::new();
    let mut loaded = 0_u64;
    for (_, line) in feed
        .changes
        .range((
            std::ops::Bound::Excluded(through),
            std::ops::Bound::Unbounded,
        ))
        .rev()
    {
        if !super::live_context::owner_relevant(line) || !matches_scope(line, feed, filter) {
            continue;
        }
        loaded += 1;
        if line.to.as_deref() == Some("stale") {
            if let Some(task) = line.task.as_deref() {
                stale.insert(task);
            }
        }
        if line.to.as_deref() == Some("done") && !line.failed {
            if let Some(task) = line.task.as_deref() {
                landed.insert(task);
            }
        }
        let entry = grouped.entry(line.task.as_deref()).or_insert((line, 0));
        entry.1 += 1;
        // A failure or explicit stale transition must survive a later routine update.
        if priority(line) < priority(entry.0) {
            entry.0 = line;
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
        .map(|(line, events)| {
            let (phrase, mark, hue) = summary(line);
            NowDigestRow {
                mark,
                hue,
                phrase,
                events,
                task: line.task.as_deref().map(Id::from),
                time: super::stamp(&line.at, feed),
                hint: if events == 1 {
                    line.text.clone()
                } else {
                    format!("{} ({events} events)", line.text)
                },
                action: line
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
    // Keys carry the digest's prefix: the page head and the Closed heading count stale and
    // landed tasks on the same page, and each `stat-<key>` test id must name one count.
    let landed = (!landed.is_empty()).then(|| {
        Stat::new(
            "since-landed",
            StatIcon::Landed,
            landed.len().to_string(),
            Hue::Ok,
            format!(
                "{} tasks are recorded as landed since your last visit in loaded history",
                landed.len()
            ),
        )
    });
    let mut stats = Vec::new();
    stats.extend(landed.clone());
    if !stale.is_empty() {
        stats.push(Stat::new(
            "since-stale",
            StatIcon::Stale,
            stale.len().to_string(),
            Hue::Held,
            format!(
                "{} tasks have recorded stale transitions since your last visit in loaded history",
                stale.len()
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
        since, stats, rows, events, landed,
        coverage: "Task outcomes cover explicit transitions in loaded changes only. Change kinds do not distinguish a stale gate or a worker reply; project scopes omit events without a known project.".into(),
        more: Button::new(if events.total { format!("All {} events", events.count) }
            else { format!("{} loaded events", events.count) }, Action::Go(Destination::Changes), ButtonKind::Quiet),
        notice,
    }
}

fn priority(line: &ChangeLine) -> u8 {
    if line.failed {
        0
    } else if line.to.as_deref() == Some("stale") {
        1
    } else {
        2
    }
}

fn matches_scope(line: &ChangeLine, feed: &Feed, filter: &ProjectFilter) -> bool {
    if *filter == ProjectFilter::All {
        return true;
    }
    let Some(id) = line.task.as_deref() else {
        return false;
    };
    if let Some(task) = feed.board.task(id) {
        return filter.matches(task.project.as_deref());
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

fn summary(line: &ChangeLine) -> (&'static str, Mark, Hue) {
    let kind = if line.kind == ChangeKind::Other {
        line.class.as_str()
    } else {
        match line.kind {
            ChangeKind::Gate => "gate",
            ChangeKind::Review => "review",
            ChangeKind::Land => "land",
            ChangeKind::Work => "work",
            ChangeKind::Attention => "attention",
            ChangeKind::Question => "question",
            ChangeKind::Queue => "queue",
            ChangeKind::Plan => "plan",
            ChangeKind::Comment => "comment",
            ChangeKind::Annotation => "annotation",
            ChangeKind::Automation => "automation",
            ChangeKind::Policy => "policy",
            ChangeKind::Created => "created",
            ChangeKind::Moved => "moved",
            ChangeKind::Other => "other",
        }
    };
    if line.failed {
        return (
            match kind {
                "gate" => "gate failed",
                "review" => "review failed",
                "work" => "run failed",
                "land" => "landing failed",
                "queue" => "dispatch failed",
                "plan" => "plan failed",
                _ => "failed",
            },
            Mark::Fail,
            Hue::Fail,
        );
    }
    if line.to.as_deref() == Some("stale") {
        return (
            match kind {
                "gate" => "gate stale",
                "review" => "review stale",
                _ => "stale",
            },
            Mark::Stale,
            Hue::Held,
        );
    }
    if line.to.as_deref() == Some("done") {
        return ("landed", Mark::Done, Hue::Ok);
    }
    match (kind, line.to.as_deref()) {
        ("gate", Some("running")) => return ("gate started", Mark::Active, Hue::Corral),
        ("moved" | "work", Some("running")) => {
            return ("worker running", Mark::Active, Hue::Worker);
        }
        ("moved" | "queue", Some("ready")) => return ("queued", Mark::Next, Hue::Neutral),
        ("moved" | "review", Some("review")) => {
            return ("ready for review", Mark::Next, Hue::Corral);
        }
        ("attention", Some("needs_me")) => return ("blocked", Mark::Active, Hue::You),
        ("moved", Some("needs_me")) => return ("blocked", Mark::Na, Hue::Held),
        ("moved", Some("dropped")) => return ("dropped", Mark::Na, Hue::Neutral),
        ("moved", Some("backlog" | "idea")) => return ("upcoming", Mark::Next, Hue::Neutral),
        _ => {}
    }
    let phrase = match kind {
        "gate" => "gate updated",
        "review" => "review updated",
        "land" => "landing updated",
        "work" => "worker updated",
        "attention" => "attention changed",
        "question" => "question updated",
        "queue" => "queue updated",
        "plan" => "plan updated",
        "comment" => "comment added",
        "annotation" => "annotation updated",
        "automation" => "automation updated",
        "policy" => "policy updated",
        "created" => "created",
        "moved" => "moved",
        _ => "updated",
    };
    let (mark, hue) = match kind {
        "gate" | "review" => (Mark::Active, Hue::Corral),
        "work" => (Mark::Active, Hue::Worker),
        "question" | "attention" => (Mark::Active, Hue::You),
        _ => (Mark::Na, Hue::Neutral),
    };
    (phrase, mark, hue)
}
