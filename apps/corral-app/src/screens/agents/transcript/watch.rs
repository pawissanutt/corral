//! What a daemon session page keeps going while it is open: one watcher of the session's live
//! turn (released when the page closes or shows another session, as the thread scope is, through
//! the source it was opened on), a re-read of the recorded transcript each time the host says it
//! fell behind (not while the owner reads an earlier page), and the fetch of the complete words
//! behind every preview on the shown page.

use corral_protocol::detail::DetailRef;
use nsh_core::prelude::*;

use crate::model::Id;
use crate::model::agents::{self, Action};
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
    {
        let id = id.clone();
        effect(move || {
            let now = reread.get();
            let Some(before) = seen.replace(Some(now)) else {
                return;
            };
            if before == now || now.is_none() {
                return;
            }
            untracked(|| {
                let earlier = app.agents.with(|state| state.earlier.contains(id.as_str()));
                if !earlier {
                    run(
                        app,
                        Action::Refresh(DetailRef::Session { id: id.to_string() }),
                    );
                }
            });
        });
    }
    let wanted = computed({
        let id = id.clone();
        move || app.feed.with(|feed| agents::fetches(feed, &id))
    });
    effect(move || {
        let tokens = wanted.get();
        untracked(|| {
            for token in tokens {
                run(
                    app,
                    Action::Fetch {
                        session: id.clone(),
                        token,
                    },
                );
            }
        });
    });
}
