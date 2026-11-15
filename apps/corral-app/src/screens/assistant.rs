//! Assistant threads, which live inside Agents: the thread page with its composer docked, the
//! transcript the Ask drawer also shows, the quick-thread tile and the session blocks Agents
//! places, and a pending assistant prompt as Inbox shows it. Everything renders
//! `model::assistant`; every button runs its reducer: the fixture preview's, or with the daemon
//! the live one, whose canonical sends, commands and prompt answers the app performs.

mod agents;
mod attachments;
mod cards;
mod page;
mod parts;
mod side_facts;
mod transcript;
mod verbs;

use std::time::Duration;

use corral_protocol::rows::AssistantRow;
use nsh_core::prelude::*;

use crate::model::assistant::{Mode, OpOutcome, ThreadRef};
use crate::model::{Form, Id};
use crate::state::AppState;

pub use agents::{agents_head, agents_tail};
pub use page::{
    live_prompt, live_thread, live_thread_phone, live_thread_title, prompt, thread, thread_phone,
};
pub use side_facts::{identified, stacked};
pub use verbs::session_verbs;

/// One revealed step of the fixture's streamed answer, as the mockup's interval.
const STREAM_STEP: Duration = Duration::from_millis(260);
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
/// opens so the first answer does not wait for the host.
pub(crate) fn open_scope(app: AppState, scope: Id, warm: bool) {
    let opening = scope.clone();
    untracked(|| {
        app.reduce(app.assistant, move |state, feed, _, _| {
            state.subscribe(opening.as_str(), warm, feed)
        });
    });
    cleanup(move || {
        app.reduce(app.assistant, move |state, _, _, _| {
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

/// Reveals the fixture's streamed answer step by step while the assistant answers
/// (`State::tick`). Every fixture view that shows the answer calls this; a step only applies to
/// the position it was scheduled from, so two views on screen at once still reveal one step per
/// tick.
fn drive_stream(app: AppState) {
    let pending = stored(None::<Spawned<()>>);
    effect(move || {
        let from = app.assistant.with(|state| {
            (state.mode == Mode::Answering && state.stream < 1.0).then_some(state.stream.to_bits())
        });
        if let Some(old) = pending.replace(None) {
            old.cancel();
        }
        if let Some(from) = from {
            let task = set_timeout(STREAM_STEP, move || {
                app.assistant.update(|state| {
                    if state.stream.to_bits() == from {
                        state.tick();
                    }
                });
            });
            let _ = pending.replace(Some(task));
        }
    });
}
