//! What the Usage page shows, read tracked: the fixture preview's illustrative page, or corral's
//! recorded usage rows, series and session costs in the viewer's time zone. Also keeps the usage
//! series requested while the page is open and revalidates it when the usage rows move.

use corral_protocol::detail::DetailBody;
use jiff::tz::TimeZone;
use nsh_core::prelude::*;

use crate::model::daemon::Feed;
use crate::model::usage::{self, Inputs, Page};
use crate::state::AppState;

/// Where the recorded usage stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Ready,
    /// The last rows received, kept while corrald is unreachable.
    Stale,
    Loading,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Shown {
    pub page: Page,
    pub status: Status,
}

fn status(feed: &Feed) -> Status {
    match feed.topics.get("usage") {
        None => Status::Loading,
        Some(topic) => match (&topic.error, feed.usage.is_empty()) {
            (Some(error), true) => Status::Failed(error.message.clone()),
            (Some(_), false) => Status::Stale,
            (None, _) if topic.stale => Status::Stale,
            (None, true) if topic.loading => Status::Loading,
            (None, _) => Status::Ready,
        },
    }
}

/// Tracked: the page's content now.
pub fn shown(app: AppState) -> Shown {
    if !app.daemon() {
        return Shown {
            page: usage::fixture_page(),
            status: Status::Ready,
        };
    }
    let period = app.usage.with(|state| state.period);
    let tz = TimeZone::system();
    app.feed.with(|feed| {
        let cache = feed.detail(&usage::series_request(period));
        let series = cache.and_then(|cache| match cache.body() {
            Some(DetailBody::UsageSeries(series)) => Some(series),
            _ => None,
        });
        let series_error = cache
            .filter(|cache| cache.body().is_none())
            .and_then(|cache| cache.error.as_ref())
            .map(|error| error.message.as_str());
        let page = usage::live_page(&Inputs {
            rows: feed.usage.values().collect(),
            series,
            series_error,
            agents: feed.agents.values().collect(),
            assistants: feed.assistants.values().collect(),
            period,
            now: feed.now / 60 * 60,
            tz: &tz,
        });
        Shown {
            page,
            status: status(feed),
        }
    })
}

/// Keeps the usage series for the selected period requested, revalidating it whenever a usage
/// row's revision moves.
pub fn keep_loaded(app: AppState) {
    let wanted = computed(move || {
        if !app.daemon() {
            return None;
        }
        let period = app.usage.with(|state| state.period);
        let mark = app.feed.with(|feed| {
            feed.usage
                .values()
                .fold(feed.usage.len() as u64, |mark, row| {
                    mark.wrapping_mul(31).wrapping_add(row.rev)
                })
        });
        Some((usage::series_request(period), mark))
    });
    let last = stored(None::<u64>);
    effect(move || {
        let Some((series, mark)) = wanted.get() else {
            return;
        };
        untracked(|| {
            if last
                .replace(Some(mark))
                .is_some_and(|before| before != mark)
            {
                app.refresh(series);
            } else {
                app.load(series);
            }
        });
    });
}
