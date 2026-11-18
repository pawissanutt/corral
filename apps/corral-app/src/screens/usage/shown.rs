//! What the Usage page shows, read tracked, and the requests that keep it current. The client
//! revalidates the grouped series whenever the usage topic moves.

use nsh_core::prelude::*;

use crate::model::Effect;
use crate::model::usage::{self, Page};
use crate::state::AppState;

/// Tracked: the page's content now.
pub fn shown(app: AppState) -> Page {
    let by = app.usage.with(|state| state.by);
    app.feed.with(|feed| usage::page(by, feed))
}

/// Asks corrald again for the usage totals and the selected grouping: the bounded, explicit
/// retry beside a failed read (one press, one request each).
pub fn retry(app: AppState) {
    let by = untracked(|| app.usage.with(|state| state.by));
    app.refresh(usage::series_request(by));
    app.act(app.usage, |_, _| vec![Effect::RetryTopic("usage".into())]);
}

/// Keeps the selected grouping's series requested while the page is open.
pub fn keep_loaded(app: AppState) {
    let wanted = computed(move || {
        app.daemon()
            .then(|| usage::series_request(app.usage.with(|state| state.by)))
    });
    effect(move || {
        if let Some(series) = wanted.get() {
            untracked(|| app.load(series));
        }
    });
}
