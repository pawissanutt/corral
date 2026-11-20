//! Keeps served details loaded while a view shows them: each is pinned in the client's cache,
//! read when it is missing, read again when it went stale, and let go when the view no longer
//! wants it. A failed read waits for the owner's Retry; nothing loops. Requests and pins go to
//! the source the view mounted on (`AppState::scope`).

use corral_protocol::detail::DetailRef;
use nsh_core::prelude::*;

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
