//! What the Timeline shows, read tracked: the fixture preview's illustrative day, or the
//! daemon's recorded Timeline and Burnup details in the viewer's time zone. Also keeps those
//! details requested while the page is open and revalidates them when the board moves.

use corral_protocol::detail::DetailBody;
use jiff::tz::TimeZone;
use nsh_core::prelude::*;

use crate::model::daemon::{DetailCache, Feed};
use crate::model::timeline::{self, BurnupSeries, Gap, Plot};
use crate::state::AppState;

/// Where a recorded detail stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Ready,
    /// The last complete body, kept while corrald is unreachable or revalidating failed.
    Stale,
    Loading,
    Failed(String),
}

/// Everything the page draws.
#[derive(Debug, Clone, PartialEq)]
pub struct Shown {
    pub plot: Option<Plot>,
    pub status: Status,
    pub gaps_title: &'static str,
    pub gaps: Vec<Gap>,
    /// Said when the range has no gaps worth a look.
    pub gaps_empty: Option<&'static str>,
    pub gaps_note: &'static str,
    pub burnup: Option<BurnupSeries>,
    pub burnup_status: Status,
}

impl Shown {
    /// A complete answer with no lanes at all.
    pub fn lanes_empty(&self) -> bool {
        self.plot.as_ref().is_some_and(|plot| plot.lanes.is_empty())
    }
}

fn state_of(cache: Option<&DetailCache>) -> Status {
    let Some(cache) = cache else {
        return Status::Loading;
    };
    match (cache.body(), &cache.error) {
        (None, Some(error)) => Status::Failed(error.message.clone()),
        (None, None) => Status::Loading,
        (Some(_), Some(_)) => Status::Stale,
        (Some(_), None) if cache.stale => Status::Stale,
        (Some(_), None) => Status::Ready,
    }
}

/// The clock to lay out on: whole minutes, so the page does not redraw every second.
fn minute(feed: &Feed) -> u64 {
    feed.now / 60 * 60
}

/// Tracked: the page's content now.
pub fn shown(app: AppState) -> Shown {
    let state = app.timeline.get();
    if !app.daemon() {
        return Shown {
            plot: Some(timeline::fixture_plot()),
            status: Status::Ready,
            gaps_title: timeline::gaps_title(state.range),
            gaps: timeline::fixture_gaps(),
            gaps_empty: None,
            gaps_note: timeline::GAPS_DISCLOSURE,
            burnup: Some(timeline::fixture_burnup()),
            burnup_status: Status::Ready,
        };
    }
    let tz = TimeZone::system();
    app.feed.with(|feed| {
        let now = minute(feed);
        let window = timeline::window(state.range, now, &tz);
        let (plot, status, gaps) = match &window {
            Some(window) => {
                let cache = feed.detail(&timeline::request(&state, window));
                let plot = cache
                    .and_then(DetailCache::body)
                    .and_then(|body| match body {
                        DetailBody::Timeline(page) => Some(timeline::live_plot(page, window, &tz)),
                        _ => None,
                    });
                let gaps = plot
                    .as_ref()
                    .map(|plot| timeline::live_gaps(plot, window, &tz))
                    .unwrap_or_default();
                (plot, state_of(cache), gaps)
            }
            None => (None, Status::Loading, Vec::new()),
        };
        let burnup_cache = feed.detail(&timeline::burnup_request());
        let burnup = burnup_cache
            .and_then(DetailCache::body)
            .and_then(|body| match body {
                DetailBody::Burnup(burnup) => timeline::burnup_series(burnup, now, &tz),
                _ => None,
            });
        Shown {
            gaps_empty: plot
                .is_some()
                .then(|| timeline::empty_gaps_line(state.range)),
            plot,
            status,
            gaps_title: timeline::gaps_title(state.range),
            gaps,
            gaps_note: timeline::LIVE_GAPS_NOTE,
            burnup,
            burnup_status: state_of(burnup_cache),
        }
    })
}

/// A number that moves whenever a board row's revision does, so the recorded spans are
/// revalidated as work changes phase.
fn board_mark(feed: &Feed) -> u64 {
    feed.board
        .tasks()
        .iter()
        .fold(feed.board.epoch, |mark, task| {
            mark.wrapping_mul(31).wrapping_add(task.rev)
        })
}

/// Keeps the Timeline and Burnup details requested while the page is open: requested when the
/// range, grouping or day changes, revalidated (with their etags) when the board moves.
pub fn keep_loaded(app: AppState) {
    let wanted = computed(move || {
        if !app.daemon() {
            return None;
        }
        let state = app.timeline.get();
        let tz = TimeZone::system();
        app.feed.with(|feed| {
            let window = timeline::window(state.range, minute(feed), &tz)?;
            Some((
                timeline::request(&state, &window),
                timeline::burnup_request(),
                board_mark(feed),
            ))
        })
    });
    let last = stored(None::<u64>);
    effect(move || {
        let Some((spans, burnup, mark)) = wanted.get() else {
            return;
        };
        untracked(|| {
            let moved = last
                .replace(Some(mark))
                .is_some_and(|before| before != mark);
            if moved {
                app.refresh(spans);
                app.refresh(burnup);
            } else {
                app.load(spans);
                app.load(burnup);
            }
        });
    });
}
