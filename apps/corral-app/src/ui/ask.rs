//! The Ask surfaces beside the composer: the composer docked under a thread, the global hotkey
//! window and the phone share sheet, with the model pill and list they share.

pub(crate) mod drawer;
mod hotkey;
pub(crate) mod models;
mod share;

use nsh_core::prelude::*;

use crate::model::Form;
use crate::model::assistant::{self, QUICK_THREAD};
use crate::state::AppState;
use crate::ui::composer::{Host, ListAt, field};

/// Runs one of the assistant model's typed actions with the app's draft and target, then the
/// effects it returns.
pub(crate) fn act(app: AppState, action: assistant::Action) {
    let target = app.ask_target.get_untracked();
    let text = app.ask_draft.get_untracked();
    app.act(app.assistant, move |state, env| {
        state.act(action, &text, target, env)
    });
}

/// The composer docked under a thread page. The quick thread keeps the Assistant | PM switch
/// and the model pill; a named session talks to that session only, so it has no switch, and a
/// pane session has no pill (Pi's own picker): the model decides both. Its model list opens above
/// the field on desktop and in a terminal, inline under the pill on a phone.
pub fn docked(app: AppState, thread: &'static str) -> AnyElement {
    let session = (thread != QUICK_THREAD).then_some(thread);
    let list = if app.form == Form::Phone {
        ListAt::Inline
    } else {
        ListAt::Above
    };
    field(
        app,
        Host {
            session,
            list,
            docked: true,
        },
    )
}

/// The small Ask window the global shortcut opens, shown while `app.hotkey` holds: on desktop a
/// window centred over a dimmed backdrop that stands in for the desktop, closed by Esc or its
/// Hide control; on a phone and in a terminal the same content in the form's modal. Draws nothing
/// while closed.
pub fn hotkey(app: AppState) -> AnyElement {
    hotkey::hotkey(app)
}

/// The phone share sheet's steps over the shared page, shown while `app.share` holds: the system
/// sheet, the caption and Send, saved on this phone, filed. On desktop and in a terminal the
/// same steps in the form's modal, so the demo still closes cleanly. Draws nothing while closed.
pub fn share(app: AppState) -> AnyElement {
    share::share(app)
}
