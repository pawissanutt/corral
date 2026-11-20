//! Keeps served details loaded while a view shows them: each is pinned in the client's cache,
//! read when it is missing, read again when it went stale, and let go when the view no longer
//! wants it. A failed read waits for the owner's Retry; nothing loops. Requests and pins go to
//! the source the view mounted on (`AppState::scope`).

use corral_protocol::detail::DetailRef;
use nsh_core::prelude::*;

use crate::model::Id;
use crate::model::daemon::{Feed, Link};
use crate::model::work;
use crate::state::AppState;

/// Keeps every detail of `targets` loaded while corrald grants `capability` (none: always).
pub(crate) fn keep_details(
    app: AppState,
    targets: Memo<Vec<DetailRef>>,
    capability: Option<&'static str>,
) {
    let app = untracked(|| app.scope());
    let pinned = stored(Vec::<DetailRef>::new());
    effect(move || {
        let wanted = targets.get();
        let was = pinned.replace(wanted.clone());
        untracked(|| {
            for gone in was.iter().filter(|target| !wanted.contains(target)) {
                app.watch_detail(gone.clone(), false);
            }
            for new in wanted.iter().filter(|target| !was.contains(target)) {
                app.watch_detail(new.clone(), true);
            }
        });
        if capability.is_some_and(|capability| !app.capable(capability)) {
            return;
        }
        for target in wanted {
            let needed = app.with_detail(&target, |cache| {
                cache.is_none_or(|cache| cache.stale && !cache.loading && cache.error.is_none())
            });
            if needed {
                untracked(|| app.load(target));
            }
        }
    });
    // The owner may already be letting go of its values when this runs.
    cleanup(move || {
        for gone in pinned.try_with(std::mem::take).unwrap_or_default() {
            app.watch_detail(gone, false);
        }
    });
}

/// A task to keep loaded: its id, whether the open-task list carries it, and whether its detail
/// is missing or stale while the link is up.
pub(crate) type Wanted = Option<(Id, bool, bool)>;

/// Keeps a task's detail loaded while `target` names it: requests it when absent or stale,
/// revalidates it once when it leaves or joins the open-task list, and pins it in the client's
/// cache while it is shown, letting go when the target changes or the view goes. A served row's
/// change refreshes the pinned detail in the client itself. Requests and pins go to the source
/// the view mounted on (`AppState::scope`).
pub(crate) fn keep_loaded(app: AppState, target: Memo<Wanted>) {
    let app = untracked(|| app.scope());
    let seen = stored(None::<(Id, bool)>);
    let pinned = stored(None::<Id>);
    effect(move || {
        let wanted = target.get();
        let shown = wanted.as_ref().map(|(id, _, _)| id.clone());
        if pinned.with(|pinned| *pinned != shown) {
            let was = pinned.replace(shown.clone());
            untracked(|| {
                if let Some(gone) = was {
                    app.watch_detail(work::task_ref(&gone), false);
                }
                if let Some(new) = &shown {
                    app.watch_detail(work::task_ref(new), true);
                }
            });
        }
        let Some((id, open, needed)) = wanted else {
            return;
        };
        let reference = work::task_ref(&id);
        let previous = seen.replace(Some((id.clone(), open)));
        let moved = previous.is_some_and(|(known, was_open)| known == id && was_open != open);
        untracked(|| {
            if moved {
                app.refresh(reference);
            } else if needed {
                app.load(reference);
            }
        });
    });
    // The owner may already be letting go of its values when this runs.
    cleanup(move || {
        if let Some(gone) = pinned.try_with(Option::take).flatten() {
            app.watch_detail(work::task_ref(&gone), false);
        }
    });
}

/// What to keep loaded for a task by id, on the open-task list or off it (tracked).
pub(crate) fn wanted_id(feed: &Feed, id: &Id) -> Wanted {
    let needed = feed.link == Link::Up
        && feed
            .detail(&work::task_ref(id))
            .is_none_or(|cache| cache.stale && !cache.loading);
    Some((id.clone(), feed.board.task(id).is_some(), needed))
}
