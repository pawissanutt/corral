//! Assistant threads, which live inside Agents: the thread page with its composer docked, the
//! transcript the Ask drawer also shows and the session block Agents places under its roster.
//! Everything renders the served threads of the selected feed through `model::assistant`;
//! every button runs its reducer, whose canonical sends, session controls, engine operations
//! and answers the app performs.

mod agents;
mod attachments;
mod cards;
mod page;
mod parts;
mod side_facts;
mod transcript;

use corral_protocol::app::SESSION_V1;
use corral_protocol::detail::DetailRef;
use corral_protocol::rows::AssistantRow;
use nsh_core::prelude::*;

use crate::model::assistant::{OpOutcome, ThreadRef};
use crate::model::{Form, Id};
use crate::state::AppState;

pub use agents::agents_tail;
pub use page::{live_thread, live_thread_phone, live_thread_title};
pub use side_facts::{identified, stacked};

/// The transcript rows' inset in the Ask drawer (`.drawer .tm` padding 7 16) and on the phone.
const DRAWER_INSET: f32 = 16.0;
const PHONE_INSET: f32 = 14.0;

/// The messages of `source` alone, for the Ask drawer and terminal pane: its own scroll view
/// filling its container, opening at the newest message. Place it directly; do not wrap it in
/// another scroll.
pub fn items(app: AppState, source: ThreadRef) -> AnyElement {
    let inset = match app.form {
        Form::Phone => PHONE_INSET,
        Form::Desktop | Form::Terminal => DRAWER_INSET,
    };
    transcript::list(app, source, inset)
}

/// Subscribes the served thread `scope` while the calling view lives: the first view opens the
/// subscription and the last one closes it. `warm` asks a cold host to start, as Ask does when it
/// opens so the first answer does not wait for the host. Opening and closing both reach the
/// source mounted here, so a data-source change never moves a subscription count between feeds.
/// From a corrald that serves sessions, the thread's session is watched too, so its answer
/// streams in, and its recorded transcript is read for the questions it asks the owner, again
/// whenever its row changes.
pub(crate) fn open_scope(app: AppState, scope: Id, warm: bool) {
    let scoped = app.scope();
    let opening = scope.clone();
    untracked(|| {
        scoped.reduce(scoped.assistant, move |state, feed, _, _| {
            state.subscribe(opening.as_str(), warm, feed)
        });
    });
    if untracked(|| app.capable(SESSION_V1)) {
        crate::screens::agents::watch_session(app, scope.clone());
        let target = DetailRef::Session {
            id: scope.to_string(),
        };
        let watched = scope.clone();
        let row = computed(move || {
            app.feed.with(|feed| {
                crate::model::assistant::row(feed, watched.as_str())
                    .map(|row| (row.id.clone(), row.state, row.turn))
            })
        });
        let seen = stored(false);
        effect(move || {
            row.with(|_| ());
            if seen.replace(true) {
                untracked(|| scoped.refresh(target.clone()));
            } else {
                untracked(|| scoped.load(target.clone()));
            }
        });
    }
    cleanup(move || {
        scoped.reduce(scoped.assistant, move |state, _, _, _| {
            state.unsubscribe(scope.as_str())
        });
    });
}

/// Settles the replies to the assistant session operations this app sent (a profile change, a
/// new session) as they arrive, opening a new session's thread once corrald created it.
pub(crate) fn watch_ops(app: AppState) {
    effect(move || {
        let keys = app.assistant.with(|state| state.live.waiting_ops());
        for key in keys {
            let outcome = app.with_operation(&key, |op| match op {
                None => OpOutcome::Pending,
                Some(op) if op.loading => OpOutcome::Pending,
                Some(op) => match &op.error {
                    Some(error) => OpOutcome::Failed(error.message.clone()),
                    None => op.decode::<AssistantRow>().map_or_else(
                        || OpOutcome::Failed("corrald replied without the session".into()),
                        OpOutcome::Row,
                    ),
                },
            });
            // A changed profile shows through its row; only a refusal or a new session needs
            // settling, and settling removes the key from the waiting set.
            let creating = app
                .assistant
                .with_untracked(|state| state.live.new_session.as_ref() == Some(&key));
            let settle = match &outcome {
                OpOutcome::Pending => false,
                OpOutcome::Failed(_) => true,
                OpOutcome::Row(_) => creating,
            };
            if settle {
                untracked(|| {
                    app.reduce(app.assistant, move |state, _, _, _| {
                        state.settle(&key, outcome)
                    });
                });
            }
        }
    });
}
