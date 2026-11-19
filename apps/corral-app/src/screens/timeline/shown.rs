//! What the Timeline shows, read tracked: the selected feed's recorded Timeline and Burnup
//! details in the viewer's time zone (the one the driver put on the feed). Also keeps those
//! details requested while the page is open; the client revalidates them as tasks, plans and
//! runs change. The burn-up is the scope the owner picked.

use corral_protocol::detail::{DetailBody, DetailRef};
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
    pub burnup: Option<BurnupSeries>,
    pub burnup_status: Status,
}

impl Shown {
    /// A complete answer with no lanes at all.
    pub fn lanes_empty(&self) -> bool {
        self.plot.as_ref().is_some_and(|plot| plot.lanes.is_empty())
    }

    /// The burn-up failed for the same cause as the spans, which the page already says once.
    pub fn burnup_failed_alike(&self) -> bool {
        matches!(
            (&self.status, &self.burnup_status),
            (Status::Failed(spans), Status::Failed(burnup)) if spans == burnup
        )
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

/// The viewer's zone as the driver reported it; UTC, said as such by the zone-named ticks, when
/// the platform could not tell.
fn zone(feed: &Feed) -> TimeZone {
    feed.time_zone.clone().unwrap_or(TimeZone::UTC)
}

/// Tracked: the page's content now.
pub fn shown(app: AppState) -> Shown {
    let state = app.timeline.get();
    app.feed.with(|feed| {
        let now = minute(feed);
        let window = timeline::window(state.range, now, &zone(feed));
        let (plot, status, gaps) = match &window {
            Some(window) => {
                let cache = feed.detail(&timeline::request(&state, window));
                let plot = cache
                    .and_then(DetailCache::body)
                    .and_then(|body| match body {
                        DetailBody::Timeline(page) => {
                            Some(timeline::live_plot(page, window, state.grouping, feed))
                        }
                        _ => None,
                    });
                let gaps = plot
                    .as_ref()
                    .map(|plot| timeline::live_gaps(plot, window, feed))
                    .unwrap_or_default();
                (plot, state_of(cache), gaps)
            }
            None => (None, Status::Loading, Vec::new()),
        };
        let (burnup, burnup_status) = burnup_parts(feed, &timeline::burnup_request(&state), now);
        Shown {
            gaps_empty: plot
                .is_some()
                .then(|| timeline::empty_gaps_line(state.range)),
            plot,
            status,
            gaps_title: timeline::gaps_title(state.range),
            gaps,
            burnup,
            burnup_status,
        }
    })
}

/// A burn-up detail as drawn: its series and where its request stands.
pub type BurnupParts = (Option<BurnupSeries>, Status);

fn burnup_parts(feed: &Feed, request: &DetailRef, now: u64) -> BurnupParts {
    let cache = feed.detail(request);
    let series = cache
        .and_then(DetailCache::body)
        .and_then(|body| match body {
            DetailBody::Burnup(burnup) => timeline::burnup_series(burnup, now, feed),
            _ => None,
        });
    (series, state_of(cache))
}

/// Tracked: one fixed scope's burn-up, for a page outside the Timeline (a project's).
pub fn burnup_of(app: AppState, request: &DetailRef) -> BurnupParts {
    app.feed
        .with(|feed| burnup_parts(feed, request, minute(feed)))
}

/// Keeps one fixed scope's burn-up requested while the page showing it is open.
pub fn keep_burnup(app: AppState, request: DetailRef) {
    effect(move || {
        if app.daemon() {
            let request = request.clone();
            untracked(|| app.load(request));
        }
    });
}

/// Keeps the Timeline and Burnup details requested while the page is open: requested when the
/// range, grouping, burn-up scope or day changes.
pub fn keep_loaded(app: AppState) {
    let wanted = computed(move || {
        if !app.daemon() {
            return None;
        }
        let state = app.timeline.get();
        app.feed.with(|feed| {
            let window = timeline::window(state.range, minute(feed), &zone(feed))?;
            Some((
                timeline::request(&state, &window),
                timeline::burnup_request(&state),
            ))
        })
    });
    effect(move || {
        let Some((spans, burnup)) = wanted.get() else {
            return;
        };
        untracked(|| {
            app.load(spans);
            app.load(burnup);
        });
    });
}
