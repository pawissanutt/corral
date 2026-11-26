//! What a view of a session's live stream keeps going while it is mounted: one watcher of the
//! session's live topic (released when the view closes or shows another session, through the
//! source it was opened on), and a re-read of the recorded transcript each time the host says it
//! fell behind.

use corral_protocol::detail::{DetailBody, DetailRef};
use nsh_core::prelude::*;

use crate::client::requests;
use crate::model::session::Action;
use crate::model::{DataState, Id};
use crate::state::AppState;

use super::super::run;

fn target(id: &Id) -> DetailRef {
    DetailRef::Session { id: id.to_string() }
}

/// The retained snapshot cannot be used to continue towards the current live tail, but it
/// can still be read. Leaving it behind takes an explicit jump.
pub(super) fn needs_latest(app: AppState, id: &Id) -> bool {
    if app.data.get() == DataState::Fixture {
        return false;
    }
    app.with_detail(&target(id), |cache| {
        cache.is_some_and(|cache| {
            matches!(cache.body(), Some(DetailBody::Session(_)))
                && cache.error.as_ref().is_some_and(|error| {
                    matches!(error.code.as_str(), "history_changed" | "conflict")
                })
        })
    })
}

pub(super) fn latest(app: AppState, id: &Id) {
    if untracked(|| needs_latest(app, id)) {
        _ = requests::latest_transcript(id.to_string());
    }
}

pub fn watch(app: AppState, id: Id) {
    let scoped = app.scope();
    let session = id.clone();
    untracked(|| {
        run(
            scoped,
            Action::Watch {
                session: session.clone(),
                on: true,
            },
        );
    });
    cleanup(move || {
        run(scoped, Action::Watch { session, on: false });
    });
    let target = target(&id);
    let input = computed({
        let target = target.clone();
        move || {
            scoped.feed.with(|feed| {
                let cache = feed.detail(&target);
                (
                    if feed.capable(corral_protocol::app::SESSION_TRANSCRIPT_V1) {
                        None
                    } else {
                        feed.live.get(id.as_str()).map(|turn| turn.reread)
                    },
                    cache.is_some_and(|cache| cache.loading),
                    cache
                        .and_then(|cache| cache.error.as_ref())
                        .is_some_and(|error| error.code == "history_changed"),
                )
            })
        }
    });
    let seen = stored(input.get_untracked().0);
    effect(move || {
        let (now, loading, too_deep) = input.get();
        // Keep newer notices pending while a page or a replacement is still being read.
        // Do not repeatedly attempt a same-depth replacement once it hit the hard bound.
        if loading || too_deep || seen.replace(now) == now || now.is_none() {
            return;
        }
        untracked(|| run(scoped, Action::Refresh(target.clone())));
    });
}
