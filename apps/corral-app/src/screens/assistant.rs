//! Assistant threads, which live inside Agents: the thread page with its composer docked, the
//! transcript the Ask drawer also shows, the quick-thread tile and the session blocks Agents
//! places, and a pending assistant prompt as Inbox shows it. Everything renders
//! `model::assistant`; every button runs `State::act`.

mod agents;
mod cards;
mod page;
mod parts;
mod transcript;

use std::time::Duration;

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::assistant::Mode;
use crate::state::AppState;

pub use agents::{agents_head, agents_tail};
pub use page::{prompt, thread, thread_phone};

/// One revealed step of the streamed answer, as the mockup's interval.
const STREAM_STEP: Duration = Duration::from_millis(260);
/// The transcript rows' inset in the Ask drawer (`.drawer .tm` padding 7 16) and on the phone.
const DRAWER_INSET: f32 = 16.0;
const PHONE_INSET: f32 = 14.0;

/// The messages of thread `id` alone, for the Ask drawer and terminal pane: its own scroll view
/// filling its container, opening at the newest message. Place it directly; do not wrap it in
/// another scroll.
pub fn items(app: AppState, id: &'static str) -> AnyElement {
    let inset = match app.form {
        Form::Phone => PHONE_INSET,
        Form::Desktop | Form::Terminal => DRAWER_INSET,
    };
    transcript::list(app, id, inset)
}

/// Reveals the streamed answer step by step while the assistant answers (`State::tick`). Every
/// view that shows the answer calls this; a step only applies to the position it was scheduled
/// from, so two views on screen at once still reveal one step per tick.
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
