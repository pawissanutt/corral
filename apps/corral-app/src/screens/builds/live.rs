//! Keeps the shown build's detail loaded, current and pinned in the client's cache, the way
//! Plans keeps its shown plan: loaded when first shown, gone stale or dropped from the cache,
//! revalidated when its row revision moves; a failed read is asked again only when the build is
//! shown anew, since the detail offers Retry. Requests and pins go to the source the view
//! mounted on (`AppState::scope`).

use corral_protocol::detail::DetailRef;
use nsh_core::prelude::*;

use crate::model::Id;
use crate::model::builds;
use crate::model::daemon::Feed;
use crate::model::projects::FilterScreen;
use crate::state::AppState;

/// What the client's cache holds of the build's detail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cached {
    Missing,
    Loading,
    Current,
    Stale,
    Failed,
}

fn cached(feed: &Feed, target: &DetailRef) -> Cached {
    match feed.detail(target) {
        None => Cached::Missing,
        Some(cache) if cache.loading => Cached::Loading,
        Some(cache) if cache.error.is_some() => Cached::Failed,
        Some(cache) if cache.stale => Cached::Stale,
        Some(_) => Cached::Current,
    }
}

#[derive(Debug, Clone, PartialEq)]
struct Want {
    build: Id,
    rev: Option<u64>,
    cached: Cached,
}

fn keep_current(app: AppState, build: impl Fn() -> Option<Id> + 'static) {
    let app = untracked(|| app.scope());
    let wanted = computed(move || {
        if !app.daemon() {
            return None;
        }
        build().map(|id| {
            app.feed.with(|feed| Want {
                rev: feed.builds.get(id.as_str()).map(|row| row.rev),
                cached: cached(feed, &builds::build_ref(&id)),
                build: id,
            })
        })
    });
    let last = stored(None::<Want>);
    let pinned = stored(None::<DetailRef>);
    effect(move || {
        let Some(now) = wanted.get() else {
            return;
        };
        let before = last.replace(Some(now.clone()));
        let target = builds::build_ref(&now.build);
        untracked(|| {
            let fresh = before
                .as_ref()
                .is_none_or(|before| before.build != now.build);
            let moved = !fresh && before.as_ref().is_some_and(|before| before.rev != now.rev);
            if pinned.with(|shown| shown.as_ref() != Some(&target)) {
                if let Some(gone) = pinned.replace(Some(target.clone())) {
                    app.watch_detail(gone, false);
                }
                app.watch_detail(target.clone(), true);
            }
            if moved {
                app.refresh(target);
            } else if fresh || now.cached != Cached::Failed {
                app.load(target);
            }
        });
    });
    // The owner may already be letting go of its values when this runs.
    cleanup(move || {
        if let Some(gone) = pinned.try_with(Option::take).flatten() {
            app.watch_detail(gone, false);
        }
    });
}

/// Keeps the desktop and terminal detail's build current: the root selection, or the first
/// build the Builds scope lists.
pub(super) fn keep_selected_loaded(app: AppState) {
    keep_current(app, move || {
        let selection = app.selection();
        let filter = app.project_filter(FilterScreen::Builds);
        app.feed
            .with(|feed| builds::current_build_filtered(&selection, feed, &filter))
    });
}

/// Keeps one build current while its pushed phone page is up.
pub(super) fn keep_loaded(app: AppState, id: Id) {
    keep_current(app, move || Some(id.clone()));
}
