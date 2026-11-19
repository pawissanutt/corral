//! What the Usage page shows, read tracked, and the request that keeps it current. The client
//! revalidates the period's series whenever the usage topic moves.

use nsh_core::prelude::*;

use crate::model::Effect;
use crate::model::usage::{self, Page};
use crate::state::AppState;

/// Tracked: the page's content now.
pub fn shown(app: AppState) -> Page {
    let state = app.usage.get();
    app.feed.with(|feed| usage::page(&state, feed))
}

/// Asks corrald again for the usage totals and the period's series: the bounded, explicit
/// retry beside a failed read (one press, one request each).
pub fn retry(app: AppState) {
    let state = untracked(|| app.usage.get());
    app.refresh(
        app.feed
            .with_untracked(|feed| usage::series_request(&state, &usage::local(feed))),
    );
    app.act(app.usage, |_, _| vec![Effect::RetryTopic("usage".into())]);
}

/// Keeps the chosen period's series requested while the page is open: again when the period,
/// the grouping, the local day or the device's offset changes.
pub fn keep_loaded(app: AppState) {
    let wanted = computed(move || {
        app.daemon().then(|| {
            let state = app.usage.get();
            app.feed
                .with(|feed| usage::series_request(&state, &usage::local(feed)))
        })
    });
    effect(move || {
        if let Some(series) = wanted.get() {
            untracked(|| app.load(series));
        }
    });
}
