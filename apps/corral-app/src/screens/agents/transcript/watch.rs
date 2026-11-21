//! What a view of a session's live stream keeps going while it is mounted: one watcher of the
//! session's live topic (released when the view closes or shows another session, through the
//! source it was opened on), and a re-read of the recorded transcript each time the host says it
//! fell behind.

use corral_protocol::detail::DetailRef;
use nsh_core::prelude::*;

use crate::model::Id;
use crate::model::session::Action;
use crate::state::AppState;

use super::super::run;

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
    let reread = computed({
        let id = id.clone();
        move || {
            app.feed
                .with(|feed| feed.live.get(id.as_str()).map(|turn| turn.reread))
        }
    });
    let seen = stored(None::<Option<u64>>);
    effect(move || {
        let now = reread.get();
        let Some(before) = seen.replace(Some(now)) else {
            return;
        };
        if before == now || now.is_none() {
            return;
        }
        untracked(|| {
            run(
                app,
                Action::Refresh(DetailRef::Session { id: id.to_string() }),
            );
        });
    });
}
