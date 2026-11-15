//! What the Usage page shows, read tracked: the selected feed's recorded usage rows, series and
//! session costs in the viewer's time zone. Also keeps the usage series requested while the page
//! is open and revalidates it when the usage rows move.

use corral_protocol::detail::DetailBody;
use jiff::tz::TimeZone;
use nsh_core::prelude::*;

use crate::model::Effect;
use crate::model::daemon::Feed;
use crate::model::usage::{self, Inputs, Page};
use crate::state::AppState;

/// The subscription topic of the usage rows.
const TOPIC: &str = "usage";

/// Where the recorded usage rows stand. A failed read is not a status here: the page's own
/// `notices` say it once, with the daemon's cause.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Ready,
    /// The last rows received, kept while corrald is unreachable or failing.
    Stale,
    Loading,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Shown {
    pub page: Page,
    pub status: Status,
}

fn status(feed: &Feed) -> Status {
    match feed.topics.get(TOPIC) {
        None => Status::Loading,
        Some(topic) => match (&topic.error, feed.usage.is_empty()) {
            (Some(_), true) => Status::Ready,
            (Some(_), false) => Status::Stale,
            (None, _) if topic.stale => Status::Stale,
            (None, true) if topic.loading => Status::Loading,
            (None, _) => Status::Ready,
        },
    }
}

/// Tracked: the page's content now.
pub fn shown(app: AppState) -> Shown {
    let period = app.usage.with(|state| state.period);
    app.feed.with(|feed| {
        let tz = feed.time_zone.clone().unwrap_or(TimeZone::UTC);
        let topic = feed.topics.get(TOPIC);
        let cache = feed.detail(&usage::series_request(period));
        let series = cache.and_then(|cache| match cache.body() {
            Some(DetailBody::UsageSeries(series)) => Some(series),
            _ => None,
        });
        // A failed refresh is said even while the last series stays readable (LA-09).
        let series_error = cache
            .and_then(|cache| cache.error.as_ref())
            .map(|error| error.message.as_str());
        let page = usage::live_page(&Inputs {
            rows: feed.usage.values().collect(),
            rows_error: topic
                .and_then(|topic| topic.error.as_ref())
                .map(|error| error.message.as_str()),
            rows_loading: topic.is_none_or(|topic| topic.loading),
            series,
            series_error,
            series_stale: cache.is_some_and(|cache| cache.stale),
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

/// Asks corrald again for the usage rows and the selected period's series: the bounded,
/// explicit retry beside a failed read (one press, one request each).
pub fn retry(app: AppState) {
    let period = untracked(|| app.usage.with(|state| state.period));
    app.refresh(usage::series_request(period));
    app.act(app.usage, |_, _| vec![Effect::RetryTopic(TOPIC.into())]);
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
